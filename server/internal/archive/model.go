package archive

import "time"

const SchemaVersion = 1

type Rarity string

const (
	Common    Rarity = "common"
	Uncommon  Rarity = "uncommon"
	Rare      Rarity = "rare"
	VeryRare  Rarity = "very_rare"
	Legendary Rarity = "legendary"
)

type Category string

const (
	Origin    Category = "origin"
	Voice     Category = "voice"
	Social    Category = "social"
	Time      Category = "time"
	Return    Category = "return"
	Silence   Category = "silence"
	Scale     Category = "scale"
	Text      Category = "text"
	RareEvent Category = "rare"
	Eden      Category = "eden"
)

type Event struct {
	Evening        *EveningMemory    `json:"evening,omitempty"`
	Game           *GameMemory       `json:"game,omitempty"`
	ID             string            `json:"id"`
	EventType      string            `json:"event_type"`
	Category       Category          `json:"category"`
	RoomID         uint64            `json:"room_id"`
	ServerID       string            `json:"server_id"`
	OccurredAt     time.Time         `json:"occurred_at"`
	TriggeredBy    []string          `json:"triggered_by,omitempty"`
	Participants   []string          `json:"participants,omitempty"`
	Metadata       map[string]any    `json:"metadata,omitempty"`
	Importance     int               `json:"importance"`
	Rarity         Rarity            `json:"rarity"`
	RuleVersion    int               `json:"rule_version,omitempty"`
	Titles         map[string]string `json:"titles,omitempty"`
	Descriptions   map[string]string `json:"descriptions,omitempty"`
	TitleKey       string            `json:"title_key"`
	DescriptionKey string            `json:"description_key"`
	DedupeKey      string            `json:"dedupe_key"`
}

type MemberStats struct {
	FirstSeenAt     time.Time `json:"first_seen_at"`
	LastSeenAt      time.Time `json:"last_seen_at"`
	LastVoiceAt     time.Time `json:"last_voice_at,omitempty"`
	VoiceSessions   uint64    `json:"voice_sessions"`
	SpeakingSeconds int64     `json:"speaking_seconds"`
}

type PairStats struct {
	Members    []string  `json:"members"`
	Sessions   uint64    `json:"sessions"`
	LastSeenAt time.Time `json:"last_seen_at"`
}

type RoomStats struct {
	// Derived, immutable between session completions; rebuilt on load rather
	// than persisted. Keeps built-in tick predicates independent of history size.
	sessionSummary         *sessionSummary
	RoomID                 uint64                 `json:"room_id"`
	RoomName               string                 `json:"room_name"`
	CreatedAt              time.Time              `json:"created_at"`
	FirstMemberJoinAt      time.Time              `json:"first_member_join_at,omitempty"`
	FirstVoiceAt           time.Time              `json:"first_voice_at,omitempty"`
	LastVoiceAt            time.Time              `json:"last_voice_at,omitempty"`
	FirstTextAt            time.Time              `json:"first_text_at,omitempty"`
	LastActiveAt           time.Time              `json:"last_active_at,omitempty"`
	ActiveVoiceSeconds     int64                  `json:"active_voice_seconds"`
	PresenceSeconds        int64                  `json:"presence_seconds"`
	SpeakingSeconds        int64                  `json:"speaking_seconds"`
	SessionCount           uint64                 `json:"session_count"`
	TextMessageCount       uint64                 `json:"text_message_count"`
	LongestSessionSeconds  int64                  `json:"longest_session_seconds"`
	LatestSessionEndedAt   time.Time              `json:"latest_session_ended_at,omitempty"`
	EarliestMorningVoiceAt time.Time              `json:"earliest_morning_voice_at,omitempty"`
	MaxConcurrentMembers   int                    `json:"max_concurrent_members"`
	MaxConcurrentSpeaking  int                    `json:"max_concurrent_speaking"`
	RestartCount           uint64                 `json:"restart_count"`
	LongestSilenceSeconds  int64                  `json:"longest_silence_seconds"`
	ActiveDays             map[string]bool        `json:"active_days"`
	ActiveHours            map[int]uint64         `json:"active_hours"`
	Members                map[string]MemberStats `json:"members"`
	Pairs                  map[string]PairStats   `json:"pairs"`
}

type sessionSummary struct {
	maxPairSessions   uint64
	maxMemberSessions uint64
}

func summarizeSessions(stats RoomStats) sessionSummary {
	var summary sessionSummary
	for _, pair := range stats.Pairs {
		summary.maxPairSessions = max(summary.maxPairSessions, pair.Sessions)
	}
	for _, member := range stats.Members {
		summary.maxMemberSessions = max(summary.maxMemberSessions, member.VoiceSessions)
	}
	return summary
}

type RoomSnapshot struct {
	ActiveDayCount        int                           `json:"active_day_count"`
	HistoricalMemberCount int                           `json:"historical_member_count"`
	Stats                 RoomStats                     `json:"stats"`
	AgeDays               int64                         `json:"age_days"`
	Events                []Event                       `json:"events"`
	ParticipantProfiles   map[string]ParticipantProfile `json:"participant_profiles,omitempty"`
	HasMore               bool                          `json:"has_more"`
	Enabled               bool                          `json:"enabled"`
}

// ParticipantProfile is response-only display data. Persisted events retain
// stable client IDs so nickname and avatar changes never alter dedupe/history.
type ParticipantProfile struct {
	ClientID    string `json:"client_id"`
	DisplayName string `json:"display_name"`
	Avatar      string `json:"avatar,omitempty"`
}

type persistedState struct {
	Version int                  `json:"version"`
	Rooms   map[uint64]RoomStats `json:"rooms"`
	Events  map[uint64][]Event   `json:"events"`
	Dedupe  map[string]time.Time `json:"dedupe"`
}
