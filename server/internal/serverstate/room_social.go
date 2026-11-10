package serverstate

import (
	"edenvoice/server/internal/protocol"
	"errors"
	"sort"
	"time"
)

var ErrRoomSocialAccess = errors.New("authenticated membership in the requested room is required")
var ErrReactionCooldown = errors.New("wait two seconds before another reaction")

// The generation invalidates uploads even after leaving and rejoining a room.
func (s *State) RoomSocialMember(id, channel uint64) (uint64, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	session := s.sessions[id]
	if session == nil || session.ClientID == "" || channel == 0 || session.ChannelID != channel {
		return 0, false
	}
	return session.RoomGeneration, true
}

func (s *State) RoomSocialStatus(id, channel uint64) ([]protocol.AwayPresence, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	session := s.sessions[id]
	if session == nil || session.ClientID == "" || channel == 0 || session.ChannelID != channel {
		return nil, ErrRoomSocialAccess
	}
	away := []protocol.AwayPresence{}
	for _, member := range s.roomMembers[channel] {
		if member.Away {
			away = append(away, protocol.AwayPresence{SessionID: member.ID})
		}
	}
	sort.Slice(away, func(i, j int) bool { return away[i].SessionID < away[j].SessionID })
	return away, nil
}

// Membership validation, presence mutation and room delivery share one lock.
// Sender.Send only queues a bounded control frame; it never performs socket I/O.
func (s *State) ApplyRoomSocial(id uint64, req protocol.RoomSocialRequest, now time.Time) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	session := s.sessions[id]
	if session == nil || session.ClientID == "" || req.ChannelID == 0 || session.ChannelID != req.ChannelID {
		return ErrRoomSocialAccess
	}
	event := protocol.RoomSocialEvent{RequestID: req.RequestID, ChannelID: req.ChannelID, SessionID: id, Kind: req.Action}
	switch req.Action {
	case "away":
		if req.Active == nil {
			return errors.New("away requires an explicit active boolean")
		}
		session.Away = *req.Active
		event.Active = session.Away
		if session.Away {
			session.Muted = true
			// This ordinary event is understood by every client, including legacy
			// sessions that have not subscribed to social events.
			s.broadcastVoiceStateLocked(session)
		}
	case "reaction":
		if !protocol.ValidRoomReaction(req.Emoji) {
			return errors.New("unsupported reaction")
		}
		if !session.LastReactionAt.IsZero() && now.Sub(session.LastReactionAt) < 2*time.Second {
			return ErrReactionCooldown
		}
		session.LastReactionAt = now
		event.Emoji = req.Emoji
		event.ExpiresAtUnixMS = now.Add(2500 * time.Millisecond).UnixMilli()
	default:
		return errors.New("unsupported social action")
	}
	s.broadcastSocialLocked(event)
	return nil
}
func (s *State) broadcastSocialLocked(event protocol.RoomSocialEvent) {
	ack, _ := protocol.NewEnvelope(protocol.TypeRoomSocialEvent, event)
	event.RequestID = ""
	broadcast, _ := protocol.NewEnvelope(protocol.TypeRoomSocialEvent, event)
	for _, member := range s.roomMembers[event.ChannelID] {
		if member.RoomSocialSupported && member.Sender != nil {
			if member.ID == event.SessionID {
				_ = member.Sender.Send(ack)
			} else {
				_ = member.Sender.Send(broadcast)
			}
		}
	}
}

// Clearing away never opens the microphone. Unmute is handled by SetMuted,
// while explicit return, move and disconnect only clear presence.
func (s *State) clearAwayLocked(session *Session) {
	if !session.Away {
		return
	}
	session.Away = false
	s.broadcastSocialLocked(protocol.RoomSocialEvent{Kind: "away", ChannelID: session.ChannelID, SessionID: session.ID, Active: false})
}
func (s *State) broadcastVoiceStateLocked(session *Session) {
	user := userFromSession(session)
	envelope, _ := protocol.NewEnvelope(protocol.TypeUserStateChanged, protocol.UserStateChanged{GameActivity: user.GameActivity, SessionID: user.SessionID, Muted: user.Muted, Deafened: user.Deafened, AdminMuted: user.AdminMuted, ServerMuted: user.ServerMuted, ChannelMuted: user.ChannelMuted})
	for _, member := range s.sessions {
		if member.Sender != nil {
			_ = member.Sender.Send(envelope)
		}
	}
}

// Avoid a room-change race between disk reads and outgoing private image data.
func (s *State) SendRoomSocial(id, channel, generation uint64, env protocol.Envelope) bool {
	s.mu.RLock()
	defer s.mu.RUnlock()
	session := s.sessions[id]
	if session == nil || session.ClientID == "" || session.ChannelID != channel || session.RoomGeneration != generation || session.Sender == nil {
		return false
	}
	return session.Sender.Send(env) == nil
}

func (s *State) EnableRoomSocial(id uint64) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if session := s.sessions[id]; session != nil {
		session.RoomSocialSupported = true
	}
}
