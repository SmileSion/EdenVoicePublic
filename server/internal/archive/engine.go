package archive

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"sort"
	"time"
)

type Engine struct{ rulesByTrigger map[Trigger][]Rule }

func NewEngine(rules []Rule) *Engine {
	indexed := make(map[Trigger][]Rule)
	for _, rule := range rules {
		indexed[rule.Trigger] = append(indexed[rule.Trigger], rule)
	}
	return &Engine{rulesByTrigger: indexed}
}

func (e *Engine) Evaluate(roomID uint64, serverID string, ctx EvalContext, seen map[string]time.Time) []Event {
	var result []Event
	for _, rule := range e.rulesByTrigger[ctx.Trigger] {
		contexts := []EvalContext{ctx}
		if rule.Contexts != nil {
			contexts = rule.Contexts(ctx)
		}
		for _, ctx := range contexts {
			if !rule.matches(ctx) {
				continue
			}
			key := fmt.Sprintf("%s:%d", rule.ID, roomID)
			if rule.Key != nil {
				key += ":" + rule.Key(ctx)
			}
			if last, ok := seen[key]; ok && (rule.Dedupe == DedupeForever || ctx.Now.Sub(last) < rule.Cooldown) {
				continue
			}
			participants := append([]string(nil), ctx.Participants...)
			sort.Strings(participants)
			triggeredBy := []string(nil)
			if rule.Actors != nil {
				triggeredBy = append(triggeredBy, rule.Actors(ctx)...)
			} else if ctx.MemberID != "" {
				triggeredBy = append(triggeredBy, ctx.MemberID)
			} else {
				triggeredBy = append(triggeredBy, participants...)
			}
			sort.Strings(triggeredBy)
			eventID := stableEventID(key, ctx.Now, rule.Dedupe)
			importance := min(100, rule.Importance+importanceBonus(ctx))
			result = append(result, Event{ID: eventID, EventType: rule.EventType, Category: rule.Category, RoomID: roomID, ServerID: serverID, OccurredAt: ctx.Now, TriggeredBy: triggeredBy, Participants: participants, Metadata: eventMetadata(rule, ctx), Importance: importance, Rarity: rule.Rarity, RuleVersion: rule.Version, Titles: cloneStringMap(rule.Titles), Descriptions: cloneStringMap(rule.Descriptions), TitleKey: "archive." + rule.EventType + ".title", DescriptionKey: "archive." + rule.EventType + ".description", DedupeKey: key})
			seen[key] = ctx.Now
		}
	}
	return result
}

func stableEventID(key string, at time.Time, scope DedupeScope) string {
	seed := key
	if scope == DedupeCooldown {
		seed += ":" + at.UTC().Format(time.RFC3339Nano)
	}
	digest := sha256.Sum256([]byte(seed))
	return "evt_" + hex.EncodeToString(digest[:12])
}

func importanceBonus(c EvalContext) int {
	bonus := 0
	if c.Now.Hour() < 5 {
		bonus += 8
	}
	if len(c.Participants) >= 5 {
		bonus += 8
	}
	if c.SilenceDuration >= 30*24*time.Hour {
		bonus += 15
	}
	if c.SessionDuration >= 3*time.Hour {
		bonus += 10
	}
	return bonus
}

func eventMetadata(rule Rule, c EvalContext) map[string]any {
	m := map[string]any{"threshold": rule.Threshold, "participant_count": len(c.Participants)}
	if rule.Facts != nil {
		for key, value := range rule.Facts(c) {
			m[key] = value
		}
	}
	if rule.Version > 0 {
		m["rule_version"] = rule.Version
	}
	if len(c.ReturningMembers) > 0 {
		m["returning_member_count"] = len(c.ReturningMembers)
	}
	if c.SessionDuration > 0 {
		m["session_seconds"] = int64(c.SessionDuration / time.Second)
	}
	if c.SilenceDuration > 0 {
		m["silence_seconds"] = int64(c.SilenceDuration / time.Second)
	}
	return m
}
