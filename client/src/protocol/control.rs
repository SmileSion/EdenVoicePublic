use serde::{Deserialize, Deserializer, Serialize, Serializer, de::DeserializeOwned};
use serde_json::{Map, Value};
use std::io;

#[derive(Clone, Debug)]
pub struct Envelope {
    pub kind: MessageType,
    pub data: Map<String, Value>,
}

impl Serialize for Envelope {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields = self.data.clone();
        fields.insert(
            "type".into(),
            serde_json::to_value(self.kind).map_err(serde::ser::Error::custom)?,
        );
        Value::Object(fields).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Envelope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = Map::<String, Value>::deserialize(deserializer)?;
        let kind = fields
            .remove("type")
            .ok_or_else(|| serde::de::Error::custom("control message type is required"))
            .and_then(|value| MessageType::deserialize(value).map_err(serde::de::Error::custom))?;
        Ok(Self { kind, data: fields })
    }
}

impl Envelope {
    pub fn new<T: Serialize>(kind: MessageType, data: T) -> serde_json::Result<Self> {
        let value = serde_json::to_value(data)?;
        let Value::Object(data) = value else {
            return Err(serde_json::Error::io(io::Error::new(
                io::ErrorKind::InvalidData,
                "control data must be an object",
            )));
        };
        Ok(Self { kind, data })
    }

    pub fn decode<T: DeserializeOwned>(&self) -> serde_json::Result<T> {
        serde_json::from_value(Value::Object(self.data.clone()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    Hello,
    IdentityChallenge,
    IdentityResponse,
    Welcome,
    ServerState,
    JoinChannel,
    SetChannelPassword,
    LeaveChannel,
    ChannelJoined,
    UserJoined,
    UserLeft,
    UserMoved,
    SetMute,
    SetDeafen,
    SetDisplayName,
    SetAvatar,
    SetGameActivity,
    GetGameCatalog,
    GameCatalog,
    GetAvatar,
    AvatarData,
    SendTextMessage,
    TextMessage,
    ChatHistory,
    QueryChat,
    ChatQueryResult,
    ChatHeads,
    GetRoomArchive,
    RoomArchive,
    RoomSocialRequest,
    RoomSocialEvent,
    ScreenShareRequest,
    ScreenShareState,
    WatchRequest,
    WatchState,
    VoiceActivity,
    UserStateChanged,
    Ping,
    Pong,
    Error,
    RegisterCandidates,
    PeerCandidates,
    RouteUpdate,
    NetworkReport,
    SetTransportMode,
    SetRoomNetworkPolicy,
    AudioQoeFeedback,
    ProvisionMaster,
    CreateChannel,
    EditChannel,
    DeleteChannel,
    SetChannelAdmin,
    SetServerRole,
    AdminMute,
    KickChannel,
    KickServer,
    Ban,
    Unban,
    ListBans,
    BanList,
    MoveUser,
    Capabilities,
    RequestMediaRekey,
    MediaRekeyPrepare,
    MediaRekeyAck,
    MediaRekeyCommit,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub kind: String,
    pub address: String,
    pub priority: u32,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CandidateRegistration {
    pub candidates: Vec<Candidate>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerCandidates {
    pub peer_id: u64,
    pub generation: u64,
    pub candidates: Vec<Candidate>,
    #[serde(default)]
    pub security_key: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NetworkReport {
    pub peer_id: u64,
    pub qualified: bool,
    pub rtt_ms: f64,
    pub packet_loss_percent: f64,
    pub jitter_ms: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteUpdate {
    pub room_id: u64,
    pub topology: String,
    pub transport: String,
    pub reason: String,
    #[serde(default = "default_room_network_policy")]
    pub room_network_policy: String,
    pub qualification_ms: u64,
    pub minimum_dwell_ms: u64,
    pub migration_cooldown_ms: u64,
    pub degrade_duration_ms: u64,
    pub max_rtt_ms: f64,
    pub max_loss_percent: f64,
    pub max_jitter_ms: f64,
    #[serde(default = "default_degrade_rtt_ms")]
    pub degrade_rtt_ms: f64,
    #[serde(default = "default_degrade_loss_percent")]
    pub degrade_loss_percent: f64,
    #[serde(default = "default_degrade_jitter_ms")]
    pub degrade_jitter_ms: f64,
}
fn default_room_network_policy() -> String {
    "auto".into()
}
fn default_degrade_rtt_ms() -> f64 {
    250.0
}
fn default_degrade_loss_percent() -> f64 {
    5.0
}
fn default_degrade_jitter_ms() -> f64 {
    40.0
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransportModeRequest {
    pub mode: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoomNetworkPolicyRequest {
    pub policy: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<u64>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioQoeFeedback {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_packets: Option<u64>,
    /// None preserves legacy peers; false explicitly marks unavailable probes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_metrics_available: Option<bool>,
    pub peer_id: u64,
    pub window_ms: u32,
    pub transport_loss_percent: f64,
    pub late_loss_percent: f64,
    pub effective_audio_loss_percent: f64,
    pub burst_loss_percent: f64,
    pub burst_count: u32,
    pub max_consecutive_loss: u32,
    pub packet_reorder_percent: f64,
    pub jitter_ms: f64,
    pub rtt_ms: f64,
    pub jitter_buffer_delay_ms: f64,
    pub jitter_buffer_underruns: u64,
    pub plc_frames: u64,
    pub fec_recovered_packets: u64,
    pub playout_resync_count: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub id: u64,
    #[serde(default)]
    pub parent_id: u64,
    pub name: String,
    pub sort_order: i32,
    #[serde(default)]
    pub user_limit: u32,
    #[serde(default = "default_channel_network_policy")]
    pub network_policy: String,
    #[serde(default = "default_channel_audio_quality_policy")]
    pub audio_quality_policy: String,
    #[serde(default)]
    pub temporary: bool,
}

fn default_channel_network_policy() -> String {
    "auto".into()
}

fn default_channel_audio_quality_policy() -> String {
    "auto".into()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct User {
    #[serde(default)]
    pub game_activity: Option<GameActivity>,
    pub session_id: u64,
    pub nickname: String,
    #[serde(default)]
    pub channel_id: u64,
    pub muted: bool,
    pub deafened: bool,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub server_role: String,
    #[serde(default)]
    pub channel_role: String,
    #[serde(default)]
    pub avatar: String,
    #[serde(default)]
    pub admin_muted: bool,
    #[serde(default)]
    pub server_muted: bool,
    #[serde(default)]
    pub channel_muted: bool,
}

#[derive(Serialize)]
pub struct Hello<'a> {
    pub client_features: Vec<&'static str>,
    pub client_version: &'a str,
    pub client_platform: &'a str,
    pub nickname: &'a str,
    pub protocol_version: u16,
    pub security_capabilities: Vec<&'static str>,
    pub client_id: &'a str,
    pub client_public_key: &'a str,
    pub client_ephemeral_key: &'a str,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdentityChallenge {
    #[serde(default)]
    pub server_password_required: bool,
    pub challenge: String,
    pub server_public_key: String,
    pub server_fingerprint: String,
    pub server_ephemeral_key: String,
    pub required_capabilities: Vec<String>,
}
#[derive(Serialize)]
pub struct IdentityResponse {
    #[serde(skip_serializing_if="String::is_empty")]
    pub server_password: String,
    pub signature: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Welcome {
    pub session_id: u64,
    pub server_name: String,
    pub voice_addr: String,
    #[serde(default = "default_media_queue_size")]
    pub media_queue_size: usize,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub server_fingerprint: String,
    #[serde(default)]
    pub media_key_epoch: u16,
    #[serde(default)]
    pub server_role: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub network_policy_channel_ids: Vec<u64>,
    #[serde(default)]
    pub server_features: Vec<String>,
}
fn default_media_queue_size() -> usize {
    256
}
#[derive(Clone, Debug, Deserialize)]
pub struct ServerState {
    #[serde(default)] pub chat_heads: std::collections::BTreeMap<u64,String>,
    #[serde(default)] pub announcement_revision: String,
    #[serde(default)] pub announcement: Option<serde_json::Value>,
    #[serde(default)] pub client_updates: Vec<serde_json::Value>,
    #[serde(default)] pub password_rooms_supported: bool,
    #[serde(default)]
    pub locked_channel_ids: Vec<u64>,
    #[serde(default)]
    pub game_catalog_version: u64,
    #[serde(default)]
    pub game_catalog: Option<crate::game_catalog::Catalog>,
    #[serde(default)]
    pub room_scenes: std::collections::BTreeMap<u64, RoomScene>,
    pub server_name: String,
    pub channels: Vec<Channel>,
    pub users: Vec<User>,
}
#[derive(Serialize)]
pub struct ChannelRequest {
    pub channel_id: u64,
}
#[derive(Deserialize)]
pub struct UserEvent {
    pub user: User,
}
#[derive(Deserialize)]
pub struct UserLeft {
    pub session_id: u64,
}
#[derive(Deserialize)]
pub struct UserMoved {
    pub session_id: u64,
    #[serde(default)]
    pub channel_id: u64,
}
#[derive(Serialize)]
pub struct StateRequest {
    pub enabled: bool,
}
#[derive(Serialize)]
pub struct AvatarRequest {
    pub avatar: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AvatarData {
    pub avatar: String,
    #[serde(default)]
    pub data: String,
}
#[derive(Serialize)]
pub struct SendTextMessageRequest {
    pub text: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextMessage {
    pub id: u64,
    pub channel_id: u64,
    pub sender_session_id: u64,
    #[serde(default)]
    pub sender_client_id: String,
    pub sender_name: String,
    #[serde(default)]
    pub sender_avatar: String,
    pub text: String,
    pub sent_at_unix_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<crate::protocol::room_social::Image>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatHistory {
    pub channel_id: u64,
    pub messages: Vec<TextMessage>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatQuery {
    #[serde(default)] pub before_id:String,
    pub request_id: String,
    pub channel_id: u64,
    pub cursor: String,
    pub query: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatQueryResult {
    pub request_id: String,
    pub channel_id: u64,
    pub messages: Vec<TextMessage>,
    pub next_cursor: String,
    pub error: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ArchiveGameMemory {
    pub game_id: String,
    pub title: String,
    pub started_at: String,
    pub ended_at: String,
    pub duration_seconds: i64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ArchiveEveningMemory {
    pub started_at: String,
    pub ended_at: String,
    pub duration_seconds: i64,
    pub games: Vec<String>,
    pub movies: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ArchiveEvent {
    #[serde(default)]
    pub evening: Option<ArchiveEveningMemory>,
    #[serde(default)]
    pub game: Option<ArchiveGameMemory>,
    pub id: String,
    pub event_type: String,
    pub category: String,
    pub room_id: u64,
    pub occurred_at: String,
    #[serde(default)]
    pub triggered_by: Vec<String>,
    #[serde(default)]
    pub participants: Vec<String>,
    pub importance: i32,
    pub rarity: String,
    #[serde(default)]
    pub rule_version: i32,
    #[serde(default)]
    pub titles: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub descriptions: std::collections::BTreeMap<String, String>,
    pub title_key: String,
    pub description_key: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ArchiveParticipantProfile {
    pub client_id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub avatar: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ArchiveRoomStats {
    pub room_id: u64,
    #[serde(default)]
    pub room_name: String,
    pub created_at: String,
    #[serde(default)]
    pub active_voice_seconds: i64,
    #[serde(default)]
    pub session_count: u64,
    #[serde(default)]
    pub longest_session_seconds: i64,
    #[serde(default)]
    pub max_concurrent_members: usize,
    #[serde(default, deserialize_with = "null_collection_default")]
    pub active_days: std::collections::BTreeMap<String, bool>,
    #[serde(default, deserialize_with = "null_collection_default")]
    pub members: std::collections::BTreeMap<String, serde_json::Value>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct RoomArchive {
    #[serde(default)]
    pub active_day_count: usize,
    #[serde(default)]
    pub historical_member_count: usize,
    pub stats: ArchiveRoomStats,
    #[serde(default)]
    pub age_days: i64,
    #[serde(default, deserialize_with = "null_collection_default")]
    pub events: Vec<ArchiveEvent>,
    #[serde(default)]
    pub participant_profiles: std::collections::BTreeMap<String, ArchiveParticipantProfile>,
    #[serde(default)]
    pub has_more: bool,
    #[serde(default)]
    pub enabled: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RoomArchiveRequest {
    pub channel_id: u64,
    pub all: bool,
    pub limit: usize,
}

// Older archive servers emitted null for empty maps and event lists.
fn null_collection_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}
#[derive(Deserialize)]
pub struct UserStateChanged {
    #[serde(default)]
    pub game_activity: Option<GameActivity>,
    pub session_id: u64,
    pub muted: bool,
    pub deafened: bool,
    #[serde(default)]
    pub admin_muted: bool,
    #[serde(default)]
    pub server_muted: bool,
    #[serde(default)]
    pub channel_muted: bool,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct Ping {
    pub nonce: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerError {
    pub code: String,
    pub message: String,
}

pub fn server_error_message(error: &ServerError) -> String {
    match error.code.as_str() {
        "server_password_required" => "此服务器需要密码，请输入服务器密码后重新连接。".into(),
        "server_password_invalid" => "服务器密码不正确，请重新输入。".into(),
        "server_password_rate_limited" => "连接尝试过于频繁，请一分钟后重试。".into(),
        "banned" => "此身份已被服务器封禁。".into(),
        "kicked_from_server" => "你已被管理员踢出服务器，可手动重新连接。".into(),
        "permission_denied" => "没有权限执行此操作。".into(),
        "identity_authentication_failed" | "identity_admission_failed" => {
            "身份验证失败，请检查本地身份文件后重试。".into()
        }
        "secure_mode_required" => "服务器要求 v0.5 安全模式，请升级客户端。".into(),
        "unknown_channel" => "频道不存在或已被删除。".into(),
        "channel_full" => "频道人数已达到上限。".into(),
        "invalid_nickname" => "用户名无效，请使用 1–64 字节的有效文字。".into(),
        "invalid_hello" => "服务器拒绝了连接握手，请确认客户端版本。".into(),
        "invalid_message" => "请求无效或目标状态已经变化，请刷新后重试。".into(),
        _ => {
            if error.message.trim().is_empty() {
                format!("服务器拒绝了请求（{}）。", error.code)
            } else {
                format!("服务器拒绝了请求：{}", error.message)
            }
        }
    }
}

#[derive(Serialize)]
pub struct ProvisionMasterRequest {
    pub token: String,
}
#[derive(Serialize)]
pub struct DisplayNameRequest {
    pub display_name: String,
}
#[derive(Serialize)]
pub struct ChannelMutationRequest {
    pub channel: Channel,
}
#[derive(Serialize)]
pub struct ChannelRoleRequest {
    pub channel_id: u64,
    pub client_id: String,
    pub enabled: bool,
}
#[derive(Serialize)]
pub struct ServerRoleRequest {
    pub client_id: String,
    pub role: String,
}
#[derive(Serialize)]
pub struct TargetRequest {
    pub client_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
#[derive(Serialize)]
pub struct BanListRequest {
    pub offset: usize,
    pub limit: usize,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BanRecord {
    pub client_id: String,
    #[serde(default)]
    pub display_name: String,
    pub reason: String,
    pub created_at: String,
    #[serde(default)]
    pub expires_at: Option<String>,
    pub operator_client_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BanList {
    pub bans: Vec<BanRecord>,
    pub offset: usize,
    pub total: usize,
}
#[derive(Deserialize)]
pub struct Capabilities {
    pub server_role: String,
    pub actions: Vec<String>,
    #[serde(default)]
    pub network_policy_channel_ids: Vec<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MediaRekey {
    pub epoch: u16,
    #[serde(default)]
    pub salt: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trip() {
        let value =
            Envelope::new(MessageType::JoinChannel, ChannelRequest { channel_id: 7 }).unwrap();
        let line = serde_json::to_string(&value).unwrap();
        assert_eq!(line, r#"{"channel_id":7,"type":"join_channel"}"#);
        let decoded: Envelope = serde_json::from_str(&line).unwrap();
        let request: ChannelRequestOwned = decoded.decode().unwrap();
        assert_eq!(request.channel_id, 7);
    }

    #[test]
    fn rejects_missing_type() {
        assert!(serde_json::from_str::<Envelope>(r#"{"nickname":"Sion"}"#).is_err());
    }

    #[test]
    fn network_policy_channel_scope_is_backward_compatible() {
        let current: Envelope = serde_json::from_str(
            r#"{"type":"capabilities","server_role":"USER","actions":["network.policy.set"],"network_policy_channel_ids":[7,9]}"#,
        )
        .unwrap();
        let current: Capabilities = current.decode().unwrap();
        assert_eq!(current.network_policy_channel_ids, [7, 9]);

        let legacy: Envelope = serde_json::from_str(
            r#"{"type":"capabilities","server_role":"ADMIN","actions":["network.policy.set"]}"#,
        )
        .unwrap();
        let legacy: Capabilities = legacy.decode().unwrap();
        assert!(legacy.network_policy_channel_ids.is_empty());
    }

    #[test]
    fn channel_policies_and_temporary_flag_are_backward_compatible() {
        let current: Channel = serde_json::from_str(
            r#"{"id":7,"name":"Games","sort_order":0,"user_limit":12,"network_policy":"force_sfu","audio_quality_policy":"quality_10","temporary":true}"#,
        )
        .unwrap();
        assert_eq!(current.network_policy, "force_sfu");
        assert_eq!(current.audio_quality_policy, "quality_10");
        assert!(current.temporary);
        assert_eq!(current.user_limit, 12);

        let legacy: Channel =
            serde_json::from_str(r#"{"id":7,"name":"Games","sort_order":0}"#).unwrap();
        assert_eq!(legacy.network_policy, "auto");
        assert_eq!(legacy.audio_quality_policy, "auto");
        assert!(!legacy.temporary);
        assert_eq!(legacy.user_limit, 0);
    }

    #[test]
    fn moderation_target_carries_exact_session_identity_binding() {
        let value = Envelope::new(
            MessageType::AdminMute,
            TargetRequest {
                client_id: "identity".into(),
                session_id: Some(42),
                channel_id: None,
                enabled: Some(true),
                reason: None,
            },
        )
        .unwrap();
        let encoded = serde_json::to_value(value).unwrap();
        assert_eq!(encoded["client_id"], "identity");
        assert_eq!(encoded["session_id"], 42);
        assert_eq!(encoded["enabled"], true);
        assert!(encoded.get("channel_id").is_none());
    }

    #[test]
    fn legacy_user_state_change_defaults_admin_mute_to_false() {
        let event: UserStateChanged =
            serde_json::from_str(r#"{"session_id":1,"muted":true,"deafened":false}"#)
                .expect("legacy event should remain compatible");
        assert!(!event.admin_muted);
        assert!(!event.server_muted);
        assert!(!event.channel_muted);
    }

    #[test]
    fn server_errors_have_actionable_user_messages() {
        assert_eq!(
            server_error_message(&ServerError {
                code: "banned".into(),
                message: "this identity is banned".into(),
            }),
            "此身份已被服务器封禁。"
        );
        assert_eq!(
            server_error_message(&ServerError {
                code: "permission_denied".into(),
                message: "you do not have permission".into(),
            }),
            "没有权限执行此操作。"
        );
        assert_eq!(
            server_error_message(&ServerError {
                code: "kicked_from_server".into(),
                message: "kicked by administrator".into(),
            }),
            "你已被管理员踢出服务器，可手动重新连接。"
        );
        assert!(
            server_error_message(&ServerError {
                code: "future_error".into(),
                message: "retry later".into(),
            })
            .contains("retry later")
        );
    }

    #[test]
    fn older_route_updates_get_safe_degradation_defaults() {
        let route: RouteUpdate = serde_json::from_str(
            r#"{"room_id":1,"topology":"p2p_evaluating","transport":"sfu","reason":"test","qualification_ms":1500,"minimum_dwell_ms":1500,"migration_cooldown_ms":10000,"degrade_duration_ms":1500,"max_rtt_ms":80,"max_loss_percent":2,"max_jitter_ms":15}"#,
        )
        .unwrap();
        assert_eq!(route.degrade_rtt_ms, 250.0);
        assert_eq!(route.degrade_loss_percent, 5.0);
        assert_eq!(route.degrade_jitter_ms, 40.0);
    }

    #[test]
    fn older_servers_default_to_no_archive_feature() {
        let welcome: Welcome = serde_json::from_str(
            r#"{"session_id":1,"server_name":"Old","voice_addr":"127.0.0.1:9001"}"#,
        )
        .unwrap();
        assert!(welcome.server_features.is_empty());
    }

    #[test]
    fn text_messages_and_avatar_fields_round_trip_compatibly() {
        let envelope = Envelope::new(
            MessageType::TextMessage,
            TextMessage {
                id: 9,
                channel_id: 3,
                sender_session_id: 7,
                sender_client_id: "client".into(),
                sender_name: "Alice".into(),
                sender_avatar: "K".into(),
                text: "hello".into(),
                sent_at_unix_ms: 42,
                image: None,
            },
        )
        .unwrap();
        let decoded: TextMessage = envelope.decode().unwrap();
        assert_eq!(decoded.sender_avatar, "K");
        assert_eq!(decoded.text, "hello");

        let legacy: User = serde_json::from_str(
            r#"{"session_id":1,"nickname":"Legacy","muted":false,"deafened":false}"#,
        )
        .unwrap();
        assert!(legacy.avatar.is_empty());

        let avatar = Envelope::new(
            MessageType::AvatarData,
            AvatarData {
                avatar: "custom:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                    .into(),
                data: "data:image/jpeg;base64,AA==".into(),
            },
        )
        .unwrap();
        let decoded: AvatarData = avatar.decode().unwrap();
        assert!(decoded.avatar.starts_with("custom:"));
        assert!(decoded.data.starts_with("data:image/jpeg;base64,"));
    }

    #[test]
    fn room_archive_round_trips_stable_event_keys() {
        let envelope = Envelope::new(
            MessageType::RoomArchive,
            RoomArchive {
                stats: ArchiveRoomStats {
                    room_id: 7,
                    room_name: "Eden".into(),
                    active_voice_seconds: 3600,
                    ..Default::default()
                },
                age_days: 30,
                events: vec![ArchiveEvent {
                    id: "evt_1".into(),
                    event_type: "voice_total_1h".into(),
                    rule_version: 2,
                    titles: std::collections::BTreeMap::from([(
                        "zh-CN".into(),
                        "服务端自定义记忆".into(),
                    )]),
                    descriptions: std::collections::BTreeMap::from([(
                        "zh-CN".into(),
                        "无需客户端内置词条".into(),
                    )]),
                    triggered_by: vec!["client-a".into()],
                    title_key: "archive.voice_total_1h.title".into(),
                    description_key: "archive.voice_total_1h.description".into(),
                    ..Default::default()
                }],
                participant_profiles: std::collections::BTreeMap::from([(
                    "client-a".into(),
                    ArchiveParticipantProfile {
                        client_id: "client-a".into(),
                        display_name: "Eden".into(),
                        avatar: "E".into(),
                    },
                )]),
                enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        let decoded: RoomArchive = envelope.decode().unwrap();
        assert_eq!(decoded.stats.active_voice_seconds, 3600);
        assert_eq!(decoded.events[0].event_type, "voice_total_1h");
        assert_eq!(decoded.events[0].rule_version, 2);
        assert_eq!(decoded.events[0].titles["zh-CN"], "服务端自定义记忆");
        assert_eq!(
            decoded.events[0].descriptions["zh-CN"],
            "无需客户端内置词条"
        );
        assert_eq!(decoded.events[0].title_key, "archive.voice_total_1h.title");
        assert_eq!(decoded.events[0].triggered_by, ["client-a"]);
        assert_eq!(
            decoded.participant_profiles["client-a"].display_name,
            "Eden"
        );
    }

    #[test]
    fn room_archive_decodes_shared_go_wire_fixture_and_legacy_nulls() {
        let fixture =
            include_str!("../../../server/internal/control/testdata/room-archive-empty.json");
        let envelope: Envelope = serde_json::from_str(fixture).unwrap();
        let archive: RoomArchive = envelope.decode().unwrap();
        assert_eq!(archive.stats.room_id, 1);
        assert!(archive.enabled && archive.events.is_empty());
        assert!(archive.stats.active_days.is_empty() && archive.stats.members.is_empty());
        let mut legacy: serde_json::Value = serde_json::from_str(fixture).unwrap();
        legacy["stats"]["active_days"] = serde_json::Value::Null;
        legacy["stats"]["members"] = serde_json::Value::Null;
        legacy["events"] = serde_json::Value::Null;
        let envelope: Envelope = serde_json::from_value(legacy).unwrap();
        let archive: RoomArchive = envelope.decode().unwrap();
        assert!(archive.events.is_empty() && archive.stats.members.is_empty());
    }

    #[derive(Deserialize)]
    struct ChannelRequestOwned {
        channel_id: u64,
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameActivity {
    pub kind: String,
    pub title: String,
    pub source: String,
    pub updated_at: i64,
    pub expires_at: i64,
}
#[derive(Serialize)]
pub struct GameActivityRequest {
    pub game_id: String,
}

#[cfg(test)]
mod game_activity_tests {
    use super::*;
    #[test]
    fn activity_wire_round_trip_and_clear() {
        let active:UserStateChanged=serde_json::from_str(r#"{"session_id":1,"muted":false,"deafened":false,"game_activity":{"kind":"game","title":"Counter-Strike 2","source":"windows","updatedAt":1000,"expiresAt":61000}}"#).unwrap();
        assert_eq!(active.game_activity.unwrap().expires_at, 61000);
        let cleared: UserStateChanged = serde_json::from_str(
            r#"{"session_id":1,"muted":false,"deafened":false,"game_activity":null}"#,
        )
        .unwrap();
        assert!(cleared.game_activity.is_none());
        let old: UserStateChanged =
            serde_json::from_str(r#"{"session_id":1,"muted":false,"deafened":false}"#).unwrap();
        assert!(old.game_activity.is_none());
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RoomScene {
    pub game_id: String,
    pub title: String,
}

#[cfg(test)]
mod archive_game_compatibility {
    use super::*;
    #[test]
    fn legacy_and_game_memories_share_the_archive_protocol() {
        let mut value = serde_json::to_value(ArchiveEvent::default()).unwrap();
        value.as_object_mut().unwrap().remove("game");
        let legacy: ArchiveEvent = serde_json::from_value(value.clone()).unwrap();
        assert!(legacy.game.is_none());
        value["game"] = serde_json::json!({"game_id":"cs2","title":"Counter-Strike 2","started_at":"2026-09-14T20:00:00Z","ended_at":"2026-09-14T20:02:00Z","duration_seconds":120});
        let current: ArchiveEvent = serde_json::from_value(value).unwrap();
        assert_eq!(current.game.unwrap().duration_seconds, 120);
    }
}
