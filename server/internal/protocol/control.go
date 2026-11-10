package protocol

import (
	"bytes"
	"crypto/sha256"
	"edenvoice/server/internal/announcements"
	"edenvoice/server/internal/clientupdates"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"image/jpeg"
	"strings"
	"time"
)

const (
	MaxControlMessageBytes = 64 * 1024
	MaxNicknameBytes       = 64
	MaxChatMessageBytes    = 2 * 1024
	MaxChannelUserLimit    = 10_000
	CustomAvatarPrefix     = "data:image/jpeg;base64,"
	CustomAvatarRefPrefix  = "custom:"
	MaxAvatarJPEGBytes     = 12 * 1024
	AvatarSide             = 128
)

func IsSupportedAvatar(avatar string) bool {
	_, _, err := CanonicalAvatar(avatar)
	return err == nil
}

func CanonicalAvatar(avatar string) (string, string, error) {
	switch avatar {
	case "", "L", "K", "M", "R", "E":
		return avatar, "", nil
	}
	if hash, ok := strings.CutPrefix(avatar, CustomAvatarRefPrefix); ok {
		if len(hash) != sha256.Size*2 {
			return "", "", errors.New("invalid custom avatar reference")
		}
		for _, character := range hash {
			if !strings.ContainsRune("0123456789abcdef", character) {
				return "", "", errors.New("invalid custom avatar reference")
			}
		}
		return avatar, "", nil
	}
	encoded, ok := strings.CutPrefix(avatar, CustomAvatarPrefix)
	if !ok || len(encoded) > base64.StdEncoding.EncodedLen(MaxAvatarJPEGBytes) {
		return "", "", errors.New("invalid custom avatar data")
	}
	data, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil || len(data) == 0 || len(data) > MaxAvatarJPEGBytes {
		return "", "", errors.New("invalid custom avatar data")
	}
	config, err := jpeg.DecodeConfig(bytes.NewReader(data))
	if err != nil || config.Width != AvatarSide || config.Height != AvatarSide {
		return "", "", errors.New("custom avatar must be a 128x128 JPEG")
	}
	digest := sha256.Sum256(data)
	return fmt.Sprintf("%s%x", CustomAvatarRefPrefix, digest), avatar, nil
}

type MessageType string

const (
	TypeHello                MessageType = "hello"
	TypeIdentityChallenge    MessageType = "identity_challenge"
	TypeIdentityResponse     MessageType = "identity_response"
	TypeWelcome              MessageType = "welcome"
	TypeServerState          MessageType = "server_state"
	TypeJoinChannel          MessageType = "join_channel"
	TypeLeaveChannel         MessageType = "leave_channel"
	TypeChannelJoined        MessageType = "channel_joined"
	TypeUserJoined           MessageType = "user_joined"
	TypeUserLeft             MessageType = "user_left"
	TypeUserMoved            MessageType = "user_moved"
	TypeSetMute              MessageType = "set_mute"
	TypeSetDeafen            MessageType = "set_deafen"
	TypeSetDisplayName       MessageType = "set_display_name"
	TypeGetGameCatalog       MessageType = "get_game_catalog"
	TypeGameCatalog          MessageType = "game_catalog"
	TypeSetGameActivity      MessageType = "set_game_activity"
	TypeSetAvatar            MessageType = "set_avatar"
	TypeGetAvatar            MessageType = "get_avatar"
	TypeAvatarData           MessageType = "avatar_data"
	TypeSendTextMessage      MessageType = "send_text_message"
	TypeTextMessage          MessageType = "text_message"
	TypeChatHistory          MessageType = "chat_history"
	TypeGetRoomArchive       MessageType = "get_room_archive"
	TypeRoomArchive          MessageType = "room_archive"
	TypeVoiceActivity        MessageType = "voice_activity"
	TypeUserStateChanged     MessageType = "user_state_changed"
	TypePing                 MessageType = "ping"
	TypePong                 MessageType = "pong"
	TypeError                MessageType = "error"
	TypeRegisterCandidates   MessageType = "register_candidates"
	TypePeerCandidates       MessageType = "peer_candidates"
	TypeRouteUpdate          MessageType = "route_update"
	TypeNetworkReport        MessageType = "network_report"
	TypeSetTransportMode     MessageType = "set_transport_mode"
	TypeSetRoomNetworkPolicy MessageType = "set_room_network_policy"
	TypeAudioQoEFeedback     MessageType = "audio_qoe_feedback"
	TypeSubscribeStreams     MessageType = "subscribe_streams"
	TypeProvisionMaster      MessageType = "provision_master"
	TypeCreateChannel        MessageType = "create_channel"
	TypeEditChannel          MessageType = "edit_channel"
	TypeDeleteChannel        MessageType = "delete_channel"
	TypeSetChannelAdmin      MessageType = "set_channel_admin"
	TypeSetServerRole        MessageType = "set_server_role"
	TypeAdminMute            MessageType = "admin_mute"
	TypeKickChannel          MessageType = "kick_channel"
	TypeKickServer           MessageType = "kick_server"
	TypeBan                  MessageType = "ban"
	TypeUnban                MessageType = "unban"
	TypeListBans             MessageType = "list_bans"
	TypeBanList              MessageType = "ban_list"
	TypeMoveUser             MessageType = "move_user"
	TypeCapabilities         MessageType = "capabilities"
	TypeRequestMediaRekey    MessageType = "request_media_rekey"
	TypeMediaRekeyPrepare    MessageType = "media_rekey_prepare"
	TypeMediaRekeyAck        MessageType = "media_rekey_ack"
	TypeMediaRekeyCommit     MessageType = "media_rekey_commit"
)

type Envelope struct {
	Type MessageType     `json:"-"`
	Data json.RawMessage `json:"-"`
}

func (e Envelope) MarshalJSON() ([]byte, error) {
	fields := make(map[string]json.RawMessage)
	typeValue, _ := json.Marshal(e.Type)
	fields["type"] = typeValue
	if len(e.Data) > 0 && string(e.Data) != "null" {
		var data map[string]json.RawMessage
		if err := json.Unmarshal(e.Data, &data); err != nil {
			return nil, fmt.Errorf("control data must be an object: %w", err)
		}
		for key, value := range data {
			if key != "type" {
				fields[key] = value
			}
		}
	}
	return json.Marshal(fields)
}

func (e *Envelope) UnmarshalJSON(bytes []byte) error {
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(bytes, &fields); err != nil {
		return err
	}
	typeValue, ok := fields["type"]
	if !ok {
		return errors.New("control message type is required")
	}
	if err := json.Unmarshal(typeValue, &e.Type); err != nil {
		return err
	}
	delete(fields, "type")
	data, err := json.Marshal(fields)
	if err != nil {
		return err
	}
	e.Data = data
	return nil
}

func NewEnvelope(kind MessageType, value any) (Envelope, error) {
	if value == nil {
		return Envelope{Type: kind}, nil
	}
	data, err := json.Marshal(value)
	return Envelope{Type: kind, Data: data}, err
}

func (e Envelope) Decode(value any) error { return json.Unmarshal(e.Data, value) }

type Hello struct {
	ClientFeatures       []string `json:"client_features,omitempty"`
	ClientVersion        string   `json:"client_version,omitempty"`
	ClientPlatform       string   `json:"client_platform,omitempty"`
	Nickname             string   `json:"nickname"`
	ProtocolVersion      int      `json:"protocol_version,omitempty"`
	SecurityCapabilities []string `json:"security_capabilities,omitempty"`
	ClientID             string   `json:"client_id,omitempty"`
	ClientPublicKey      string   `json:"client_public_key,omitempty"`
	ClientEphemeralKey   string   `json:"client_ephemeral_key,omitempty"`
}
type IdentityChallenge struct {
	ServerPasswordRequired bool     `json:"server_password_required,omitempty"`
	Challenge              string   `json:"challenge"`
	ServerPublicKey        string   `json:"server_public_key"`
	ServerFingerprint      string   `json:"server_fingerprint"`
	ServerEphemeralKey     string   `json:"server_ephemeral_key"`
	RequiredCapabilities   []string `json:"required_capabilities"`
}
type IdentityResponse struct {
	ServerPassword string `json:"server_password,omitempty"`
	Signature      string `json:"signature"`
}
type Welcome struct {
	SessionID               uint64   `json:"session_id"`
	ServerName              string   `json:"server_name"`
	VoiceAddr               string   `json:"voice_addr"`
	MediaQueueSize          int      `json:"media_queue_size"`
	ClientID                string   `json:"client_id,omitempty"`
	ServerFingerprint       string   `json:"server_fingerprint,omitempty"`
	MediaKeyEpoch           uint16   `json:"media_key_epoch,omitempty"`
	ServerRole              string   `json:"server_role,omitempty"`
	Capabilities            []string `json:"capabilities,omitempty"`
	NetworkPolicyChannelIDs []uint64 `json:"network_policy_channel_ids,omitempty"`
	ServerFeatures          []string `json:"server_features,omitempty"`
}
type Channel struct {
	ID                 uint64 `json:"id"`
	ParentID           uint64 `json:"parent_id,omitempty"`
	Name               string `json:"name"`
	SortOrder          int    `json:"sort_order"`
	UserLimit          uint32 `json:"user_limit,omitempty"`
	NetworkPolicy      string `json:"network_policy,omitempty"`
	AudioQualityPolicy string `json:"audio_quality_policy,omitempty"`
	Temporary          bool   `json:"temporary,omitempty"`
	TemporarySpecified bool   `json:"-"`
}

func (c *Channel) UnmarshalJSON(data []byte) error {
	type channelAlias Channel
	*c = Channel{}
	decoded := struct {
		*channelAlias
		Temporary *bool `json:"temporary"`
	}{channelAlias: (*channelAlias)(c)}
	if err := json.Unmarshal(data, &decoded); err != nil {
		return err
	}
	if decoded.Temporary != nil {
		c.Temporary = *decoded.Temporary
		c.TemporarySpecified = true
	}
	return nil
}

type User struct {
	GameActivity *GameActivity `json:"game_activity,omitempty"`
	SessionID    uint64        `json:"session_id"`
	Nickname     string        `json:"nickname"`
	ChannelID    uint64        `json:"channel_id,omitempty"`
	Muted        bool          `json:"muted"`
	Deafened     bool          `json:"deafened"`
	ClientID     string        `json:"client_id,omitempty"`
	ServerRole   string        `json:"server_role,omitempty"`
	ChannelRole  string        `json:"channel_role,omitempty"`
	Avatar       string        `json:"avatar,omitempty"`
	// AdminMuted is the effective administrative mute for backward compatibility.
	AdminMuted   bool `json:"admin_muted,omitempty"`
	ServerMuted  bool `json:"server_muted,omitempty"`
	ChannelMuted bool `json:"channel_muted,omitempty"`
}
type ServerState struct {
	ChatHeads              map[uint64]string           `json:"chat_heads,omitempty"`
	AnnouncementRevision   uint64                      `json:"announcement_revision,string"`
	Announcement           *announcements.Announcement `json:"announcement,omitempty"`
	ClientUpdates          []clientupdates.Release     `json:"client_updates,omitempty"`
	PasswordRoomsSupported bool                        `json:"password_rooms_supported,omitempty"`
	LockedChannelIDs       []uint64                    `json:"locked_channel_ids,omitempty"`
	GameCatalogVersion     uint64                      `json:"game_catalog_version"`
	GameCatalog            *GameCatalog                `json:"game_catalog,omitempty"`
	RoomScenes             map[uint64]RoomScene        `json:"room_scenes"`
	ServerName             string                      `json:"server_name"`
	Channels               []Channel                   `json:"channels"`
	Users                  []User                      `json:"users"`
}
type ChannelRequest struct {
	Password  string `json:"password,omitempty"`
	ChannelID uint64 `json:"channel_id"`
}
type UserEvent struct {
	User User `json:"user"`
}
type UserLeft struct {
	SessionID uint64 `json:"session_id"`
}
type UserMoved struct {
	SessionID uint64 `json:"session_id"`
	ChannelID uint64 `json:"channel_id,omitempty"`
}
type StateRequest struct {
	Enabled bool `json:"enabled"`
}
type DisplayNameRequest struct {
	DisplayName string `json:"display_name"`
}
type AvatarRequest struct {
	Avatar string `json:"avatar"`
}
type AvatarData struct {
	Avatar string `json:"avatar"`
	Data   string `json:"data,omitempty"`
}
type SendTextMessageRequest struct {
	Text string `json:"text"`
}
type TextMessage struct {
	Image           *ChatImage `json:"image,omitempty"`
	ID              uint64     `json:"id"`
	ChannelID       uint64     `json:"channel_id"`
	SenderSessionID uint64     `json:"sender_session_id"`
	SenderClientID  string     `json:"sender_client_id,omitempty"`
	SenderName      string     `json:"sender_name"`
	SenderAvatar    string     `json:"sender_avatar,omitempty"`
	Text            string     `json:"text"`
	SentAtUnixMS    int64      `json:"sent_at_unix_ms"`
}
type ChatHistory struct {
	ChannelID uint64        `json:"channel_id"`
	Messages  []TextMessage `json:"messages"`
}
type RoomArchiveRequest struct {
	ChannelID uint64 `json:"channel_id"`
	All       bool   `json:"all,omitempty"`
	Limit     int    `json:"limit,omitempty"`
}
type UserStateChanged struct {
	GameActivity *GameActivity `json:"game_activity"`
	SessionID    uint64        `json:"session_id"`
	Muted        bool          `json:"muted"`
	Deafened     bool          `json:"deafened"`
	AdminMuted   bool          `json:"admin_muted,omitempty"`
	ServerMuted  bool          `json:"server_muted,omitempty"`
	ChannelMuted bool          `json:"channel_muted,omitempty"`
}
type Ping struct {
	Nonce uint64 `json:"nonce"`
}
type Error struct {
	Code    string `json:"code"`
	Message string `json:"message"`
}

type Candidate struct {
	Kind     string `json:"kind"`
	Address  string `json:"address"`
	Priority uint32 `json:"priority"`
}
type CandidateRegistration struct {
	Candidates []Candidate `json:"candidates"`
}
type PeerCandidates struct {
	PeerID      uint64      `json:"peer_id"`
	Generation  uint64      `json:"generation"`
	Candidates  []Candidate `json:"candidates"`
	SecurityKey string      `json:"security_key,omitempty"`
}
type NetworkReport struct {
	PeerID                    uint64  `json:"peer_id"`
	Qualified                 bool    `json:"qualified"`
	RTTMS                     float64 `json:"rtt_ms"`
	Loss                      float64 `json:"packet_loss_percent"`
	JitterMS                  float64 `json:"jitter_ms"`
	LateLossPercent           float64 `json:"late_loss_percent,omitempty"`
	EffectiveAudioLossPercent float64 `json:"effective_audio_loss_percent,omitempty"`
	BurstLossPercent          float64 `json:"burst_loss_percent,omitempty"`
	MaxConsecutiveLoss        uint32  `json:"max_consecutive_loss,omitempty"`
	PacketReorderPercent      float64 `json:"packet_reorder_percent,omitempty"`
	NetworkQuality            string  `json:"network_quality,omitempty"`
	NetworkTrend              string  `json:"network_trend,omitempty"`
}
type TransportModeRequest struct {
	Mode string `json:"mode"`
}
type RoomNetworkPolicyRequest struct {
	Policy    string `json:"policy"`
	ChannelID uint64 `json:"channel_id,omitempty"`
}
type AudioQoEFeedback struct {
	ObservedPackets           *uint64 `json:"observed_packets,omitempty"`
	PathMetricsAvailable      *bool   `json:"path_metrics_available,omitempty"`
	PeerID                    uint64  `json:"peer_id"`
	WindowMS                  uint32  `json:"window_ms"`
	TransportLossPercent      float64 `json:"transport_loss_percent"`
	LateLossPercent           float64 `json:"late_loss_percent"`
	EffectiveAudioLossPercent float64 `json:"effective_audio_loss_percent"`
	BurstLossPercent          float64 `json:"burst_loss_percent"`
	BurstCount                uint32  `json:"burst_count"`
	MaxConsecutiveLoss        uint32  `json:"max_consecutive_loss"`
	PacketReorderPercent      float64 `json:"packet_reorder_percent"`
	JitterMS                  float64 `json:"jitter_ms"`
	RTTMS                     float64 `json:"rtt_ms"`
	JitterBufferDelayMS       float64 `json:"jitter_buffer_delay_ms"`
	JitterBufferUnderruns     uint64  `json:"jitter_buffer_underruns"`
	PLCFrames                 uint64  `json:"plc_frames"`
	FECRecoveredPackets       uint64  `json:"fec_recovered_packets"`
	PlayoutResyncCount        uint64  `json:"playout_resync_count"`
}
type SubscriptionRequest struct {
	StreamIDs []uint64 `json:"stream_ids"`
}
type RouteUpdate struct {
	RoomID              uint64  `json:"room_id"`
	Topology            string  `json:"topology"`
	Transport           string  `json:"transport"`
	Reason              string  `json:"reason"`
	RoomNetworkPolicy   string  `json:"room_network_policy"`
	QualificationMS     int     `json:"qualification_ms"`
	MinimumDwellMS      int     `json:"minimum_dwell_ms"`
	MigrationCooldownMS int     `json:"migration_cooldown_ms"`
	DegradeDurationMS   int     `json:"degrade_duration_ms"`
	MaxRTTMS            int     `json:"max_rtt_ms"`
	MaxLossPercent      float64 `json:"max_loss_percent"`
	MaxJitterMS         int     `json:"max_jitter_ms"`
	DegradeRTTMS        int     `json:"degrade_rtt_ms"`
	DegradeLossPercent  float64 `json:"degrade_loss_percent"`
	DegradeJitterMS     int     `json:"degrade_jitter_ms"`
}

type ProvisionMasterRequest struct {
	Token string `json:"token"`
}
type ChannelMutationRequest struct {
	Password string  `json:"password,omitempty"`
	Channel  Channel `json:"channel"`
}
type ChannelRoleRequest struct {
	ChannelID uint64 `json:"channel_id"`
	ClientID  string `json:"client_id"`
	Enabled   bool   `json:"enabled"`
}
type ServerRoleRequest struct {
	ClientID string `json:"client_id"`
	Role     string `json:"role"`
}
type TargetRequest struct {
	ClientID  string     `json:"client_id"`
	SessionID uint64     `json:"session_id,omitempty"`
	ChannelID uint64     `json:"channel_id,omitempty"`
	Enabled   bool       `json:"enabled,omitempty"`
	Reason    string     `json:"reason,omitempty"`
	ExpiresAt *time.Time `json:"expires_at,omitempty"`
}
type BanListRequest struct {
	Offset int `json:"offset,omitempty"`
	Limit  int `json:"limit,omitempty"`
}
type BanRecord struct {
	ClientID         string     `json:"client_id"`
	DisplayName      string     `json:"display_name,omitempty"`
	Reason           string     `json:"reason"`
	CreatedAt        time.Time  `json:"created_at"`
	ExpiresAt        *time.Time `json:"expires_at,omitempty"`
	OperatorClientID string     `json:"operator_client_id"`
}
type BanList struct {
	Bans   []BanRecord `json:"bans"`
	Offset int         `json:"offset"`
	Total  int         `json:"total"`
}
type Capabilities struct {
	ServerRole              string   `json:"server_role"`
	Actions                 []string `json:"actions"`
	NetworkPolicyChannelIDs []uint64 `json:"network_policy_channel_ids,omitempty"`
}
type MediaRekey struct {
	Epoch uint16 `json:"epoch"`
	Salt  string `json:"salt,omitempty"`
}
