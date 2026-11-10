package control

import (
	"edenvoice/server/internal/administration"
	"encoding/json"
	"io"
	"log/slog"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"edenvoice/server/internal/config"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/serverstate"
	"edenvoice/server/internal/watchparty"
)

func TestWatchControlMembershipAndLifecycle(t *testing.T) {
	state := serverstate.New(config.Default())
	s := New("", "", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	a, b := &recordingSender{}, &recordingSender{}
	u := state.AddAuthenticatedSession("Alice", "alice", "", "", a, nil, nil)
	v := state.AddAuthenticatedSession("Bob", "bob", "", "", b, nil, nil)
	channel := state.Snapshot().Channels[0].ID
	if err := state.Join(u.ID, channel); err != nil {
		t.Fatal(err)
	}
	if err := state.Join(v.ID, channel); err != nil {
		t.Fatal(err)
	}
	send := func(id uint64, target *recordingSender, r protocol.WatchRequest) protocol.WatchState {
		t.Helper()
		// Tests advance the rate-limit clock without sleeping.
		delete(s.watchLastRequest, id)
		e, _ := protocol.NewEnvelope(protocol.TypeWatchRequest, r)
		if err := s.dispatch(id, e, target); err != nil {
			t.Fatal(err)
		}
		var response protocol.WatchState
		if err := target.messages[len(target.messages)-1].Decode(&response); err != nil {
			t.Fatal(err)
		}
		return response
	}
	r := send(u.ID, a, protocol.WatchRequest{ChannelID: channel, Action: "join", RequestID: 1})
	if r.Error != "" || r.Epoch == 0 || r.RequestID != 1 {
		t.Fatalf("join: %+v", r)
	}
	if len(b.messages) != 0 {
		t.Fatal("non-viewer received watch state")
	}
	epoch := r.Epoch
	r = send(v.ID, b, protocol.WatchRequest{ChannelID: channel, Epoch: epoch, Action: "pause"})
	if r.Error == "" {
		t.Fatal("non-viewer controlled playback")
	}
	r = send(u.ID, a, protocol.WatchRequest{ChannelID: channel + 1000, Epoch: epoch, Action: "sync"})
	if r.Error == "" {
		t.Fatal("cross-room request accepted")
	}
	send(v.ID, b, protocol.WatchRequest{ChannelID: channel, Action: "join"})
	send(u.ID, a, protocol.WatchRequest{ChannelID: channel, Epoch: epoch, Action: "file", File: &watchparty.File{Name: "movie.mkv", Duration: 100}, Ready: true})
	r = send(u.ID, a, protocol.WatchRequest{ChannelID: channel, Epoch: epoch, Action: "play", Position: 10})
	if r.Error != "" || r.Paused {
		t.Fatalf("play: %+v", r)
	}
	var received protocol.WatchState
	if err := b.messages[len(b.messages)-1].Decode(&received); err != nil {
		t.Fatal(err)
	}
	if received.Revision != r.Revision || received.ActorID != u.ID {
		t.Fatal("broadcast mismatch")
	}
	if err := state.Leave(u.ID); err != nil {
		t.Fatal(err)
	}
	s.broadcastMove(u.ID, 0)
	if _, ok := s.watchMembers[u.ID]; ok {
		t.Fatal("move retained watcher")
	}
	s.disconnect(v.ID, false)
	if len(s.watchMembers) != 0 || len(s.watchLastRequest) != 0 {
		t.Fatal("disconnect leaked watchers")
	}
	if _, err := s.watch.Read(channel, v.ID, epoch, time.Now()); err == nil {
		t.Fatal("empty room retained")
	}
}

func TestWatchMoveNotificationEndsDepartingViewer(t *testing.T) {
	state := serverstate.New(config.Default())
	s := New("", "", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	sender := &recordingSender{}
	u := state.AddAuthenticatedSession("Alice", "alice", "", "", sender, nil, nil)
	channel := state.Snapshot().Channels[0].ID
	if err := state.Join(u.ID, channel); err != nil {
		t.Fatal(err)
	}
	e, _ := protocol.NewEnvelope(protocol.TypeWatchRequest, protocol.WatchRequest{ChannelID: channel, Action: "join", RequestID: 1})
	if err := s.dispatch(u.ID, e, sender); err != nil {
		t.Fatal(err)
	}
	var joined protocol.WatchState
	if err := sender.messages[len(sender.messages)-1].Decode(&joined); err != nil {
		t.Fatal(err)
	}
	if err := state.Leave(u.ID); err != nil {
		t.Fatal(err)
	}
	s.broadcastMove(u.ID, 0)
	var ended protocol.WatchState
	if err := sender.messages[len(sender.messages)-2].Decode(&ended); err != nil {
		t.Fatal(err)
	}
	if ended.Action != "ended" || ended.Epoch != joined.Epoch || ended.ActorID != u.ID {
		t.Fatalf("bad lifecycle notification: %+v", ended)
	}
}

func TestWatchCancelledJoinCanLeaveWithoutEpoch(t *testing.T) {
	state := serverstate.New(config.Default())
	s := New("", "", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	sender := &recordingSender{}
	u := state.AddAuthenticatedSession("Alice", "alice", "", "", sender, nil, nil)
	channel := state.Snapshot().Channels[0].ID
	_ = state.Join(u.ID, channel)
	for _, request := range []protocol.WatchRequest{{ChannelID: channel, Action: "join", RequestID: 1}, {ChannelID: channel, Action: "leave", RequestID: 2}} {
		e, _ := protocol.NewEnvelope(protocol.TypeWatchRequest, request)
		if err := s.dispatch(u.ID, e, sender); err != nil {
			t.Fatal(err)
		}
	}
	if _, ok := s.watchMembers[u.ID]; ok {
		t.Fatal("cancelled join leaked viewer")
	}
}

func TestWatchMaximumSnapshotFitsControlFrame(t *testing.T) {
	m := watchparty.New()
	now := time.Now()
	var snapshot watchparty.Snapshot
	for id := uint64(1); id <= 64; id++ {
		s, err := m.Join(1, id, now)
		if err != nil {
			t.Fatal(err)
		}
		snapshot, err = m.UpdateFile(1, id, s.Epoch, &watchparty.File{Name: strings.Repeat("<", 128), Size: ^uint64(0), Duration: 604800}, true, now)
		if err != nil {
			t.Fatal(err)
		}
	}
	e, err := protocol.NewEnvelope(protocol.TypeWatchState, protocol.WatchState{Snapshot: snapshot})
	if err != nil {
		t.Fatal(err)
	}
	data, err := json.Marshal(e)
	if err != nil || len(data)+1 > protocol.MaxControlMessageBytes {
		t.Fatalf("oversized snapshot: %d %v", len(data), err)
	}
	if _, err := m.Join(1, 65, now); err == nil {
		t.Fatal("unbounded viewers")
	}
}

func TestWatchRoomStatusCloseAndReopen(t *testing.T) {
	state := serverstate.New(config.Default())
	s := New("", "", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
	a, b := &recordingSender{}, &recordingSender{}
	u := state.AddAuthenticatedSession("host", "host", "", "", a, nil, nil)
	v := state.AddAuthenticatedSession("guest", "guest", "", "", b, nil, nil)
	channel := state.Snapshot().Channels[0].ID
	state.Join(u.ID, channel)
	state.Join(v.ID, channel)
	send := func(id uint64, out *recordingSender, action string, epoch uint64) protocol.WatchState {
		t.Helper()
		delete(s.watchLastRequest, id)
		e, _ := protocol.NewEnvelope(protocol.TypeWatchRequest, protocol.WatchRequest{ChannelID: channel, Epoch: epoch, Action: action, RequestID: 1})
		if err := s.dispatch(id, e, out); err != nil {
			t.Fatal(err)
		}
		var r protocol.WatchState
		out.messages[len(out.messages)-1].Decode(&r)
		return r
	}
	if r := send(v.ID, b, "status", 0); r.Epoch != 0 || len(s.watchMembers) != 0 || len(s.watchLastRequest) != 0 {
		t.Fatal("status allocated membership", r)
	}
	opened := send(u.ID, a, "open", 0)
	if opened.Error != "" || opened.HostID != u.ID {
		t.Fatal(opened)
	}
	visible := send(v.ID, b, "status", 0)
	if visible.Epoch != opened.Epoch || len(visible.Members) != 1 {
		t.Fatal(visible)
	}
	send(v.ID, b, "join", 0)
	if r := send(v.ID, b, "close", opened.Epoch); r.Error == "" {
		t.Fatal("guest closed room")
	}
	ended := send(u.ID, a, "close", opened.Epoch)
	if ended.Action != "closed" || len(s.watchMembers) != 0 || len(s.watchLastRequest) != 0 {
		t.Fatal(ended)
	}
	var received protocol.WatchState
	b.messages[len(b.messages)-1].Decode(&received)
	if received.Action != "closed" || received.Epoch != opened.Epoch {
		t.Fatal("guest not notified", received)
	}
	if send(v.ID, b, "status", 0).Epoch != 0 {
		t.Fatal("closed room visible")
	}
	reopened := send(v.ID, b, "open", 0)
	if reopened.Epoch == opened.Epoch || reopened.HostID != v.ID {
		t.Fatal("reopen failed", reopened)
	}
}

func TestWatchModerationRolesAndNonParticipantClose(t *testing.T) {
	for _, role := range []string{"master", "admin", "channel-admin", "other-channel-admin", "user"} {
		t.Run(role, func(t *testing.T) {
			state := serverstate.New(config.Default())
			channels := state.Snapshot().Channels
			channel := channels[0].ID
			store, token, err := administration.LoadOrCreate(filepath.Join(t.TempDir(), "admin.json"), channels)
			if err != nil {
				t.Fatal(err)
			}
			for _, id := range []string{"host", "viewer", "actor"} {
				if _, err = store.Admit(id, id+"-key", id, time.Now()); err != nil {
					t.Fatal(err)
				}
			}
			switch role {
			case "master":
				err = store.ProvisionMaster("actor", token)
			case "admin":
				err = store.SetServerRole("actor", administration.RoleAdmin)
			case "channel-admin":
				err = store.SetChannelRole(channel, "actor", administration.RoleChannelAdmin)
			case "other-channel-admin":
				err = store.SetChannelRole(channels[1].ID, "actor", administration.RoleChannelAdmin)
			}
			if err != nil {
				t.Fatal(err)
			}
			s := New("", "", state, slog.New(slog.NewTextHandler(io.Discard, nil)))
			s.administration = store
			senders := map[string]*recordingSender{}
			ids := map[string]uint64{}
			for _, name := range []string{"host", "viewer", "actor"} {
				sender := &recordingSender{}
				senders[name] = sender
				u := state.AddAuthenticatedSession(name, name, "USER", "", sender, nil, nil)
				ids[name] = u.ID
				if err = state.Join(u.ID, channel); err != nil {
					t.Fatal(err)
				}
			}
			send := func(name, action string, epoch, target uint64) protocol.WatchState {
				t.Helper()
				delete(s.watchLastRequest, ids[name])
				r := protocol.WatchRequest{ChannelID: channel, Epoch: epoch, Action: action, TargetSessionID: target, RequestID: 12}
				e, _ := protocol.NewEnvelope(protocol.TypeWatchRequest, r)
				if err := s.dispatch(ids[name], e, senders[name]); err != nil {
					t.Fatal(err)
				}
				var result protocol.WatchState
				messages := senders[name].messages
				if err := messages[len(messages)-1].Decode(&result); err != nil {
					t.Fatal(err)
				}
				return result
			}
			first := send("host", "open", 0, 0)
			send("viewer", "join", 0, 0)
			allowed := role == "master" || role == "admin" || role == "channel-admin"
			status := send("actor", "status", 0, 0)
			if status.Permissions == nil || status.Permissions.CanClose != allowed || status.Permissions.CanControl {
				t.Fatalf("wrong nonparticipant permissions: %+v", status.Permissions)
			}
			if _, ok := s.watchMembers[ids["actor"]]; ok {
				t.Fatal("status joined actor to watching")
			}
			if send("actor", "transfer_host", first.Epoch, ids["actor"]).Error == "" {
				t.Fatal("non-viewer became host")
			}
			if err := state.Join(ids["actor"], channels[1].ID); err != nil {
				t.Fatal(err)
			}
			wrongChannel := send("actor", "close", first.Epoch, 0)
			if wrongChannel.Error == "" || len(wrongChannel.Members) != 0 || wrongChannel.Permissions.CanClose {
				t.Fatal("cross-channel close or metadata access allowed")
			}
			if err := state.Join(ids["actor"], channel); err != nil {
				t.Fatal(err)
			}
			transfer := send("actor", "transfer_host", first.Epoch, ids["viewer"])
			if (transfer.Error == "") != allowed {
				t.Fatalf("transfer: %+v", transfer)
			}
			if allowed {
				var viewer protocol.WatchState
				msgs := senders["viewer"].messages
				msgs[len(msgs)-1].Decode(&viewer)
				if viewer.HostID != ids["viewer"] || !viewer.Permissions.CanControl || !viewer.Permissions.CanClose {
					t.Fatal("new host did not gain controls", viewer)
				}
				var host protocol.WatchState
				msgs = senders["host"].messages
				msgs[len(msgs)-1].Decode(&host)
				if host.Permissions.CanClose || host.Permissions.CanControl {
					t.Fatal("former host kept controls")
				}
			}
			stale := send("actor", "close", first.Epoch+1, 0)
			if stale.Error == "" || s.watch.Status(channel, time.Now()).Epoch == 0 {
				t.Fatal("stale close changed room")
			}
			result := send("actor", "close", first.Epoch, 0)
			if (result.Error == "") != allowed {
				t.Fatalf("close: %+v", result)
			}
			if allowed {
				if result.Action != "closed" || result.Permissions.CanClose || len(s.watchMembers) != 0 || len(s.watchLastRequest) != 0 {
					t.Fatal("close failed to clean membership", result)
				}
				for _, name := range []string{"host", "viewer"} {
					var r protocol.WatchState
					msgs := senders[name].messages
					msgs[len(msgs)-1].Decode(&r)
					if r.Action != "closed" {
						t.Fatal("viewer missed close", name)
					}
				}
			} else if s.watch.Status(channel, time.Now()).Epoch != first.Epoch {
				t.Fatal("denied close changed room")
			}
			if _, ok := s.watchLastRequest[ids["actor"]]; ok {
				t.Fatal("moderation leaked rate-limit entry")
			}
			// Permissions are re-evaluated after role revocation, not cached in session.
			if role == "admin" {
				send("host", "open", 0, 0)
				if err := store.SetServerRole("actor", administration.RoleUser); err != nil {
					t.Fatal(err)
				}
				r := send("actor", "status", 0, 0)
				if r.Permissions.CanClose {
					t.Fatal("revoked admin retained rights")
				}
				if send("actor", "close", r.Epoch, 0).Error == "" {
					t.Fatal("revoked admin closed room")
				}
			}
		})
	}
}
