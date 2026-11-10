package archive

import (
	"fmt"
	"sort"
	"time"
)

// GameMemory is intentionally small and public; process names and paths never enter archives.
type GameMemory struct {
	GameID          string    `json:"game_id"`
	Title           string    `json:"title"`
	StartedAt       time.Time `json:"started_at"`
	EndedAt         time.Time `json:"ended_at"`
	DurationSeconds int64     `json:"duration_seconds"`
}
type GamePresence struct {
	RoomID                            uint64
	RoomName, MemberID, GameID, Title string
}
type gameSample struct {
	At     time.Time
	People []GamePresence
}
type gameKey struct {
	RoomID uint64
	GameID string
}
type gameSession struct {
	active               bool
	start, last, written time.Time
	seconds              int64
	members              map[string]bool
	eventID              string
	title, roomName      string
}

// ObserveGames is sampled off the media path. A full mailbox drops a sample;
// the processor never turns a long observation gap into claimed play time.
func (c *Collector) ObserveGames(people []GamePresence, at time.Time) {
	select {
	case c.gameQueue <- gameSample{at, people}:
	default:
	}
}
func (p *Processor) applyGames(sample gameSample) {
	if p.games == nil {
		p.games = map[gameKey]*gameSession{}
	}
	groups := map[gameKey]map[string]bool{}
	details := map[gameKey]GamePresence{}
	for _, person := range sample.People {
		if person.RoomID == 0 || person.MemberID == "" || person.GameID == "" {
			continue
		}
		key := gameKey{person.RoomID, person.GameID}
		if groups[key] == nil {
			groups[key] = map[string]bool{}
		}
		groups[key][person.MemberID] = true
		details[key] = person
	}
	for key, session := range p.games {
		if len(groups[key]) < 2 {
			session.active = false
		}
		if !sample.At.After(session.last) {
			continue
		}
		if sample.At.Sub(session.last) > 30*time.Second {
			p.writeGame(key, session)
			delete(p.games, key)
		}
	}
	for key, members := range groups {
		if len(members) < 2 {
			continue
		}
		session := p.games[key]
		if session == nil {
			session = &gameSession{start: sample.At, last: sample.At, members: map[string]bool{}, active: true, title: details[key].Title, roomName: details[key].RoomName}
			p.games[key] = session
		} else {
			elapsed := sample.At.Sub(session.last)
			if elapsed <= 0 {
				continue
			}
			if session.active && elapsed <= 15*time.Second {
				session.seconds += int64(elapsed / time.Second)
			}
			session.last = sample.At
			session.active = true
		}
		for id := range members {
			session.members[id] = true
		}
		if session.seconds >= 120 && (session.written.IsZero() || sample.At.Sub(session.written) >= 30*time.Second) {
			p.writeGame(key, session)
		}
	}
}
func (p *Processor) writeGame(key gameKey, session *gameSession) {
	if session.seconds < 120 {
		return
	}
	p.room(key.RoomID, session.roomName, session.start)
	if session.eventID == "" {
		session.eventID = fmt.Sprintf("game:%d:%s:%d", key.RoomID, key.GameID, session.start.UnixNano())
	}
	ids := make([]string, 0, len(session.members))
	for id := range session.members {
		ids = append(ids, id)
	}
	sort.Strings(ids)
	event := Event{ID: session.eventID, EventType: "shared_game", Category: "game", RoomID: key.RoomID, ServerID: p.serverID, OccurredAt: session.start, Participants: ids, Importance: 55, Rarity: Common, RuleVersion: 1, TitleKey: "shared_game", DescriptionKey: "shared_game", DedupeKey: session.eventID,
		Titles: map[string]string{"zh-CN": "一起玩了 " + session.title, "en-US": "Playing " + session.title + " together"}, Descriptions: map[string]string{"zh-CN": fmt.Sprintf("%d 位朋友，在 %s 一起度过了 %d 分钟。", len(ids), session.roomName, session.seconds/60), "en-US": fmt.Sprintf("%d friends shared %d minutes in %s.", len(ids), session.seconds/60, session.roomName)},
		Game: &GameMemory{key.GameID, session.title, session.start, session.last, session.seconds}}
	events := p.state.Events[key.RoomID]
	for i := range events {
		if events[i].ID == session.eventID {
			events[i] = event
			p.state.Events[key.RoomID] = events
			p.revision++
			session.written = session.last
			return
		}
	}
	p.state.Events[key.RoomID] = append(events, event)
	p.revision++
	session.written = session.last
}
