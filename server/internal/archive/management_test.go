package archive

import (
	"errors"
	"path/filepath"
	"testing"
	"time"
)

type failingRuleRepository struct{ RuleRepository }

func (r failingRuleRepository) Save(DeclarativeRuleSet) (string, error) {
	return "", errors.New("simulated disk failure")
}

func TestFailedPublicationKeepsActiveRules(t *testing.T) {
	dir := t.TempDir()
	repository := failingRuleRepository{NewDeclarativeRuleStore(filepath.Join(dir, "rules.json"))}
	c, err := LoadOrCreateWithRuleRepository(filepath.Join(dir, "archive.json"), "server", repository)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	if err := c.UpsertDeclarativeRule(testDeclarativeRule("disk_failure")); err == nil {
		t.Fatal("expected storage error")
	}
	if set := c.DeclarativeRules(); set.Revision != 0 || len(set.Rules) != 0 {
		t.Fatal("failed persistence changed the active rules")
	}
}

func TestCustomEventPresentationSurvivesRuleEditAndRestart(t *testing.T) {
	path := filepath.Join(t.TempDir(), "archive.json")
	c, err := LoadOrCreate(path, "server")
	if err != nil {
		t.Fatal(err)
	}
	rule := testDeclarativeRule("immutable_memory")
	rule.Condition.Value = 1
	if err := c.UpsertDeclarativeRule(rule); err != nil {
		t.Fatal(err)
	}
	c.mu.Lock()
	c.processor.Apply(Activity{Kind: VoicePacket, RoomID: 1, MemberID: "a", At: time.Now()})
	c.mu.Unlock()
	rule.Version = 2
	rule.Titles["zh-CN"] = "改名之后"
	if err := c.UpsertDeclarativeRule(rule); err != nil {
		t.Fatal(err)
	}
	if err := c.Close(); err != nil {
		t.Fatal(err)
	}
	reloaded, err := LoadOrCreate(path, "server")
	if err != nil {
		t.Fatal(err)
	}
	defer reloaded.Close()
	for _, event := range reloaded.Snapshot(1, true, 100).Events {
		if event.EventType == rule.ID {
			if event.RuleVersion != 1 || event.Titles["zh-CN"] != "百小时的声音" {
				t.Fatal("historical text/version was rewritten")
			}
			return
		}
	}
	t.Fatal("custom event lost after restart")
}

func TestRuleDraftPublishRollbackAndAudit(t *testing.T) {
	dir := t.TempDir()
	c, err := LoadOrCreate(filepath.Join(dir, "archive.json"), "server")
	if err != nil {
		t.Fatal(err)
	}
	rule := testDeclarativeRule("managed_memory")
	if err := c.SaveRuleDraft(rule, "admin-a", 0); err != nil {
		t.Fatal(err)
	}
	if len(c.DeclarativeRules().Rules) != 0 {
		t.Fatal("draft became active before publication")
	}
	if err := c.PublishRuleDraft(rule.ID, "admin-b", 0); !errors.Is(err, ErrRuleRevisionConflict) {
		t.Fatalf("stale edit: %v", err)
	}
	if err := c.PublishRuleDraft(rule.ID, "admin-a", 1); err != nil {
		t.Fatal(err)
	}
	if len(c.DeclarativeRules().Drafts) != 0 || len(c.DeclarativeRules().Rules) != 1 {
		t.Fatal("publish did not promote draft")
	}
	if err := c.SetRuleEnabled(rule.ID, false, "admin-a", 2); err != nil {
		t.Fatal(err)
	}
	if c.DeclarativeRules().Rules[0].Enabled {
		t.Fatal("disable not persisted")
	}
	if err := c.RollbackRule(rule.ID, 1, "admin-b", 3); err != nil {
		t.Fatal(err)
	}
	set := c.DeclarativeRules()
	if !set.Rules[0].Enabled || set.Rules[0].Version != 3 || len(set.History) != 2 {
		t.Fatalf("rollback: %#v", set)
	}
	if len(set.Audit) != 4 || set.Audit[3].Action != "rollback" || set.Audit[3].Actor != "admin-b" {
		t.Fatalf("audit: %#v", set.Audit)
	}
	if err := c.CopyRule(rule.ID, "copied_memory", "admin-a", 4); err != nil {
		t.Fatal(err)
	}
	if err := c.DeleteRuleDraft("copied_memory", "admin-a", 5); err != nil {
		t.Fatal(err)
	}
	if err := c.RemoveRule(rule.ID, "admin-a", 6); err != nil {
		t.Fatal(err)
	}
	if err := c.Close(); err != nil {
		t.Fatal(err)
	}
	reloaded, err := LoadOrCreate(filepath.Join(dir, "archive.json"), "server")
	if err != nil {
		t.Fatal(err)
	}
	defer reloaded.Close()
	set = reloaded.DeclarativeRules()
	if set.Revision != 7 || len(set.Rules) != 0 || len(set.Audit) != 7 || len(set.History) != 3 {
		t.Fatalf("management state not recovered: %#v", set)
	}
}

func TestDeclarativeSessionEndEmitsPerMemberAndPerPair(t *testing.T) {
	member := testDeclarativeRule("member_session")
	member.Trigger, member.Dedupe.Scope = TriggerSessionEnd, "member"
	member.Condition = DeclarativeCondition{Metric: "member_voice_sessions", Operator: "crosses", Value: 1}
	pair := testDeclarativeRule("pair_session")
	pair.Trigger, pair.Dedupe.Scope = TriggerSessionEnd, "pair"
	pair.Condition = DeclarativeCondition{Metric: "pair_session_count", Operator: "crosses", Value: 1}
	rules, err := CompileDeclarativeRules(DeclarativeRuleSet{Rules: []DeclarativeRule{member, pair}}, DefaultRules())
	if err != nil {
		t.Fatal(err)
	}
	p := NewProcessorWithRules("server", time.Second, persistedState{}, rules)
	at := time.Date(2026, 9, 4, 12, 0, 0, 0, time.UTC)
	for _, id := range []string{"a", "b", "c"} {
		p.Apply(Activity{Kind: Joined, RoomID: 1, MemberID: id, At: at})
	}
	p.Apply(Activity{Kind: VoicePacket, RoomID: 1, MemberID: "a", At: at})
	for _, id := range []string{"a", "b", "c"} {
		p.Apply(Activity{Kind: Left, RoomID: 1, MemberID: id, At: at.Add(time.Minute)})
	}
	events := p.Apply(Activity{Kind: Tick, RoomID: 1, At: at.Add(2 * time.Minute)})
	if len(events) != 6 {
		t.Fatalf("got %d session-end events, want 3 members + 3 pairs: %#v", len(events), events)
	}
	for _, event := range events {
		if len(event.Participants) != 3 || len(event.TriggeredBy) == 0 {
			t.Fatalf("missing actors: %#v", event)
		}
	}
	if events := p.Apply(Activity{Kind: Tick, RoomID: 1, At: at.Add(3 * time.Minute)}); len(events) != 0 {
		t.Fatal("session-end was repeated")
	}
}

func TestTransitionModeAndHistoricalCatchUpAreExplicit(t *testing.T) {
	rule := testDeclarativeRule("catch_up")
	rule.Condition.Operator = "gte"
	ctx := EvalContext{Trigger: TriggerVoice, Now: time.Now(), Before: RoomStats{ActiveVoiceSeconds: 400000}, After: RoomStats{ActiveVoiceSeconds: 400001}}
	rules, err := CompileDeclarativeRules(DeclarativeRuleSet{Rules: []DeclarativeRule{rule}}, nil)
	if err != nil {
		t.Fatal(err)
	}
	if events := NewEngine(rules).Evaluate(1, "server", ctx, map[string]time.Time{}); len(events) != 0 {
		t.Fatal("default transition mode backfilled old state")
	}
	rule.MatchMode = "state"
	rules, err = CompileDeclarativeRules(DeclarativeRuleSet{Rules: []DeclarativeRule{rule}}, nil)
	if err != nil {
		t.Fatal(err)
	}
	if events := NewEngine(rules).Evaluate(1, "server", ctx, map[string]time.Time{}); len(events) != 1 {
		t.Fatal("explicit state mode did not catch up")
	}
}

func TestRulePreviewDoesNotEmitOrDedupe(t *testing.T) {
	c, err := LoadOrCreate(filepath.Join(t.TempDir(), "archive.json"), "server")
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	c.mu.Lock()
	c.processor.state.Rooms[7] = RoomStats{RoomID: 7, ActiveVoiceSeconds: 400000}
	before := len(c.processor.state.Dedupe)
	c.mu.Unlock()
	preview, err := c.PreviewRule(testDeclarativeRule("preview"))
	if err != nil {
		t.Fatal(err)
	}
	if len(preview.RoomIDs) != 1 || preview.RoomIDs[0] != 7 {
		t.Fatalf("preview: %#v", preview)
	}
	c.mu.RLock()
	defer c.mu.RUnlock()
	if len(c.processor.state.Dedupe) != before || len(c.processor.state.Events[7]) != 0 {
		t.Fatal("preview changed archive state")
	}
}

func TestParticipantAndDurationCrossingsUsePreviousContext(t *testing.T) {
	rule := testDeclarativeRule("two_members")
	rule.Trigger = TriggerJoin
	rule.Condition = DeclarativeCondition{Metric: "participant_count", Operator: "crosses", Value: 2}
	duration := testDeclarativeRule("minute_together")
	duration.Trigger = TriggerTick
	duration.Condition = DeclarativeCondition{Metric: "session_duration_seconds", Operator: "crosses", Value: 60}
	rules, err := CompileDeclarativeRules(DeclarativeRuleSet{Rules: []DeclarativeRule{rule, duration}}, nil)
	if err != nil {
		t.Fatal(err)
	}
	p := NewProcessorWithRules("server", time.Second, persistedState{}, rules)
	at := time.Now()
	p.Apply(Activity{Kind: Joined, RoomID: 1, MemberID: "a", At: at})
	if events := p.Apply(Activity{Kind: Joined, RoomID: 1, MemberID: "b", At: at}); len(events) != 1 {
		t.Fatal("participant crossing did not fire")
	}
	p.Apply(Activity{Kind: VoicePacket, RoomID: 1, MemberID: "a", At: at})
	if events := p.Apply(Activity{Kind: Tick, RoomID: 1, At: at.Add(time.Minute)}); len(events) != 1 {
		t.Fatal("duration crossing did not fire")
	}
}
