package archive

import (
	"testing"
	"time"
)

func TestSharedGameThresholdDedupAndRoomIsolation(t *testing.T) {
	p := NewProcessor("test", time.Second, persistedState{})
	start := time.Date(2026, 9, 14, 20, 0, 0, 0, time.UTC)
	people := []GamePresence{{1, "Room", "a", "cs2", "Counter-Strike 2"}, {1, "Room", "b", "cs2", "Counter-Strike 2"}, {2, "Other", "c", "cs2", "Counter-Strike 2"}}
	for sec := 0; sec < 120; sec += 5 {
		p.applyGames(gameSample{start.Add(time.Duration(sec) * time.Second), people})
	}
	if len(p.state.Events[1]) != 0 {
		t.Fatal("short launch created a memory")
	}
	for sec := 120; sec <= 300; sec += 5 {
		p.applyGames(gameSample{start.Add(time.Duration(sec) * time.Second), people})
	}
	events := p.state.Events[1]
	if len(events) != 1 || events[0].Game.DurationSeconds != 300 || len(events[0].Participants) != 2 {
		t.Fatalf("unexpected game memory: %+v", events)
	}
	if len(p.state.Events[2]) != 0 {
		t.Fatal("solo session created a memory")
	}
	// Persisted data round-trips and is still visible in highlights.
	data, err := jsonMarshal(p.state)
	if err != nil {
		t.Fatal(err)
	}
	var restored persistedState
	if err = jsonUnmarshal(data, &restored); err != nil {
		t.Fatal(err)
	}
	loaded := NewProcessor("test", time.Second, restored)
	found := false
	for _, event := range loaded.Snapshot(1, false, 100, start.Add(310*time.Second)).Events {
		if event.Game != nil {
			found = true
			if event.Game.Title != "Counter-Strike 2" {
				t.Fatal(event)
			}
		}
	}
	if !found {
		t.Fatal("lost game memory after reload")
	}
}
func TestSharedGameDuplicateIdentityAndGaps(t *testing.T) {
	p := NewProcessor("test", time.Second, persistedState{})
	start := time.Now()
	one := GamePresence{1, "Room", "a", "cs2", "CS2"}
	two := GamePresence{1, "Room", "b", "cs2", "CS2"}
	for sec := 0; sec <= 150; sec += 5 {
		p.applyGames(gameSample{start.Add(time.Duration(sec) * time.Second), []GamePresence{one, one}})
	}
	if len(p.games) != 0 {
		t.Fatal("same identity counted as two people")
	}
	for sec := 200; sec <= 330; sec += 5 {
		p.applyGames(gameSample{start.Add(time.Duration(sec) * time.Second), []GamePresence{one, two}})
	}
	p.applyGames(gameSample{start.Add(335 * time.Second), nil})
	p.applyGames(gameSample{start.Add(345 * time.Second), []GamePresence{one, two}})
	p.applyGames(gameSample{start.Add(350 * time.Second), []GamePresence{one, two}})
	p.applyGames(gameSample{start.Add(390 * time.Second), nil})
	event := p.state.Events[1][0]
	if event.Game.DurationSeconds != 135 {
		t.Fatalf("absence counted: %d", event.Game.DurationSeconds)
	}
	// Rejoining after grace starts a separate record, never spans offline hours.
	for sec := 400; sec <= 520; sec += 5 {
		p.applyGames(gameSample{start.Add(time.Duration(sec) * time.Second), []GamePresence{one, two}})
	}
	if len(p.state.Events[1]) != 2 {
		t.Fatal("distinct gathering was merged")
	}
}
