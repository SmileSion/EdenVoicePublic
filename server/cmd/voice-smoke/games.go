package main

import (
	"edenvoice/server/internal/protocol"
	"errors"
	"time"
)

func verifyGames(a, b *smokeClient, timeout time.Duration) error {
	supported := false
	for _, f := range a.welcome.ServerFeatures {
		if f == "game_catalog_v1" {
			supported = true
		}
	}
	if !supported {
		return errors.New("game catalog feature missing")
	}
	if e := a.send(protocol.TypeGetGameCatalog, struct{}{}); e != nil {
		return e
	}
	deadline := time.Now().Add(timeout)
	var catalog protocol.GameCatalog
	for time.Now().Before(deadline) {
		m, e := a.read(time.Until(deadline))
		if e != nil {
			return e
		}
		if m.Type == protocol.TypeGameCatalog {
			if e = m.Decode(&catalog); e != nil {
				return e
			}
			break
		}
	}
	if catalog.Version == 0 {
		return errors.New("catalog missing")
	}
	if e := protocol.ValidateGames(catalog.Games); e != nil {
		return e
	}
	game := ""
	for _, g := range catalog.Games {
		if g.Enabled {
			game = g.ID
			break
		}
	}
	if game == "" {
		return nil
	}
	for _, id := range []string{game, ""} {
		if e := a.send(protocol.TypeSetGameActivity, protocol.GameActivityRequest{GameID: id}); e != nil {
			return e
		}
		deadline = time.Now().Add(timeout)
		matched := false
		for time.Now().Before(deadline) {
			m, e := b.read(time.Until(deadline))
			if e != nil {
				return e
			}
			if m.Type != protocol.TypeUserStateChanged {
				continue
			}
			var event protocol.UserStateChanged
			if e = m.Decode(&event); e != nil {
				return e
			}
			if event.SessionID != a.welcome.SessionID {
				continue
			}
			if id == "" && event.GameActivity == nil || id != "" && event.GameActivity != nil && event.GameActivity.GameID == id {
				matched = true
				break
			}
		}
		if !matched {
			return errors.New("game activity propagation failed")
		}
	}
	return nil
}
