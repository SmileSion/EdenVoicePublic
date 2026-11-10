package archive

import (
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"testing"
	"time"
)

func TestSelectiveSnapshotsPreserveBeforeAndAfter(t *testing.T) {
	at := time.Date(2026, 9, 4, 12, 0, 0, 0, time.UTC)
	var before RoomStats
	var p *Processor
	checks := 0
	var rules []Rule
	for _, trigger := range []Trigger{TriggerJoin, TriggerVoice, TriggerText, TriggerLeave, TriggerSessionEnd, TriggerTick} {
		rules = append(rules, Rule{ID: string(trigger), Trigger: trigger, Predicate: func(c EvalContext) bool {
			checks++
			if !reflect.DeepEqual(c.Before, before) {
				t.Errorf("%s: previous stats changed during aggregation", c.Trigger)
			}
			if !reflect.DeepEqual(c.After, p.state.Rooms[1]) {
				t.Errorf("%s: current stats do not match aggregate", c.Trigger)
			}
			return false
		}})
	}
	p = NewProcessorWithRules("test", time.Second, persistedState{}, rules)
	p.room(1, "room", at)
	for i, step := range []struct {
		kind   ActivityKind
		member string
	}{
		{Joined, "a"}, {Joined, "b"}, {VoicePacket, "a"}, {VoicePacket, "b"},
		{TextMessage, "a"}, {Tick, ""}, {Left, "a"}, {Left, "b"}, {Tick, ""},
	} {
		before = cloneStats(p.state.Rooms[1])
		p.Apply(Activity{Kind: step.kind, RoomID: 1, MemberID: step.member, At: at.Add(time.Duration(i) * time.Second)})
	}
	if checks != 10 { // Nine activities, plus session_end on the last tick.
		t.Fatalf("rule checks = %d, want 10", checks)
	}
}

func TestSessionSummaryLoadedUpdatedAndRebuilt(t *testing.T) {
	at := time.Date(2026, 9, 4, 12, 0, 0, 0, time.UTC)
	stats := RoomStats{RoomID: 1, CreatedAt: at, Members: map[string]MemberStats{
		"a": {VoiceSessions: 9}, "b": {VoiceSessions: 9},
	}, Pairs: map[string]PairStats{pairKey("a", "b"): {Members: []string{"a", "b"}, Sessions: 9}}}
	p := NewProcessor("test", time.Second, persistedState{Rooms: map[uint64]RoomStats{1: stats}})
	old := p.state.Rooms[1]
	if hasRoomPairSessions(old, 10) || hasRoomMemberSessions(old, 10) || !hasRoomPairSessions(old, 9) {
		t.Fatal("incorrect initial summary")
	}
	for _, id := range []string{"a", "b"} {
		p.Apply(Activity{Kind: Joined, RoomID: 1, MemberID: id, At: at})
	}
	p.Apply(Activity{Kind: VoicePacket, RoomID: 1, MemberID: "a", At: at})
	for _, id := range []string{"a", "b"} {
		p.Apply(Activity{Kind: Left, RoomID: 1, MemberID: id, At: at.Add(time.Minute)})
	}
	p.Apply(Activity{Kind: Tick, RoomID: 1, At: at.Add(time.Minute + time.Second)})
	if old.sessionSummary.maxPairSessions != 9 || old.sessionSummary.maxMemberSessions != 9 {
		t.Fatal("previous immutable summary was modified")
	}
	assertEvent(t, p.state.Events[1], "same_pair_10_sessions")
	assertEvent(t, p.state.Events[1], "little_explorer")
	data, err := jsonMarshal(p.state)
	if err != nil {
		t.Fatal(err)
	}
	var restored persistedState
	if err := jsonUnmarshal(data, &restored); err != nil {
		t.Fatal(err)
	}
	if restored.Rooms[1].sessionSummary != nil {
		t.Fatal("derived summary was persisted")
	}
	reloaded := NewProcessor("test", time.Second, restored)
	if !hasRoomPairSessions(reloaded.state.Rooms[1], 10) || !hasRoomMemberSessions(reloaded.state.Rooms[1], 10) {
		t.Fatal("session summary was not rebuilt on load")
	}
}

func TestCleanFlushSkipsWriteAndFailedFlushRetries(t *testing.T) {
	at := time.Date(2026, 9, 4, 12, 0, 0, 0, time.UTC)
	path := filepath.Join(t.TempDir(), "archive.json")
	c := &Collector{path: path, processor: NewProcessorWithRules("test", time.Second, persistedState{}, nil)}
	c.processor.Apply(Activity{Kind: Created, RoomID: 1, At: at})
	if err := c.flushDirty(); err != nil {
		t.Fatal(err)
	}
	original, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	revision := c.processor.revision
	for i := 1; i <= 10; i++ {
		c.processor.Apply(Activity{Kind: Tick, RoomID: 1, At: at.Add(time.Duration(i) * time.Second)})
		if err := c.flushDirty(); err != nil {
			t.Fatal(err)
		}
	}
	current, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	if c.processor.revision != revision || !os.SameFile(original, current) || original.ModTime() != current.ModTime() {
		t.Fatal("idle ticking dirtied or rewrote the archive")
	}
	c.processor.Apply(Activity{Kind: TextMessage, RoomID: 1, At: at.Add(time.Minute)})
	c.path = filepath.Join(path, "blocked.json") // Parent is a file, not a directory.
	if err := c.flushDirty(); err == nil {
		t.Fatal("expected persistence failure")
	}
	if c.persistedRevision != revision {
		t.Fatal("failed write marked the state clean")
	}
	c.path = path
	if err := c.flushDirty(); err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var restored persistedState
	if err := jsonUnmarshal(data, &restored); err != nil {
		t.Fatal(err)
	}
	if restored.Rooms[1].TextMessageCount != 1 || c.persistedRevision != c.processor.revision {
		t.Fatal("retry did not persist the latest state")
	}
}

func TestTimerEventsAndSessionCompletionDirtyArchive(t *testing.T) {
	at := time.Date(2026, 9, 4, 12, 0, 0, 0, time.UTC)
	p := NewProcessor("test", time.Second, persistedState{})
	p.Apply(Activity{Kind: Created, RoomID: 1, At: at})
	revision := p.revision
	p.Snapshot(1, true, 100, at.Add(24*time.Hour))
	if p.revision == revision {
		t.Fatal("snapshot-generated anniversary was not marked dirty")
	}
	assertEvent(t, p.state.Events[1], "room_age_1d")
	p = NewProcessorWithRules("test", time.Second, persistedState{}, nil)
	p.Apply(Activity{Kind: Joined, RoomID: 1, MemberID: "a", At: at})
	p.Apply(Activity{Kind: VoicePacket, RoomID: 1, MemberID: "a", At: at})
	p.Apply(Activity{Kind: Left, RoomID: 1, MemberID: "a", At: at.Add(time.Minute)})
	revision = p.revision
	p.Apply(Activity{Kind: Tick, RoomID: 1, At: at.Add(2 * time.Minute)})
	if p.revision == revision || p.state.Rooms[1].SessionCount != 1 {
		t.Fatal("session completion without an event was not marked dirty")
	}
}

func BenchmarkCleanFlush(b *testing.B) {
	p := NewProcessorWithRules("bench", time.Second, persistedState{}, nil)
	c := &Collector{processor: p, persistedRevision: p.revision}
	b.ReportAllocs()
	for i := 0; i < b.N; i++ {
		if err := c.flushDirty(); err != nil {
			b.Fatal(err)
		}
	}
}

// Include historical relationships: online count alone hides snapshot costs.
func BenchmarkProcessorHistory(b *testing.B) {
	for _, members := range []int{10, 100, 500} {
		for _, kind := range []ActivityKind{VoicePacket, Tick} {
			b.Run(fmt.Sprintf("%s/members_%d", kind, members), func(b *testing.B) {
				at := time.Date(2026, 9, 4, 12, 0, 0, 0, time.UTC)
				p := NewProcessor("bench", time.Second, persistedState{})
				stats := p.room(1, "room", at)
				ids := make([]string, members)
				for i := range ids {
					ids[i] = fmt.Sprintf("member_%d", i)
					stats.Members[ids[i]] = MemberStats{FirstSeenAt: at}
					for j := 0; j < i; j++ {
						stats.Pairs[pairKey(ids[j], ids[i])] = PairStats{Members: []string{ids[j], ids[i]}, Sessions: 1}
					}
				}
				p.state.Rooms[1] = stats
				p = NewProcessor("bench", time.Second, p.state)
				p.Apply(Activity{Kind: Joined, RoomID: 1, MemberID: ids[0], At: at})
				p.Apply(Activity{Kind: VoicePacket, RoomID: 1, MemberID: ids[0], At: at})
				b.ReportAllocs()
				b.ResetTimer()
				for i := 0; i < b.N; i++ {
					p.Apply(Activity{Kind: kind, RoomID: 1, MemberID: ids[0], At: at.Add(time.Duration(i+1) * time.Second)})
				}
			})
		}
	}
}
