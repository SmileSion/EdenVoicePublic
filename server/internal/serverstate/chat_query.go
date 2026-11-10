package serverstate

import (
	"edenvoice/server/internal/protocol"
	"errors"
	"fmt"
)

func (s *State) QueryChat(channel uint64, cursor, query string, before uint64) ([]protocol.TextMessage, string, error) {
	s.chatMu.Lock()
	store := s.chatStore
	s.chatMu.Unlock()
	reader, ok := store.(interface {
		Query(uint64, string, string, ...uint64) ([]protocol.TextMessage, string, error)
	})
	if !ok {
		return nil, "", errors.New("persistent chat history unavailable")
	}
	return reader.Query(channel, cursor, query, before)
}

func (s *State) ChatHeads() map[uint64]string {
	s.mu.RLock()
	defer s.mu.RUnlock()
	heads := make(map[uint64]string)
	for channel, messages := range s.chatHistory {
		if _, exists := s.channels[channel]; exists && len(messages) > 0 {
			heads[channel] = fmt.Sprint(messages[len(messages)-1].ID)
		}
	}
	return heads
}

func (s *State) EnableChatQueries(id uint64) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if session := s.sessions[id]; session != nil {
		session.ChatQuerySupported = true
	}
}
func (s *State) BroadcastChatHeads(event protocol.Envelope) {
	s.mu.RLock()
	senders := make([]Sender, 0)
	for _, session := range s.sessions {
		if session.ChatQuerySupported && session.Sender != nil {
			senders = append(senders, session.Sender)
		}
	}
	s.mu.RUnlock()
	for _, sender := range senders {
		_ = sender.Send(event)
	}
}
