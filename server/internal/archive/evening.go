package archive

import (
	"fmt"
	"path"
	"slices"
	"strings"
	"time"
)

// EveningMemory describes one completed gathering, never lifetime totals.
type EveningMemory struct {
	StartedAt       time.Time `json:"started_at"`
	EndedAt         time.Time `json:"ended_at"`
	DurationSeconds int64     `json:"duration_seconds"`
	Games           []string  `json:"games"`
	Movies          []string  `json:"movies"`
}

func (p *Processor) evening(roomID uint64, rt *roomRuntime, ids []string) *Event {
	duration := int64(rt.emptySince.Sub(rt.sessionStarted) / time.Second)
	if len(ids) < 2 || duration < 120 {
		return nil
	}
	games := []string{}
	// Flush ongoing game sessions before selecting this gathering's activities.
	for key, session := range p.games {
		if key.RoomID == roomID {
			p.writeGame(key, session)
		}
	}
	for _, e := range p.state.Events[roomID] {
		if g := e.Game; g != nil && g.StartedAt.Before(rt.emptySince) && g.EndedAt.After(rt.sessionStarted) && !slices.Contains(games, g.Title) && len(games) < 8 {
			games = append(games, g.Title)
		}
	}
	movies := append([]string{}, rt.movies...)
	id := fmt.Sprintf("evening:%d:%d", roomID, rt.sessionStarted.UnixNano())
	return &Event{ID: id, EventType: "evening_recap", Category: Social, RoomID: roomID, ServerID: p.serverID, OccurredAt: rt.emptySince, Participants: ids, Importance: 60, Rarity: Common, RuleVersion: 1, TitleKey: "evening_recap", DescriptionKey: "evening_recap", DedupeKey: id,
		Titles: map[string]string{"zh-CN": "这一晚，我们在一起。", "en-US": "An evening, together."}, Descriptions: map[string]string{"zh-CN": fmt.Sprintf("%d 位朋友，相聚了 %d 分钟。", len(ids), duration/60), "en-US": fmt.Sprintf("%d friends, %d minutes together.", len(ids), duration/60)}, Evening: &EveningMemory{rt.sessionStarted, rt.emptySince, duration, games, movies}}
}
func (p *Processor) observeMovie(a Activity) {
	rt := p.roomRuntime(a.RoomID)
	if rt.sessionStarted.IsZero() || len(rt.participants) < 2 {
		return
	}
	title := strings.TrimSpace(path.Base(strings.ReplaceAll(a.Movie, "\\", "/")))
	if title == "" || title == "." || title == "/" {
		return
	}
	runes := []rune(title)
	if len(runes) > 100 {
		title = string(runes[:100]) + "…"
	}
	if len(rt.movies) < 8 && !slices.Contains(rt.movies, title) {
		rt.movies = append(rt.movies, title)
	}
}
