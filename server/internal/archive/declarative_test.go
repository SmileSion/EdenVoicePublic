package archive

import (
	"fmt"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func BenchmarkDeclarative500Rules(b *testing.B) {
	set := DeclarativeRuleSet{SchemaVersion: 1}
	for i := 0; i < 500; i++ {
		set.Rules = append(set.Rules, testDeclarativeRule(fmt.Sprintf("custom_%d", i)))
	}
	rules, err := CompileDeclarativeRules(set, nil)
	if err != nil {
		b.Fatal(err)
	}
	engine := NewEngine(rules)
	seen := map[string]time.Time{}
	ctx := EvalContext{Trigger: TriggerVoice, Now: time.Now(), Before: RoomStats{}, After: RoomStats{ActiveVoiceSeconds: 1}}
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		engine.Evaluate(1, "server", ctx, seen)
	}
}

func TestDeclarativeRuleCompilesAndSnapshotsPresentation(t *testing.T) {
	definition := testDeclarativeRule("hundred_hours")
	compiled, err := CompileDeclarativeRules(DeclarativeRuleSet{SchemaVersion: 1, Rules: []DeclarativeRule{definition}}, DefaultRules())
	if err != nil {
		t.Fatal(err)
	}
	before := RoomStats{ActiveVoiceSeconds: 359_999}
	after := RoomStats{ActiveVoiceSeconds: 360_000}
	events := NewEngine(compiled).Evaluate(7, "server", EvalContext{Trigger: TriggerVoice, Now: time.Now(), Before: before, After: after, MemberID: "client-a", Participants: []string{"client-a"}}, map[string]time.Time{})
	if len(events) != 1 {
		t.Fatalf("events = %d, want 1", len(events))
	}
	event := events[0]
	if event.EventType != definition.ID || event.RuleVersion != definition.Version {
		t.Fatalf("unexpected rule identity: %#v", event)
	}
	if event.Titles["zh-CN"] != "百小时的声音" || event.Descriptions["en-US"] == "" {
		t.Fatalf("presentation was not snapshotted: %#v %#v", event.Titles, event.Descriptions)
	}
	definition.Titles["zh-CN"] = "后来修改的名称"
	if event.Titles["zh-CN"] != "百小时的声音" {
		t.Fatal("historical event presentation changed with its rule definition")
	}
}

func TestRuleFileWatcherPublishesWithoutRestart(t *testing.T) {
	dir := t.TempDir()
	c, err := LoadOrCreate(filepath.Join(dir, "archive.json"), "server")
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	rule := testDeclarativeRule("live_file_memory")
	rule.Condition.Value = 1
	store := NewDeclarativeRuleStore(filepath.Join(dir, "eden-archive-rules.json"))
	if _, err := store.Save(DeclarativeRuleSet{SchemaVersion: 1, Revision: 1, Rules: []DeclarativeRule{rule}}); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(4 * time.Second)
	for len(c.DeclarativeRules().Rules) == 0 && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	if len(c.DeclarativeRules().Rules) == 0 {
		t.Fatal("background watcher did not install the file")
	}
	c.ObserveVoice(1, "alice", time.Now())
	var events []Event
	for time.Now().Before(deadline) {
		events = c.Snapshot(1, true, 100).Events
		if len(events) > 0 {
			for _, e := range events {
				if e.EventType == rule.ID && e.Titles["zh-CN"] == rule.Titles["zh-CN"] {
					return
				}
			}
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatalf("hot-loaded rule did not produce its custom event: %#v", events)
}

func TestRuleFileRejectsUnknownFieldsAndOversizedPayloads(t *testing.T) {
	path := filepath.Join(t.TempDir(), "rules.json")
	store := NewDeclarativeRuleStore(path)
	for _, data := range [][]byte{[]byte(`{"schema_version":1,"rules":[],"script":"bad"}`), make([]byte, maxRuleFileBytes+1)} {
		if err := os.WriteFile(path, data, 0600); err != nil {
			t.Fatal(err)
		}
		if _, _, err := store.Load(); err == nil {
			t.Fatal("invalid rule file was accepted")
		}
	}
}

func TestDeclarativeConditionLimits(t *testing.T) {
	for _, edit := range []func(*DeclarativeRule){
		func(r *DeclarativeRule) {
			r.Condition = DeclarativeCondition{Metric: "local_hour", Operator: "between", Values: []float64{5, 0}}
		},
		func(r *DeclarativeRule) { r.Condition.Value = -1 },
		func(r *DeclarativeRule) { r.Condition.All = []DeclarativeCondition{r.Condition} },
		func(r *DeclarativeRule) {
			leaf := r.Condition
			r.Condition = DeclarativeCondition{All: make([]DeclarativeCondition, 17)}
			for i := range r.Condition.All {
				r.Condition.All[i] = leaf
			}
		},
		func(r *DeclarativeRule) {
			for i := 0; i < 4; i++ {
				r.Condition = DeclarativeCondition{All: []DeclarativeCondition{r.Condition}}
			}
		},
	} {
		r := testDeclarativeRule("bounded")
		edit(&r)
		if _, err := CompileDeclarativeRules(DeclarativeRuleSet{Rules: []DeclarativeRule{r}}, nil); err == nil {
			t.Fatal("invalid or unbounded condition compiled")
		}
	}
}

func TestDeclarativeRuleValidationRejectsUnsafeOrAmbiguousRules(t *testing.T) {
	tests := []struct {
		name string
		edit func(*DeclarativeRule)
	}{
		{"built-in collision", func(rule *DeclarativeRule) { rule.ID = "first_voice" }},
		{"unknown metric", func(rule *DeclarativeRule) { rule.Condition.Metric = "execute_script" }},
		{"cooldown without duration", func(rule *DeclarativeRule) { rule.Dedupe.Mode = "cooldown" }},
		{"missing localized title", func(rule *DeclarativeRule) { delete(rule.Titles, "zh-CN") }},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			rule := testDeclarativeRule("custom_rule")
			tt.edit(&rule)
			if _, err := CompileDeclarativeRules(DeclarativeRuleSet{SchemaVersion: 1, Rules: []DeclarativeRule{rule}}, DefaultRules()); err == nil {
				t.Fatal("invalid rule compiled successfully")
			}
		})
	}
}

func TestCollectorCRUDPersistsAndHotReloadsDeclarativeRules(t *testing.T) {
	dir := t.TempDir()
	rulesPath := filepath.Join(dir, "rules.json")
	collector, err := LoadOrCreateWithRulesPath(filepath.Join(dir, "archive.json"), "server", rulesPath)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = collector.Close() })

	first := testDeclarativeRule("first_custom")
	if err := collector.UpsertDeclarativeRule(first); err != nil {
		t.Fatal(err)
	}
	if got := collector.DeclarativeRules(); got.Revision == 0 || len(got.Rules) != 1 || got.Rules[0].ID != first.ID {
		t.Fatalf("unexpected published rules: %#v", got)
	}

	store := NewDeclarativeRuleStore(rulesPath)
	external := DeclarativeRuleSet{SchemaVersion: 1, Revision: 50, Rules: []DeclarativeRule{testDeclarativeRule("external_rule")}}
	if _, err := store.Save(external); err != nil {
		t.Fatal(err)
	}
	if err := collector.ReloadDeclarativeRules(); err != nil {
		t.Fatal(err)
	}
	if got := collector.DeclarativeRules(); got.Revision != 50 || len(got.Rules) != 1 || got.Rules[0].ID != "external_rule" {
		t.Fatalf("external rules were not hot-reloaded: %#v", got)
	}

	invalid := cloneDeclarativeRuleSet(external)
	invalid.Revision++
	invalid.Rules[0].Condition.Metric = "not_allowed"
	if _, err := store.Save(invalid); err != nil {
		t.Fatal(err)
	}
	if err := collector.ReloadDeclarativeRules(); err == nil {
		t.Fatal("invalid external update was accepted")
	}
	if got := collector.DeclarativeRules(); got.Rules[0].ID != "external_rule" || got.Revision != 50 {
		t.Fatalf("invalid reload replaced the active rule set: %#v", got)
	}
	if collector.DeclarativeRuleError() == "" {
		t.Fatal("invalid reload did not expose its validation error")
	}

	if _, err := store.Save(external); err != nil {
		t.Fatal(err)
	}
	if err := collector.ReloadDeclarativeRules(); err != nil {
		t.Fatal(err)
	}
	if collector.DeclarativeRuleError() != "" {
		t.Fatal("successful reload did not clear the previous error")
	}
	if err := collector.DeleteDeclarativeRule("external_rule"); err != nil {
		t.Fatal(err)
	}
	if got := collector.DeclarativeRules(); len(got.Rules) != 0 {
		t.Fatalf("deleted rule remains active: %#v", got)
	}
}

func TestDeclarativePairScopeUsesStablePairDedupeAndActors(t *testing.T) {
	rule := testDeclarativeRule("pair_memory")
	rule.Trigger = TriggerTick
	rule.Condition = DeclarativeCondition{Metric: "participant_count", Operator: "eq", Value: 2}
	rule.Dedupe.Scope = "pair"
	compiled, err := CompileDeclarativeRules(DeclarativeRuleSet{SchemaVersion: 1, Rules: []DeclarativeRule{rule}}, DefaultRules())
	if err != nil {
		t.Fatal(err)
	}
	seen := map[string]time.Time{}
	ctx := EvalContext{Trigger: TriggerTick, Now: time.Now(), Participants: []string{"b", "a"}}
	events := NewEngine(compiled).Evaluate(1, "server", ctx, seen)
	if len(events) != 1 || len(events[0].TriggeredBy) != 2 || events[0].TriggeredBy[0] != "a" {
		t.Fatalf("unexpected pair event: %#v", events)
	}
	ctx.Now = ctx.Now.Add(time.Hour)
	ctx.Participants = []string{"a", "b"}
	if repeated := NewEngine(compiled).Evaluate(1, "server", ctx, seen); len(repeated) != 0 {
		t.Fatalf("same pair bypassed stable dedupe: %#v", repeated)
	}
}

func testDeclarativeRule(id string) DeclarativeRule {
	return DeclarativeRule{
		ID: id, Version: 1, Enabled: true, Category: Voice, Trigger: TriggerVoice, Rarity: Rare, Importance: 70,
		Titles:       map[string]string{"zh-CN": "百小时的声音", "en-US": "One Hundred Hours"},
		Descriptions: map[string]string{"zh-CN": "这个房间累计留下了一百小时的声音。", "en-US": "One hundred hours of voices were left in this room."},
		Condition:    DeclarativeCondition{Metric: "active_voice_seconds", Operator: "crosses", Value: 360_000},
		Dedupe:       DeclarativeDedupe{Scope: "room", Mode: "forever"},
	}
}
