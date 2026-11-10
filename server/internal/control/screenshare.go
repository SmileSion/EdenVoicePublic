package control

import (
	"context"
	"errors"
	"time"

	"edenvoice/server/internal/administration"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/screenshare"
	"edenvoice/server/internal/serverstate"
)

// ConfigureScreenSharing runs before accepting control connections.
func (s *Server) ConfigureScreenSharing(cfg screenshare.Config, paths ...string) error {
	path := ""
	if len(paths) > 0 {
		path = paths[0]
	}
	document, err := screenshare.LoadPolicy(path, cfg.Policy)
	if err != nil {
		return err
	}
	cfg.Policy = document.Policy
	manager, err := screenshare.New(cfg, screenshare.Hooks{
		Allowed: func(id, channel uint64) bool { return channel != 0 && s.state.ChannelOf(id) == channel },
		Changed: s.broadcastScreenShare,
	})
	if err != nil {
		return err
	}
	s.screenPolicyPath = path
	s.screenPolicyDocument = document
	s.screen = manager
	return nil
}
func (s *Server) ScreenSharePolicy() screenshare.Policy {
	if s.screen == nil {
		return screenshare.DefaultConfig().Policy
	}
	return s.screen.Policy()
}
func (s *Server) dispatchScreenShare(id uint64, envelope protocol.Envelope, sender serverstate.Sender) error {
	var request screenshare.Request
	if err := envelope.Decode(&request); err != nil {
		return err
	}
	if len(request.RequestID) > 64 || len(request.ShareID) > 64 {
		return errors.New("invalid screen share request")
	}
	reply := func(response screenshare.Response) {
		response.RequestID = request.RequestID
		response.Action = request.Action
		s.send(sender, protocol.TypeScreenShareState, response)
	}
	if s.screen == nil {
		reply(screenshare.Response{Error: "此服务器未启用屏幕共享"})
		return nil
	}
	channel := s.state.ChannelOf(id)
	if channel == 0 || channel != request.ChannelID {
		reply(screenshare.Response{Error: "请先进入同一个语音房间"})
		return nil
	}
	s.screenMu.Lock()
	s.screenMembers[id] = channel
	s.screenMu.Unlock()
	response := screenshare.Response{State: s.screen.Status(channel)}
	switch request.Action {
	case "status":
		reply(response)
	case "pause":
		if request.Paused == nil {
			response.Error = "隐私暂停需要明确的 paused 布尔值"
		} else if err := s.screen.Pause(id, channel, request.ShareID, *request.Paused); err != nil {
			response.Error = err.Error()
		}
		response.State = s.screen.Status(channel)
		reply(response)
	case "stop":
		moderator := s.authorize(s.state.ClientID(id), administration.ActionWatchManage, administration.Resource{ChannelID: channel}) == nil
		if err := s.screen.Stop(id, channel, request.ShareID, moderator); err != nil {
			response.Error = err.Error()
		}
		response.State = s.screen.Status(channel)
		reply(response)
	case "leave":
		s.screen.Leave(id, channel, request.ShareID)
		response.State = s.screen.Status(channel)
		reply(response)
	case "cancel":
		s.cancelScreenRequest(id)
		s.screen.RemoveSession(id)
		response.State = s.screen.Status(channel)
		reply(response)
	case "publish", "subscribe":
		// Bounded asynchronous negotiation: ICE never blocks control heartbeats,
		// mute commands, room changes, or the independent voice workers.
		s.screenMu.Lock()
		if _, busy := s.screenPending[id]; busy || len(s.screenPending) >= 32 {
			s.screenMu.Unlock()
			response.Error = "共享连接正在协商，请稍后重试"
			reply(response)
			return nil
		}
		ctx, cancel := context.WithTimeout(context.Background(), 8*time.Second)
		done := make(chan struct{})
		s.screenPending[id] = screenOperation{cancel: cancel, done: done}
		s.screenMu.Unlock()
		go func() {
			defer cancel()
			defer func() {
				s.screenMu.Lock()
				if current, ok := s.screenPending[id]; ok && current.done == done {
					delete(s.screenPending, id)
				}
				s.screenMu.Unlock()
				close(done)
			}()
			var err error
			if request.Action == "publish" {
				response.State, response.SDP, err = s.screen.Publish(ctx, id, channel, request.Settings, request.SDP)
			} else {
				response.State, response.SDP, err = s.screen.Subscribe(ctx, id, channel, request.ShareID, request.SDP)
			}
			if err != nil {
				response.Error = err.Error()
				response.State = s.screen.Status(channel)
			}
			if ctx.Err() == nil && s.state.ChannelOf(id) == channel {
				reply(response)
			}
		}()
	default:
		return errors.New("unknown screen share action")
	}
	return nil
}
func (s *Server) cancelScreenRequest(id uint64) {
	s.screenMu.Lock()
	if op, ok := s.screenPending[id]; ok {
		op.cancel()
	}
	s.screenMu.Unlock()
}
func (s *Server) leaveScreenShare(id uint64) {
	if s.screen == nil {
		return
	}
	s.cancelScreenRequest(id)
	s.screenMu.Lock()
	delete(s.screenMembers, id)
	s.screenMu.Unlock()
	s.screen.RemoveSession(id)
}
func (s *Server) broadcastScreenShare(channel uint64, reason string) {
	if s.screen == nil {
		return
	}
	s.screenMu.Lock()
	var recipients []uint64
	for id, room := range s.screenMembers {
		if room == channel {
			recipients = append(recipients, id)
		}
	}
	s.screenMu.Unlock()
	response := screenshare.Response{State: s.screen.Status(channel), Action: "state", Error: reason}
	message, err := protocol.NewEnvelope(protocol.TypeScreenShareState, response)
	if err != nil {
		return
	}
	for _, id := range recipients {
		if s.state.ChannelOf(id) == channel {
			s.state.SendTo(id, message)
		}
	}
}

type screenOperation struct {
	cancel context.CancelFunc
	done   chan struct{}
}

func (s *Server) WebScreenSharing(actor string, update *screenshare.PolicyDocument) (screenshare.PolicyDocument, error) {
	if _, err := s.WebRole(actor); err != nil {
		return screenshare.PolicyDocument{}, err
	}
	s.screenPolicyMu.Lock()
	defer s.screenPolicyMu.Unlock()
	if s.screen == nil {
		return screenshare.PolicyDocument{}, errors.New("屏幕共享模块未初始化")
	}
	if update == nil {
		return s.screenPolicyDocument, nil
	}
	if update.Revision != s.screenPolicyDocument.Revision {
		return screenshare.PolicyDocument{}, errors.New("配置已被其他管理员修改，请刷新后重试")
	}
	if err := update.Policy.Validate(); err != nil {
		return screenshare.PolicyDocument{}, err
	}
	if update.Policy == s.screenPolicyDocument.Policy {
		return s.screenPolicyDocument, nil
	}
	next := screenshare.PolicyDocument{Revision: update.Revision + 1, Policy: update.Policy}
	if err := s.screen.UpdatePolicy(next.Policy, func() error {
		return screenshare.SavePolicy(s.screenPolicyPath, next)
	}); err != nil {
		return screenshare.PolicyDocument{}, err
	}
	s.screenPolicyDocument = next
	// Also refresh clients in idle rooms so enabling sharing becomes visible.
	s.screenMu.Lock()
	rooms := map[uint64]bool{}
	for _, room := range s.screenMembers {
		rooms[room] = true
	}
	s.screenMu.Unlock()
	for room := range rooms {
		s.broadcastScreenShare(room, "")
	}
	s.WebAudit("screen sharing policy updated", actor, "")
	return next, nil
}

func (s *Server) CloseScreenSharing() {
	if s.screen != nil {
		s.screen.Close()
	}
}
