package control

import (
	"edenvoice/server/internal/administration"
	"edenvoice/server/internal/config"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/serverstate"
	"io"
	"log/slog"
	"path/filepath"
	"testing"
	"time"
)

func TestPasswordGateCannotBeBypassedByNormalJoin(t *testing.T) {
	state := serverstate.New(config.Default())
	store, _, err := administration.LoadOrCreate(filepath.Join(t.TempDir(), "admin.json"), state.Snapshot().Channels)
	if err != nil {
		t.Fatal(err)
	}
	store.Admit("member", "key", "member", time.Now())
	if err = store.SetChannelPassword(1, "room-pass"); err != nil {
		t.Fatal(err)
	}
	s := NewSecure("", "", state, slog.New(slog.NewTextHandler(io.Discard, nil)), nil, nil, false, store)
	sender := &recordingSender{}
	user := state.AddAuthenticatedSession("member", "member", "USER", "", sender, nil, nil)
	for _, pass := range []string{"", "wrong"} {
		req, _ := protocol.NewEnvelope(protocol.TypeJoinChannel, protocol.ChannelRequest{ChannelID: 1, Password: pass})
		if s.dispatch(user.ID, req, sender) == nil || state.ChannelOf(user.ID) != 0 {
			t.Fatal("join bypassed password")
		}
	}
	req, _ := protocol.NewEnvelope(protocol.MessageType("set_channel_password"), protocol.ChannelRequest{ChannelID: 1})
	if s.dispatch(user.ID, req, sender) == nil {
		t.Fatal("ordinary user cleared password")
	}
	req, _ = protocol.NewEnvelope(protocol.TypeJoinChannel, protocol.ChannelRequest{ChannelID: 1, Password: "room-pass"})
	if err = s.dispatch(user.ID, req, sender); err != nil {
		t.Fatal(err)
	}
	if state.ChannelOf(user.ID) != 1 {
		t.Fatal("correct password failed")
	}
	if len(s.snapshot().LockedChannelIDs) != 1 {
		t.Fatal("missing lock hint")
	}
}
