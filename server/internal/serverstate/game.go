package serverstate

import (
	"edenvoice/server/internal/protocol"
	"errors"
	"time"
)

// Activity is ephemeral, scoped to the authenticated connection, never persisted.
func (s *State) SetGameActivity(id uint64, gameID string, now time.Time) (bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	title, known := "", false
	for _, g := range s.gameCatalog.Games {
		if g.ID == gameID && g.Enabled {
			title, known = g.Title, true
			break
		}
	}
	if gameID != "" && !known {
		return false, errors.New("unknown or disabled game id")
	}
	session := s.sessions[id]
	if session == nil {
		return false, ErrUnknownSession
	}
	// Clearing is always allowed, even immediately after a previous update.
	if gameID == "" {
		changed := session.GameActivity != nil
		session.GameActivity = nil
		return changed, nil
	}
	if now.Sub(session.GameActivityAt) < 3*time.Second {
		return false, nil
	}
	session.GameActivityAt = now
	session.GameActivity = &protocol.GameActivity{GameID: gameID, Kind: "game", Title: title, Source: "windows", UpdatedAt: now.UnixMilli(), ExpiresAt: now.Add(60 * time.Second).UnixMilli()}
	return true, nil
}
func (s *State) ExpireGameActivities(now time.Time) []uint64 {
	s.mu.Lock()
	defer s.mu.Unlock()
	var ids []uint64
	for id, session := range s.sessions {
		if session.GameActivity != nil && session.GameActivity.ExpiresAt <= now.UnixMilli() {
			session.GameActivity = nil
			ids = append(ids, id)
		}
	}
	return ids
}

// Catalog objects are immutable after publication; snapshots can safely share them.
func (s *State) ApplyGameCatalog(c protocol.GameCatalog) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.gameCatalog = &c
	titles := map[string]string{}
	for _, g := range c.Games {
		if g.Enabled {
			titles[g.ID] = g.Title
		}
	}
	for _, u := range s.sessions {
		if a := u.GameActivity; a != nil {
			if t, ok := titles[a.GameID]; ok {
				copy := *a
				copy.Title = t
				u.GameActivity = &copy
			} else {
				u.GameActivity = nil
			}
		}
	}
	for id, d := range s.scenes {
		if d.current.GameID != "" {
			if t, ok := titles[d.current.GameID]; ok {
				d.current.Title = t
			} else {
				d = sceneDecision{}
			}
			s.scenes[id] = d
		}
	}
}

// One bounded pass per second; no disk, network or process enumeration under mu.
func (s *State) ReconcileGameScenes(now time.Time) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	changed := false
	totals := map[uint64]int{}
	counts := map[uint64]map[string]int{}
	titles := map[string]string{}
	for _, u := range s.sessions {
		if u.ChannelID == 0 {
			continue
		}
		totals[u.ChannelID]++
		if a := u.GameActivity; a != nil && a.ExpiresAt > now.UnixMilli() {
			if counts[u.ChannelID] == nil {
				counts[u.ChannelID] = map[string]int{}
			}
			counts[u.ChannelID][a.GameID]++
			titles[a.GameID] = a.Title
		}
	}
	for id := range s.scenes {
		if _, ok := s.channels[id]; !ok {
			delete(s.scenes, id)
			changed = true
		}
	}
	for id := range s.channels {
		d := s.scenes[id]
		candidate := ""
		for game, n := range counts[id] {
			if n >= 2 && n*2 > totals[id] {
				candidate = game
				break
			}
		}
		if candidate == d.current.GameID {
			d.candidate = ""
			d.since = time.Time{}
		} else {
			if d.since.IsZero() || candidate != d.candidate {
				d.candidate = candidate
				d.since = now
			}
			delay := 15 * time.Second
			if candidate == "" {
				delay = 30 * time.Second
			}
			if now.Sub(d.since) >= delay {
				d.current = protocol.RoomScene{GameID: candidate, Title: titles[candidate]}
				d.since = time.Time{}
				d.candidate = ""
				changed = true
			}
		}
		s.scenes[id] = d
	}
	return changed
}

func (s *State) GameCatalog() protocol.GameCatalog {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return *s.gameCatalog
}
