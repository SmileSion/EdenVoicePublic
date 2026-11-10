package archive

import (
	"encoding/json"
	"strings"
	"testing"
	"time"
)

func TestEveningKeepsSessionFactsAndDoesNotRepeat(t *testing.T) {
	p := NewProcessor("test", 30*time.Second, persistedState{})
	start := time.Date(2026, 9, 28, 20, 0, 0, 0, time.UTC)
	for _, id := range []string{"alice", "bob"} {
		p.Apply(Activity{Kind: Joined, RoomID: 1, MemberID: id, At: start})
	}
	p.Apply(Activity{Kind: VoicePacket, RoomID: 1, MemberID: "alice", At: start.Add(time.Second)})
	p.Apply(Activity{Kind: "watch_played", RoomID: 1, Movie: `C:\private\movie.mkv`, At: start.Add(time.Minute)})
	p.Apply(Activity{Kind: "watch_played", RoomID: 1, Movie: `C:\private\movie.mkv`, At: start.Add(time.Minute)})
	for i := 0; i <= 24; i++ {
		p.applyGames(gameSample{At: start.Add(time.Duration(i) * 5 * time.Second), People: []GamePresence{{1, "Room", "alice", "game", "Game"}, {1, "Room", "bob", "game", "Game"}}})
	}
	for _, id := range []string{"alice", "bob"} {
		p.Apply(Activity{Kind: Left, RoomID: 1, MemberID: id, At: start.Add(10 * time.Minute)})
	}
	p.Apply(Activity{Kind: Tick, RoomID: 1, At: start.Add(11 * time.Minute)})
	count := 0
	for _, e := range p.state.Events[1] {
		if e.Evening != nil {
			count++
			if e.Evening.DurationSeconds != 600 || len(e.Participants) != 2 || len(e.Evening.Movies) != 1 || e.Evening.Movies[0] != "movie.mkv" || len(e.Evening.Games) != 1 {
				t.Fatal(e.Evening)
			}
		}
	}
	if count != 1 {
		t.Fatal(count)
	}
	p.Apply(Activity{Kind: Tick, RoomID: 1, At: start.Add(12 * time.Minute)})
	b, _ := json.Marshal(p.state)
	if strings.Contains(string(b), "private") {
		t.Fatal("private path leaked")
	}
	var restored persistedState
	if e := json.Unmarshal(b, &restored); e != nil {
		t.Fatal(e)
	}
	q := NewProcessor("test", time.Second, restored)
	count = 0
	for _, e := range q.state.Events[1] {
		if e.Evening != nil {
			count++
		}
	}
	if count != 1 {
		t.Fatal("lost or duplicate recap")
	}
}
func TestSoloAndShortGatheringsDoNotMakeEvening(t *testing.T) {
	for _, members := range []int{1, 2} {
		p := NewProcessor("test", time.Second, persistedState{})
		at := time.Now()
		for i := 0; i < members; i++ {
			p.Apply(Activity{Kind: Joined, RoomID: 1, MemberID: string(rune('a' + i)), At: at})
		}
		p.Apply(Activity{Kind: VoicePacket, RoomID: 1, MemberID: "a", At: at})
		for i := 0; i < members; i++ {
			p.Apply(Activity{Kind: Left, RoomID: 1, MemberID: string(rune('a' + i)), At: at.Add(time.Minute)})
		}
		p.Apply(Activity{Kind: Tick, RoomID: 1, At: at.Add(2 * time.Minute)})
		for _, e := range p.state.Events[1] {
			if e.Evening != nil {
				t.Fatal("short gathering generated recap")
			}
		}
	}
}

func TestQuietMovieStillCreatesRecapWithoutInflatingVoiceStats(t *testing.T) {
	p := NewProcessor("test", time.Second, persistedState{})
	at := time.Now()
	for _, id := range []string{"a", "b"} {
		p.Apply(Activity{Kind: Joined, RoomID: 1, MemberID: id, At: at})
	}
	p.Apply(Activity{Kind: "watch_played", RoomID: 1, Movie: "Movie", At: at.Add(time.Second)})
	for _, id := range []string{"a", "b"} {
		p.Apply(Activity{Kind: Left, RoomID: 1, MemberID: id, At: at.Add(5 * time.Minute)})
	}
	p.Apply(Activity{Kind: Tick, RoomID: 1, At: at.Add(6 * time.Minute)})
	n := 0
	for _, e := range p.state.Events[1] {
		if e.Evening != nil {
			n++
		}
	}
	if n != 1 || p.state.Rooms[1].SessionCount != 0 || p.state.Rooms[1].ActiveVoiceSeconds != 0 {
		t.Fatal("quiet movie missing or voice stats inflated")
	}
}
