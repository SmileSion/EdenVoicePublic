package archive

import (
	"fmt"
	"time"
)

type Trigger string

const (
	TriggerCreated    Trigger = "created"
	TriggerJoin       Trigger = "join"
	TriggerVoice      Trigger = "voice"
	TriggerText       Trigger = "text"
	TriggerLeave      Trigger = "leave"
	TriggerSessionEnd Trigger = "session_end"
	TriggerTick       Trigger = "tick"
)

// EvalContext is a synchronous read-only view. Predicates and other rule
// callbacks must not mutate or retain its maps/slices after returning.
type EvalContext struct {
	PreviousTime               time.Time
	PreviousParticipants       []string
	PreviousConcurrentSpeakers int
	ConcurrentSpeakers         int
	PreviousSessionDuration    time.Duration
	PreviousLastTwoDuration    time.Duration
	PreviousLastOneDuration    time.Duration
	PreviousSilenceDuration    time.Duration
	SubjectPair                []string
	Trigger                    Trigger
	Now                        time.Time
	Before                     RoomStats
	After                      RoomStats
	Participants               []string
	ReturningMembers           []string
	SessionDuration            time.Duration
	LastTwoDuration            time.Duration
	LastOneDuration            time.Duration
	SilenceDuration            time.Duration
	MemberSilence              time.Duration
	MemberID                   string
	WasGroup                   bool
}

type DedupeScope string

const (
	DedupeForever  DedupeScope = "forever"
	DedupeCooldown DedupeScope = "cooldown"
)

type Rule struct {
	Facts        func(EvalContext) map[string]any
	Contexts     func(EvalContext) []EvalContext
	ID           string
	EventType    string
	Version      int
	Category     Category
	Trigger      Trigger
	Threshold    int64
	Cooldown     time.Duration
	Dedupe       DedupeScope
	Rarity       Rarity
	Importance   int
	Titles       map[string]string
	Descriptions map[string]string
	Predicate    func(EvalContext) bool
	Key          func(EvalContext) string
	Actors       func(EvalContext) []string
}

func (r Rule) matches(ctx EvalContext) bool {
	return r.Trigger == ctx.Trigger && r.Predicate != nil && r.Predicate(ctx)
}

func DefaultRules() []Rule {
	rules := []Rule{
		oneShot("first_room_created", Origin, TriggerCreated, Common, 35, func(c EvalContext) bool { return true }),
		oneShot("first_member_join", Origin, TriggerJoin, Common, 35, func(c EvalContext) bool { return c.Before.FirstMemberJoinAt.IsZero() }),
		oneShot("first_voice", Voice, TriggerVoice, Common, 55, func(c EvalContext) bool { return c.Before.FirstVoiceAt.IsZero() }),
		oneShot("first_text_message", Text, TriggerText, Common, 25, func(c EvalContext) bool { return c.Before.FirstTextAt.IsZero() }),
	}
	voiceThresholds := []int64{30 * 60, 60 * 60, 3 * 3600, 5 * 3600, 10 * 3600, 20 * 3600, 50 * 3600, 100 * 3600, 200 * 3600, 500 * 3600, 1000 * 3600, 2000 * 3600, 5000 * 3600}
	for _, threshold := range voiceThresholds {
		id := fmt.Sprintf("voice_total_%s", durationID(threshold))
		rules = append(rules, thresholdRule(id, Voice, TriggerVoice, threshold, rarityForThreshold(threshold, 100*3600, 1000*3600), 45))
	}
	ageDays := []int64{1, 3, 7, 14, 30, 50, 100, 180, 365, 500, 730, 1000}
	for _, days := range ageDays {
		threshold := days
		rules = append(rules, Rule{ID: fmt.Sprintf("room_age_%dd", days), EventType: fmt.Sprintf("room_age_%dd", days), Category: Time, Trigger: TriggerTick, Threshold: days, Dedupe: DedupeForever, Rarity: rarityForThreshold(days, 100, 730), Importance: 40, Predicate: func(c EvalContext) bool {
			return ageDaysAt(c.After, c.Now) >= threshold
		}})
	}
	for _, count := range []int64{2, 3, 5, 10, 20, 30, 50, 100} {
		threshold := count
		rules = append(rules, Rule{ID: fmt.Sprintf("members_%d", count), EventType: fmt.Sprintf("members_%d", count), Category: Social, Trigger: TriggerJoin, Threshold: count, Dedupe: DedupeForever, Rarity: rarityForThreshold(count, 10, 50), Importance: 40, Predicate: func(c EvalContext) bool {
			return int64(len(c.Before.Members)) < threshold && int64(len(c.After.Members)) >= threshold
		}})
	}
	for _, count := range []int64{2, 3, 5, 8, 10, 15, 20, 30} {
		threshold := count
		rules = append(rules, Rule{ID: fmt.Sprintf("concurrent_%d", count), EventType: fmt.Sprintf("concurrent_%d", count), Category: Scale, Trigger: TriggerJoin, Threshold: count, Dedupe: DedupeForever, Rarity: rarityForThreshold(count, 8, 20), Importance: 42, Predicate: func(c EvalContext) bool {
			return int64(c.Before.MaxConcurrentMembers) < threshold && int64(c.After.MaxConcurrentMembers) >= threshold
		}})
	}
	for _, item := range []struct {
		id         string
		d          time.Duration
		rarity     Rarity
		importance int
	}{{"last_two_5m", 5 * time.Minute, Uncommon, 45}, {"last_two_15m", 15 * time.Minute, Uncommon, 52}, {"last_two_30m", 30 * time.Minute, Rare, 60}, {"last_two_1h", time.Hour, Rare, 70}, {"last_two_2h", 2 * time.Hour, VeryRare, 80}, {"last_two_3h", 3 * time.Hour, VeryRare, 88}} {
		item := item
		rules = append(rules, durationRule(item.id, Social, TriggerTick, item.d, item.rarity, item.importance, func(c EvalContext) time.Duration { return c.LastTwoDuration }, 24*time.Hour))
	}
	for _, item := range []struct {
		id string
		d  time.Duration
	}{{"last_one_5m", 5 * time.Minute}, {"last_one_15m", 15 * time.Minute}, {"last_one_30m", 30 * time.Minute}} {
		item := item
		rules = append(rules, durationRule(item.id, Silence, TriggerTick, item.d, Uncommon, 35, func(c EvalContext) time.Duration { return c.LastOneDuration }, 24*time.Hour))
	}
	for _, item := range []struct {
		id string
		d  time.Duration
	}{{"session_30m", 30 * time.Minute}, {"session_1h", time.Hour}, {"session_2h", 2 * time.Hour}, {"session_3h", 3 * time.Hour}, {"session_4h", 4 * time.Hour}, {"session_6h", 6 * time.Hour}, {"session_8h", 8 * time.Hour}, {"session_10h", 10 * time.Hour}, {"session_12h", 12 * time.Hour}} {
		item := item
		rules = append(rules, durationRule(item.id, Voice, TriggerTick, item.d, rarityForThreshold(int64(item.d), int64(3*time.Hour), int64(8*time.Hour)), 45, func(c EvalContext) time.Duration { return c.SessionDuration }, 0))
	}
	rules = append(rules,
		oneShot("first_midnight_voice", Time, TriggerVoice, Rare, 58, func(c EvalContext) bool { return c.Now.Hour() == 0 }),
		cooldownRule("still_here_at_2am", Time, TriggerTick, 24*time.Hour, Rare, 64, func(c EvalContext) bool { return c.Now.Hour() == 2 && len(c.Participants) >= 2 }),
		cooldownRule("still_here_at_3am", Time, TriggerTick, 24*time.Hour, Rare, 68, func(c EvalContext) bool { return c.Now.Hour() == 3 && len(c.Participants) >= 2 }),
		cooldownRule("before_dawn", Time, TriggerTick, 24*time.Hour, Rare, 70, func(c EvalContext) bool { return c.Now.Hour() == 5 && len(c.Participants) >= 2 }),
		cooldownRule("last_two_late_night", Eden, TriggerTick, 24*time.Hour, VeryRare, 78, func(c EvalContext) bool {
			return c.WasGroup && len(c.Participants) == 2 && c.LastTwoDuration >= 15*time.Minute && c.Now.Hour() < 5
		}),
		cooldownRule("the_last_light", Eden, TriggerTick, 24*time.Hour, Rare, 60, func(c EvalContext) bool {
			return c.WasGroup && len(c.Participants) == 1 && c.LastOneDuration >= 15*time.Minute && c.Now.Hour() < 5
		}),
		cooldownRule("eden_awakened", Return, TriggerVoice, 0, VeryRare, 86, func(c EvalContext) bool { return c.SilenceDuration >= 30*24*time.Hour }),
		cooldownRule("voice_after_silence_7d", Return, TriggerVoice, 0, Rare, 72, func(c EvalContext) bool { return c.SilenceDuration >= 7*24*time.Hour }),
		withActors(cooldownRule("reunion", Return, TriggerJoin, 7*24*time.Hour, Rare, 76, func(c EvalContext) bool { return len(c.ReturningMembers) >= 2 }), func(c EvalContext) []string { return c.ReturningMembers }),
		withActors(cooldownRule("old_group_returned", Return, TriggerJoin, 30*24*time.Hour, VeryRare, 88, func(c EvalContext) bool { return len(c.ReturningMembers) >= 3 }), func(c EvalContext) []string { return c.ReturningMembers }),
		cooldownRule("room_anniversary_voice", Time, TriggerVoice, 300*24*time.Hour, VeryRare, 82, func(c EvalContext) bool {
			return ageDaysAt(c.After, c.Now) >= 365 && sameMonthDay(c.After.CreatedAt, c.Now)
		}),
		oneShot("the_old_room", RareEvent, TriggerVoice, VeryRare, 90, func(c EvalContext) bool {
			return ageDaysAt(c.After, c.Now) >= 365 && c.After.ActiveVoiceSeconds >= 500*3600
		}),
		oneShot("still_here", Eden, TriggerVoice, VeryRare, 92, func(c EvalContext) bool { return ageDaysAt(c.After, c.Now) >= 730 }),
		oneShot("a_long_night", Time, TriggerTick, Rare, 74, func(c EvalContext) bool { return c.SessionDuration >= 4*time.Hour && c.Now.Hour() < 5 }),
		oneShot("all_nighter", RareEvent, TriggerTick, VeryRare, 88, func(c EvalContext) bool {
			return c.SessionDuration >= 6*time.Hour && c.Now.Hour() >= 5 && c.Now.Hour() < 7
		}),
		cooldownRule("the_morning_we_stayed", Eden, TriggerTick, 300*24*time.Hour, Legendary, 96, func(c EvalContext) bool {
			return c.WasGroup && len(c.Participants) == 2 && c.LastTwoDuration >= 2*time.Hour && c.Now.Hour() >= 5 && c.Now.Hour() < 7
		}),
		withActors(oneShot("same_pair_10_sessions", Social, TriggerTick, Rare, 70, func(c EvalContext) bool { return hasRoomPairSessions(c.After, 10) }), func(c EvalContext) []string { return pairMembersAtLeast(c.After.Pairs, 10) }),
		withActors(oneShot("same_pair_50_sessions", Social, TriggerTick, VeryRare, 86, func(c EvalContext) bool { return hasRoomPairSessions(c.After, 50) }), func(c EvalContext) []string { return pairMembersAtLeast(c.After.Pairs, 50) }),
		oneShot("ninety_nine_days_passed", Time, TriggerTick, Rare, 67, func(c EvalContext) bool { return ageDaysAt(c.After, c.Now) >= 99 }),
		oneShot("felix_project", Voice, TriggerVoice, VeryRare, 82, func(c EvalContext) bool {
			return c.Before.ActiveVoiceSeconds < 99*3600 && c.After.ActiveVoiceSeconds >= 99*3600
		}),
		durationRule("the_solitary_island", Eden, TriggerTick, 99*time.Minute, VeryRare, 84, func(c EvalContext) time.Duration { return c.LastOneDuration }, 0),
		durationRule("only_two_on_the_planet", Eden, TriggerTick, 99*time.Minute, Legendary, 94, func(c EvalContext) time.Duration { return c.LastTwoDuration }, 0),
		withActors(oneShot("the_final_love_story", Eden, TriggerTick, Legendary, 98, func(c EvalContext) bool { return hasRoomPairSessions(c.After, 99) }), func(c EvalContext) []string { return pairMembersAtLeast(c.After.Pairs, 99) }),
		oneShot("ninety_nine_active_days", Time, TriggerVoice, VeryRare, 86, func(c EvalContext) bool { return len(c.After.ActiveDays) >= 99 }),
		cooldownRule("the_quiet_world", Return, TriggerVoice, 0, Legendary, 95, func(c EvalContext) bool { return c.SilenceDuration >= 99*24*time.Hour }),
		oneShot("the_last_guard", Eden, TriggerTick, VeryRare, 88, func(c EvalContext) bool {
			return c.WasGroup && len(c.Participants) == 1 && c.LastOneDuration >= 2*time.Hour
		}),
		withActors(oneShot("little_explorer", Social, TriggerTick, Rare, 72, func(c EvalContext) bool { return hasRoomMemberSessions(c.After, 10) }), func(c EvalContext) []string { return []string{memberWithSessions(c.After.Members, 10)} }),
		oneShot("under_the_red_star", Eden, TriggerVoice, VeryRare, 89, func(c EvalContext) bool { return ageDaysAt(c.After, c.Now) >= 99 && c.Now.Hour() >= 18 }),
	)
	for _, days := range []int64{7, 30, 90, 365} {
		threshold := time.Duration(days) * 24 * time.Hour
		rule := cooldownRule(memberReturnID(days), Return, TriggerJoin, 24*time.Hour, rarityForThreshold(days, 30, 365), 62, func(c EvalContext) bool { return c.MemberID != "" && c.MemberSilence >= threshold })
		rule.Key = func(c EvalContext) string { return c.MemberID }
		rules = append(rules, rule)
	}
	for _, count := range []int64{100, 500, 1000, 5000, 10000} {
		threshold := count
		rules = append(rules, Rule{ID: fmt.Sprintf("text_total_%d", count), EventType: fmt.Sprintf("text_total_%d", count), Category: Text, Trigger: TriggerText, Threshold: count, Dedupe: DedupeForever, Rarity: rarityForThreshold(count, 1000, 10000), Importance: 35, Predicate: func(c EvalContext) bool {
			return int64(c.Before.TextMessageCount) < threshold && int64(c.After.TextMessageCount) >= threshold
		}})
	}
	for _, count := range []int64{100, 500, 1000} {
		threshold := count
		rules = append(rules, Rule{ID: fmt.Sprintf("sessions_%d", count), EventType: fmt.Sprintf("sessions_%d", count), Category: Voice, Trigger: TriggerTick, Threshold: count, Dedupe: DedupeForever, Rarity: rarityForThreshold(count, 500, 1000), Importance: 65, Predicate: func(c EvalContext) bool { return int64(c.After.SessionCount) >= threshold }})
	}
	for _, days := range []int64{3, 7, 14, 30} {
		threshold := days
		rules = append(rules, Rule{ID: fmt.Sprintf("%d_days_of_voices", days), EventType: dayVoiceID(days), Category: Time, Trigger: TriggerVoice, Threshold: days, Dedupe: DedupeForever, Rarity: rarityForThreshold(days, 7, 30), Importance: 50, Predicate: func(c EvalContext) bool { return consecutiveDays(c.After.ActiveDays, c.Now) >= int(threshold) }})
	}
	return rules
}

func oneShot(id string, category Category, trigger Trigger, rarity Rarity, importance int, predicate func(EvalContext) bool) Rule {
	return Rule{ID: id, EventType: id, Category: category, Trigger: trigger, Dedupe: DedupeForever, Rarity: rarity, Importance: importance, Predicate: predicate}
}

func cooldownRule(id string, category Category, trigger Trigger, cooldown time.Duration, rarity Rarity, importance int, predicate func(EvalContext) bool) Rule {
	return Rule{ID: id, EventType: id, Category: category, Trigger: trigger, Cooldown: cooldown, Dedupe: DedupeCooldown, Rarity: rarity, Importance: importance, Predicate: predicate}
}

func withActors(rule Rule, actors func(EvalContext) []string) Rule {
	rule.Actors = actors
	return rule
}

func thresholdRule(id string, category Category, trigger Trigger, threshold int64, rarity Rarity, importance int) Rule {
	return Rule{ID: id, EventType: id, Category: category, Trigger: trigger, Threshold: threshold, Dedupe: DedupeForever, Rarity: rarity, Importance: importance, Predicate: func(c EvalContext) bool {
		return c.Before.ActiveVoiceSeconds < threshold && c.After.ActiveVoiceSeconds >= threshold
	}}
}

func durationRule(id string, category Category, trigger Trigger, threshold time.Duration, rarity Rarity, importance int, value func(EvalContext) time.Duration, cooldown time.Duration) Rule {
	dedupe := DedupeForever
	if cooldown > 0 {
		dedupe = DedupeCooldown
	}
	return Rule{ID: id, EventType: id, Category: category, Trigger: trigger, Threshold: int64(threshold / time.Second), Cooldown: cooldown, Dedupe: dedupe, Rarity: rarity, Importance: importance, Predicate: func(c EvalContext) bool { v := value(c); return v >= threshold && v < threshold+time.Minute }}
}

func durationID(seconds int64) string {
	if seconds%3600 == 0 {
		return fmt.Sprintf("%dh", seconds/3600)
	}
	return fmt.Sprintf("%dm", seconds/60)
}
func dayVoiceID(days int64) string {
	names := map[int64]string{3: "three_days_of_voices", 7: "seven_days_of_voices", 14: "fourteen_days_of_voices", 30: "thirty_days_of_voices"}
	return names[days]
}
func memberReturnID(days int64) string {
	if days == 365 {
		return "member_return_1y"
	}
	return fmt.Sprintf("member_return_%dd", days)
}
func rarityForThreshold(value, rareAt, veryRareAt int64) Rarity {
	if value >= veryRareAt {
		return VeryRare
	}
	if value >= rareAt {
		return Rare
	}
	return Uncommon
}
func ageDaysAt(stats RoomStats, now time.Time) int64 {
	if stats.CreatedAt.IsZero() || now.Before(stats.CreatedAt) {
		return 0
	}
	return int64(now.Sub(stats.CreatedAt) / (24 * time.Hour))
}

func consecutiveDays(days map[string]bool, now time.Time) int {
	count := 0
	for day := dayStart(now); days[day.Format("2006-01-02")]; day = day.AddDate(0, 0, -1) {
		count++
	}
	return count
}

func dayStart(value time.Time) time.Time {
	y, m, d := value.Date()
	return time.Date(y, m, d, 0, 0, 0, 0, value.Location())
}

func sameMonthDay(left, right time.Time) bool {
	return left.Month() == right.Month() && left.Day() == right.Day()
}
func hasRoomPairSessions(stats RoomStats, threshold uint64) bool {
	if stats.sessionSummary != nil {
		return stats.sessionSummary.maxPairSessions >= threshold
	}
	return hasPairSessions(stats.Pairs, threshold)
}

func hasRoomMemberSessions(stats RoomStats, threshold uint64) bool {
	if stats.sessionSummary != nil {
		return stats.sessionSummary.maxMemberSessions >= threshold
	}
	return memberWithSessions(stats.Members, threshold) != ""
}

func hasPairSessions(pairs map[string]PairStats, threshold uint64) bool {
	for _, pair := range pairs {
		if pair.Sessions >= threshold {
			return true
		}
	}
	return false
}

func pairMembersAtLeast(pairs map[string]PairStats, threshold uint64) []string {
	for _, pair := range pairs {
		if pair.Sessions >= threshold {
			return append([]string(nil), pair.Members...)
		}
	}
	return nil
}

func memberWithSessions(members map[string]MemberStats, threshold uint64) string {
	result := ""
	for clientID, member := range members {
		if member.VoiceSessions >= threshold && (result == "" || clientID < result) {
			result = clientID
		}
	}
	return result
}
