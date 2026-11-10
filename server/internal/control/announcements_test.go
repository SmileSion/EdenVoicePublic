package control

import (
	"edenvoice/server/internal/administration"
	"edenvoice/server/internal/announcements"
	"edenvoice/server/internal/config"
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/serverstate"
	"io"
	"log/slog"
	"path/filepath"
	"testing"
	"time"
)

func TestAnnouncementPublicationReachesOnlineAndJoiningSnapshots(t *testing.T) {
	state := serverstate.New(config.Default())
	admin, token, err := administration.LoadOrCreate(filepath.Join(t.TempDir(), "admin.json"), state.Snapshot().Channels)
	if err != nil {
		t.Fatal(err)
	}
	if _, err = admin.Admit("master", "key", "Master", time.Now()); err != nil {
		t.Fatal(err)
	}
	if err = admin.ProvisionMaster("master", token); err != nil {
		t.Fatal(err)
	}
	s := NewSecure("", "", state, slog.New(slog.NewTextHandler(io.Discard, nil)), nil, nil, false, admin)
	receiver := &recordingSender{}
	user := state.AddAuthenticatedSession("Friend", "friend", "USER", "", receiver, nil, nil)
	if err = state.Join(user.ID, 1); err != nil {
		t.Fatal(err)
	}
	d, err := s.WebAnnouncements("master", "save", 1, announcements.Content{Title: "First", Body: "Hello"})
	if err != nil {
		t.Fatal(err)
	}
	if s.snapshot().Announcement != nil {
		t.Fatal("draft leaked")
	}
	d, err = s.WebAnnouncements("master", "publish", d.Revision, announcements.Content{})
	if err != nil {
		t.Fatal(err)
	}
	check := func(title string) {
		t.Helper()
		var got protocol.ServerState
		if err := receiver.messages[len(receiver.messages)-1].Decode(&got); err != nil {
			t.Fatal(err)
		}
		fresh := s.snapshot()
		if got.AnnouncementRevision != s.announcements.Snapshot().Revision || fresh.AnnouncementRevision != got.AnnouncementRevision {
			t.Fatal("missing lifecycle revision")
		}
		if title == "" {
			if got.Announcement != nil || fresh.Announcement != nil {
				t.Fatal("withdrawal not propagated")
			}
			return
		}
		if got.Announcement == nil || fresh.Announcement == nil || got.Announcement.Title != title || fresh.Announcement.Title != title {
			t.Fatal("latest not propagated", got.Announcement, fresh.Announcement)
		}
	}
	check("First")
	d, err = s.WebAnnouncements("master", "save", d.Revision, announcements.Content{Title: "Latest", Body: "Only this"})
	if err != nil {
		t.Fatal(err)
	}
	d, err = s.WebAnnouncements("master", "publish", d.Revision, announcements.Content{})
	if err != nil {
		t.Fatal(err)
	}
	check("Latest")
	_, err = s.WebAnnouncements("master", "withdraw", d.Revision, announcements.Content{})
	if err != nil {
		t.Fatal(err)
	}
	check("")
	if state.ChannelOf(user.ID) != 1 {
		t.Fatal("announcement interrupted room membership")
	}
}
