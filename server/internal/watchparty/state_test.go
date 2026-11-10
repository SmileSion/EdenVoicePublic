package watchparty

import (
	"math"
	"sync"
	"testing"
	"time"
)

func TestPlaybackAndIsolation(t *testing.T) {
	m := New()
	now := time.Unix(100, 0)
	s, err := m.Join(1, 10, now)
	if err != nil {
		t.Fatal(err)
	}
	epoch := s.Epoch
	if _, err = m.Control(1, 10, epoch, "play", 0, now, false); err == nil {
		t.Fatal("unready player controlled room")
	}
	f := &File{Name: "movie.mkv", Size: 123, Duration: 7200}
	if _, err = m.UpdateFile(1, 10, epoch, f, true, now); err != nil {
		t.Fatal(err)
	}
	f.Name = "changed"
	s, err = m.Control(1, 10, epoch, "play", 30, now, false)
	if err != nil {
		t.Fatal(err)
	}
	s, err = m.Read(1, 10, epoch, now.Add(2*time.Second))
	if err != nil || s.Position != 32 || s.Paused || s.Members[0].File.Name != "movie.mkv" {
		t.Fatalf("bad playback: %+v %v", s, err)
	}
	s.Members[0].File.Name = "mutated snapshot"
	s, _ = m.Read(1, 10, epoch, now)
	if s.Members[0].File.Name != "movie.mkv" {
		t.Fatal("snapshot aliases state")
	}
	if _, err = m.Control(1, 11, epoch, "pause", 32, now, false); err == nil {
		t.Fatal("outsider controlled room")
	}
	if _, err = m.Read(2, 10, epoch, now); err == nil {
		t.Fatal("cross-room read")
	}
	s, err = m.Control(1, 10, epoch, "pause", 32, now, false)
	if err != nil {
		t.Fatal(err)
	}
	s, _ = m.Read(1, 10, epoch, now.Add(time.Hour))
	if s.Position != 32 || !s.Paused {
		t.Fatal("paused clock advanced")
	}
	if _, ok := m.Leave(1, 10, now); !ok {
		t.Fatal("leave failed")
	}
	s, _ = m.Join(1, 10, now)
	if s.Epoch == epoch {
		t.Fatal("recreated session reused epoch")
	}
	if _, err = m.Read(1, 10, epoch, now); err == nil {
		t.Fatal("stale epoch accepted")
	}
}

func TestInvalidInputDoesNotMutate(t *testing.T) {
	m := New()
	now := time.Now()
	s, _ := m.Join(1, 1, now)
	e := s.Epoch
	for _, name := range []string{"", "../movie.mkv", "C:\\movie.mkv", "bad\nname", "\xff"} {
		if _, err := m.UpdateFile(1, 1, e, &File{Name: name, Duration: 1}, true, now); err == nil {
			t.Fatalf("accepted %q", name)
		}
	}
	s, _ = m.UpdateFile(1, 1, e, &File{Name: "film", Duration: 10}, true, now)
	revision := s.Revision
	for _, p := range []float64{-1, math.NaN(), math.Inf(1), maxDuration + 1} {
		if _, err := m.Control(1, 1, e, "seek", p, now, false); err == nil {
			t.Fatalf("accepted position %v", p)
		}
	}
	if _, err := m.Control(1, 1, e, "run", 0, now, false); err == nil {
		t.Fatal("accepted arbitrary command")
	}
	s, _ = m.Read(1, 1, e, now)
	if s.Revision != revision {
		t.Fatal("invalid request mutated state")
	}
}

func TestConcurrentMembers(t *testing.T) {
	m := New()
	now := time.Now()
	var wg sync.WaitGroup
	for i := uint64(1); i <= 32; i++ {
		wg.Add(1)
		go func(id uint64) {
			defer wg.Done()
			s, err := m.Join(1, id, now)
			if err != nil {
				t.Error(err)
				return
			}
			_, err = m.Read(1, id, s.Epoch, now)
			if err != nil {
				t.Error(err)
			}
		}(i)
	}
	wg.Wait()
	s, _ := m.Join(1, 1, now)
	if len(s.Members) != 32 || s.Revision != 32 {
		t.Fatalf("unexpected members/revision: %+v", s)
	}
}

func TestRoomCloseRequiresHostAndNewRoomHasNewEpoch(t *testing.T) {
	m := New()
	now := time.Now()
	first, _ := m.Join(42, 1, now)
	m.Join(42, 2, now)
	if first.HostID != 1 {
		t.Fatal("creator must be host")
	}
	if _, err := m.Close(42, 2, first.Epoch, now, false); err == nil {
		t.Fatal("non-host closed room")
	}
	ended, err := m.Close(42, 1, first.Epoch, now, false)
	if err != nil || len(ended.Members) != 2 {
		t.Fatal(ended, err)
	}
	if m.Status(42, now).Epoch != 0 {
		t.Fatal("room still open")
	}
	next, _ := m.Join(42, 2, now)
	if next.Epoch == first.Epoch {
		t.Fatal("reopened room reused epoch")
	}
	if _, err := m.Control(42, 2, first.Epoch, "play", 0, now, false); err == nil {
		t.Fatal("stale control accepted")
	}
}

func TestHostTransferAndPlayerReplacementClearsReadiness(t *testing.T) {
	m := New()
	now := time.Now()
	s, _ := m.Join(42, 1, now)
	m.Join(42, 2, now)
	m.UpdateFile(42, 2, s.Epoch, &File{Name: "a.mkv", Duration: 100}, true, now)
	remaining, _ := m.Leave(42, 1, now)
	if remaining.HostID != 2 {
		t.Fatal("host was not transferred")
	}
	s, err := m.Unready(42, 2, s.Epoch, now)
	if err != nil || s.Members[0].Ready || s.Members[0].File != nil {
		t.Fatal(s, err)
	}
	if _, err = m.Control(42, 2, s.Epoch, "play", 0, now, false); err == nil {
		t.Fatal("unready player controlled room")
	}
}

func TestExplicitHostTransferAndModeratorPermissions(t *testing.T) {
	m := New()
	now := time.Now()
	first, _ := m.Join(7, 1, now)
	epoch := first.Epoch
	m.Join(7, 2, now)
	for _, id := range []uint64{1, 2} {
		m.UpdateFile(7, id, epoch, &File{Name: "film", Duration: 100}, true, now)
	}
	if _, err := m.Control(7, 2, epoch, "play", 10, now, false); err == nil {
		t.Fatal("ordinary ready viewer controlled playback")
	}
	if _, err := m.TransferHost(7, 2, epoch, 2, now, false); err == nil {
		t.Fatal("viewer promoted themselves")
	}
	if _, err := m.TransferHost(7, 1, epoch, 99, now, false); err == nil {
		t.Fatal("outsider became host")
	}
	playing, err := m.Control(7, 2, epoch, "play", 10, now, true)
	if err != nil {
		t.Fatal(err)
	}
	transferred, err := m.TransferHost(7, 1, epoch, 2, now.Add(time.Second), false)
	if err != nil || transferred.HostID != 2 || transferred.Position != 11 || transferred.Paused || transferred.Revision != playing.Revision+1 {
		t.Fatal(transferred, err)
	}
	if _, err := m.Close(7, 1, epoch, now, false); err == nil {
		t.Fatal("former host retained close permission")
	}
	if _, err := m.Control(7, 1, epoch, "pause", 11, now, false); err == nil {
		t.Fatal("former host retained playback permission")
	}
	if _, err := m.TransferHost(7, 99, epoch, 1, now, true); err != nil {
		t.Fatal("non-viewer moderator cannot transfer", err)
	}
	if _, err := m.Close(7, 99, epoch+1, now, true); err == nil {
		t.Fatal("moderator bypassed epoch")
	}
	if _, err := m.Close(7, 99, epoch, now, true); err != nil {
		t.Fatal("non-viewer moderator cannot close", err)
	}
}
