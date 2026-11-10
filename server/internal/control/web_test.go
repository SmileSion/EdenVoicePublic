package control

import (
	"io"
	"log/slog"
	"path/filepath"
	"testing"
	"time"

	"edenvoice/server/internal/administration"
	"edenvoice/server/internal/config"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/serverstate"
)

func TestWebWatchCloseUsesIdentityAuthorizationAndEpoch(t *testing.T) {
	state := serverstate.New(config.Default())
	store, token, err := administration.LoadOrCreate(filepath.Join(t.TempDir(), "admin.json"), state.Snapshot().Channels)
	if err != nil {
		t.Fatal(err)
	}
	for _, id := range []string{"master", "admin", "viewer"} {
		if _, err = store.Admit(id, id+"-key", id, time.Now()); err != nil {
			t.Fatal(err)
		}
	}
	if err = store.ProvisionMaster("master", token); err != nil {
		t.Fatal(err)
	}
	if err = store.SetServerRole("admin", administration.RoleAdmin); err != nil {
		t.Fatal(err)
	}
	s := NewSecure("", "", state, slog.New(slog.NewTextHandler(io.Discard, nil)), nil, nil, false, store)
	receiver := &recordingSender{}
	viewer := state.AddAuthenticatedSession("Viewer", "viewer", "USER", "", receiver, nil, nil)
	if err = state.Join(viewer.ID, 1); err != nil {
		t.Fatal(err)
	}
	first, err := s.watch.Join(1, viewer.ID, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	s.watchMembers[viewer.ID] = 1
	s.watchLastRequest[viewer.ID] = time.Now()
	if err = s.WebCloseWatch("viewer", 1, first.Epoch); err == nil {
		t.Fatal("ordinary identity closed via web")
	}
	if err = s.WebCloseWatch("admin", 1, first.Epoch+1); err == nil {
		t.Fatal("stale epoch closed room")
	}
	if err = s.WebCloseWatch("admin", 1, first.Epoch); err != nil {
		t.Fatal(err)
	}
	if len(s.watchMembers) != 0 || len(s.watchLastRequest) != 0 {
		t.Fatal("close leaked participants")
	}
	var result protocol.WatchState
	if err = receiver.messages[len(receiver.messages)-1].Decode(&result); err != nil || result.Action != "closed" {
		t.Fatal(result, err)
	}
	expired := time.Now().Add(-time.Hour)
	ban, _ := protocol.NewEnvelope(protocol.TypeBan, protocol.TargetRequest{ClientID: "viewer", Reason: "test", ExpiresAt: &expired})
	if err = s.WebCommand("admin", ban); err == nil {
		t.Fatal("expired ban accepted")
	}
	ban, _ = protocol.NewEnvelope(protocol.TypeBan, protocol.TargetRequest{ClientID: "master", Reason: "test"})
	if err = s.WebCommand("admin", ban); err == nil {
		t.Fatal("admin banned master")
	}
	if state.ChannelOf(viewer.ID) != 1 {
		t.Fatal("closing watching disconnected voice")
	}
}
