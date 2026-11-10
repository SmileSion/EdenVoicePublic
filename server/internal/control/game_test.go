package control

import (
	"bytes"
	"edenvoice/server/internal/config"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/serverstate"
	"encoding/json"
	"io"
	"log/slog"
	"testing"
	"time"
)

func TestGameActivityUsesAuthenticatedSenderAndLegacyEvent(t *testing.T) {
	state := serverstate.New(config.Default())
	s := New("", "", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	a, b := &recordingSender{}, &recordingSender{}
	alice := state.AddSession("Alice", a)
	bob := state.AddSession("Bob", b)
	e, _ := protocol.NewEnvelope(protocol.TypeSetGameActivity, map[string]any{"game_id": "cs2", "session_id": bob.ID, "title": "injected", "expiresAt": 9999999999999})
	if err := s.dispatch(alice.ID, e, a); err != nil {
		t.Fatal(err)
	}
	user, _ := state.User(bob.ID)
	if user.GameActivity != nil {
		t.Fatal("spoofed target")
	}
	if len(b.messages) != 1 || b.messages[0].Type != protocol.TypeUserStateChanged {
		t.Fatal("expected backward-compatible state event")
	}
	var event protocol.UserStateChanged
	if err := b.messages[0].Decode(&event); err != nil {
		t.Fatal(err)
	}
	if event.SessionID != alice.ID || event.GameActivity.Title != "Counter-Strike 2" {
		t.Fatal(event)
	}
	if event.GameActivity.ExpiresAt > time.Now().Add(61*time.Second).UnixMilli() {
		t.Fatal("untrusted ttl")
	}
	snapshot := s.snapshot()
	data, _ := json.Marshal(snapshot)
	if !bytes.Contains(data, []byte(`"game_activity"`)) {
		t.Fatal("snapshot")
	}
	clear, _ := protocol.NewEnvelope(protocol.TypeSetGameActivity, protocol.GameActivityRequest{})
	if err := s.dispatch(alice.ID, clear, a); err != nil {
		t.Fatal(err)
	}
	if err := b.messages[len(b.messages)-1].Decode(&event); err != nil || event.GameActivity != nil {
		t.Fatal("did not broadcast null activity", err)
	}
}
