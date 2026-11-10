package control

import (
	"edenvoice/server/internal/chatstore"
	"edenvoice/server/internal/config"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/serverstate"
	"io"
	"log/slog"
	"testing"
	"time"
)

func TestHistoryQueriesStayInJoinedRoomAndRateLimit(t *testing.T) {
	state := serverstate.New(config.Default())
	store, err := chatstore.Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	if err = state.RestoreChat(store); err != nil {
		t.Fatal(err)
	}
	s := New("", "", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	sender := &recordingSender{}
	session := state.AddSession("user", sender)
	if err = state.Join(session.ID, 1); err != nil {
		t.Fatal(err)
	}
	if _, err = state.RecordTextMessage(session.ID, "history needle", time.Now()); err != nil {
		t.Fatal(err)
	}
	query := func(channel uint64) map[string]any {
		env, _ := protocol.NewEnvelope("query_chat", chatQuery{RequestID: "q", ChannelID: channel, Query: "needle"})
		if err = s.queryChat(session.ID, env, sender); err != nil {
			t.Fatal(err)
		}
		var result map[string]any
		if err = sender.messages[len(sender.messages)-1].Decode(&result); err != nil {
			t.Fatal(err)
		}
		return result
	}
	if query(2)["error"] == "" {
		t.Fatal("history leaked across rooms")
	}
	result := query(1)
	if result["error"] != "" || len(result["messages"].([]any)) != 1 {
		t.Fatalf("query failed: %v", result)
	}
	if query(1)["error"] == "" {
		t.Fatal("missing query limit")
	}
}

func TestChatActivityDoesNotSendUnknownEventsToOldClients(t *testing.T) {
	state := serverstate.New(config.Default())
	old, newer := &recordingSender{}, &recordingSender{}
	state.AddSession("old", old)
	session := state.AddSession("new", newer)
	state.EnableChatQueries(session.ID)
	event, _ := protocol.NewEnvelope("chat_heads", map[string]any{"heads": map[string]string{"1": "42"}})
	state.BroadcastChatHeads(event)
	if len(old.messages) != 0 || len(newer.messages) != 1 {
		t.Fatal("old clients must not receive unknown control messages")
	}
}
