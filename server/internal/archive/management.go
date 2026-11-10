package archive

import (
	"errors"
	"fmt"
	"sort"
	"strings"
	"time"
)

var ErrRuleRevisionConflict = errors.New("rule revision conflict; reload before saving")

type BuiltinRuleSummary struct {
	ID         string   `json:"id"`
	Category   Category `json:"category"`
	Trigger    Trigger  `json:"trigger"`
	Rarity     Rarity   `json:"rarity"`
	Importance int      `json:"importance"`
	ReadOnly   bool     `json:"read_only"`
}

func BuiltinRuleCatalog() []BuiltinRuleSummary {
	result := make([]BuiltinRuleSummary, 0)
	for _, rule := range DefaultRules() {
		result = append(result, BuiltinRuleSummary{rule.ID, rule.Category, rule.Trigger, rule.Rarity, rule.Importance, true})
	}
	return result
}

func (c *Collector) RemoveRule(id, actor string, expectedRevision uint64) error {
	return c.manageRules(actor, "delete", id, expectedRevision, func(set *DeclarativeRuleSet) error {
		archiveRuleRevision(set, id)
		updated, found := deleteRule(*set, id)
		if !found {
			return fmt.Errorf("rule %q not found", id)
		}
		*set = updated
		return nil
	})
}

// Management methods have no network exposure. Their caller must authenticate
// an administrator before passing the stable identity as actor.
func (c *Collector) SaveRuleDraft(rule DeclarativeRule, actor string, expectedRevision uint64) error {
	return c.manageRules(actor, "save_draft", rule.ID, expectedRevision, func(set *DeclarativeRuleSet) error {
		candidate := upsertRule(*set, rule)
		if _, err := CompileDeclarativeRules(candidate, DefaultRules()); err != nil {
			return err
		}
		for _, active := range set.Rules {
			if active.ID == rule.ID && rule.Version <= active.Version {
				return errors.New("draft version must exceed the published version")
			}
		}
		for i := range set.Drafts {
			if set.Drafts[i].ID == rule.ID {
				set.Drafts[i] = cloneDeclarativeRule(rule)
				return nil
			}
		}
		set.Drafts = append(set.Drafts, cloneDeclarativeRule(rule))
		return nil
	})
}

func (c *Collector) DeleteRuleDraft(id, actor string, expectedRevision uint64) error {
	return c.manageRules(actor, "delete_draft", id, expectedRevision, func(set *DeclarativeRuleSet) error {
		for i := range set.Drafts {
			if set.Drafts[i].ID == id {
				set.Drafts = append(set.Drafts[:i], set.Drafts[i+1:]...)
				return nil
			}
		}
		return fmt.Errorf("draft %q not found", id)
	})
}

func (c *Collector) PublishRuleDraft(id, actor string, expectedRevision uint64) error {
	return c.manageRules(actor, "publish", id, expectedRevision, func(set *DeclarativeRuleSet) error {
		for i, rule := range set.Drafts {
			if rule.ID != id {
				continue
			}
			for _, active := range set.Rules {
				if active.ID == id && rule.Version <= active.Version {
					return errors.New("draft is stale; increase its version before publishing")
				}
			}
			archiveRuleRevision(set, id)
			updated := upsertRule(*set, rule)
			updated.Drafts = append(updated.Drafts[:i], updated.Drafts[i+1:]...)
			*set = updated
			return nil
		}
		return fmt.Errorf("draft %q not found", id)
	})
}

func (c *Collector) SetRuleEnabled(id string, enabled bool, actor string, expectedRevision uint64) error {
	return c.manageRules(actor, "set_enabled", id, expectedRevision, func(set *DeclarativeRuleSet) error {
		for i := range set.Rules {
			if set.Rules[i].ID == id {
				archiveRuleRevision(set, id)
				set.Rules[i].Enabled = enabled
				set.Rules[i].Version++
				return nil
			}
		}
		return fmt.Errorf("rule %q not found", id)
	})
}

func (c *Collector) RollbackRule(id string, version int, actor string, expectedRevision uint64) error {
	return c.manageRules(actor, "rollback", id, expectedRevision, func(set *DeclarativeRuleSet) error {
		var target *DeclarativeRule
		latest := 0
		for _, entry := range set.History {
			if entry.ID == id {
				latest = max(latest, entry.Version)
				if entry.Version == version {
					copy := cloneDeclarativeRule(entry)
					target = &copy
				}
			}
		}
		for _, entry := range set.Rules {
			if entry.ID == id {
				latest = max(latest, entry.Version)
			}
		}
		if target == nil {
			return fmt.Errorf("historical rule %q version %d not found", id, version)
		}
		archiveRuleRevision(set, id)
		target.Version = latest + 1 // rollback is a new publication, never rewrites history
		*set = upsertRule(*set, *target)
		return nil
	})
}

func (c *Collector) CopyRule(sourceID, newID, actor string, expectedRevision uint64) error {
	return c.manageRules(actor, "copy_draft", newID, expectedRevision, func(set *DeclarativeRuleSet) error {
		for _, entry := range append(append([]DeclarativeRule(nil), set.Rules...), set.Drafts...) {
			if entry.ID == newID {
				return errors.New("destination ID already exists")
			}
		}
		for _, entry := range set.Rules {
			if entry.ID == sourceID {
				copy := cloneDeclarativeRule(entry)
				copy.ID, copy.Version = newID, 1
				if _, err := CompileDeclarativeRules(upsertRule(*set, copy), DefaultRules()); err != nil {
					return err
				}
				set.Drafts = append(set.Drafts, copy)
				return nil
			}
		}
		return fmt.Errorf("rule %q not found", sourceID)
	})
}

func (c *Collector) manageRules(actor, action, id string, expected uint64, mutate func(*DeclarativeRuleSet) error) error {
	if strings.TrimSpace(actor) == "" || len(actor) > 128 {
		return errors.New("a bounded administrator identity is required")
	}
	c.rulesUpdate.Lock()
	defer c.rulesUpdate.Unlock()
	c.mu.RLock()
	set := cloneDeclarativeRuleSet(c.ruleSet)
	c.mu.RUnlock()
	if set.Revision != expected {
		return ErrRuleRevisionConflict
	}
	if err := mutate(&set); err != nil {
		return err
	}
	set.Revision = expected + 1
	set.Audit = append(set.Audit, RuleAudit{Revision: set.Revision, Actor: actor, Action: action, RuleID: id, At: time.Now().UTC()})
	if len(set.Audit) > 1000 {
		set.Audit = set.Audit[len(set.Audit)-1000:]
	}
	return c.replaceDeclarativeRulesLocked(set)
}

func archiveRuleRevision(set *DeclarativeRuleSet, id string) {
	for _, entry := range set.Rules {
		if entry.ID == id {
			set.History = append(set.History, cloneDeclarativeRule(entry))
			if len(set.History) > 100 {
				set.History = set.History[len(set.History)-100:]
			}
			return
		}
	}
}

type RulePreview struct {
	RoomIDs             []uint64 `json:"room_ids"`
	RequiresLiveContext []string `json:"requires_live_context,omitempty"`
	Note                string   `json:"note"`
}

// Preview estimates current stored counters only, not an event-history replay.
// It never calls Apply, emits events, or consumes dedupe keys.
func (c *Collector) PreviewRule(rule DeclarativeRule) (RulePreview, error) {
	if _, err := CompileDeclarativeRules(DeclarativeRuleSet{Rules: []DeclarativeRule{rule}}, DefaultRules()); err != nil {
		return RulePreview{}, err
	}
	copy := cloneDeclarativeRule(rule)
	result := RulePreview{RoomIDs: []uint64{}, Note: "Current-state estimate; crosses is evaluated as gte, dedupe and past trigger timing are not replayed."}
	metrics := make(map[string]bool)
	var prepare func(*DeclarativeCondition)
	prepare = func(condition *DeclarativeCondition) {
		if condition.Operator == "crosses" {
			condition.Operator = "gte"
		}
		if condition.Operator == "changed" {
			metrics[condition.Metric] = true
		}
		switch condition.Metric {
		case "participant_count", "concurrent_speakers", "session_duration_seconds", "last_two_duration_seconds", "last_one_duration_seconds", "silence_seconds", "local_hour", "member_voice_sessions", "pair_session_count":
			metrics[condition.Metric] = true
		}
		for i := range condition.All {
			prepare(&condition.All[i])
		}
		for i := range condition.Any {
			prepare(&condition.Any[i])
		}
	}
	prepare(&copy.Condition)
	if copy.Dedupe.Scope == "member" || copy.Dedupe.Scope == "pair" {
		metrics[copy.Dedupe.Scope+"_scope"] = true
	}
	for metric := range metrics {
		result.RequiresLiveContext = append(result.RequiresLiveContext, metric)
	}
	sort.Strings(result.RequiresLiveContext)
	if len(metrics) > 0 {
		result.Note = "This rule requires live context; no historical match count is claimed."
		return result, nil
	}
	predicate, err := compileCondition(copy.Condition)
	if err != nil {
		return result, err
	}
	c.mu.RLock()
	rooms := make([]RoomStats, 0, len(c.processor.state.Rooms))
	for _, stats := range c.processor.state.Rooms {
		rooms = append(rooms, cloneStats(stats))
	}
	c.mu.RUnlock()
	now := time.Now()
	for _, stats := range rooms {
		if predicate(EvalContext{Now: now, PreviousTime: now, Before: stats, After: stats}) {
			result.RoomIDs = append(result.RoomIDs, stats.RoomID)
		}
	}
	sort.Slice(result.RoomIDs, func(i, j int) bool { return result.RoomIDs[i] < result.RoomIDs[j] })
	return result, nil
}
