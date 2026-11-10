package control

import (
	"edenvoice/server/internal/archive"
	"edenvoice/server/internal/config"
	"edenvoice/server/internal/serverstate"
	"io"
	"log/slog"
	"testing"
	"time"
)

func TestArchiveGameWireKeepsPublicCardAndStripsPrivateMetadata(t *testing.T) {
	s := New("", "", serverstate.New(config.Default()), slog.New(slog.NewTextHandler(io.Discard, nil)))
	sender := &recordingSender{}
	event := archive.Event{ID: "game-1", EventType: "shared_game", Category: "game", RoomID: 1, OccurredAt: time.Now(), Game: &archive.GameMemory{GameID: "cs2", Title: "Counter-Strike 2", DurationSeconds: 120}, Metadata: map[string]any{"private": "secret"}}
	if err := s.sendBoundedArchive(sender, archive.RoomSnapshot{Stats: archive.RoomStats{RoomID: 1}, Events: []archive.Event{event}, Enabled: true}); err != nil {
		t.Fatal(err)
	}
	var result archive.RoomSnapshot
	if err := sender.messages[0].Decode(&result); err != nil {
		t.Fatal(err)
	}
	if result.Events[0].Game == nil || result.Events[0].Game.GameID != "cs2" || result.Events[0].Metadata != nil {
		t.Fatalf("unexpected public event: %+v", result.Events[0])
	}
}
