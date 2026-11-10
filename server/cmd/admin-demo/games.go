package main

import (
	"edenvoice/server/internal/protocol"
	"edenvoice/server/internal/serverstate"
	"encoding/json"
	"net/http"
	"strconv"
	"sync"
	"time"
)

// These simulated-member endpoints exist only in the isolated demo executable.
func gameDemoHandler(next http.Handler, state *serverstate.State, ids []uint64) http.Handler {
	var mu sync.Mutex
	now := time.Now()
	playing := map[uint64]string{}
	for _, id := range ids {
		_ = state.Join(id, 2)
	}
	refresh := func() {
		for id, game := range playing {
			_, _ = state.SetGameActivity(id, game, now)
		}
		state.ReconcileGameScenes(now)
	}
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/demo/state" && r.URL.Path != "/demo/change" {
			next.ServeHTTP(w, r)
			return
		}
		if r.Host != "127.0.0.1:9181" {
			http.Error(w, "invalid host", 403)
			return
		}
		mu.Lock()
		defer mu.Unlock()
		if r.URL.Path == "/demo/change" {
			if r.Method != "POST" || r.Header.Get("Origin") != "http://127.0.0.1:9181" {
				http.Error(w, "invalid origin or method", 403)
				return
			}
			var q struct {
				ID      uint64 `json:"id,string"`
				Game    string `json:"game"`
				Advance int    `json:"advance"`
			}
			if json.NewDecoder(http.MaxBytesReader(w, r.Body, 1024)).Decode(&q) != nil {
				http.Error(w, "invalid request", 400)
				return
			}
			if q.Advance > 0 && q.Advance <= 30 {
				now = now.Add(time.Duration(q.Advance) * time.Second)
			} else {
				found := false
				for _, id := range ids {
					if id == q.ID {
						found = true
					}
				}
				if !found {
					http.Error(w, "unknown demo member", 400)
					return
				}
				if _, e := state.SetGameActivity(q.ID, q.Game, now.Add(3*time.Second)); e != nil {
					http.Error(w, e.Error(), 400)
					return
				}
				playing[q.ID] = q.Game
			}
		}
		refresh()
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(func() map[string]any {
			snapshot := state.Snapshot()
			users := []any{}
			for _, u := range snapshot.Users {
				users = append(users, struct {
					protocol.User
					ID string `json:"session_id"`
				}{u, strconv.FormatUint(u.SessionID, 10)})
			}
			return map[string]any{"state": map[string]any{"users": users, "channels": snapshot.Channels, "room_scenes": snapshot.RoomScenes, "game_catalog_version": snapshot.GameCatalogVersion}, "catalog": state.GameCatalog(), "now": now.UnixMilli()}
		}())
	})
}
