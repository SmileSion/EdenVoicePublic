package archive

import (
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"
)

func TestThresholdRulesAreDataDrivenAndDeduplicated(t *testing.T) {
	start := time.Date(2026, 1, 1, 20, 0, 0, 0, time.UTC)
	p := NewProcessor("server", time.Second, persistedState{})
	p.Apply(Activity{Kind: Created, RoomID: 1, RoomName: "Eden", At: start})
	p.Apply(Activity{Kind: Joined, RoomID: 1, MemberID: "a", At: start})
	for second := 0; second <= 3600; second++ {
		p.Apply(Activity{Kind: VoicePacket, RoomID: 1, MemberID: "a", At: start.Add(time.Duration(second) * time.Second)})
	}
	counts := map[string]int{}
	for _, event := range p.state.Events[1] {
		counts[event.EventType]++
	}
	if counts["voice_total_30m"] != 1 || counts["voice_total_1h"] != 1 {
		t.Fatalf("threshold events = %#v", counts)
	}
	if counts["first_voice"] != 1 {
		t.Fatalf("first_voice count = %d", counts["first_voice"])
	}
}

func TestLastTwoAndLastOneRequireFormerGroup(t *testing.T) {
	start := time.Date(2026, 2, 1, 1, 0, 0, 0, time.UTC)
	p := NewProcessor("server", time.Second, persistedState{})
	for _, id := range []string{"a", "b", "c", "d", "e"} {
		p.Apply(Activity{Kind: Joined, RoomID: 2, MemberID: id, At: start})
	}
	for _, id := range []string{"c", "d", "e"} {
		p.Apply(Activity{Kind: Left, RoomID: 2, MemberID: id, At: start.Add(time.Minute)})
	}
	p.Apply(Activity{Kind: Tick, RoomID: 2, At: start.Add(6 * time.Minute)})
	assertEvent(t, p.state.Events[2], "last_two_5m")
	p.Apply(Activity{Kind: Left, RoomID: 2, MemberID: "b", At: start.Add(7 * time.Minute)})
	p.Apply(Activity{Kind: Tick, RoomID: 2, At: start.Add(22 * time.Minute)})
	assertEvent(t, p.state.Events[2], "last_one_15m")
}

func TestIdlePresenceDoesNotCountAsActiveVoice(t *testing.T) {
	start := time.Date(2026, 3, 1, 10, 0, 0, 0, time.UTC)
	p := NewProcessor("server", time.Second, persistedState{})
	p.Apply(Activity{Kind: Joined, RoomID: 3, MemberID: "a", At: start})
	p.Apply(Activity{Kind: Left, RoomID: 3, MemberID: "a", At: start.Add(8 * time.Hour)})
	p.Apply(Activity{Kind: Tick, RoomID: 3, At: start.Add(8*time.Hour + time.Second)})
	stats := p.state.Rooms[3]
	if stats.ActiveVoiceSeconds != 0 {
		t.Fatalf("idle presence counted as voice: %d", stats.ActiveVoiceSeconds)
	}
	if stats.PresenceSeconds != int64(8*time.Hour/time.Second) {
		t.Fatalf("presence = %d", stats.PresenceSeconds)
	}
}

func TestSilenceReturnAndPersistenceRecovery(t *testing.T) {
	start := time.Date(2026, 4, 1, 10, 0, 0, 0, time.UTC)
	dir := t.TempDir()
	path := filepath.Join(dir, "eden-archive.json")
	c, err := LoadOrCreate(path, "server")
	if err != nil {
		t.Fatal(err)
	}
	c.Observe(Activity{Kind: Joined, RoomID: 4, MemberID: "a", At: start})
	c.ObserveVoice(4, "a", start)
	time.Sleep(20 * time.Millisecond)
	c.ObserveVoice(4, "a", start.Add(31*24*time.Hour))
	time.Sleep(20 * time.Millisecond)
	if err := c.Close(); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(path); err != nil {
		t.Fatal(err)
	}
	reloaded, err := LoadOrCreate(path, "server")
	if err != nil {
		t.Fatal(err)
	}
	snapshot := reloaded.Snapshot(4, true, 100)
	if snapshot.Stats.ActiveVoiceSeconds != 2 {
		t.Fatalf("active seconds after reload = %d", snapshot.Stats.ActiveVoiceSeconds)
	}
	assertEvent(t, snapshot.Events, "eden_awakened")
	_ = reloaded.Close()
}

func TestRoomAgeUsesStableDayThreshold(t *testing.T) {
	start := time.Date(2024, 2, 29, 12, 0, 0, 0, time.UTC)
	p := NewProcessor("server", time.Second, persistedState{})
	p.Apply(Activity{Kind: Created, RoomID: 5, At: start})
	p.Apply(Activity{Kind: Tick, RoomID: 5, At: start.Add(7 * 24 * time.Hour)})
	assertEvent(t, p.state.Events[5], "room_age_7d")
}

func TestRuleEngineCanEmitMultipleCrossedMilestones(t *testing.T) {
	engine := NewEngine(DefaultRules())
	start := time.Date(2026, 5, 1, 12, 0, 0, 0, time.UTC)
	before := RoomStats{CreatedAt: start, ActiveVoiceSeconds: 29 * 60}
	after := before
	after.ActiveVoiceSeconds = 3600
	events := engine.Evaluate(6, "server", EvalContext{Trigger: TriggerVoice, Now: start.Add(time.Hour), Before: before, After: after}, map[string]time.Time{})
	assertEvent(t, events, "voice_total_30m")
	assertEvent(t, events, "voice_total_1h")
}

func TestEventsKeepStableTriggeringClientIDs(t *testing.T) {
	start := time.Date(2026, 5, 2, 12, 0, 0, 0, time.UTC)
	p := NewProcessor("server", time.Second, persistedState{})
	p.Apply(Activity{Kind: Joined, RoomID: 14, MemberID: "client-a", At: start})
	p.Apply(Activity{Kind: Joined, RoomID: 14, MemberID: "client-b", At: start})
	events := p.Apply(Activity{Kind: VoicePacket, RoomID: 14, MemberID: "client-a", At: start.Add(time.Second)})
	event := findEvent(events, "first_voice")
	if event == nil {
		t.Fatal("missing first_voice")
	}
	if len(event.TriggeredBy) != 1 || event.TriggeredBy[0] != "client-a" {
		t.Fatalf("triggered_by = %#v", event.TriggeredBy)
	}
	if len(event.Participants) != 2 {
		t.Fatalf("participants = %#v", event.Participants)
	}
}

func TestEdenInspiredRulesUseRuleDataAndActorResolvers(t *testing.T) {
	now := time.Date(2026, 9, 9, 20, 0, 0, 0, time.UTC)
	before := RoomStats{CreatedAt: now.Add(-100 * 24 * time.Hour), ActiveVoiceSeconds: 99*3600 - 1, ActiveDays: map[string]bool{}, Members: map[string]MemberStats{}, Pairs: map[string]PairStats{}}
	after := cloneStats(before)
	after.ActiveVoiceSeconds = 99 * 3600
	voiceEvents := NewEngine(DefaultRules()).Evaluate(15, "server", EvalContext{Trigger: TriggerVoice, Now: now, Before: before, After: after, MemberID: "client-a", Participants: []string{"client-a"}}, map[string]time.Time{})
	assertEvent(t, voiceEvents, "felix_project")
	assertEvent(t, voiceEvents, "under_the_red_star")

	pairStats := cloneStats(after)
	pairStats.Pairs["pair"] = PairStats{Members: []string{"client-a", "client-b"}, Sessions: 99}
	pairStats.Members["client-a"] = MemberStats{VoiceSessions: 10}
	tickEvents := NewEngine(DefaultRules()).Evaluate(16, "server", EvalContext{Trigger: TriggerTick, Now: now, Before: after, After: pairStats, Participants: []string{"client-a", "client-b"}, LastTwoDuration: 99 * time.Minute}, map[string]time.Time{})
	finalStory := findEvent(tickEvents, "the_final_love_story")
	if finalStory == nil || len(finalStory.TriggeredBy) != 2 {
		t.Fatalf("final story actors = %#v", finalStory)
	}
	assertEvent(t, tickEvents, "only_two_on_the_planet")
	assertEvent(t, tickEvents, "little_explorer")
}

func TestCooldownAndClockRollbackDoNotDuplicate(t *testing.T) {
	engine := NewEngine(DefaultRules())
	now := time.Date(2026, 6, 1, 3, 0, 0, 0, time.UTC)
	seen := map[string]time.Time{}
	context := EvalContext{Trigger: TriggerTick, Now: now, Participants: []string{"a", "b"}}
	assertEvent(t, engine.Evaluate(7, "server", context, seen), "still_here_at_3am")
	if events := engine.Evaluate(7, "server", context, seen); len(events) != 0 {
		t.Fatalf("duplicate cooldown events: %#v", events)
	}
	context.Now = now.Add(-time.Hour)
	if events := engine.Evaluate(7, "server", context, seen); hasEvent(events, "still_here_at_3am") {
		t.Fatalf("clock rollback duplicated event: %#v", events)
	}
}

func TestReconnectWithinGraceKeepsOneSession(t *testing.T) {
	start := time.Date(2026, 7, 1, 20, 0, 0, 0, time.UTC)
	p := NewProcessor("server", 30*time.Second, persistedState{})
	p.Apply(Activity{Kind: Joined, RoomID: 8, MemberID: "a", At: start})
	p.Apply(Activity{Kind: VoicePacket, RoomID: 8, MemberID: "a", At: start.Add(time.Second)})
	p.Apply(Activity{Kind: Left, RoomID: 8, MemberID: "a", At: start.Add(time.Minute)})
	p.Apply(Activity{Kind: Joined, RoomID: 8, MemberID: "a", At: start.Add(70 * time.Second)})
	p.Apply(Activity{Kind: Left, RoomID: 8, MemberID: "a", At: start.Add(2 * time.Minute)})
	p.Apply(Activity{Kind: Tick, RoomID: 8, At: start.Add(2*time.Minute + 31*time.Second)})
	if sessions := p.state.Rooms[8].SessionCount; sessions != 1 {
		t.Fatalf("session count = %d", sessions)
	}
}

func TestActiveDayUsesEventTimezone(t *testing.T) {
	zone := time.FixedZone("UTC+8", 8*3600)
	start := time.Date(2026, 8, 1, 23, 59, 59, 0, zone)
	p := NewProcessor("server", time.Second, persistedState{})
	p.Apply(Activity{Kind: VoicePacket, RoomID: 9, MemberID: "a", At: start})
	p.Apply(Activity{Kind: VoicePacket, RoomID: 9, MemberID: "a", At: start.Add(2 * time.Second)})
	if len(p.state.Rooms[9].ActiveDays) != 2 {
		t.Fatalf("active days = %#v", p.state.Rooms[9].ActiveDays)
	}
}

func TestReturningMembersCreateReunionMilestones(t *testing.T) {
	start := time.Date(2026, 8, 1, 20, 0, 0, 0, time.UTC)
	p := NewProcessor("server", time.Second, persistedState{})
	for _, id := range []string{"a", "b", "c"} {
		p.Apply(Activity{Kind: Joined, RoomID: 10, MemberID: id, At: start})
		p.Apply(Activity{Kind: Left, RoomID: 10, MemberID: id, At: start.Add(time.Minute)})
	}
	returnedAt := start.Add(31 * 24 * time.Hour)
	for _, id := range []string{"a", "b", "c"} {
		p.Apply(Activity{Kind: Joined, RoomID: 10, MemberID: id, At: returnedAt})
	}
	assertEvent(t, p.state.Events[10], "reunion")
	assertEvent(t, p.state.Events[10], "old_group_returned")
}

func TestObserveVoiceNeverWaitsForProcessorLock(t *testing.T) {
	c, err := LoadOrCreate(filepath.Join(t.TempDir(), "archive.json"), "server")
	if err != nil {
		t.Fatal(err)
	}
	c.mu.Lock()
	done := make(chan struct{})
	go func() {
		c.ObserveVoice(11, "speaker", time.Now())
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(100 * time.Millisecond):
		c.mu.Unlock()
		t.Fatal("voice observation waited for the processor/persistence lock")
	}
	c.mu.Unlock()
	if err := c.Close(); err != nil {
		t.Fatal(err)
	}
}

func TestObserveVoiceCoalescesConcurrentReportsPerSecond(t *testing.T) {
	c, err := LoadOrCreate(filepath.Join(t.TempDir(), "archive.json"), "server")
	if err != nil {
		t.Fatal(err)
	}
	at := time.Date(2026, 9, 1, 12, 0, 0, 0, time.UTC)
	var group sync.WaitGroup
	for range 64 {
		group.Add(1)
		go func() {
			defer group.Done()
			c.ObserveVoice(12, "speaker", at)
		}()
	}
	group.Wait()
	if err := c.Close(); err != nil {
		t.Fatal(err)
	}
	if seconds := c.processor.state.Rooms[12].ActiveVoiceSeconds; seconds != 1 {
		t.Fatalf("active voice seconds = %d, want 1", seconds)
	}
}

func BenchmarkObserveVoiceDuplicate(b *testing.B) {
	c := &Collector{queue: make(chan Activity, 1)}
	at := time.Date(2026, 9, 1, 12, 0, 0, 0, time.UTC)
	c.ObserveVoice(13, "speaker", at)
	b.ReportAllocs()
	b.ResetTimer()
	b.RunParallel(func(pb *testing.PB) {
		for pb.Next() {
			c.ObserveVoice(13, "speaker", at)
		}
	})
}

func assertEvent(t *testing.T, events []Event, eventType string) {
	t.Helper()
	for _, event := range events {
		if event.EventType == eventType {
			return
		}
	}
	t.Fatalf("missing event %s in %#v", eventType, events)
}

func hasEvent(events []Event, eventType string) bool {
	for _, event := range events {
		if event.EventType == eventType {
			return true
		}
	}
	return false
}

func findEvent(events []Event, eventType string) *Event {
	for index := range events {
		if events[index].EventType == eventType {
			return &events[index]
		}
	}
	return nil
}
