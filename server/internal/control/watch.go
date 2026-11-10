package control

import (
	"errors"
	"time"

	"edenvoice/server/internal/administration"
	"edenvoice/server/internal/archive"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/serverstate"
	"edenvoice/server/internal/watchparty"
)

// Serialize watch transitions AND enqueueing their results, so recipients see
// monotonically ordered revisions. This lock is never taken by voice workers.
func (s *Server) dispatchWatch(id uint64, envelope protocol.Envelope, sender serverstate.Sender) error {
	var request protocol.WatchRequest
	if err := envelope.Decode(&request); err != nil {
		return err
	}
	s.watchMu.Lock()
	defer s.watchMu.Unlock()
	now := time.Now()
	fail := func(message string) error {
		current := watchparty.Snapshot{ChannelID: request.ChannelID, Epoch: request.Epoch}
		if request.ChannelID != 0 && s.state.ChannelOf(id) == request.ChannelID {
			current = s.watch.Status(request.ChannelID, now)
		}
		s.sendWatch(id, sender, protocol.WatchState{Snapshot: current, RequestID: request.RequestID, ActorID: id, Action: request.Action, Error: message})
		return nil
	}
	channel := s.state.ChannelOf(id)
	if channel == 0 || channel != request.ChannelID {
		return fail("watch room no longer matches voice room")
	}
	// Limit active viewers only; unaffiliated requests never allocate map entries.
	if last, exists := s.watchLastRequest[id]; exists && request.Action != "leave" && now.Sub(last) < 40*time.Millisecond {
		return fail("watch commands are too frequent")
	}
	var state watchparty.Snapshot
	var err error
	moderator := s.authorize(s.state.ClientID(id), administration.ActionWatchManage, administration.Resource{ChannelID: channel}) == nil
	switch request.Action {
	case "status":
		s.sendWatch(id, sender, protocol.WatchState{Snapshot: s.watch.Status(channel, now), RequestID: request.RequestID, ActorID: id, Action: "status"})
		return nil
	case "close":
		state, err = s.watch.Close(channel, id, request.Epoch, now, moderator)
		if err != nil {
			return fail(err.Error())
		}
		response := protocol.WatchState{Snapshot: state, RequestID: request.RequestID, ActorID: id, Action: "closed"}
		s.broadcastWatchLocked(response)
		if s.watchMembers[id] != channel {
			s.sendWatch(id, sender, response)
		}
		for _, member := range state.Members {
			delete(s.watchMembers, member.SessionID)
			delete(s.watchLastRequest, member.SessionID)
		}
		return nil
	case "transfer_host":
		if s.state.ChannelOf(request.TargetSessionID) != channel {
			return fail("new host must be in this voice channel")
		}
		state, err = s.watch.TransferHost(channel, id, request.Epoch, request.TargetSessionID, now, moderator)
	case "unready":
		state, err = s.watch.Unready(channel, id, request.Epoch, now)
	case "open", "join":
		if old := s.watchMembers[id]; old != 0 && old != channel {
			s.leaveWatchLocked(id, now)
		}
		state, err = s.watch.Join(channel, id, now)
		if err == nil {
			s.watchMembers[id] = channel
		}
	case "leave":
		if s.watchMembers[id] != channel {
			return nil
		}
		if request.Epoch != 0 {
			if _, err = s.watch.Read(channel, id, request.Epoch, now); err != nil {
				return fail(err.Error())
			}
		}
		s.leaveWatchLocked(id, now)
		s.send(sender, protocol.TypeWatchState, protocol.WatchState{Snapshot: watchparty.Snapshot{ChannelID: channel, Epoch: request.Epoch}, RequestID: request.RequestID, Action: "leave", ActorID: id})
		return nil
	case "file":
		state, err = s.watch.UpdateFile(channel, id, request.Epoch, request.File, request.Ready, now)
	case "play", "pause", "seek":
		state, err = s.watch.Control(channel, id, request.Epoch, request.Action, request.Position, now, moderator)
	case "sync":
		state, err = s.watch.Read(channel, id, request.Epoch, now)
	default:
		err = errors.New("unknown watch action")
	}
	if err != nil {
		return fail(err.Error())
	}
	if s.watchMembers[id] == channel {
		s.watchLastRequest[id] = now
	}
	// Room movement can race between the authorization read and this point.
	// The move hook also cleans up, but do not emit a successful stale response.
	if s.state.ChannelOf(id) != channel {
		s.leaveWatchLocked(id, now)
		return fail("voice room changed")
	}
	if s.archive != nil && !state.Paused {
		ready := 0
		title := ""
		for _, member := range state.Members {
			if member.Ready && s.state.ChannelOf(member.SessionID) == channel {
				ready++
			}
			if member.SessionID == state.HostID && member.File != nil {
				title = member.File.Name
			}
		}
		if ready >= 2 && title != "" {
			s.archive.Observe(archive.Activity{Kind: "watch_played", RoomID: channel, Movie: title, At: now})
		}
	}
	response := protocol.WatchState{Snapshot: state, RequestID: request.RequestID, ActorID: id, Action: request.Action}
	if request.Action == "sync" {
		s.sendWatch(id, sender, response)
	} else {
		s.broadcastWatchLocked(response)
		if s.watchMembers[id] != channel {
			s.sendWatch(id, sender, response)
		}
	}
	return nil
}

func (s *Server) broadcastWatchLocked(response protocol.WatchState) {
	for _, member := range response.Members {
		if s.state.ChannelOf(member.SessionID) == response.ChannelID {
			personal := response
			personal.Permissions = s.watchPermissions(member.SessionID, response)
			message, err := protocol.NewEnvelope(protocol.TypeWatchState, personal)
			if err == nil {
				s.state.SendTo(member.SessionID, message)
			}
		}
	}
}

func (s *Server) sendWatch(id uint64, sender serverstate.Sender, response protocol.WatchState) {
	response.Permissions = s.watchPermissions(id, response)
	s.send(sender, protocol.TypeWatchState, response)
}

func (s *Server) watchPermissions(id uint64, response protocol.WatchState) *protocol.WatchPermissions {
	p := &protocol.WatchPermissions{Role: "guest"}
	if response.ChannelID == 0 || s.state.ChannelOf(id) != response.ChannelID {
		return p
	}
	participant := false
	for _, member := range response.Members {
		participant = participant || member.SessionID == id
	}
	if participant {
		p.Role = "viewer"
	}
	host := response.HostID == id && response.Epoch != 0
	if host {
		p.Role = "host"
	}
	clientID := s.state.ClientID(id)
	moderator := s.authorize(clientID, administration.ActionWatchManage, administration.Resource{ChannelID: response.ChannelID}) == nil
	if moderator {
		p.Role = "channel_admin"
		if user, ok := s.administration.User(clientID); ok {
			switch user.ServerRole {
			case administration.RoleMaster:
				p.Role = "master"
			case administration.RoleAdmin:
				p.Role = "server_admin"
			}
		}
	}
	open := response.Epoch != 0 && response.Action != "closed" && response.Action != "ended"
	p.CanClose = open && (host || moderator)
	p.CanTransferHost = p.CanClose
	p.CanControl = p.CanClose && participant
	return p
}

func (s *Server) leaveWatchLocked(id uint64, now time.Time) {
	channel := s.watchMembers[id]
	delete(s.watchMembers, id)
	delete(s.watchLastRequest, id)
	if channel == 0 {
		return
	}
	if state, ok := s.watch.Leave(channel, id, now); ok {
		ended, _ := protocol.NewEnvelope(protocol.TypeWatchState, protocol.WatchState{Snapshot: watchparty.Snapshot{ChannelID: channel, Epoch: state.Epoch}, ActorID: id, Action: "ended"})
		s.state.SendTo(id, ended)
		s.broadcastWatchLocked(protocol.WatchState{Snapshot: state, ActorID: id, Action: "member_left"})
	}
}

func (s *Server) leaveWatchOnMove(id, channel uint64) {
	s.watchMu.Lock()
	defer s.watchMu.Unlock()
	if previous := s.watchMembers[id]; previous != 0 && previous != channel {
		s.leaveWatchLocked(id, time.Now())
	}
}
