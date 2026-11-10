package serverstate

import (
	"edenvoice/server/internal/protocol"
	"testing"
	"time"
)

func TestGameActivityLifecycle(t *testing.T) {
	s := newTestState()
	u := s.AddSession("Alice", nil)
	now := time.Now()
	if changed, err := s.SetGameActivity(u.ID, "unknown.exe", now); err == nil || changed {
		t.Fatal("unknown game accepted")
	}
	if changed, err := s.SetGameActivity(u.ID, "cs2", now); err != nil || !changed {
		t.Fatal(err)
	}
	user, _ := s.User(u.ID)
	if user.GameActivity.Title != "Counter-Strike 2" || user.GameActivity.ExpiresAt != now.Add(time.Minute).UnixMilli() {
		t.Fatal(user.GameActivity)
	}
	if changed, _ := s.SetGameActivity(u.ID, "dota2", now.Add(time.Second)); changed {
		t.Fatal("rate limit bypassed")
	}
	if changed, _ := s.SetGameActivity(u.ID, "", now.Add(time.Second)); !changed {
		t.Fatal("clearing was rate limited")
	}
	user, _ = s.User(u.ID)
	if user.GameActivity != nil {
		t.Fatal("clear did not remove activity")
	}
	s.SetGameActivity(u.ID, "dota2", now.Add(4*time.Second))
	if len(s.ExpireGameActivities(now.Add(63*time.Second))) != 0 {
		t.Fatal("expired too early")
	}
	if ids := s.ExpireGameActivities(now.Add(64 * time.Second)); len(ids) != 1 || ids[0] != u.ID {
		t.Fatal(ids)
	}
	if user, _ = s.User(u.ID); user.GameActivity != nil {
		t.Fatal("stale activity")
	}
}

func TestAutomaticScenesMajorityStabilityAndExpiry(t *testing.T) {
	s := newTestState()
	now := time.Unix(1000, 0)
	ids := []uint64{}
	for i := 0; i < 4; i++ {
		u := s.AddSession("member", nil)
		_ = s.Join(u.ID, 1)
		ids = append(ids, u.ID)
	}
	for _, id := range ids[:2] {
		s.SetGameActivity(id, "cs2", now)
	}
	s.ReconcileGameScenes(now)
	s.ReconcileGameScenes(now.Add(16 * time.Second))
	if len(s.Snapshot().RoomScenes) != 0 {
		t.Fatal("half is not majority")
	}
	s.SetGameActivity(ids[2], "cs2", now.Add(16*time.Second))
	s.ReconcileGameScenes(now.Add(16 * time.Second))
	if s.ReconcileGameScenes(now.Add(30 * time.Second)) {
		t.Fatal("entered early")
	}
	if !s.ReconcileGameScenes(now.Add(31*time.Second)) || s.Snapshot().RoomScenes[1].GameID != "cs2" {
		t.Fatal("did not enter")
	}
	s.SetGameActivity(ids[2], "", now.Add(32*time.Second))
	s.ReconcileGameScenes(now.Add(32 * time.Second))
	s.ReconcileGameScenes(now.Add(50 * time.Second))
	if s.Snapshot().RoomScenes[1].GameID != "cs2" {
		t.Fatal("left early")
	}
	s.SetGameActivity(ids[2], "cs2", now.Add(51*time.Second))
	s.ReconcileGameScenes(now.Add(51 * time.Second))
	if s.ReconcileGameScenes(now.Add(59 * time.Second)) {
		t.Fatal("jitter")
	}
	s.ReconcileGameScenes(now.Add(60 * time.Second))
	if s.ReconcileGameScenes(now.Add(89 * time.Second)) {
		t.Fatal("expiry grace too short")
	}
	if !s.ReconcileGameScenes(now.Add(90*time.Second)) || len(s.Snapshot().RoomScenes) != 0 {
		t.Fatal("expiry did not restore")
	}
}
func TestCatalogDisableAndRename(t *testing.T) {
	s := newTestState()
	u := s.AddSession("member", nil)
	s.SetGameActivity(u.ID, "cs2", time.Now())
	g := protocol.DefaultGames()
	g[0].Title = "renamed"
	s.ApplyGameCatalog(protocol.GameCatalog{Version: 2, Games: g})
	user, _ := s.User(u.ID)
	if user.GameActivity.Title != "renamed" {
		t.Fatal(user)
	}
	g = protocol.DefaultGames()
	g[0].Enabled = false
	s.ApplyGameCatalog(protocol.GameCatalog{Version: 3, Games: g})
	user, _ = s.User(u.ID)
	if user.GameActivity != nil {
		t.Fatal("disabled activity remains")
	}
	if _, e := s.SetGameActivity(u.ID, "cs2", time.Now().Add(10*time.Second)); e == nil {
		t.Fatal("disabled accepted")
	}
}
func TestOnePlayerNeverChangesScene(t *testing.T) {
	s := newTestState()
	u := s.AddSession("solo", nil)
	s.Join(u.ID, 1)
	now := time.Now()
	s.SetGameActivity(u.ID, "cs2", now)
	s.ReconcileGameScenes(now)
	s.ReconcileGameScenes(now.Add(16 * time.Second))
	if len(s.Snapshot().RoomScenes) != 0 {
		t.Fatal("solo scene")
	}
}
