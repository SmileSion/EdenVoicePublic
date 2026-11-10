package archive

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"edenvoice/server/internal/privatefs"
)

type ActivityKind string

const (
	Created     ActivityKind = "created"
	Joined      ActivityKind = "joined"
	Left        ActivityKind = "left"
	VoicePacket ActivityKind = "voice_packet"
	TextMessage ActivityKind = "text_message"
	Tick        ActivityKind = "tick"
)

type Activity struct {
	Movie    string
	Kind     ActivityKind
	RoomID   uint64
	RoomName string
	MemberID string
	At       time.Time
}

type roomRuntime struct {
	movies           []string
	lastEvalAt       time.Time
	participants     map[string]time.Time
	voiceMembers     map[string]time.Time
	sessionStarted   time.Time
	emptySince       time.Time
	lastActiveSecond time.Time
	lastTwoSince     time.Time
	lastOneSince     time.Time
	wasGroup         bool
	sessionMembers   map[string]bool
	returningMembers map[string]time.Time
	hadVoice         bool
}

type Processor struct {
	games    map[gameKey]*gameSession
	serverID string
	grace    time.Duration
	engine   *Engine
	state    persistedState
	runtime  map[uint64]*roomRuntime
	// Mutations are serialized by Collector.mu. Runtime-only timer updates do
	// not dirty the persisted archive.
	revision uint64
}

func NewProcessor(serverID string, grace time.Duration, state persistedState) *Processor {
	return NewProcessorWithRules(serverID, grace, state, DefaultRules())
}

func NewProcessorWithRules(serverID string, grace time.Duration, state persistedState, rules []Rule) *Processor {
	if grace <= 0 {
		grace = 30 * time.Second
	}
	state = normalizeState(state)
	return &Processor{serverID: serverID, grace: grace, engine: NewEngine(rules), state: state, runtime: make(map[uint64]*roomRuntime), revision: 1}
}

func (p *Processor) ReplaceRules(rules []Rule) { p.engine = NewEngine(rules) }

func (p *Processor) Apply(activity Activity) []Event {
	if activity.RoomID == 0 {
		return nil
	}
	if activity.At.IsZero() {
		activity.At = time.Now()
	}
	if activity.Kind == "watch_played" {
		p.observeMovie(activity)
		return nil
	}
	before := p.room(activity.RoomID, activity.RoomName, activity.At)
	rt := p.roomRuntime(activity.RoomID)
	// Before/After are synchronous read-only rule views. Preserve only maps
	// this activity can mutate; copying historical pairs on every voice sample
	// or idle tick otherwise makes work proportional to the entire history.
	before = snapshotBeforeActivity(before, activity.Kind, rt.hadVoice && !rt.emptySince.IsZero() && activity.At.Sub(rt.emptySince) >= p.grace && !rt.sessionStarted.IsZero())
	if activity.Kind != Tick {
		p.revision++
	}
	previousAt := rt.lastEvalAt
	if previousAt.IsZero() {
		previousAt = activity.At
	}
	previousParticipants := participantIDs(rt.participants)
	previousSpeaking := len(rt.voiceMembers)
	previousSession, previousTwo, previousOne := time.Duration(0), time.Duration(0), time.Duration(0)
	if rt.hadVoice && !rt.sessionStarted.IsZero() {
		previousSession = previousAt.Sub(rt.sessionStarted)
	}
	if !rt.lastTwoSince.IsZero() {
		previousTwo = previousAt.Sub(rt.lastTwoSince)
	}
	if !rt.lastOneSince.IsZero() {
		previousOne = previousAt.Sub(rt.lastOneSince)
	}
	for id, last := range rt.voiceMembers {
		if activity.At.Sub(last) > 1500*time.Millisecond {
			delete(rt.voiceMembers, id)
		}
	}
	var ended []Event
	trigger := mapTrigger(activity.Kind)
	var silence time.Duration
	lastRelevantActivity := before.LastActiveAt
	if activity.Kind == VoicePacket && !before.LastVoiceAt.IsZero() {
		lastRelevantActivity = before.LastVoiceAt
	}
	if !lastRelevantActivity.IsZero() && activity.At.After(lastRelevantActivity) {
		silence = activity.At.Sub(lastRelevantActivity)
	}
	var memberSilence time.Duration
	if member := before.Members[activity.MemberID]; !member.LastSeenAt.IsZero() && activity.At.After(member.LastSeenAt) {
		memberSilence = activity.At.Sub(member.LastSeenAt)
	}
	switch activity.Kind {
	case Created:
	case Joined:
		if activity.MemberID == "" {
			return nil
		}
		if !rt.emptySince.IsZero() && activity.At.Sub(rt.emptySince) >= p.grace {
			ended = p.finishIfExpired(activity.RoomID, activity.At)
			before = snapshotBeforeActivity(p.state.Rooms[activity.RoomID], Joined, false)
		}
		if len(rt.participants) == 0 && rt.sessionStarted.IsZero() {
			rt.sessionStarted = activity.At
			rt.sessionMembers = make(map[string]bool)
		}
		rt.emptySince = time.Time{}
		rt.participants[activity.MemberID] = activity.At
		rt.sessionMembers[activity.MemberID] = true
		for id, returnedAt := range rt.returningMembers {
			if activity.At.Sub(returnedAt) > 10*time.Minute {
				delete(rt.returningMembers, id)
			}
		}
		if memberSilence >= 7*24*time.Hour {
			rt.returningMembers[activity.MemberID] = activity.At
		}
		stats := p.state.Rooms[activity.RoomID]
		if stats.FirstMemberJoinAt.IsZero() {
			stats.FirstMemberJoinAt = activity.At
		}
		member := stats.Members[activity.MemberID]
		if member.FirstSeenAt.IsZero() {
			member.FirstSeenAt = activity.At
		}
		member.LastSeenAt = activity.At
		stats.Members[activity.MemberID] = member
		stats.MaxConcurrentMembers = max(stats.MaxConcurrentMembers, len(rt.participants))
		p.state.Rooms[activity.RoomID] = stats
	case Left:
		joinedAt, ok := rt.participants[activity.MemberID]
		if ok && activity.At.After(joinedAt) {
			stats := p.state.Rooms[activity.RoomID]
			stats.PresenceSeconds += int64(activity.At.Sub(joinedAt) / time.Second)
			p.state.Rooms[activity.RoomID] = stats
		}
		delete(rt.participants, activity.MemberID)
		delete(rt.voiceMembers, activity.MemberID)
		if len(rt.participants) == 0 {
			rt.emptySince = activity.At
		}
	case VoicePacket:
		if activity.MemberID == "" {
			return nil
		}
		second := activity.At.Truncate(time.Second)
		rt.hadVoice = true
		stats := p.state.Rooms[activity.RoomID]
		if int64(silence/time.Second) > stats.LongestSilenceSeconds {
			stats.LongestSilenceSeconds = int64(silence / time.Second)
		}
		if silence >= 7*24*time.Hour {
			stats.RestartCount++
		}
		if stats.FirstVoiceAt.IsZero() {
			stats.FirstVoiceAt = activity.At
		}
		if !rt.lastActiveSecond.Equal(second) {
			stats.ActiveVoiceSeconds++
			stats.ActiveDays[dayStart(activity.At).Format("2006-01-02")] = true
			stats.ActiveHours[activity.At.Hour()]++
			rt.lastActiveSecond = second
		}
		member := stats.Members[activity.MemberID]
		if member.FirstSeenAt.IsZero() {
			member.FirstSeenAt = activity.At
		}
		member.LastSeenAt = activity.At
		member.LastVoiceAt = activity.At
		member.SpeakingSeconds++
		stats.Members[activity.MemberID] = member
		stats.SpeakingSeconds++
		stats.LastActiveAt = activity.At
		stats.LastVoiceAt = activity.At
		rt.voiceMembers[activity.MemberID] = activity.At
		for id, last := range rt.voiceMembers {
			if activity.At.Sub(last) > 1500*time.Millisecond {
				delete(rt.voiceMembers, id)
			}
		}
		stats.MaxConcurrentSpeaking = max(stats.MaxConcurrentSpeaking, len(rt.voiceMembers))
		if activity.At.Hour() >= 5 && activity.At.Hour() < 7 && (stats.EarliestMorningVoiceAt.IsZero() || clockSeconds(activity.At) < clockSeconds(stats.EarliestMorningVoiceAt)) {
			stats.EarliestMorningVoiceAt = activity.At
		}
		p.state.Rooms[activity.RoomID] = stats
	case TextMessage:
		stats := p.state.Rooms[activity.RoomID]
		if stats.FirstTextAt.IsZero() {
			stats.FirstTextAt = activity.At
		}
		stats.TextMessageCount++
		stats.LastActiveAt = activity.At
		p.state.Rooms[activity.RoomID] = stats
	case Tick:
		ended = p.finishIfExpired(activity.RoomID, activity.At)
	default:
		return nil
	}
	p.updateRemaining(activity.RoomID, activity.At)
	after := p.state.Rooms[activity.RoomID]
	participants := participantIDs(rt.participants)
	ctx := EvalContext{Trigger: trigger, Now: activity.At, Before: before, After: after, Participants: participants, ReturningMembers: participantIDs(rt.returningMembers), SilenceDuration: silence, MemberSilence: memberSilence, MemberID: activity.MemberID, WasGroup: rt.wasGroup}
	ctx.PreviousTime = previousAt
	ctx.PreviousParticipants = previousParticipants
	ctx.PreviousConcurrentSpeakers, ctx.ConcurrentSpeakers = previousSpeaking, len(rt.voiceMembers)
	ctx.PreviousSessionDuration, ctx.PreviousLastTwoDuration, ctx.PreviousLastOneDuration = previousSession, previousTwo, previousOne
	if !lastRelevantActivity.IsZero() && previousAt.After(lastRelevantActivity) {
		ctx.PreviousSilenceDuration = previousAt.Sub(lastRelevantActivity)
	}
	if rt.hadVoice && !rt.sessionStarted.IsZero() {
		ctx.SessionDuration = activity.At.Sub(rt.sessionStarted)
	}
	if !rt.lastTwoSince.IsZero() {
		ctx.LastTwoDuration = activity.At.Sub(rt.lastTwoSince)
	}
	if !rt.lastOneSince.IsZero() {
		ctx.LastOneDuration = activity.At.Sub(rt.lastOneSince)
	}
	events := p.engine.Evaluate(activity.RoomID, p.serverID, ctx, p.state.Dedupe)
	rt.lastEvalAt = activity.At
	if len(events) > 0 {
		p.state.Events[activity.RoomID] = append(p.state.Events[activity.RoomID], events...)
		p.revision++
	}
	return append(ended, events...)
}

func (p *Processor) updateRemaining(roomID uint64, at time.Time) {
	rt := p.roomRuntime(roomID)
	count := len(rt.participants)
	if count >= 3 {
		rt.wasGroup = true
		rt.lastTwoSince = time.Time{}
		rt.lastOneSince = time.Time{}
		return
	}
	if !rt.wasGroup {
		return
	}
	if count == 2 {
		if rt.lastTwoSince.IsZero() {
			rt.lastTwoSince = at
		}
		rt.lastOneSince = time.Time{}
	} else if count == 1 {
		if rt.lastOneSince.IsZero() {
			rt.lastOneSince = at
		}
		rt.lastTwoSince = time.Time{}
	} else {
		rt.lastOneSince = time.Time{}
		rt.lastTwoSince = time.Time{}
	}
}

func (p *Processor) finishIfExpired(roomID uint64, at time.Time) []Event {
	rt := p.roomRuntime(roomID)
	if rt.emptySince.IsZero() || at.Sub(rt.emptySince) < p.grace || rt.sessionStarted.IsZero() {
		return nil
	}
	if !rt.hadVoice {
		ids := make([]string, 0, len(rt.sessionMembers))
		for id := range rt.sessionMembers {
			ids = append(ids, id)
		}
		sort.Strings(ids)
		var evenings []Event
		if recap := p.evening(roomID, rt, ids); recap != nil && (len(recap.Evening.Games) > 0 || len(recap.Evening.Movies) > 0) {
			evenings = append(evenings, *recap)
			p.state.Events[roomID] = append(p.state.Events[roomID], *recap)
			p.revision++
		}
		rt.movies = nil
		rt.sessionStarted = time.Time{}
		rt.emptySince = time.Time{}
		rt.wasGroup = false
		rt.sessionMembers = make(map[string]bool)
		return evenings
	}
	stats := p.state.Rooms[roomID]
	before := snapshotBeforeActivity(stats, Tick, true)
	var summary sessionSummary
	if stats.sessionSummary == nil {
		summary = summarizeSessions(stats)
	} else {
		summary = *stats.sessionSummary
	}
	duration := rt.emptySince.Sub(rt.sessionStarted)
	stats.SessionCount++
	stats.LongestSessionSeconds = max(stats.LongestSessionSeconds, int64(duration/time.Second))
	stats.LatestSessionEndedAt = rt.emptySince
	ids := make([]string, 0, len(rt.sessionMembers))
	for id := range rt.sessionMembers {
		ids = append(ids, id)
	}
	sort.Strings(ids)
	for _, id := range ids {
		m := stats.Members[id]
		m.VoiceSessions++
		summary.maxMemberSessions = max(summary.maxMemberSessions, m.VoiceSessions)
		stats.Members[id] = m
	}
	for i := 0; i < len(ids); i++ {
		for j := i + 1; j < len(ids); j++ {
			key := pairKey(ids[i], ids[j])
			pair := stats.Pairs[key]
			pair.Members = []string{ids[i], ids[j]}
			pair.Sessions++
			summary.maxPairSessions = max(summary.maxPairSessions, pair.Sessions)
			pair.LastSeenAt = rt.emptySince
			stats.Pairs[key] = pair
		}
	}
	stats.sessionSummary = &summary
	p.state.Rooms[roomID] = stats
	p.revision++
	ctx := EvalContext{Trigger: TriggerSessionEnd, Now: at, PreviousTime: at, Before: before, After: stats, Participants: ids, PreviousParticipants: ids, SessionDuration: duration}
	events := p.engine.Evaluate(roomID, p.serverID, ctx, p.state.Dedupe)
	if recap := p.evening(roomID, rt, ids); recap != nil {
		events = append(events, *recap)
	}
	p.state.Events[roomID] = append(p.state.Events[roomID], events...)
	rt.movies = nil
	rt.sessionStarted = time.Time{}
	rt.emptySince = time.Time{}
	rt.wasGroup = false
	rt.hadVoice = false
	rt.sessionMembers = make(map[string]bool)
	return events
}

func (p *Processor) Snapshot(roomID uint64, all bool, limit int, now time.Time) RoomSnapshot {
	if now.IsZero() {
		now = time.Now()
	}
	p.Apply(Activity{Kind: Tick, RoomID: roomID, At: now})
	stats := cloneStats(p.state.Rooms[roomID])
	events := append([]Event(nil), p.state.Events[roomID]...)
	sort.SliceStable(events, func(i, j int) bool { return events[i].OccurredAt.After(events[j].OccurredAt) })
	if !all {
		filtered := events[:0]
		for _, event := range events {
			if event.Importance >= 50 {
				filtered = append(filtered, event)
			}
		}
		events = filtered
	}
	if limit <= 0 || limit > 100 {
		limit = 100
	}
	hasMore := len(events) > limit
	if hasMore {
		events = events[:limit]
	}
	return RoomSnapshot{Stats: stats, AgeDays: ageDaysAt(stats, now), Events: events, HasMore: hasMore, Enabled: true}
}

func (p *Processor) room(id uint64, name string, at time.Time) RoomStats {
	stats, ok := p.state.Rooms[id]
	if !ok {
		stats = RoomStats{RoomID: id, RoomName: name, CreatedAt: at, ActiveDays: map[string]bool{}, ActiveHours: map[int]uint64{}, Members: map[string]MemberStats{}, Pairs: map[string]PairStats{}}
		stats.sessionSummary = &sessionSummary{}
		p.state.Rooms[id] = stats
		p.revision++
	}
	if name != "" && stats.RoomName != name {
		stats.RoomName = name
		p.state.Rooms[id] = stats
		p.revision++
	}
	return stats
}
func (p *Processor) roomRuntime(id uint64) *roomRuntime {
	rt := p.runtime[id]
	if rt == nil {
		rt = &roomRuntime{participants: map[string]time.Time{}, voiceMembers: map[string]time.Time{}, sessionMembers: map[string]bool{}, returningMembers: map[string]time.Time{}}
		p.runtime[id] = rt
	}
	return rt
}
func participantIDs(values map[string]time.Time) []string {
	result := make([]string, 0, len(values))
	for id := range values {
		result = append(result, id)
	}
	sort.Strings(result)
	return result
}
func pairKey(a, b string) string {
	if a > b {
		a, b = b, a
	}
	return a + "\x00" + b
}
func clockSeconds(v time.Time) int { return v.Hour()*3600 + v.Minute()*60 + v.Second() }
func mapTrigger(kind ActivityKind) Trigger {
	switch kind {
	case VoicePacket:
		return TriggerVoice
	case TextMessage:
		return TriggerText
	case Joined:
		return TriggerJoin
	case Left:
		return TriggerLeave
	case Tick:
		return TriggerTick
	default:
		return TriggerCreated
	}
}
func snapshotBeforeActivity(v RoomStats, kind ActivityKind, finishesSession bool) RoomStats {
	if kind == Joined || kind == VoicePacket || finishesSession {
		v.Members = cloneMap(v.Members)
	}
	if kind == VoicePacket {
		v.ActiveDays = cloneMap(v.ActiveDays)
		v.ActiveHours = cloneMap(v.ActiveHours)
	}
	if finishesSession {
		v.Pairs = cloneMap(v.Pairs)
	}
	return v
}

func cloneStats(v RoomStats) RoomStats {
	v.ActiveDays = cloneMap(v.ActiveDays)
	v.ActiveHours = cloneMap(v.ActiveHours)
	v.Members = cloneMap(v.Members)
	v.Pairs = cloneMap(v.Pairs)
	return v
}
func cloneMap[K comparable, V any](source map[K]V) map[K]V {
	target := make(map[K]V, len(source))
	for k, v := range source {
		target[k] = v
	}
	return target
}

type Collector struct {
	gameQueue    chan gameSample
	mu           sync.RWMutex
	rulesUpdate  sync.Mutex
	path         string
	processor    *Processor
	queue        chan Activity
	stop         chan struct{}
	done         chan struct{}
	voiceSamples sync.Map
	ruleStore    RuleRepository
	ruleSet      DeclarativeRuleSet
	ruleDigest   string
	ruleStamp    string
	ruleError    string
	// Only the run goroutine (and Close after it exits) writes this value.
	persistedRevision uint64
}

type voiceSampleKey struct {
	roomID   uint64
	memberID string
}

func LoadOrCreate(path, serverID string) (*Collector, error) {
	return LoadOrCreateWithRulesPath(path, serverID, defaultRulesPath(path))
}

func LoadOrCreateWithRulesPath(path, serverID, rulesPath string) (*Collector, error) {
	return LoadOrCreateWithRuleRepository(path, serverID, NewDeclarativeRuleStore(rulesPath))
}

func LoadOrCreateWithRuleRepository(path, serverID string, ruleStore RuleRepository) (*Collector, error) {
	if ruleStore == nil {
		return nil, errors.New("rule repository is required")
	}
	state := persistedState{}
	bytes, err := os.ReadFile(path)
	if err == nil {
		if err := privatefs.Ensure(path); err != nil {
			return nil, err
		}
		if err := jsonUnmarshal(bytes, &state); err != nil {
			return nil, fmt.Errorf("decode archive state: %w", err)
		}
	} else if !errors.Is(err, os.ErrNotExist) {
		return nil, err
	}
	ruleStamp, err := ruleStore.Stamp()
	if err != nil {
		return nil, err
	}
	ruleSet, ruleDigest, err := ruleStore.Load()
	if err != nil {
		return nil, err
	}
	compiled, err := CompileDeclarativeRules(ruleSet, DefaultRules())
	if err != nil {
		return nil, fmt.Errorf("compile declarative archive rules: %w", err)
	}
	rules := append(DefaultRules(), compiled...)
	c := &Collector{path: path, processor: NewProcessorWithRules(validateServerID(serverID), 30*time.Second, state, rules), queue: make(chan Activity, 2048), gameQueue: make(chan gameSample, 1), stop: make(chan struct{}), done: make(chan struct{}), ruleStore: ruleStore, ruleSet: cloneDeclarativeRuleSet(ruleSet), ruleDigest: ruleDigest, ruleStamp: ruleStamp}
	go c.run()
	return c, nil
}

func (c *Collector) DeclarativeRules() DeclarativeRuleSet {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return cloneDeclarativeRuleSet(c.ruleSet)
}

func (c *Collector) DeclarativeRuleError() string {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.ruleError
}

func (c *Collector) ReplaceDeclarativeRules(set DeclarativeRuleSet) error {
	c.rulesUpdate.Lock()
	defer c.rulesUpdate.Unlock()
	return c.replaceDeclarativeRulesLocked(set)
}

func (c *Collector) replaceDeclarativeRulesLocked(set DeclarativeRuleSet) error {
	c.mu.RLock()
	currentRevision := c.ruleSet.Revision
	currentSet := cloneDeclarativeRuleSet(c.ruleSet)
	c.mu.RUnlock()
	set = cloneDeclarativeRuleSet(set)
	if set.SchemaVersion == 0 {
		set.SchemaVersion = DeclarativeRulesSchemaVersion
	}
	if set.Revision <= currentRevision {
		set.Revision = currentRevision + 1
	}
	compiled, err := CompileDeclarativeRules(set, DefaultRules())
	if err != nil {
		return err
	}
	if err := validateRuleVersions(currentSet, set); err != nil {
		return err
	}
	digest, err := c.ruleStore.Save(set)
	if err != nil {
		return err
	}
	// Force one subsequent check: an external writer may replace the document
	// immediately after Save, so a post-save stat cannot describe our snapshot.
	c.installDeclarativeRules(set, compiled, digest, "pending-check", "")
	return nil
}

func (c *Collector) UpsertDeclarativeRule(rule DeclarativeRule) error {
	c.rulesUpdate.Lock()
	defer c.rulesUpdate.Unlock()
	c.mu.RLock()
	set := cloneDeclarativeRuleSet(c.ruleSet)
	c.mu.RUnlock()
	return c.replaceDeclarativeRulesLocked(upsertRule(set, rule))
}

func (c *Collector) DeleteDeclarativeRule(id string) error {
	c.rulesUpdate.Lock()
	defer c.rulesUpdate.Unlock()
	c.mu.RLock()
	set := cloneDeclarativeRuleSet(c.ruleSet)
	c.mu.RUnlock()
	updated, found := deleteRule(set, id)
	if !found {
		return fmt.Errorf("declarative rule %q not found", id)
	}
	return c.replaceDeclarativeRulesLocked(updated)
}

func (c *Collector) ReloadDeclarativeRules() error {
	c.rulesUpdate.Lock()
	defer c.rulesUpdate.Unlock()
	stamp, err := c.ruleStore.Stamp()
	if err != nil {
		c.setDeclarativeRuleError(err)
		return err
	}
	c.mu.RLock()
	currentStamp := c.ruleStamp
	c.mu.RUnlock()
	if stamp == currentStamp {
		return nil
	}
	set, digest, err := c.ruleStore.Load()
	if err != nil {
		c.setDeclarativeRuleError(err)
		return err
	}
	c.mu.RLock()
	currentDigest := c.ruleDigest
	c.mu.RUnlock()
	if digest == currentDigest {
		c.mu.Lock()
		c.ruleStamp = stamp
		c.ruleError = ""
		c.mu.Unlock()
		return nil
	}
	compiled, err := CompileDeclarativeRules(set, DefaultRules())
	if err != nil {
		c.setDeclarativeRuleError(err)
		return err
	}
	c.mu.RLock()
	current := cloneDeclarativeRuleSet(c.ruleSet)
	c.mu.RUnlock()
	if err := validateRuleVersions(current, set); err != nil {
		c.setDeclarativeRuleError(err)
		return err
	}
	if set.Revision <= current.Revision {
		set.Revision = current.Revision + 1
	}
	set.Audit = append(current.Audit, RuleAudit{Revision: set.Revision, Actor: "local-file", Action: "reload", At: time.Now().UTC()})
	if len(set.Audit) > 1000 {
		set.Audit = set.Audit[len(set.Audit)-1000:]
	}
	for _, prior := range current.Rules {
		unchanged := false
		for _, candidate := range set.Rules {
			if candidate.ID == prior.ID && candidate.Version == prior.Version {
				unchanged = true
				break
			}
		}
		if !unchanged {
			archiveRuleRevision(&current, prior.ID)
		}
	}
	set.History = current.History
	digest, err = c.ruleStore.Save(set)
	if err != nil {
		c.setDeclarativeRuleError(err)
		return err
	}
	c.installDeclarativeRules(set, compiled, digest, "pending-check", "")
	return nil
}

func (c *Collector) installDeclarativeRules(set DeclarativeRuleSet, compiled []Rule, digest, stamp, ruleError string) {
	rules := append(DefaultRules(), compiled...)
	c.mu.Lock()
	c.processor.ReplaceRules(rules)
	c.ruleSet = cloneDeclarativeRuleSet(set)
	c.ruleDigest = digest
	c.ruleStamp = stamp
	c.ruleError = ruleError
	c.mu.Unlock()
}

func (c *Collector) setDeclarativeRuleError(err error) {
	c.mu.Lock()
	c.ruleError = err.Error()
	c.mu.Unlock()
}

func (c *Collector) Observe(activity Activity) {
	select {
	case c.queue <- activity:
	default:
	}
}
func (c *Collector) ObserveVoice(roomID uint64, memberID string, at time.Time) {
	if roomID == 0 || memberID == "" {
		return
	}
	key := voiceSampleKey{roomID: roomID, memberID: memberID}
	value, ok := c.voiceSamples.Load(key)
	if !ok {
		value, _ = c.voiceSamples.LoadOrStore(key, &atomic.Int64{})
	}
	stamp := value.(*atomic.Int64)
	second := at.Unix()
	for {
		previous := stamp.Load()
		if previous >= second {
			return
		}
		if stamp.CompareAndSwap(previous, second) {
			break
		}
	}
	c.Observe(Activity{Kind: VoicePacket, RoomID: roomID, MemberID: memberID, At: at})
}
func (c *Collector) Snapshot(roomID uint64, all bool, limit int) RoomSnapshot {
	c.mu.Lock()
	defer c.mu.Unlock()
	snapshot := c.processor.Snapshot(roomID, all, limit, time.Now())
	snapshot.Enabled = true
	return snapshot
}
func (c *Collector) Close() error {
	close(c.stop)
	<-c.done
	c.mu.Lock()
	for key, session := range c.processor.games {
		c.processor.writeGame(key, session)
	}
	c.mu.Unlock()
	return c.flushDirty()
}
func (c *Collector) run() {
	defer close(c.done)
	tick := time.NewTicker(time.Second)
	flush := time.NewTicker(2 * time.Second)
	defer tick.Stop()
	defer flush.Stop()
	for {
		select {
		case sample := <-c.gameQueue:
			c.mu.Lock()
			c.processor.applyGames(sample)
			c.mu.Unlock()
		case activity := <-c.queue:
			c.mu.Lock()
			c.processor.Apply(activity)
			c.mu.Unlock()
		case now := <-tick.C:
			_ = c.ReloadDeclarativeRules()
			c.mu.Lock()
			for id := range c.processor.state.Rooms {
				c.processor.Apply(Activity{Kind: Tick, RoomID: id, At: now})
			}
			c.mu.Unlock()
		case <-flush.C:
			_ = c.flushDirty()
		case <-c.stop:
			for {
				select {
				case sample := <-c.gameQueue:
					c.mu.Lock()
					c.processor.applyGames(sample)
					c.mu.Unlock()
				case activity := <-c.queue:
					c.mu.Lock()
					c.processor.Apply(activity)
					c.mu.Unlock()
				default:
					return
				}
			}
		}
	}
}

// Keep the existing two-second durability interval, but do no cloning,
// serialization or disk writes when only runtime clocks have changed.
func (c *Collector) flushDirty() error {
	c.mu.Lock()
	revision := c.processor.revision
	if revision == c.persistedRevision {
		c.mu.Unlock()
		return nil
	}
	state := clonePersistedState(c.processor.state)
	c.mu.Unlock()
	if err := c.persist(state); err != nil {
		return err // Retain the dirty revision so the next flush retries.
	}
	c.persistedRevision = revision
	return nil
}

func (c *Collector) persist(state persistedState) error {
	state.Version = SchemaVersion
	bytes, err := jsonMarshal(state)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(c.path), 0700); err != nil {
		return err
	}
	tmp := c.path + ".tmp"
	if err := os.WriteFile(tmp, bytes, 0600); err != nil {
		return err
	}
	return os.Rename(tmp, c.path)
}

func clonePersistedState(source persistedState) persistedState {
	target := persistedState{Version: source.Version, Rooms: make(map[uint64]RoomStats, len(source.Rooms)), Events: make(map[uint64][]Event, len(source.Events)), Dedupe: cloneMap(source.Dedupe)}
	for id, stats := range source.Rooms {
		target.Rooms[id] = cloneStats(stats)
	}
	for id, events := range source.Events {
		cloned := make([]Event, len(events))
		for index, event := range events {
			cloned[index] = event
			cloned[index].TriggeredBy = append([]string(nil), event.TriggeredBy...)
			cloned[index].Participants = append([]string(nil), event.Participants...)
			cloned[index].Metadata = cloneMap(event.Metadata)
			cloned[index].Titles = cloneStringMap(event.Titles)
			cloned[index].Descriptions = cloneStringMap(event.Descriptions)
		}
		target.Events[id] = cloned
	}
	return target
}

func normalizeState(state persistedState) persistedState {
	state.Version = SchemaVersion
	if state.Rooms == nil {
		state.Rooms = map[uint64]RoomStats{}
	}
	if state.Events == nil {
		state.Events = map[uint64][]Event{}
	}
	if state.Dedupe == nil {
		state.Dedupe = map[string]time.Time{}
	}
	for id, stats := range state.Rooms {
		if stats.ActiveDays == nil {
			stats.ActiveDays = map[string]bool{}
		}
		if stats.ActiveHours == nil {
			stats.ActiveHours = map[int]uint64{}
		}
		if stats.Members == nil {
			stats.Members = map[string]MemberStats{}
		}
		if stats.Pairs == nil {
			stats.Pairs = map[string]PairStats{}
		}
		summary := summarizeSessions(stats)
		stats.sessionSummary = &summary
		state.Rooms[id] = stats
	}
	return state
}

// Small indirections keep persistence deterministic and make corrupt-file tests simple.
var jsonMarshal = func(v any) ([]byte, error) { return marshalJSON(v) }
var jsonUnmarshal = func(b []byte, v any) error { return unmarshalJSON(b, v) }

func validateServerID(value string) string {
	value = strings.TrimSpace(value)
	if value == "" {
		return "edenvoice"
	}
	return value
}
