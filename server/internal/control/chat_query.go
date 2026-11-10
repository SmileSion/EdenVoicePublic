package control

import (
	"errors"
	"strconv"
	"time"
	"unicode/utf8"

	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/serverstate"
)

type chatQuery struct {
	BeforeID  string `json:"before_id"`
	RequestID string `json:"request_id"`
	ChannelID uint64 `json:"channel_id"`
	Cursor    string `json:"cursor"`
	Query     string `json:"query"`
}

func (s *Server) queryChat(id uint64, envelope protocol.Envelope, sender serverstate.Sender) error {
	var req chatQuery
	if err := envelope.Decode(&req); err != nil {
		return err
	}
	if req.RequestID == "" || len(req.RequestID) > 64 || len(req.Cursor) > 20 || len(req.Query) > 256 || !utf8.ValidString(req.Query) {
		return errors.New("invalid history query")
	}
	before := uint64(0)
	if req.BeforeID != "" {
		var err error
		before, err = strconv.ParseUint(req.BeforeID, 10, 64)
		if err != nil {
			return errors.New("invalid history boundary")
		}
	}
	response := map[string]any{"request_id": req.RequestID, "channel_id": req.ChannelID, "messages": []protocol.TextMessage{}, "next_cursor": "", "error": ""}
	user, ok := s.state.User(id)
	if !ok || req.ChannelID == 0 || user.ChannelID != req.ChannelID {
		response["error"] = "请先加入对应房间再查询聊天记录"
		s.send(sender, "chat_query_result", response)
		return nil
	}
	s.chatQueryMu.Lock()
	last := s.chatLastQuery[id]
	allowed := time.Since(last) >= 200*time.Millisecond
	if allowed {
		s.chatLastQuery[id] = time.Now()
	}
	s.chatQueryMu.Unlock()
	if !allowed {
		response["error"] = "查询过于频繁，请稍后重试"
		s.send(sender, "chat_query_result", response)
		return nil
	}
	select {
	case s.chatQuerySlots <- struct{}{}:
		defer func() { <-s.chatQuerySlots }()
	default:
		response["error"] = "历史查询繁忙，请稍后重试"
		s.send(sender, "chat_query_result", response)
		return nil
	}
	messages, cursor, err := s.state.QueryChat(req.ChannelID, req.Cursor, req.Query, before)
	if err != nil {
		response["error"] = "历史记录暂不可用，请稍后重试"
	} else {
		response["messages"] = messages
		response["next_cursor"] = cursor
	}
	// Administrative moves may occur while the disk is read.
	user, ok = s.state.User(id)
	if !ok || user.ChannelID != req.ChannelID {
		return nil
	}
	s.send(sender, "chat_query_result", response)
	return nil
}

func (s *Server) broadcastChatHeads(message protocol.TextMessage) {
	heads, _ := protocol.NewEnvelope("chat_heads", map[string]any{"heads": map[uint64]string{message.ChannelID: strconv.FormatUint(message.ID, 10)}})
	s.state.BroadcastChatHeads(heads)
}
