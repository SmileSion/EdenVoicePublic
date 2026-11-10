package archive

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"os"
	"path/filepath"
	"reflect"
	"regexp"
	"sort"
	"strings"
	"sync"
	"time"

	"edenvoice/server/internal/privatefs"
)

const (
	DeclarativeRulesSchemaVersion = 1
	maxDeclarativeRules           = 500
	maxConditionNodes             = 16
	maxConditionDepth             = 3
	maxRuleFileBytes              = 4 << 20
)

type DeclarativeRuleSet struct {
	SchemaVersion int               `json:"schema_version"`
	Revision      uint64            `json:"revision"`
	Rules         []DeclarativeRule `json:"rules"`
	Drafts        []DeclarativeRule `json:"drafts,omitempty"`
	History       []DeclarativeRule `json:"history,omitempty"`
	Audit         []RuleAudit       `json:"audit,omitempty"`
}

type RuleAudit struct {
	Revision uint64    `json:"revision"`
	Actor    string    `json:"actor"`
	Action   string    `json:"action"`
	RuleID   string    `json:"rule_id,omitempty"`
	At       time.Time `json:"at"`
}

// RuleRepository is the transport/storage boundary for a future authenticated
// management application. Save must atomically replace one complete document.
type RuleRepository interface {
	Load() (DeclarativeRuleSet, string, error)
	Save(DeclarativeRuleSet) (string, error)
	Stamp() (string, error)
}

type DeclarativeRule struct {
	ID           string               `json:"id"`
	Version      int                  `json:"version"`
	Enabled      bool                 `json:"enabled"`
	Category     Category             `json:"category"`
	Trigger      Trigger              `json:"trigger"`
	Rarity       Rarity               `json:"rarity"`
	Importance   int                  `json:"importance"`
	Titles       map[string]string    `json:"titles"`
	Descriptions map[string]string    `json:"descriptions"`
	Condition    DeclarativeCondition `json:"condition"`
	Dedupe       DeclarativeDedupe    `json:"dedupe"`
	MatchMode    string               `json:"match_mode,omitempty"`
}

type DeclarativeCondition struct {
	All      []DeclarativeCondition `json:"all,omitempty"`
	Any      []DeclarativeCondition `json:"any,omitempty"`
	Metric   string                 `json:"metric,omitempty"`
	Operator string                 `json:"operator,omitempty"`
	Value    float64                `json:"value,omitempty"`
	Values   []float64              `json:"values,omitempty"`
}

type DeclarativeDedupe struct {
	Scope           string `json:"scope"`
	Mode            string `json:"mode"`
	CooldownSeconds int64  `json:"cooldown_seconds,omitempty"`
}

type DeclarativeRuleStore struct {
	path string
	mu   sync.Mutex
}

func NewDeclarativeRuleStore(path string) *DeclarativeRuleStore {
	return &DeclarativeRuleStore{path: path}
}

func (s *DeclarativeRuleStore) Path() string { return s.path }

func (s *DeclarativeRuleStore) Stamp() (string, error) {
	info, err := os.Stat(s.path)
	if errors.Is(err, os.ErrNotExist) {
		return "", nil
	}
	if err != nil {
		return "", err
	}
	return fmt.Sprintf("%d:%d", info.ModTime().UnixNano(), info.Size()), nil
}

func (s *DeclarativeRuleStore) Load() (DeclarativeRuleSet, string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.loadLocked()
}

func (s *DeclarativeRuleStore) loadLocked() (DeclarativeRuleSet, string, error) {
	set := DeclarativeRuleSet{SchemaVersion: DeclarativeRulesSchemaVersion, Rules: []DeclarativeRule{}}
	file, err := os.Open(s.path)
	if errors.Is(err, os.ErrNotExist) {
		return set, "", nil
	}
	if err != nil {
		return DeclarativeRuleSet{}, "", err
	}
	defer file.Close()
	data, err := io.ReadAll(io.LimitReader(file, maxRuleFileBytes+1))
	if err != nil || len(data) > maxRuleFileBytes {
		return DeclarativeRuleSet{}, "", errors.New("rules file exceeds 4 MiB or cannot be read")
	}
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&set); err != nil {
		return DeclarativeRuleSet{}, "", fmt.Errorf("decode declarative archive rules: %w", err)
	}
	if err := decoder.Decode(new(any)); err != io.EOF {
		return DeclarativeRuleSet{}, "", errors.New("rules file must contain exactly one JSON document")
	}
	if set.SchemaVersion == 0 {
		set.SchemaVersion = DeclarativeRulesSchemaVersion
	}
	return set, digestBytes(data), nil
}

func (s *DeclarativeRuleStore) Save(set DeclarativeRuleSet) (string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	set.SchemaVersion = DeclarativeRulesSchemaVersion
	if set.Rules == nil {
		set.Rules = []DeclarativeRule{}
	}
	data, err := json.MarshalIndent(set, "", "  ")
	if err != nil {
		return "", err
	}
	data = append(data, '\n')
	if err := privatefs.PrepareDirectory(s.path); err != nil {
		return "", err
	}
	if len(data) > maxRuleFileBytes {
		return "", errors.New("rules file exceeds 4 MiB")
	}
	file, err := os.CreateTemp(filepath.Dir(s.path), ".archive-rules-*")
	if err != nil {
		return "", err
	}
	tmp := file.Name()
	defer os.Remove(tmp)
	if _, err := file.Write(data); err != nil {
		file.Close()
		return "", err
	}
	if err := file.Sync(); err != nil {
		file.Close()
		return "", err
	}
	if err := file.Close(); err != nil {
		return "", err
	}
	if err := os.Rename(tmp, s.path); err != nil {
		return "", err
	}
	return digestBytes(data), nil
}

func digestBytes(data []byte) string {
	digest := sha256.Sum256(data)
	return hex.EncodeToString(digest[:])
}

var declarativeRuleID = regexp.MustCompile(`^[a-z][a-z0-9_]{0,63}$`)

func CompileDeclarativeRules(set DeclarativeRuleSet, builtins []Rule) ([]Rule, error) {
	if len(set.Drafts) > maxDeclarativeRules || len(set.History) > 100 || len(set.Audit) > 1000 {
		return nil, errors.New("draft/history/audit retention limits exceeded")
	}
	if set.SchemaVersion != 0 && set.SchemaVersion != DeclarativeRulesSchemaVersion {
		return nil, fmt.Errorf("unsupported declarative rules schema_version %d", set.SchemaVersion)
	}
	if len(set.Rules) > maxDeclarativeRules {
		return nil, fmt.Errorf("too many declarative rules: %d > %d", len(set.Rules), maxDeclarativeRules)
	}
	reserved := make(map[string]bool, len(builtins))
	for _, rule := range builtins {
		reserved[rule.ID] = true
	}
	seen := make(map[string]bool, len(set.Rules))
	compiled := make([]Rule, 0, len(set.Rules))
	for index := range set.Rules {
		definition := cloneDeclarativeRule(set.Rules[index])
		if !declarativeRuleID.MatchString(definition.ID) {
			return nil, fmt.Errorf("rule[%d]: invalid id %q", index, definition.ID)
		}
		if reserved[definition.ID] {
			return nil, fmt.Errorf("rule %q conflicts with a built-in rule", definition.ID)
		}
		if seen[definition.ID] {
			return nil, fmt.Errorf("duplicate declarative rule %q", definition.ID)
		}
		seen[definition.ID] = true
		if err := validateDeclarativeRule(definition); err != nil {
			return nil, fmt.Errorf("rule %q: %w", definition.ID, err)
		}
		if !definition.Enabled {
			continue
		}
		rule, err := compileDeclarativeRule(definition)
		if err != nil {
			return nil, fmt.Errorf("rule %q: %w", definition.ID, err)
		}
		compiled = append(compiled, rule)
	}
	return compiled, nil
}

func validateDeclarativeRule(rule DeclarativeRule) error {
	if rule.MatchMode != "" && rule.MatchMode != "transition" && rule.MatchMode != "state" {
		return errors.New("match_mode must be transition or state")
	}
	if rule.Version <= 0 || rule.Version > 2147483647 {
		return errors.New("version must be a positive int32")
	}
	if !validCategory(rule.Category) {
		return fmt.Errorf("invalid category %q", rule.Category)
	}
	if !validTrigger(rule.Trigger) {
		return fmt.Errorf("invalid trigger %q", rule.Trigger)
	}
	if !validRarity(rule.Rarity) {
		return fmt.Errorf("invalid rarity %q", rule.Rarity)
	}
	if rule.Importance < 0 || rule.Importance > 100 {
		return errors.New("importance must be between 0 and 100")
	}
	if err := validateLocalizedCopy("titles", rule.Titles); err != nil {
		return err
	}
	if err := validateLocalizedCopy("descriptions", rule.Descriptions); err != nil {
		return err
	}
	nodes := 0
	if err := validateCondition(rule.Condition, 1, &nodes); err != nil {
		return err
	}
	return validateDedupe(rule.Dedupe)
}

func validateLocalizedCopy(name string, values map[string]string) error {
	if len(values) > 8 {
		return errors.New("at most 8 locales are supported")
	}
	if strings.TrimSpace(values["zh-CN"]) == "" {
		return fmt.Errorf("%s.zh-CN is required", name)
	}
	for locale, value := range values {
		if len(locale) > 16 || len(value) > 512 || strings.TrimSpace(value) == "" {
			return fmt.Errorf("invalid %s entry for locale %q", name, locale)
		}
	}
	return nil
}

func validateCondition(condition DeclarativeCondition, depth int, nodes *int) error {
	(*nodes)++
	if depth > maxConditionDepth {
		return fmt.Errorf("condition depth exceeds %d", maxConditionDepth)
	}
	if *nodes > maxConditionNodes {
		return fmt.Errorf("condition count exceeds %d", maxConditionNodes)
	}
	groups := 0
	if len(condition.All) > 0 {
		groups++
	}
	if len(condition.Any) > 0 {
		groups++
	}
	if condition.Metric != "" || condition.Operator != "" {
		groups++
	}
	if groups != 1 {
		return errors.New("condition must contain exactly one of all, any, or metric/operator")
	}
	children := condition.All
	if len(condition.Any) > 0 {
		children = condition.Any
	}
	if len(children) > 0 {
		if condition.Value != 0 || len(condition.Values) != 0 {
			return errors.New("group conditions cannot contain values")
		}
		for _, child := range children {
			if err := validateCondition(child, depth+1, nodes); err != nil {
				return err
			}
		}
		return nil
	}
	if !validMetric(condition.Metric) {
		return fmt.Errorf("invalid metric %q", condition.Metric)
	}
	if !validOperator(condition.Operator) {
		return fmt.Errorf("invalid operator %q", condition.Operator)
	}
	if condition.Value < 0 || condition.Value > 1e12 || math.IsNaN(condition.Value) || math.IsInf(condition.Value, 0) {
		return errors.New("condition value must be finite")
	}
	for _, value := range condition.Values {
		if value < 0 || value > 1e12 || math.IsNaN(value) || math.IsInf(value, 0) {
			return errors.New("condition values must be finite")
		}
	}
	if condition.Operator == "between" && len(condition.Values) != 2 {
		return errors.New("between requires exactly two values")
	}
	if condition.Operator == "between" && condition.Values[0] > condition.Values[1] {
		return errors.New("between bounds must be ascending")
	}
	if condition.Operator != "between" && condition.Operator != "in" && len(condition.Values) > 0 {
		return errors.New("values is only supported by between and in")
	}
	if condition.Operator == "in" && (len(condition.Values) == 0 || len(condition.Values) > 32) {
		return errors.New("in requires between 1 and 32 values")
	}
	return nil
}

func validateDedupe(dedupe DeclarativeDedupe) error {
	if dedupe.Scope == "" {
		dedupe.Scope = "room"
	}
	if dedupe.Mode == "" {
		dedupe.Mode = "forever"
	}
	if dedupe.Scope != "room" && dedupe.Scope != "member" && dedupe.Scope != "pair" {
		return fmt.Errorf("invalid dedupe scope %q", dedupe.Scope)
	}
	if dedupe.Mode != "forever" && dedupe.Mode != "cooldown" {
		return fmt.Errorf("invalid dedupe mode %q", dedupe.Mode)
	}
	if dedupe.Mode == "cooldown" && dedupe.CooldownSeconds <= 0 {
		return errors.New("cooldown mode requires cooldown_seconds > 0")
	}
	if dedupe.CooldownSeconds < 0 || (dedupe.Mode == "forever" && dedupe.CooldownSeconds != 0) {
		return errors.New("forever mode must not specify a cooldown")
	}
	if dedupe.CooldownSeconds > int64((10*365*24*time.Hour)/time.Second) {
		return errors.New("cooldown_seconds exceeds 10 years")
	}
	return nil
}

func compileDeclarativeRule(definition DeclarativeRule) (Rule, error) {
	predicate, err := compileCondition(definition.Condition)
	if err != nil {
		return Rule{}, err
	}
	if definition.MatchMode != "state" {
		current := predicate
		predicate = func(ctx EvalContext) bool { return current(ctx) && !current(previousContext(ctx)) }
	}
	dedupe := definition.Dedupe
	if dedupe.Scope == "" {
		dedupe.Scope = "room"
	}
	if dedupe.Mode == "" {
		dedupe.Mode = "forever"
	}
	rule := Rule{
		ID: definition.ID, EventType: definition.ID, Version: definition.Version,
		Category: definition.Category, Trigger: definition.Trigger, Rarity: definition.Rarity,
		Importance: definition.Importance, Titles: cloneStringMap(definition.Titles), Descriptions: cloneStringMap(definition.Descriptions),
		Dedupe: DedupeForever, Predicate: predicate,
	}
	rule.Facts = func(ctx EvalContext) map[string]any {
		metrics := make(map[string]any)
		var visit func(DeclarativeCondition)
		visit = func(condition DeclarativeCondition) {
			if condition.Metric != "" {
				before, after := metricValues(condition.Metric, ctx)
				metrics[condition.Metric] = []float64{before, after}
			}
			for _, child := range condition.All {
				visit(child)
			}
			for _, child := range condition.Any {
				visit(child)
			}
		}
		visit(definition.Condition)
		return map[string]any{"metrics": metrics, "condition": cloneCondition(definition.Condition), "match_mode": definition.MatchMode}
	}
	if dedupe.Mode == "cooldown" {
		rule.Dedupe = DedupeCooldown
		rule.Cooldown = time.Duration(dedupe.CooldownSeconds) * time.Second
	}
	switch dedupe.Scope {
	case "member":
		rule.Contexts = func(ctx EvalContext) []EvalContext {
			if ctx.MemberID != "" {
				return []EvalContext{ctx}
			}
			result := make([]EvalContext, 0, len(ctx.Participants))
			for _, id := range ctx.Participants {
				subject := ctx
				subject.MemberID = id
				result = append(result, subject)
			}
			return result
		}
		original := rule.Predicate
		rule.Predicate = func(ctx EvalContext) bool { return ctx.MemberID != "" && original(ctx) }
		rule.Key = func(ctx EvalContext) string { return ctx.MemberID }
		rule.Actors = func(ctx EvalContext) []string { return []string{ctx.MemberID} }
	case "pair":
		rule.Contexts = func(ctx EvalContext) []EvalContext {
			members := append([]string(nil), ctx.Participants...)
			sort.Strings(members)
			var result []EvalContext
			for i := 0; i < len(members); i++ {
				for j := i + 1; j < len(members); j++ {
					subject := ctx
					subject.SubjectPair = []string{members[i], members[j]}
					result = append(result, subject)
				}
			}
			return result
		}
		original := rule.Predicate
		rule.Predicate = func(ctx EvalContext) bool { return len(ctx.SubjectPair) == 2 && original(ctx) }
		rule.Key = func(ctx EvalContext) string {
			return strings.Join(ctx.SubjectPair, "\x00")
		}
		rule.Actors = func(ctx EvalContext) []string { return append([]string(nil), ctx.SubjectPair...) }
	}
	return rule, nil
}

func previousContext(ctx EvalContext) EvalContext {
	ctx.After = ctx.Before
	ctx.Participants = ctx.PreviousParticipants
	ctx.ConcurrentSpeakers = ctx.PreviousConcurrentSpeakers
	ctx.SessionDuration = ctx.PreviousSessionDuration
	ctx.LastTwoDuration = ctx.PreviousLastTwoDuration
	ctx.LastOneDuration = ctx.PreviousLastOneDuration
	ctx.SilenceDuration = ctx.PreviousSilenceDuration
	if !ctx.PreviousTime.IsZero() {
		ctx.Now = ctx.PreviousTime
	}
	return ctx
}

func compileCondition(condition DeclarativeCondition) (func(EvalContext) bool, error) {
	if len(condition.All) > 0 {
		children := make([]func(EvalContext) bool, 0, len(condition.All))
		for _, child := range condition.All {
			compiled, err := compileCondition(child)
			if err != nil {
				return nil, err
			}
			children = append(children, compiled)
		}
		return func(ctx EvalContext) bool {
			for _, child := range children {
				if !child(ctx) {
					return false
				}
			}
			return true
		}, nil
	}
	if len(condition.Any) > 0 {
		children := make([]func(EvalContext) bool, 0, len(condition.Any))
		for _, child := range condition.Any {
			compiled, err := compileCondition(child)
			if err != nil {
				return nil, err
			}
			children = append(children, compiled)
		}
		return func(ctx EvalContext) bool {
			for _, child := range children {
				if child(ctx) {
					return true
				}
			}
			return false
		}, nil
	}
	metric, operator, value, values := condition.Metric, condition.Operator, condition.Value, append([]float64(nil), condition.Values...)
	return func(ctx EvalContext) bool {
		before, after := metricValues(metric, ctx)
		switch operator {
		case "eq":
			return after == value
		case "gt":
			return after > value
		case "gte":
			return after >= value
		case "lt":
			return after < value
		case "lte":
			return after <= value
		case "between":
			return after >= values[0] && after <= values[1]
		case "crosses":
			return before < value && after >= value
		case "changed":
			return before != after
		case "in":
			for _, candidate := range values {
				if after == candidate {
					return true
				}
			}
		}
		return false
	}, nil
}

func metricValues(metric string, ctx EvalContext) (float64, float64) {
	switch metric {
	case "active_voice_seconds":
		return float64(ctx.Before.ActiveVoiceSeconds), float64(ctx.After.ActiveVoiceSeconds)
	case "room_age_days":
		previous := ctx.PreviousTime
		if previous.IsZero() {
			previous = ctx.Now
		}
		return float64(ageDaysAt(ctx.Before, previous)), float64(ageDaysAt(ctx.After, ctx.Now))
	case "active_days":
		return float64(len(ctx.Before.ActiveDays)), float64(len(ctx.After.ActiveDays))
	case "distinct_members":
		return float64(len(ctx.Before.Members)), float64(len(ctx.After.Members))
	case "participant_count":
		return float64(len(ctx.PreviousParticipants)), float64(len(ctx.Participants))
	case "concurrent_speakers":
		return float64(ctx.PreviousConcurrentSpeakers), float64(ctx.ConcurrentSpeakers)
	case "session_count":
		return float64(ctx.Before.SessionCount), float64(ctx.After.SessionCount)
	case "session_duration_seconds":
		return ctx.PreviousSessionDuration.Seconds(), ctx.SessionDuration.Seconds()
	case "silence_seconds":
		return ctx.PreviousSilenceDuration.Seconds(), ctx.SilenceDuration.Seconds()
	case "last_two_duration_seconds":
		return ctx.PreviousLastTwoDuration.Seconds(), ctx.LastTwoDuration.Seconds()
	case "last_one_duration_seconds":
		return ctx.PreviousLastOneDuration.Seconds(), ctx.LastOneDuration.Seconds()
	case "member_voice_sessions":
		return float64(ctx.Before.Members[ctx.MemberID].VoiceSessions), float64(ctx.After.Members[ctx.MemberID].VoiceSessions)
	case "pair_session_count":
		before, after := uint64(0), uint64(0)
		if len(ctx.SubjectPair) == 2 {
			key := pairKey(ctx.SubjectPair[0], ctx.SubjectPair[1])
			before, after = ctx.Before.Pairs[key].Sessions, ctx.After.Pairs[key].Sessions
		} else {
			for _, pair := range ctx.Before.Pairs {
				before = max(before, pair.Sessions)
			}
			for _, pair := range ctx.After.Pairs {
				after = max(after, pair.Sessions)
			}
		}
		return float64(before), float64(after)
	case "text_message_count":
		return float64(ctx.Before.TextMessageCount), float64(ctx.After.TextMessageCount)
	case "local_hour":
		previous := ctx.PreviousTime
		if previous.IsZero() {
			previous = ctx.Now
		}
		return float64(previous.Hour()), float64(ctx.Now.Hour())
	case "consecutive_active_days":
		return float64(consecutiveDays(ctx.Before.ActiveDays, ctx.Now)), float64(consecutiveDays(ctx.After.ActiveDays, ctx.Now))
	}
	return 0, 0
}

func validMetric(metric string) bool {
	switch metric {
	case "active_voice_seconds", "room_age_days", "active_days", "consecutive_active_days", "distinct_members", "participant_count", "concurrent_speakers", "session_count", "session_duration_seconds", "silence_seconds", "last_two_duration_seconds", "last_one_duration_seconds", "member_voice_sessions", "pair_session_count", "text_message_count", "local_hour":
		return true
	default:
		return false
	}
}

func validOperator(operator string) bool {
	switch operator {
	case "eq", "gt", "gte", "lt", "lte", "between", "crosses", "changed", "in":
		return true
	default:
		return false
	}
}

func validTrigger(trigger Trigger) bool {
	switch trigger {
	case TriggerCreated, TriggerJoin, TriggerVoice, TriggerText, TriggerLeave, TriggerSessionEnd, TriggerTick:
		return true
	default:
		return false
	}
}

func validCategory(category Category) bool {
	switch category {
	case Origin, Voice, Social, Time, Return, Silence, Scale, Text, RareEvent, Eden:
		return true
	default:
		return false
	}
}

func validRarity(rarity Rarity) bool {
	switch rarity {
	case Common, Uncommon, Rare, VeryRare, Legendary:
		return true
	default:
		return false
	}
}

func cloneDeclarativeRuleSet(source DeclarativeRuleSet) DeclarativeRuleSet {
	target := DeclarativeRuleSet{SchemaVersion: source.SchemaVersion, Revision: source.Revision, Rules: make([]DeclarativeRule, len(source.Rules))}
	for index := range source.Rules {
		target.Rules[index] = cloneDeclarativeRule(source.Rules[index])
	}
	target.Drafts = make([]DeclarativeRule, len(source.Drafts))
	for i := range source.Drafts {
		target.Drafts[i] = cloneDeclarativeRule(source.Drafts[i])
	}
	target.History = make([]DeclarativeRule, len(source.History))
	for i := range source.History {
		target.History[i] = cloneDeclarativeRule(source.History[i])
	}
	target.Audit = append([]RuleAudit(nil), source.Audit...)
	return target
}

func cloneDeclarativeRule(source DeclarativeRule) DeclarativeRule {
	source.Titles = cloneStringMap(source.Titles)
	source.Descriptions = cloneStringMap(source.Descriptions)
	source.Condition = cloneCondition(source.Condition)
	return source
}

func cloneCondition(source DeclarativeCondition) DeclarativeCondition {
	source.Values = append([]float64(nil), source.Values...)
	source.All = append([]DeclarativeCondition(nil), source.All...)
	for index := range source.All {
		source.All[index] = cloneCondition(source.All[index])
	}
	source.Any = append([]DeclarativeCondition(nil), source.Any...)
	for index := range source.Any {
		source.Any[index] = cloneCondition(source.Any[index])
	}
	return source
}

func cloneStringMap(source map[string]string) map[string]string {
	if len(source) == 0 {
		return nil
	}
	target := make(map[string]string, len(source))
	for key, value := range source {
		target[key] = value
	}
	return target
}

func upsertRule(set DeclarativeRuleSet, rule DeclarativeRule) DeclarativeRuleSet {
	set = cloneDeclarativeRuleSet(set)
	updated := false
	for index := range set.Rules {
		if set.Rules[index].ID == rule.ID {
			set.Rules[index] = cloneDeclarativeRule(rule)
			updated = true
			break
		}
	}
	if !updated {
		set.Rules = append(set.Rules, cloneDeclarativeRule(rule))
	}
	sort.Slice(set.Rules, func(i, j int) bool { return set.Rules[i].ID < set.Rules[j].ID })
	set.Revision++
	return set
}

func deleteRule(set DeclarativeRuleSet, id string) (DeclarativeRuleSet, bool) {
	set = cloneDeclarativeRuleSet(set)
	for index := range set.Rules {
		if set.Rules[index].ID == id {
			set.Rules = append(set.Rules[:index], set.Rules[index+1:]...)
			set.Revision++
			return set, true
		}
	}
	return set, false
}

func defaultRulesPath(archivePath string) string {
	return filepath.Join(filepath.Dir(archivePath), "eden-archive-rules.json")
}

func validateRuleVersions(current, next DeclarativeRuleSet) error {
	for _, candidate := range next.Rules {
		for _, active := range current.Rules {
			if active.ID == candidate.ID && !reflect.DeepEqual(active, candidate) && candidate.Version <= active.Version {
				return fmt.Errorf("rule %q changed without a higher version", candidate.ID)
			}
		}
	}
	return nil
}
