mod chat;
mod incident;
mod screenshare;
mod room_social;
mod system_audio;
mod updates;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock, mpsc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

mod invitations;
mod close_behavior;
mod mpv;
mod tray;
mod window_scale;
mod watch;
mod game_presence;
use std::sync::atomic::{AtomicBool,Ordering};
static EXITING: AtomicBool = AtomicBool::new(false);

use edenvoice_client::{
    application::{
        AppCommand, Application, AudioPreferences, InputMode, NoiseSuppressionMode,
        NoiseSuppressionStrength,
    },
    config::{Language, SavedServer, ShortcutBindings, ThemeMode},
    model::{ConnectionState, MicTestStatus},
    network::RoomNetworkPolicy,
    protocol::control::Channel,
};
use serde::{Deserialize, Serialize};
use tauri::{
    Emitter, LogicalSize, Manager, PhysicalPosition, Position, Size, State, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use zeroize::Zeroizing;

// Debian's MinGW headers may emit glibc-style fortified memory calls while
// cross-compiling vendored C code. Windows has no matching runtime symbols, so
// provide the two checked operations that Opus needs for the GNU target only.
#[cfg(all(target_os = "windows", target_env = "gnu"))]
mod mingw_fortify_compat {
    use std::ffi::{c_int, c_void};

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __memcpy_chk(
        destination: *mut c_void,
        source: *const c_void,
        count: usize,
        destination_size: usize,
    ) -> *mut c_void {
        if count > destination_size {
            std::process::abort();
        }
        unsafe {
            std::ptr::copy_nonoverlapping(source.cast::<u8>(), destination.cast::<u8>(), count);
        }
        destination
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __memset_chk(
        destination: *mut c_void,
        value: c_int,
        count: usize,
        destination_size: usize,
    ) -> *mut c_void {
        if count > destination_size {
            std::process::abort();
        }
        unsafe {
            std::ptr::write_bytes(destination.cast::<u8>(), value as u8, count);
        }
        destination
    }
}

#[derive(Clone)]
struct BridgeState {
    incidents: Arc<std::sync::Mutex<incident::Recorder>>,
    commands: mpsc::SyncSender<CoreCommand>,
    snapshot: Arc<RwLock<CoreSnapshot>>,
    watch: Arc<RwLock<watch::View>>,
    stopped: Arc<AtomicBool>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CoreCommand {
    #[serde(skip)]
    QueryChat {request:chat::Request,reply:chat::Reply},
    #[serde(skip)]
    RoomSocial { context: room_social::Context, request: room_social::UiRequest, reply: room_social::Reply },
    SetMpvPath { path: String },
    #[serde(skip)]
    SaveMpvPath { path: String, reply: mpsc::SyncSender<Result<String,String>> },
    WatchRoom { action: String, #[serde(default, rename="targetSessionId")] target_session_id: Option<String>, #[serde(rename="channelId")] channel_id: String, #[serde(rename="sessionId")] session_id: String, #[serde(rename="serverAddress")] server_address: String },
    WatchOpen { file: String, #[serde(rename="channelId")] channel_id: String, #[serde(rename="sessionId")] session_id: String, #[serde(rename="serverAddress")] server_address: String },
    WatchReopen { #[serde(rename="channelId")] channel_id: String, #[serde(rename="sessionId")] session_id: String, #[serde(rename="serverAddress")] server_address: String },
    WatchControl { action: String, position: Option<f64> },
    WatchLeave,
    Connect {
        #[serde(default, rename="serverPassword")]
        server_password: String,
        address: String,
        nickname: String,
        remember: bool,
        #[serde(default, rename = "autoConnect")]
        auto_connect: Option<bool>,
    },
    Disconnect,
    RetryAudio,
    JoinChannel {
        #[serde(rename = "channelId")]
        channel_id: String,
    },
    JoinProtectedChannel { #[serde(rename="channelId")] channel_id:String, password:String },
    SetChannelPassword { #[serde(rename="channelId")] channel_id:String, password:String },
    LeaveChannel,
    SetMuted {
        muted: bool,
    },
    SetDeafened {
        deafened: bool,
    },
    SetPushToTalkPressed {
        pressed: bool,
    },
    SendTextMessage {
        text: String,
    },
    RequestRoomArchive {
        #[serde(rename = "channelId")]
        channel_id: String,
        all: bool,
    },
    CloseRoomArchive,
    SetUserVolume {
        #[serde(rename = "sessionId")]
        session_id: String,
        volume: f32,
    },
    SetUserLocalMute {
        #[serde(rename = "sessionId")]
        session_id: String,
        muted: bool,
    },
    AdminMute {
        #[serde(rename = "sessionId")]
        session_id: String,
        enabled: bool,
    },
    KickChannel {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    KickServer {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    BanUser {
        #[serde(rename = "sessionId")]
        session_id: String,
        reason: String,
    },
    MoveUser {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "channelId")]
        channel_id: String,
    },
    ListBans {
        offset: usize,
        limit: usize,
    },
    Unban {
        #[serde(rename = "clientId")]
        client_id: String,
    },
    SetChannelAdmin {
        #[serde(rename = "sessionId")]
        session_id: String,
        enabled: bool,
    },
    SetServerAdmin {
        #[serde(rename = "sessionId")]
        session_id: String,
        enabled: bool,
    },
    SetChannelNetworkPolicy {
        #[serde(rename = "channelId")]
        channel_id: String,
        policy: String,
    },
    CreateChannel {
        channel: ChannelSnapshot,
        #[serde(default)] password: Option<String>,
    },
    EditChannel {
        channel: ChannelSnapshot,
    },
    DeleteChannel {
        #[serde(rename = "channelId")]
        channel_id: String,
    },
    SetShareGameActivity { enabled: bool },
    SetAvatar {
        avatar: String,
    },
    SetDisplayName {
        #[serde(rename = "displayName")]
        display_name: String,
    },
    UpdateSavedServer {
        index: usize,
        profile: SavedServerSnapshot,
    },
    AddSavedServer {
        profile: SavedServerSnapshot,
    },
    DeleteSavedServer {
        index: usize,
    },
    SetTheme {
        theme: String,
    },
    SetLanguage {
        language: String,
    },
    SetMiniOverlay {
        enabled: bool,
        mode: String,
    },
    SetMiniOverlayPosition {
        x: i32,
        y: i32,
    },
    SetAutoConnectLastServer {
        enabled: bool,
    },
    SetAutoConnect {
        enabled: bool,
        #[serde(rename = "serverAddress")]
        server_address: String,
    },
    UpdateShortcuts {
        shortcuts: ShortcutSnapshot,
    },
    ScreenShare {
        server_address: String,
        self_session_id: String,
        request: screenshare::ScreenRequest,
    },
    UpdateAudioPreferences {
        #[serde(rename = "inputDevice")]
        input_device: String,
        #[serde(rename = "outputDevice")]
        output_device: String,
        #[serde(rename = "inputGain")]
        input_gain: f32,
        #[serde(rename = "outputVolume")]
        output_volume: f32,
        #[serde(rename = "inputMode")]
        input_mode: String,
        #[serde(rename = "noiseSuppression")]
        noise_suppression: String,
        #[serde(rename = "noiseSuppressionStrength")]
        noise_suppression_strength: String,
        #[serde(rename = "echoCancellation")]
        echo_cancellation: bool,
        #[serde(rename = "automaticGain")]
        automatic_gain: bool,
    },
    ProvisionMaster {
        token: String,
    },
    TrustServer {
        endpoint: String,
        fingerprint: String,
    },
    RefreshDevices,
    PlayOutputTest,
    StartMicTest,
    PlayMicTestOriginal,
    PlayMicTestProcessed,
    StopMicTest,
    Shutdown,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChannelSnapshot {
    id: String,
    parent_id: String,
    name: String,
    sort_order: i32,
    user_limit: u32,
    network_policy: String,
    audio_quality_policy: String,
    temporary: bool,
}

impl TryFrom<ChannelSnapshot> for Channel {
    type Error = String;

    fn try_from(value: ChannelSnapshot) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id.parse().map_err(|_| "invalid channel id")?,
            parent_id: value
                .parent_id
                .parse()
                .map_err(|_| "invalid parent channel id")?,
            name: value.name,
            sort_order: value.sort_order,
            user_limit: value.user_limit.min(10_000),
            network_policy: value.network_policy,
            audio_quality_policy: value.audio_quality_policy,
            temporary: value.temporary,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct UserSnapshot {
    game_activity: Option<edenvoice_client::protocol::control::GameActivity>,
    session_id: String,
    client_id: String,
    nickname: String,
    avatar: String,
    channel_id: String,
    muted: bool,
    deafened: bool,
    admin_muted: bool,
    locally_muted: bool,
    speaking: bool,
    server_role: String,
    channel_role: String,
    volume: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatMessageSnapshot {
    id: String,
    channel_id: String,
    sender_session_id: String,
    sender_client_id: String,
    sender_name: String,
    sender_avatar: String,
    text: String,
    sent_at_unix_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<edenvoice_client::protocol::room_social::Image>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct AudioSnapshot {
    inputs: Vec<String>,
    outputs: Vec<String>,
    selected_input: String,
    selected_output: String,
    input_gain: f32,
    output_volume: f32,
    input_mode: String,
    noise_suppression: String,
    noise_suppression_strength: String,
    echo_cancellation: bool,
    automatic_gain: bool,
    state: String,
    mic_test_status: String,
    device_error: String,
    dsp_error: String,
    active_input: String,
    active_output: String,
    input_sample_rate: Option<u32>,
    output_sample_rate: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticSnapshot {
    timing: edenvoice_client::audio::AudioTiming,
    rtt_ms: Option<u64>,
    control_rtt_ms: Option<u64>,
    jitter_ms: f64,
    effective_audio_loss_percent: f64,
    transport: String,
    network_quality: String,
    opus_bitrate_bps: i32,
    fec_recovered_packets: u64,
    plc_frames: u64,
    jitter_buffer_delay_ms: f64,
    jitter_depth_frames: usize,
    jitter_buffer_underruns: u64,
    playback_underruns: u64,
    capture_overruns: u64,
    packets_sent: u64,
    packets_received: u64,
    packets_lost: u64,
    packets_reordered: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct BanSnapshot {
    client_id: String,
    display_name: String,
    reason: String,
    created_at: String,
    expires_at: String,
    operator_client_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShortcutSnapshot {
    toggle_window: String,
    toggle_mic: String,
    toggle_speaker: String,
    push_to_talk: String,
    toggle_members: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedServerSnapshot {
    display_name: String,
    server_name: String,
    address: String,
    nickname: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveEventSnapshot {
    evening: Option<edenvoice_client::protocol::control::ArchiveEveningMemory>,
    game: Option<edenvoice_client::protocol::control::ArchiveGameMemory>,
    id: String,
    event_type: String,
    category: String,
    occurred_at: String,
    triggered_by: Vec<String>,
    participants: Vec<String>,
    importance: i32,
    rarity: String,
    rule_version: i32,
    titles: BTreeMap<String, String>,
    descriptions: BTreeMap<String, String>,
    title_key: String,
    description_key: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveParticipantSnapshot {
    client_id: String,
    display_name: String,
    avatar: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveStatsSnapshot {
    room_id: String,
    room_name: String,
    created_at: String,
    active_voice_seconds: i64,
    session_count: u64,
    longest_session_seconds: i64,
    max_concurrent_members: usize,
    active_days: usize,
    historical_members: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct RoomArchiveSnapshot {
    stats: ArchiveStatsSnapshot,
    age_days: i64,
    events: Vec<ArchiveEventSnapshot>,
    participant_profiles: BTreeMap<String, ArchiveParticipantSnapshot>,
    has_more: bool,
    enabled: bool,
}

impl From<&edenvoice_client::protocol::control::RoomArchive> for RoomArchiveSnapshot {
    fn from(value: &edenvoice_client::protocol::control::RoomArchive) -> Self {
        Self {
            stats: ArchiveStatsSnapshot {
                room_id: value.stats.room_id.to_string(),
                room_name: value.stats.room_name.clone(),
                created_at: value.stats.created_at.clone(),
                active_voice_seconds: value.stats.active_voice_seconds,
                session_count: value.stats.session_count,
                longest_session_seconds: value.stats.longest_session_seconds,
                max_concurrent_members: value.stats.max_concurrent_members,
                active_days: value.active_day_count.max(value.stats.active_days.len()),
                historical_members: value.historical_member_count.max(value.stats.members.len()),
            },
            age_days: value.age_days,
            events: value
                .events
                .iter()
                .map(|event| ArchiveEventSnapshot {
                    evening: event.evening.clone(),
                    game: event.game.clone(),
                    id: event.id.clone(),
                    event_type: event.event_type.clone(),
                    category: event.category.clone(),
                    occurred_at: event.occurred_at.clone(),
                    triggered_by: event.triggered_by.clone(),
                    participants: event.participants.clone(),
                    importance: event.importance,
                    rarity: event.rarity.clone(),
                    rule_version: event.rule_version,
                    titles: event.titles.clone(),
                    descriptions: event.descriptions.clone(),
                    title_key: event.title_key.clone(),
                    description_key: event.description_key.clone(),
                })
                .collect(),
            participant_profiles: value
                .participant_profiles
                .iter()
                .map(|(client_id, profile)| {
                    (
                        client_id.clone(),
                        ArchiveParticipantSnapshot {
                            client_id: profile.client_id.clone(),
                            display_name: profile.display_name.clone(),
                            avatar: profile.avatar.clone(),
                        },
                    )
                })
                .collect(),
            has_more: value.has_more,
            enabled: value.enabled,
        }
    }
}

impl From<SavedServerSnapshot> for SavedServer {
    fn from(value: SavedServerSnapshot) -> Self {
        Self {
            display_name: value.display_name,
            server_name: value.server_name,
            address: value.address,
            nickname: value.nickname,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CoreSnapshot {
    chat_query_supported: bool,
    chat_heads: std::collections::BTreeMap<String,String>,
    revision: u64,
    connection: String,
    connection_detail: String,
    error: String,
    trust_endpoint: String,
    trust_fingerprint: String,
    server_name: String,
    server_address: String,
    nickname: String,
    avatar: String,
    client_id: String,
    self_session_id: String,
    server_role: String,
    theme: String,
    language: String,
    share_game_activity: bool,
    game_detection_available: bool,
    game_activity_supported: bool,
    announcement_scope: String,
    announcement_revision: String,
    announcement: Option<serde_json::Value>,
    #[serde(skip)]
    client_updates: Vec<serde_json::Value>,
    password_rooms_supported: bool,
    locked_channel_ids: Vec<String>,
    room_scenes: std::collections::BTreeMap<String, edenvoice_client::protocol::control::RoomScene>,
    mini_overlay_enabled: bool,
    mini_overlay_mode: String,
    #[serde(skip_serializing)]
    mini_overlay_position_x: Option<i32>,
    #[serde(skip_serializing)]
    mini_overlay_position_y: Option<i32>,
    auto_connect_last_server: bool,
    auto_connect_server: String,
    auto_connecting: bool,
    saved_servers: Vec<SavedServerSnapshot>,
    capabilities: Vec<String>,
    network_policy_channel_ids: Vec<String>,
    bans: Vec<BanSnapshot>,
    ban_list_offset: usize,
    ban_list_total: usize,
    channels: Vec<ChannelSnapshot>,
    users: Vec<UserSnapshot>,
    avatar_cache_revision: u64,
    messages: Vec<ChatMessageSnapshot>,
    archive_supported: bool,
    self_away: bool,
    muted_speech_notice: Option<edenvoice_client::audio::MutedSpeechNotice>,
    room_social_epoch: String,
    room_social_supported: bool,
    room_images_supported: bool,
    screen_share_supported: bool,
    watch_supported: bool,
    muted: bool,
    deafened: bool,
    input_level: f32,
    output_level: f32,
    latency_ms: Option<u64>,
    control_latency_ms: Option<u64>,
    transport: String,
    network_quality: String,
    diagnostics: DiagnosticSnapshot,
    audio: AudioSnapshot,
    shortcuts: ShortcutSnapshot,
}

impl Default for CoreSnapshot {
    fn default() -> Self {
        Self {
            revision: 0,
            chat_query_supported:false,chat_heads:Default::default(),
            connection: "disconnected".into(),
            connection_detail: String::new(),
            error: String::new(),
            trust_endpoint: String::new(),
            trust_fingerprint: String::new(),
            server_name: String::new(),
            server_address: String::new(),
            nickname: String::new(),
            avatar: String::new(),
            client_id: String::new(),
            self_session_id: String::new(),
            server_role: String::new(),
            theme: "system".into(),
            language: "chinese".into(),
            share_game_activity: true,
            game_detection_available: cfg!(windows),
            game_activity_supported: false,
            announcement_scope: String::new(),
            announcement_revision: String::new(),
            announcement: None,
            client_updates: Vec::new(),
            password_rooms_supported: false,
            locked_channel_ids: Default::default(),
            room_scenes: Default::default(),
            mini_overlay_enabled: false,
            mini_overlay_mode: "avatars".into(),
            mini_overlay_position_x: None,
            mini_overlay_position_y: None,
            auto_connect_last_server: true,
            auto_connect_server: String::new(),
            auto_connecting: false,
            saved_servers: Vec::new(),
            capabilities: Vec::new(),
            network_policy_channel_ids: Vec::new(),
            bans: Vec::new(),
            ban_list_offset: 0,
            ban_list_total: 0,
            channels: Vec::new(),
            users: Vec::new(),
            avatar_cache_revision: 0,
            messages: Vec::new(),
            archive_supported: false,
            self_away: false,
            muted_speech_notice: None,
            room_social_epoch: "0".into(),
            room_social_supported: false,
            room_images_supported: false,
            screen_share_supported: false,
            watch_supported: false,
            muted: false,
            deafened: false,
            input_level: 0.0,
            output_level: 0.0,
            latency_ms: None,
            control_latency_ms: None,
            transport: "OFFLINE".into(),
            network_quality: "good".into(),
            diagnostics: DiagnosticSnapshot::default(),
            audio: AudioSnapshot::default(),
            shortcuts: ShortcutSnapshot::default(),
        }
    }
}

impl CoreSnapshot {
    fn from_application(core: &Application, revision: u64) -> Self {
        let state = core.state();
        let preferences = core.audio_preferences();
        let (connection, connection_detail) = match &state.connection {
            ConnectionState::Disconnected => ("disconnected", String::new()),
            ConnectionState::Connecting => ("connecting", "正在建立安全控制连接".into()),
            ConnectionState::Connected => ("connected", "已认证".into()),
            ConnectionState::Reconnecting => ("reconnecting", "连接中断，正在恢复".into()),
            ConnectionState::Error(detail) => ("error", detail.clone()),
        };
        let channels = state
            .channels
            .iter()
            .map(|channel| ChannelSnapshot {
                id: channel.id.to_string(),
                parent_id: channel.parent_id.to_string(),
                name: channel.name.clone(),
                sort_order: channel.sort_order,
                user_limit: channel.user_limit,
                network_policy: channel.network_policy.clone(),
                audio_quality_policy: channel.audio_quality_policy.clone(),
                temporary: channel.temporary,
            })
            .collect();
        let users = state
            .users
            .values()
            .map(|user| UserSnapshot {
                session_id: user.session_id.to_string(),
                client_id: user.client_id.clone(),
                nickname: user.nickname.clone(),
                avatar: user.avatar.clone(),
                game_activity: user.game_activity.clone(),
                channel_id: user.channel_id.to_string(),
                muted: user.muted,
                deafened: user.deafened,
                admin_muted: user.admin_muted,
                locally_muted: state.is_user_locally_muted(user.session_id),
                speaking: state.is_talking(user.session_id),
                server_role: user.server_role.clone(),
                channel_role: user.channel_role.clone(),
                volume: state.user_volume(user.session_id),
            })
            .collect();
        let messages = state
            .chat_messages
            .iter()
            .map(|message| ChatMessageSnapshot {
                id: message.id.to_string(),
                channel_id: message.channel_id.to_string(),
                sender_session_id: message.sender_session_id.to_string(),
                sender_client_id: message.sender_client_id.clone(),
                sender_name: message.sender_name.clone(),
                sender_avatar: message.sender_avatar.clone(),
                text: message.text.clone(),
                sent_at_unix_ms: message.sent_at_unix_ms,
                image: message.image.clone(),
            })
            .collect();
        Self {
            revision,
            chat_query_supported:state.chat_query_supported,
            chat_heads:state.chat_heads.iter().map(|(channel,id)|(channel.to_string(),id.clone())).collect(),
            connection: connection.into(),
            connection_detail,
            error: core.error().unwrap_or_default().into(),
            trust_endpoint: state.trust_endpoint.clone(),
            trust_fingerprint: state.trust_fingerprint.clone(),
            server_name: state.server_name.clone(),
            server_address: core.server_address().into(),
            nickname: core.nickname().into(),
            avatar: core.avatar().into(),
            client_id: core.client_id().into(),
            self_session_id: state
                .self_session
                .map(|id| id.to_string())
                .unwrap_or_default(),
            server_role: state.server_role.clone(),
            theme: format!("{:?}", core.theme()).to_lowercase(),
            language: format!("{:?}", core.language()).to_lowercase(),
            share_game_activity: core.share_game_activity(),
            game_detection_available: cfg!(windows),
            game_activity_supported: state.game_activity_supported,
            announcement_scope: state.announcement_scope.clone(),
            announcement_revision: state.announcement_revision.to_string(),
            announcement: state.announcement.clone(),
            client_updates: state.client_updates.clone(),
            password_rooms_supported: state.password_rooms_supported,
            locked_channel_ids: state.locked_channel_ids.iter().map(|id|id.to_string()).collect(),
            room_scenes: state.room_scenes.iter().map(|(id, scene)| (id.to_string(), scene.clone())).collect(),
            mini_overlay_enabled: core.mini_overlay_enabled(),
            mini_overlay_mode: if core.mini_overlay_mode() == "list" {
                "list".into()
            } else {
                "avatars".into()
            },
            mini_overlay_position_x: core.mini_overlay_position().map(|position| position.0),
            mini_overlay_position_y: core.mini_overlay_position().map(|position| position.1),
            auto_connect_last_server: core.auto_connect_last_server(),
            auto_connect_server: core.auto_connect_server().into(),
            auto_connecting: core.auto_connecting(),
            saved_servers: core
                .saved_servers()
                .iter()
                .map(|profile| SavedServerSnapshot {
                    display_name: profile.display_name.clone(),
                    server_name: profile.server_name.clone(),
                    address: profile.address.clone(),
                    nickname: profile.nickname.clone(),
                })
                .collect(),
            capabilities: state.capabilities.clone(),
            network_policy_channel_ids: state
                .network_policy_channel_ids
                .iter()
                .map(u64::to_string)
                .collect(),
            bans: state
                .bans
                .iter()
                .map(|ban| BanSnapshot {
                    client_id: ban.client_id.clone(),
                    display_name: ban.display_name.clone(),
                    reason: ban.reason.clone(),
                    created_at: ban.created_at.clone(),
                    expires_at: ban.expires_at.clone().unwrap_or_default(),
                    operator_client_id: ban.operator_client_id.clone(),
                })
                .collect(),
            ban_list_offset: state.ban_list_offset,
            ban_list_total: state.ban_list_total,
            channels,
            users,
            avatar_cache_revision: state.avatar_cache_revision,
            messages,
            archive_supported: state.archive_supported,
            self_away: state.self_away,
            muted_speech_notice: state.muted_speech_notice.clone(),
            room_social_epoch: state.room_social_epoch.to_string(),
            room_social_supported: state.room_social_supported,
            room_images_supported: state.room_images_supported,
            screen_share_supported: state.screen_share_supported,
            watch_supported: state.watch_supported,
            muted: state.muted,
            deafened: state.deafened,
            input_level: state.input_level,
            output_level: state.output_level,
            latency_ms: state.rtt_ms,
            control_latency_ms: state.control_rtt_ms,
            transport: if state.actual_transport.is_empty() {
                state.current_transport.clone()
            } else {
                state.actual_transport.clone()
            },
            network_quality: state.network_quality.clone(),
            diagnostics: DiagnosticSnapshot {
                rtt_ms: state.rtt_ms,
                control_rtt_ms: state.control_rtt_ms,
                jitter_ms: state.jitter_ms,
                effective_audio_loss_percent: state.effective_audio_loss_percent,
                transport: if state.actual_transport.is_empty() {
                    state.current_transport.clone()
                } else {
                    state.actual_transport.clone()
                },
                network_quality: state.network_quality.clone(),
                opus_bitrate_bps: state.opus_bitrate_bps,
                fec_recovered_packets: state.fec_recovered_packets,
                plc_frames: state.plc_frames,
                jitter_buffer_delay_ms: state.jitter_buffer_delay_ms,
                jitter_depth_frames: state.jitter_depth,
                jitter_buffer_underruns: state.jitter_buffer_underruns,
                playback_underruns: state.playback_underruns,
                capture_overruns: state.capture_overruns,
                timing: state.audio_timing.clone(),
                packets_sent: state.packets_sent,
                packets_received: state.packets_received,
                packets_lost: state.packets_lost,
                packets_reordered: state.packets_reordered,
            },
            audio: AudioSnapshot {
                inputs: core.devices().inputs.clone(),
                outputs: core.devices().outputs.clone(),
                selected_input: preferences.input_device.unwrap_or_default(),
                selected_output: preferences.output_device.unwrap_or_default(),
                input_gain: preferences.input_gain,
                output_volume: preferences.output_volume,
                input_mode: input_mode_wire(preferences.input_mode).into(),
                noise_suppression: noise_suppression_wire(preferences.noise_suppression_mode)
                    .into(),
                noise_suppression_strength: format!("{:?}", preferences.noise_suppression_strength)
                    .to_lowercase(),
                echo_cancellation: preferences.echo_cancellation,
                automatic_gain: preferences.automatic_gain,
                state: format!("{:?}", state.audio_state).to_lowercase(),
                mic_test_status: match state.mic_test_status {
                    MicTestStatus::Idle => "idle",
                    MicTestStatus::Recording => "recording",
                    MicTestStatus::PreviewReady => "preview_ready",
                    MicTestStatus::PlayingOriginal => "playing_original",
                    MicTestStatus::PlayingProcessed => "playing_processed",
                }
                .into(),
                device_error: state.device_error.clone().unwrap_or_default(),
                dsp_error: state.dsp_error.clone().unwrap_or_default(),
                active_input: state.active_input_device.clone().unwrap_or_default(),
                active_output: state.active_output_device.clone().unwrap_or_default(),
                input_sample_rate: state.active_input_sample_rate,
                output_sample_rate: state.active_output_sample_rate,
            },
            shortcuts: {
                let shortcuts = core.shortcut_bindings();
                ShortcutSnapshot {
                    toggle_window: shortcuts.toggle_window.clone(),
                    toggle_mic: shortcuts.toggle_mic.clone(),
                    toggle_speaker: shortcuts.toggle_speaker.clone(),
                    push_to_talk: shortcuts.push_to_talk.clone(),
                    toggle_members: shortcuts.toggle_members.clone(),
                }
            },
        }
    }
}

fn parse_id(value: &str, name: &str) -> Result<u64, String> {
    value.parse().map_err(|_| format!("invalid {name}"))
}

fn input_mode_wire(mode: InputMode) -> &'static str {
    match mode {
        InputMode::VoiceActivity => "voice_activity",
        InputMode::PushToTalk => "push_to_talk",
    }
}

fn noise_suppression_wire(mode: NoiseSuppressionMode) -> &'static str {
    match mode {
        NoiseSuppressionMode::Off => "off",
        NoiseSuppressionMode::Standard => "standard",
        NoiseSuppressionMode::RnNoise => "rnnoise",
        NoiseSuppressionMode::DeepFilterNet => "deepfilternet",
        NoiseSuppressionMode::DeepFilterNetStrong => "deepfilternet_strong",
    }
}

fn map_command(core: &Application, command: CoreCommand) -> Result<AppCommand, String> {
    Ok(match command {
        CoreCommand::Connect {
            server_password,
            address,
            nickname,
            remember,
            auto_connect,
        } => AppCommand::Connect {
            server_password: server_password.into(),
            address,
            nickname,
            remember,
            auto_connect,
        },
        CoreCommand::Disconnect => AppCommand::Disconnect,
        CoreCommand::RetryAudio => AppCommand::RetryAudio,
        CoreCommand::JoinChannel { channel_id } => {
            AppCommand::JoinChannel(parse_id(&channel_id, "channel id")?)
        }
        CoreCommand::JoinProtectedChannel{channel_id,password} => AppCommand::JoinProtectedChannel{channel_id:parse_id(&channel_id,"channel id")?,password},
        CoreCommand::SetChannelPassword{channel_id,password} => AppCommand::SetChannelPassword{channel_id:parse_id(&channel_id,"channel id")?,password},
        CoreCommand::LeaveChannel => AppCommand::LeaveChannel,
        CoreCommand::SetMuted { muted } => AppCommand::SetMuted(muted),
        CoreCommand::SetDeafened { deafened } => AppCommand::SetDeafened(deafened),
        CoreCommand::SetPushToTalkPressed { pressed } => AppCommand::SetPushToTalkPressed(pressed),
        CoreCommand::SendTextMessage { text } => AppCommand::SendTextMessage(text),
        CoreCommand::QueryChat { .. } | CoreCommand::SaveMpvPath { .. } | CoreCommand::WatchRoom { .. } | CoreCommand::SetMpvPath { .. } | CoreCommand::WatchOpen { .. } | CoreCommand::WatchReopen { .. } | CoreCommand::WatchControl { .. } | CoreCommand::WatchLeave => return Err("watch commands are handled by the core worker".into()),
        CoreCommand::RequestRoomArchive { channel_id, all } => AppCommand::RequestRoomArchive {
            channel_id: parse_id(&channel_id, "channel id")?,
            all,
        },
        CoreCommand::CloseRoomArchive => AppCommand::CloseRoomArchive,
        CoreCommand::SetUserVolume { session_id, volume } => AppCommand::SetUserVolume {
            session_id: parse_id(&session_id, "session id")?,
            volume,
        },
        CoreCommand::SetUserLocalMute { session_id, muted } => AppCommand::SetUserLocalMute {
            session_id: parse_id(&session_id, "session id")?,
            muted,
        },
        CoreCommand::AdminMute {
            session_id,
            enabled,
        } => AppCommand::AdminMute {
            session_id: parse_id(&session_id, "session id")?,
            enabled,
        },
        CoreCommand::KickChannel { session_id } => {
            AppCommand::KickChannel(parse_id(&session_id, "session id")?)
        }
        CoreCommand::KickServer { session_id } => {
            AppCommand::KickServer(parse_id(&session_id, "session id")?)
        }
        CoreCommand::BanUser { session_id, reason } => AppCommand::BanUser {
            session_id: parse_id(&session_id, "session id")?,
            reason,
        },
        CoreCommand::MoveUser {
            session_id,
            channel_id,
        } => AppCommand::MoveUser {
            session_id: parse_id(&session_id, "session id")?,
            channel_id: parse_id(&channel_id, "channel id")?,
        },
        CoreCommand::ListBans { offset, limit } => AppCommand::ListBans { offset, limit },
        CoreCommand::Unban { client_id } => AppCommand::Unban(client_id),
        CoreCommand::SetChannelAdmin {
            session_id,
            enabled,
        } => AppCommand::SetChannelAdmin {
            session_id: parse_id(&session_id, "session id")?,
            enabled,
        },
        CoreCommand::SetServerAdmin {
            session_id,
            enabled,
        } => AppCommand::SetServerAdmin {
            session_id: parse_id(&session_id, "session id")?,
            enabled,
        },
        CoreCommand::SetChannelNetworkPolicy { channel_id, policy } => {
            AppCommand::SetChannelNetworkPolicy {
                channel_id: parse_id(&channel_id, "channel id")?,
                policy: match policy.as_str() {
                    "auto" => RoomNetworkPolicy::Auto,
                    "force_p2p" => RoomNetworkPolicy::ForceP2p,
                    "force_sfu" => RoomNetworkPolicy::ForceSfu,
                    _ => return Err("invalid room network policy".into()),
                },
            }
        }
        CoreCommand::CreateChannel { channel, password } => if let Some(password)=password {AppCommand::CreateProtectedChannel{channel:channel.try_into()?,password}} else {AppCommand::CreateChannel(channel.try_into()?)},
        CoreCommand::EditChannel { channel } => AppCommand::EditChannel(channel.try_into()?),
        CoreCommand::DeleteChannel { channel_id } => {
            AppCommand::DeleteChannel(parse_id(&channel_id, "channel id")?)
        }
        CoreCommand::SetShareGameActivity { enabled } => AppCommand::SetShareGameActivity(enabled),
        CoreCommand::SetAvatar { avatar } => AppCommand::SetAvatar(avatar),
        CoreCommand::SetDisplayName { display_name } => AppCommand::SetDisplayName(display_name),
        CoreCommand::UpdateSavedServer { .. } => {
            return Err("saved server updates are handled by the core worker".into());
        }
        CoreCommand::AddSavedServer { .. } | CoreCommand::DeleteSavedServer { .. } => {
            return Err("saved server changes are handled by the core worker".into());
        }
        CoreCommand::SetTheme { theme } => AppCommand::SetTheme(match theme.as_str() {
            "system" => ThemeMode::System,
            "light" => ThemeMode::Light,
            "dark" => ThemeMode::Dark,
            _ => return Err("invalid theme".into()),
        }),
        CoreCommand::SetLanguage { language } => AppCommand::SetLanguage(match language.as_str() {
            "chinese" => Language::Chinese,
            "english" => Language::English,
            _ => return Err("invalid language".into()),
        }),
        CoreCommand::SetMiniOverlay { enabled, mode } => {
            AppCommand::SetMiniOverlay { enabled, mode }
        }
        CoreCommand::SetMiniOverlayPosition { x, y } => AppCommand::SetMiniOverlayPosition { x, y },
        CoreCommand::SetAutoConnectLastServer { enabled } => {
            AppCommand::SetAutoConnectLastServer(enabled)
        }
        CoreCommand::SetAutoConnect {
            enabled,
            server_address,
        } => AppCommand::SetAutoConnect {
            enabled,
            server_address,
        },
        CoreCommand::UpdateShortcuts { shortcuts } => {
            AppCommand::UpdateShortcuts(ShortcutBindings {
                toggle_window: shortcuts.toggle_window,
                toggle_mic: shortcuts.toggle_mic,
                toggle_speaker: shortcuts.toggle_speaker,
                push_to_talk: shortcuts.push_to_talk,
                toggle_members: shortcuts.toggle_members,
            })
        }
        CoreCommand::RoomSocial { .. } => return Err("room interaction is handled by its coordinator".into()),
        CoreCommand::ScreenShare { server_address, self_session_id, request } => {
            if core.server_address() != server_address || core.state().self_session.map(|v| v.to_string()).unwrap_or_default() != self_session_id {
                return Err("连接已改变，请重新打开屏幕共享".into());
            }
            AppCommand::ScreenShare(request.into_core()?)
        }
        CoreCommand::UpdateAudioPreferences {
            input_device,
            output_device,
            input_gain,
            output_volume,
            input_mode,
            noise_suppression,
            noise_suppression_strength,
            echo_cancellation,
            automatic_gain,
        } => AppCommand::UpdateAudioPreferences(AudioPreferences {
            input_device: (!input_device.is_empty()).then_some(input_device),
            output_device: (!output_device.is_empty()).then_some(output_device),
            input_gain,
            output_volume,
            input_mode: match input_mode.as_str() {
                "voiceactivity" | "voice_activity" => InputMode::VoiceActivity,
                "pushtotalk" | "push_to_talk" => InputMode::PushToTalk,
                _ => return Err("invalid input mode".into()),
            },
            echo_cancellation,
            automatic_gain,
            noise_suppression_mode: match noise_suppression.as_str() {
                "off" => NoiseSuppressionMode::Off,
                "standard" => NoiseSuppressionMode::Standard,
                "rnnoise" => NoiseSuppressionMode::RnNoise,
                "deepfilternet"
                | "deep_filter_net"
                | "deepfilternet_natural"
                | "deep_filter_net_natural" => NoiseSuppressionMode::DeepFilterNet,
                "deepfilternet_strong" | "deep_filter_net_strong" => {
                    NoiseSuppressionMode::DeepFilterNetStrong
                }
                _ => return Err("invalid noise suppression mode".into()),
            },
            noise_suppression_strength: match noise_suppression_strength.as_str() {
                "low" => NoiseSuppressionStrength::Low,
                "medium" => NoiseSuppressionStrength::Medium,
                "high" => NoiseSuppressionStrength::High,
                _ => return Err("invalid noise suppression strength".into()),
            },
        }),
        CoreCommand::ProvisionMaster { token } => AppCommand::ProvisionMaster(token),
        CoreCommand::TrustServer {
            endpoint,
            fingerprint,
        } => AppCommand::TrustServer {
            endpoint,
            fingerprint,
        },
        CoreCommand::RefreshDevices => AppCommand::RefreshDevices,
        CoreCommand::PlayOutputTest => AppCommand::PlayOutputTest,
        CoreCommand::StartMicTest => AppCommand::StartMicTest,
        CoreCommand::PlayMicTestOriginal => AppCommand::PlayMicTestOriginal,
        CoreCommand::PlayMicTestProcessed => AppCommand::PlayMicTestProcessed,
        CoreCommand::StopMicTest => AppCommand::StopMicTest,
        CoreCommand::Shutdown => AppCommand::Shutdown,
    })
}

#[tauri::command]
async fn query_chat(request: chat::Request, state: State<'_,BridgeState>) -> Result<chat::Page,String> {
    let commands=state.commands.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (reply,receiver)=mpsc::sync_channel(1);
        commands.try_send(CoreCommand::QueryChat{request,reply}).map_err(|_|"历史查询繁忙".to_string())?;
        receiver.recv_timeout(Duration::from_secs(12)).map_err(|_|"历史查询超时".to_string())?
    }).await.map_err(|e|e.to_string())?
}

#[tauri::command]
fn get_snapshot(state: State<'_, BridgeState>) -> Result<CoreSnapshot, String> {
    state
        .snapshot
        .read()
        .map(|snapshot| snapshot.clone())
        .map_err(|_| "core state lock is poisoned".into())
}

#[tauri::command]
fn get_watch_snapshot(state: State<'_, BridgeState>) -> Result<watch::View,String> {
    state.watch.read().map(|value|value.clone()).map_err(|e|e.to_string())
}

#[tauri::command]
async fn select_mpv_path() -> Result<Option<String>,String> {
    tauri::async_runtime::spawn_blocking(|| rfd::FileDialog::new().set_title("选择 mpv 可执行文件 / Select mpv executable").pick_file().map(|p|p.to_string_lossy().into_owned()))
        .await.map_err(|e|e.to_string())
}

#[tauri::command]
async fn validate_mpv_path(path:String) -> Result<String,String> {
    tauri::async_runtime::spawn_blocking(move || mpv::executable_path(&path).map(|p|p.to_string_lossy().into_owned()))
        .await.map_err(|e|e.to_string())?
}

#[tauri::command]
async fn save_mpv_path(path:String, state:State<'_,BridgeState>) -> Result<String,String> {
    let commands=state.commands.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let path=mpv::executable_path(&path)?.to_string_lossy().into_owned();
        let (reply,rx)=mpsc::sync_channel(1);
        commands.try_send(CoreCommand::SaveMpvPath{path,reply}).map_err(|e|e.to_string())?;
        rx.recv_timeout(Duration::from_secs(5)).map_err(|_|"保存播放器设置超时，请重试".to_string())?
    }).await.map_err(|e|e.to_string())?
}

#[tauri::command]
async fn select_watch_file() -> Result<Option<String>,String> {
    tauri::async_runtime::spawn_blocking(|| rfd::FileDialog::new().set_title("选择本地视频 / Select local video").pick_file().map(|p|p.to_string_lossy().into_owned()))
        .await.map_err(|e|e.to_string())
}

#[tauri::command]
fn voice_incident(action: String, reason: Option<String>, state: State<'_, BridgeState>) -> Result<incident::Status,String> {
    let mut recorder=state.incidents.lock().map_err(|_|"诊断记录暂不可用")?;
    let now=incident::now_ms();
    match action.as_str(){
        "mark"=>recorder.mark(reason.as_deref().unwrap_or("other"),now)?,
        "clear"=>recorder.clear(),
        "status"=>{},
        _=>return Err("未知诊断操作".into())
    }
    Ok(recorder.status(now))
}

#[tauri::command]
fn export_diagnostics(report: String, app: tauri::AppHandle) -> Result<String, String> {
    let download_dir = app
        .path()
        .download_dir()
        .map_err(|error| error.to_string())?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis();
    write_diagnostic_report(&download_dir, &report, timestamp)
        .map(|path| path.display().to_string())
}

fn write_diagnostic_report(
    directory: &Path,
    report: &str,
    timestamp: u128,
) -> Result<PathBuf, String> {
    if report.len() > 512 * 1024 {
        return Err("诊断报告超过 512 KiB 限制".into());
    }
    let value: serde_json::Value =
        serde_json::from_str(report).map_err(|error| format!("诊断报告不是有效 JSON：{error}"))?;
    if value.get("schema").and_then(serde_json::Value::as_str)
        != Some("edenvoice.voice-diagnostics.v1")
    {
        return Err("诊断报告 schema 无效".into());
    }
    if contains_sensitive_diagnostic_key(&value) {
        return Err("诊断报告包含禁止导出的敏感字段".into());
    }
    fs::create_dir_all(directory).map_err(|error| format!("创建导出目录失败：{error}"))?;
    let path = directory.join(format!("edenvoice-diagnostics-{timestamp}.json"));
    let bytes = serde_json::to_vec_pretty(&value).map_err(|error| error.to_string())?;
    fs::write(&path, bytes).map_err(|error| format!("写入诊断报告失败：{error}"))?;
    Ok(path)
}

fn contains_sensitive_diagnostic_key(value: &serde_json::Value) -> bool {
    const FORBIDDEN: &[&str] = &[
        "nickname",
        "messages",
        "serverAddress",
        "clientId",
        "activeInput",
        "activeOutput",
        "serverName",
        "trustFingerprint",
    ];
    match value {
        serde_json::Value::Object(values) => values.iter().any(|(key, value)| {
            FORBIDDEN.contains(&key.as_str()) || contains_sensitive_diagnostic_key(value)
        }),
        serde_json::Value::Array(values) => values.iter().any(contains_sensitive_diagnostic_key),
        _ => false,
    }
}

#[tauri::command]
fn dispatch_core(command: CoreCommand, state: State<'_, BridgeState>) -> Result<(), String> {
    state
        .commands
        .try_send(command)
        .map_err(|error| format!("core command queue is unavailable: {error}"))
}

#[tauri::command]
async fn request_room_social(context: room_social::Context, request: room_social::UiRequest, state: State<'_ , BridgeState>) -> Result<serde_json::Value, String> {
    let commands = state.commands.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (reply, receiver) = mpsc::sync_channel(1);
        commands.try_send(CoreCommand::RoomSocial {context, request, reply}).map_err(|_| "房间互动请求繁忙，请稍后重试".to_string())?;
        receiver.recv_timeout(Duration::from_secs(17)).map_err(|_| "房间互动请求超时，请重试".to_string())?
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
fn get_avatar_data(reference: String) -> Option<String> {
    edenvoice_client::avatar::data_for_reference(&reference).ok()
}

#[tauri::command]
async fn export_archive_card(image: String) -> Result<bool, String> {
    use base64::Engine;
    if image.len() > 8 * 1024 * 1024 { return Err("图片过大".into()); }
    let bytes = base64::engine::general_purpose::STANDARD.decode(image).map_err(|_| "图片格式无效")?;
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") { return Err("仅支持 PNG 图片".into()); }
    let Some(path) = rfd::AsyncFileDialog::new().add_filter("PNG 图片", &["png"]).set_file_name("Eden-Archive.png").save_file().await else { return Ok(false); };
    path.write(&bytes).await.map_err(|e| format!("保存失败：{e}"))?;
    Ok(true)
}

#[tauri::command]
fn export_user_data(password: String) -> Result<Option<String>, String> {
    let password = Zeroizing::new(password);
    let Some(path) = rfd::FileDialog::new()
        .add_filter("EdenVoice 加密备份", &["edenvoice-backup"])
        .set_file_name("EdenVoice-backup.edenvoice-backup")
        .save_file()
    else {
        return Ok(None);
    };
    let bytes = edenvoice_client::backup::export_encrypted(&password)
        .map_err(|error| format!("导出失败：{error}"))?;
    fs::write(&path, bytes).map_err(|error| format!("写入备份文件失败：{error}"))?;
    Ok(Some(path.display().to_string()))
}

#[tauri::command]
fn import_user_data(password: String) -> Result<Option<String>, String> {
    let password = Zeroizing::new(password);
    let Some(path) = rfd::FileDialog::new()
        .add_filter("EdenVoice 加密备份", &["edenvoice-backup"])
        .pick_file()
    else {
        return Ok(None);
    };
    let metadata = fs::metadata(&path).map_err(|error| format!("无法读取备份文件：{error}"))?;
    if metadata.len() > 4 * 1024 * 1024 {
        return Err("备份文件超过 4 MiB 限制".into());
    }
    let bytes = fs::read(&path).map_err(|error| format!("无法读取备份文件：{error}"))?;
    let imported = edenvoice_client::backup::import_encrypted(&bytes, &password)
        .map_err(|error| format!("导入失败：{error}"))?;
    Ok(Some(imported.client_id))
}

#[tauri::command]
async fn restart_client(app: tauri::AppHandle, state: State<'_,BridgeState>) -> Result<(),String> {
    let state=state.inner().clone();
    let _=tauri::async_runtime::spawn_blocking(move || shutdown_and_wait(&state)).await;
    EXITING.store(true,Ordering::Release);
    app.restart();
}

fn shutdown_and_wait(state: &BridgeState) {
    mpv::shutdown_all();
    let started=Instant::now();
    while state.commands.try_send(CoreCommand::Shutdown).is_err() && !state.stopped.load(Ordering::Acquire) {
        if started.elapsed()>Duration::from_secs(2) { break; }
        thread::sleep(Duration::from_millis(20));
    }
    while !state.stopped.load(Ordering::Acquire) || mpv::workers_running() {
        if started.elapsed()>Duration::from_secs(5) { break; }
        thread::sleep(Duration::from_millis(20));
    }
}

const MINI_OVERLAY_LABEL: &str = "mini-overlay";

fn joined_channel_id(snapshot: &CoreSnapshot) -> Option<&str> {
    let channel_id = snapshot
        .users
        .iter()
        .find(|user| user.session_id == snapshot.self_session_id)?
        .channel_id
        .as_str();
    snapshot
        .channels
        .iter()
        .any(|channel| channel.id == channel_id)
        .then_some(channel_id)
}

fn mini_overlay_size(snapshot: &CoreSnapshot) -> LogicalSize<f64> {
    let member_count = joined_channel_id(snapshot)
        .map(|channel_id| {
            snapshot
                .users
                .iter()
                .filter(|user| user.channel_id == channel_id)
                .count()
        })
        .unwrap_or_default();
    if snapshot.mini_overlay_mode == "list" {
        LogicalSize::new(190.0, 12.0 + member_count.clamp(1, 8) as f64 * 34.0)
    } else if member_count == 0 {
        LogicalSize::new(174.0, 50.0)
    } else {
        let visible_count = member_count.clamp(1, 6) as f64;
        LogicalSize::new(13.0 + visible_count * 35.0, 50.0)
    }
}

fn mini_overlay_target(snapshot: &CoreSnapshot) -> Option<(u32, u32)> {
    let should_show = snapshot.mini_overlay_enabled
        && matches!(snapshot.connection.as_str(), "connected" | "reconnecting")
        && joined_channel_id(snapshot).is_some();
    should_show.then(|| {
        let size = mini_overlay_size(snapshot);
        (size.width as u32, size.height as u32)
    })
}

fn clamp_window_position(
    position: (i32, i32),
    window_size: (u32, u32),
    displays: &[(i32, i32, u32, u32)],
) -> (i32, i32) {
    let window_left = i64::from(position.0);
    let window_top = i64::from(position.1);
    let window_right = window_left + i64::from(window_size.0);
    let window_bottom = window_top + i64::from(window_size.1);
    let minimum_visible_width = i64::from(window_size.0.min(32));
    let minimum_visible_height = i64::from(window_size.1.min(32));
    let is_reachable = displays.iter().any(|&(x, y, width, height)| {
        let display_left = i64::from(x);
        let display_top = i64::from(y);
        let display_right = display_left + i64::from(width);
        let display_bottom = display_top + i64::from(height);
        let visible_width = window_right.min(display_right) - window_left.max(display_left);
        let visible_height = window_bottom.min(display_bottom) - window_top.max(display_top);
        visible_width >= minimum_visible_width && visible_height >= minimum_visible_height
    });
    if is_reachable {
        return position;
    }

    let Some(&(display_x, display_y, display_width, display_height)) =
        displays.iter().min_by_key(|&&(x, y, width, height)| {
            let left = i64::from(x);
            let top = i64::from(y);
            let right = left + i64::from(width);
            let bottom = top + i64::from(height);
            let position_x = i64::from(position.0);
            let position_y = i64::from(position.1);
            let dx = if position_x < left {
                left - position_x
            } else if position_x >= right {
                position_x - right + 1
            } else {
                0
            };
            let dy = if position_y < top {
                top - position_y
            } else if position_y >= bottom {
                position_y - bottom + 1
            } else {
                0
            };
            dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy))
        })
    else {
        return position;
    };

    let max_x = i64::from(display_x) + (i64::from(display_width) - i64::from(window_size.0)).max(0);
    let max_y =
        i64::from(display_y) + (i64::from(display_height) - i64::from(window_size.1)).max(0);
    (
        i64::from(position.0).clamp(i64::from(display_x), max_x) as i32,
        i64::from(position.1).clamp(i64::from(display_y), max_y) as i32,
    )
}

fn sync_mini_overlay(
    app: &tauri::AppHandle,
    snapshot: &CoreSnapshot,
    commands: &mpsc::SyncSender<CoreCommand>,
) {
    let Some((width, height)) = mini_overlay_target(snapshot) else {
        if let Some(window) = app.get_webview_window(MINI_OVERLAY_LABEL) {
            let _ = window.close();
        }
        return;
    };

    let size = LogicalSize::new(f64::from(width), f64::from(height));
    if let Some(window) = app.get_webview_window(MINI_OVERLAY_LABEL) {
        // Speaking-state snapshots arrive frequently. Re-applying the native size on
        // every snapshot interrupts an in-progress window drag, which is especially
        // noticeable in the taller list layout.
        let needs_resize = window
            .scale_factor()
            .and_then(|scale| {
                window.inner_size().map(|current| {
                    let target = size.to_physical::<u32>(scale);
                    current != target
                })
            })
            .unwrap_or(true);
        if needs_resize {
            let _ = window.set_size(Size::Logical(size));
        }
        return;
    }

    let url = WebviewUrl::App("index.html?mini=1".into());
    match WebviewWindowBuilder::new(app, MINI_OVERLAY_LABEL, url)
        .title("EdenVoice Mini")
        .inner_size(size.width, size.height)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .focused(false)
        .focusable(false)
        .visible(false)
        .build()
    {
        Ok(window) => {
            let _ = window.set_zoom(1.0);
            if let (Some(x), Some(y)) = (
                snapshot.mini_overlay_position_x,
                snapshot.mini_overlay_position_y,
            ) {
                let displays = window
                    .available_monitors()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|monitor| {
                        let position = monitor.position();
                        let size = monitor.size();
                        (position.x, position.y, size.width, size.height)
                    })
                    .collect::<Vec<_>>();
                let outer_size = window.outer_size().unwrap_or_default();
                let restored =
                    clamp_window_position((x, y), (outer_size.width, outer_size.height), &displays);
                let _ = window.set_position(Position::Physical(PhysicalPosition::new(
                    restored.0, restored.1,
                )));
                if restored != (x, y) {
                    let _ = commands.try_send(CoreCommand::SetMiniOverlayPosition {
                        x: restored.0,
                        y: restored.1,
                    });
                }
            }
            let position_commands = commands.clone();
            window.on_window_event(move |event| {
                if let WindowEvent::Moved(position) = event {
                    let _ = position_commands.try_send(CoreCommand::SetMiniOverlayPosition {
                        x: position.x,
                        y: position.y,
                    });
                }
            });
            let _ = window.show();
        }
        Err(error) => eprintln!("failed to open mini overlay: {error}"),
    }
}

#[tauri::command]
fn resize_member_panel(open: bool, window: WebviewWindow) -> Result<bool, String> {
    let err = |e: tauri::Error| e.to_string();
    if window.is_maximized().map_err(err)? || window.is_fullscreen().map_err(err)? { return Ok(false); }
    let scale = window_scale::pixels_per_unit(&window).map_err(err)?;
    let size = window.inner_size().map_err(err)?.to_logical::<f64>(scale);
    let width = size.width + if open { 208.0 } else { -208.0 };
    if width < 600.0 { return Ok(false); }
    if open {
        let monitor = window.current_monitor().map_err(err)?.ok_or("No monitor")?;
        let position = window.outer_position().map_err(err)?;
        let outer = window.outer_size().map_err(err)?;
        let right = f64::from(position.x) + f64::from(outer.width) + 208.0 * scale;
        if right > f64::from(monitor.work_area().position.x) + f64::from(monitor.work_area().size.width) { return Ok(false); }
    }
    window.set_size(window_scale::size(&window, width, size.height).map_err(err)?).map_err(err)?;
    Ok(true)
}

#[tauri::command]
fn window_action(action: String, window: WebviewWindow) -> Result<(), String> {
    let _ = window_scale::apply_zoom(&window);
    match action.as_str() {
        "minimize" => window.minimize(),
        "maximize" if window.is_maximized().unwrap_or(false) => window.unmaximize(),
        "maximize" => window.maximize(),
        "compact" => {
            window.unmaximize().map_err(|error| error.to_string())?;
            window_scale::set_mode(&window, true)
        }
        "restore" => window_scale::set_mode(&window, false),
        "close" => window.close(),
        _ => return Err("unknown window action".into()),
    }
    .map_err(|error| error.to_string())
}

// Send only changed top-level fields: fast level updates never resend chat text.
fn snapshot_patch(previous: &CoreSnapshot, next: &CoreSnapshot) -> serde_json::Map<String, serde_json::Value> {
    let mut changes = serde_json::Map::new();
    macro_rules! field {
        ($key:literal, $field:ident) => {
            if previous.$field != next.$field {
                changes.insert($key.into(), serde_json::to_value(&next.$field).expect("serializable snapshot field"));
            }
        };
    }
    field!("revision", revision);
    field!("chatQuerySupported", chat_query_supported);
    field!("chatHeads", chat_heads);
    field!("connection", connection);
    field!("connectionDetail", connection_detail);
    field!("error", error);
    field!("trustEndpoint", trust_endpoint);
    field!("trustFingerprint", trust_fingerprint);
    field!("serverName", server_name);
    field!("serverAddress", server_address);
    field!("nickname", nickname);
    field!("avatar", avatar);
    field!("clientId", client_id);
    field!("selfSessionId", self_session_id);
    field!("serverRole", server_role);
    field!("theme", theme);
    field!("language", language);
    field!("shareGameActivity", share_game_activity);
    field!("gameDetectionAvailable", game_detection_available);
    field!("gameActivitySupported", game_activity_supported);
    field!("passwordRoomsSupported", password_rooms_supported);
    field!("lockedChannelIds", locked_channel_ids);
    field!("roomScenes", room_scenes);
    field!("miniOverlayEnabled", mini_overlay_enabled);
    field!("miniOverlayMode", mini_overlay_mode);
    field!("autoConnectLastServer", auto_connect_last_server);
    field!("autoConnectServer", auto_connect_server);
    field!("autoConnecting", auto_connecting);
    field!("savedServers", saved_servers);
    field!("capabilities", capabilities);
    field!("networkPolicyChannelIds", network_policy_channel_ids);
    field!("bans", bans);
    field!("banListOffset", ban_list_offset);
    field!("banListTotal", ban_list_total);
    field!("channels", channels);
    field!("users", users);
    field!("avatarCacheRevision", avatar_cache_revision);
    field!("messages", messages);
    field!("archiveSupported", archive_supported);
    field!("selfAway", self_away);
    field!("mutedSpeechNotice", muted_speech_notice);
    field!("roomSocialEpoch", room_social_epoch);
    field!("roomSocialSupported", room_social_supported);
    field!("roomImagesSupported", room_images_supported);
    field!("screenShareSupported", screen_share_supported);
    field!("watchSupported", watch_supported);
    field!("muted", muted);
    field!("deafened", deafened);
    field!("inputLevel", input_level);
    field!("outputLevel", output_level);
    field!("latencyMs", latency_ms);
    field!("controlLatencyMs", control_latency_ms);
    field!("transport", transport);
    field!("networkQuality", network_quality);
    field!("diagnostics", diagnostics);
    field!("audio", audio);
    field!("shortcuts", shortcuts);
    changes
}

fn start_core(app_handle: tauri::AppHandle) -> BridgeState {
    let (commands, receiver) = mpsc::sync_channel(128);
    let worker_commands = commands.clone();
    let snapshot = Arc::new(RwLock::new(CoreSnapshot::default()));
    let worker_snapshot = Arc::clone(&snapshot);
    let incidents = Arc::new(std::sync::Mutex::new(incident::Recorder::default()));
    let worker_incidents=Arc::clone(&incidents);
    let watch = Arc::new(RwLock::new(watch::View::default()));
    let worker_watch = Arc::clone(&watch);
    let stopped=Arc::new(AtomicBool::new(false));
    let worker_stopped=Arc::clone(&stopped);
    thread::Builder::new()
        .name("edenvoice-core".into())
        .spawn(move || {
            let mut core = Application::new_with_auto_connect(!invitations::has_pending(&app_handle));
            let mut watching = watch::Coordinator::default();
            let mut chat = chat::Coordinator::default();
            let mut social = room_social::Coordinator::default();
            let mut game_presence = game_presence::Tracker::new();
            let mut previous_watch = watch::View::default();
            let mut revision = 0_u64;
            let mut previous = CoreSnapshot::default();
            let mut last_ui_publish = Instant::now() - Duration::from_secs(1);
            let mut previous_mini_overlay_target = None;
            let mut pending_mini_overlay_position = None;
            loop {
                let mut shutdown = false;
                let mut commanded = false;
                while let Ok(command) = receiver.try_recv() {
                    commanded = true;
                    shutdown |= matches!(&command, CoreCommand::Shutdown);
                    match command {
                        CoreCommand::QueryChat {request,reply} => chat.request(&core,request,reply),
                        CoreCommand::RoomSocial {context,request,reply} => social.request(&mut core,context,request,reply),
                        CoreCommand::SetMpvPath { path } => { if let Err(e)=core.set_mpv_path(path) { watching.view.error=e; } },
                        CoreCommand::SaveMpvPath { path, reply } => { let result=core.set_mpv_path(path.clone()).map(|_|path); let _=reply.try_send(result); },
                        CoreCommand::WatchRoom { action, channel_id, session_id, server_address, target_session_id } => {
                            if core.server_address()==server_address
                                && let (Ok(channel),Ok(session))=(channel_id.parse(),session_id.parse()) { watching.room(&mut core,&action,channel,session,target_session_id.and_then(|id| id.parse().ok())); }
                        },
                        CoreCommand::WatchOpen { file, channel_id, session_id, server_address } => {
                            if core.server_address() == server_address {
                                if let (Ok(channel),Ok(session)) = (channel_id.parse(),session_id.parse()) {
                                    watching.open(&mut core,file,channel,session);
                                }
                            } else { watching.view.error="服务器已改变，请重新打开观影 / Server changed".into(); }
                        }
                        CoreCommand::WatchReopen { channel_id, session_id, server_address } => {
                            if core.server_address() == server_address {
                                if let (Ok(channel),Ok(session)) = (channel_id.parse(),session_id.parse()) {
                                    watching.reopen(&mut core,channel,session);
                                }
                            } else { watching.view.error="服务器已改变，请重新打开观影 / Server changed".into(); }
                        }
                        CoreCommand::WatchControl { action,position } => watching.control(&action,position),
                        CoreCommand::WatchLeave => watching.leave(&mut core),
                        CoreCommand::UpdateSavedServer { index, profile } => {
                            if !core.update_saved_server(index, profile.into()) {
                                eprintln!("ignored invalid saved server update");
                            }
                        }
                        CoreCommand::AddSavedServer { profile } => {
                            if !core.add_saved_server(profile.into()) {
                                eprintln!("ignored invalid saved server addition");
                            }
                        }
                        CoreCommand::DeleteSavedServer { index } => {
                            if !core.delete_saved_server(index) {
                                eprintln!("ignored invalid saved server deletion");
                            }
                        }
                        CoreCommand::SetMiniOverlayPosition { x, y } => {
                            pending_mini_overlay_position = Some((x, y, Instant::now()));
                        }
                        command => match map_command(&core, command) {
                            Ok(command) => core.dispatch(command),
                            Err(error) => eprintln!("ignored invalid UI command: {error}"),
                        },
                    }
                }
                let position_is_ready =
                    pending_mini_overlay_position
                        .as_ref()
                        .is_some_and(|(_, _, changed_at)| {
                            shutdown || changed_at.elapsed() >= Duration::from_millis(250)
                        });
                if position_is_ready {
                    let (x, y, _) = pending_mini_overlay_position.take().unwrap();
                    core.dispatch(AppCommand::SetMiniOverlayPosition { x, y });
                }
                core.update();
                chat.tick(&mut core);
                for event in social.tick(&mut core) {
                    let _ = app_handle.emit("room-social", event);
                }
                for message in core.take_screen_share_messages() {
                    let _ = app_handle.emit("screen-share-state", screenshare::event(message, core.server_address(), core.state().self_session));
                }
                watching.tick(&mut core);
                game_presence.tick(&core);
                if watching.view != previous_watch {
                    if let Ok(mut target)=worker_watch.write() { *target=watching.view.clone(); }
                    let _=app_handle.emit("watch-state",&watching.view);
                    previous_watch=watching.view.clone();
                }
                if let Some(archive) = core.take_room_archive() {
                    let _ = app_handle.emit("room-archive", RoomArchiveSnapshot::from(&archive));
                }
                let visible = app_handle.get_webview_window("main").is_some_and(|w| w.is_visible().unwrap_or(true) && !w.is_minimized().unwrap_or(false));
                let interval = if visible || previous_mini_overlay_target.is_some() { Duration::from_millis(100) } else { Duration::from_millis(500) };
                if commanded || last_ui_publish.elapsed() >= interval {
                    last_ui_publish = Instant::now();
                    let next = CoreSnapshot::from_application(&core, revision);
                    if let Ok(mut recorder)=worker_incidents.try_lock(){recorder.sample(&next,incident::now_ms());}
                    if next != previous {
                        revision += 1;
                        let mut next = next;
                        next.revision = revision;
                        if let Ok(mut target) = worker_snapshot.write() {
                            *target = next.clone();
                        }
                        let mini_overlay_target = mini_overlay_target(&next);
                        let overlay_exists =
                            app_handle.get_webview_window(MINI_OVERLAY_LABEL).is_some();
                        let overlay_visibility_out_of_sync =
                            mini_overlay_target.is_some() != overlay_exists;
                        if mini_overlay_target != previous_mini_overlay_target
                            || overlay_visibility_out_of_sync
                        {
                            sync_mini_overlay(&app_handle, &next, &worker_commands);
                            previous_mini_overlay_target = mini_overlay_target;
                        }
                        let patch = snapshot_patch(&previous, &next);
                        let _ = app_handle.emit("core-state-patch", serde_json::json!({"baseRevision":previous.revision,"revision":revision,"changes":patch}));
                        previous = next;
                    }
                }
                if shutdown {
                    watching.leave(&mut core);
                    break;
                }
                thread::sleep(Duration::from_millis(33));
            }
            worker_stopped.store(true,Ordering::Release);
        })
        .expect("failed to start EdenVoice core worker");
    BridgeState { incidents, commands, snapshot, watch, stopped }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| { if let Some(w)=app.get_webview_window("main") {let _=w.show();let _=w.unminimize();let _=w.set_focus();} }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            invitations::setup(app.handle())?;
            app.manage(updates::UpdateState::default());
            updates::install_on_startup(app.handle());
            close_behavior::setup(app.handle());
            if let Err(error) = tray::setup(app.handle()) {
                eprintln!("system tray unavailable; using normal window controls: {error}");
            }
            if let Some(window) = app.get_webview_window("main") {
                window_scale::setup(&window)?;
                let restored_window = window.clone();
                window.on_window_event(move |event| {
                    match event {
                        WindowEvent::Focused(true) => { let _ = window_scale::apply_zoom(&restored_window); }
                        WindowEvent::Focused(false) => {
                            if let Some(state) = restored_window.try_state::<BridgeState>() {
                                let _ = state.commands.try_send(CoreCommand::SetPushToTalkPressed { pressed: false });
                            }
                        }
                        WindowEvent::CloseRequested { api, .. } if !EXITING.load(Ordering::Acquire) => {
                            api.prevent_close();
                            close_behavior::request(&restored_window);
                        }
                        _ => {}
                    }
                });
            }
            let state = start_core(app.handle().clone());
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            invitations::take_server_invitation,
            close_behavior::get_close_preferences,
            close_behavior::set_close_behavior,
            close_behavior::resolve_window_close,
            system_audio::start_screen_audio,
            system_audio::read_screen_audio,
            system_audio::stop_screen_audio,
            system_audio::set_screen_audio_paused,
            query_chat,
            get_snapshot,
            export_diagnostics,
            voice_incident,
            dispatch_core,
            get_watch_snapshot,
            select_watch_file,
            select_mpv_path,
            validate_mpv_path,
            save_mpv_path,
            get_avatar_data,
            request_room_social,
            export_user_data,
            export_archive_card,
            import_user_data,
            restart_client,
            updates::check_update,
            updates::pending_update,
            updates::cancel_pending_update,
            updates::download_update,
            updates::install_update,
            resize_member_panel,
            window_action
        ])
        .build(tauri::generate_context!())
        .expect("error while building EdenVoice desktop")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event
                && !EXITING.swap(true, Ordering::AcqRel)
            {
                api.prevent_exit();
                let app = app.clone();
                let state = app.state::<BridgeState>().inner().clone();
                thread::spawn(move || {
                    shutdown_and_wait(&state);
                    app.exit(code.unwrap_or(0));
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_delta_omits_unchanged_chat_and_members() {
        let previous=CoreSnapshot {revision:1,..CoreSnapshot::default()};
        let mut next=previous.clone();next.revision=2;next.input_level=0.5;
        let patch=snapshot_patch(&previous,&next);
        assert_eq!(patch.len(),2);assert!(!patch.contains_key("messages"));assert!(!patch.contains_key("users"));
    }

    #[test]
    fn room_social_epoch_is_published_even_if_the_final_channel_is_unchanged() {
        let previous = CoreSnapshot::default();
        let mut next = previous.clone();
        next.room_social_epoch = "2".into();
        let patch = snapshot_patch(&previous, &next);
        assert_eq!(patch.get("roomSocialEpoch"), Some(&serde_json::json!("2")));
        assert!(!patch.contains_key("messages"));
        assert!(!patch.contains_key("users"));
    }

    #[test]
    fn frequent_core_snapshots_never_serialize_archive_content() {
        let snapshot = serde_json::to_value(CoreSnapshot::default()).unwrap();
        assert!(snapshot.get("roomArchive").is_none());
        assert!(snapshot.get("archiveSupported").is_some());
        let archive = edenvoice_client::protocol::control::RoomArchive {
            stats: edenvoice_client::protocol::control::ArchiveRoomStats {
                room_id: 7,
                ..Default::default()
            },
            ..Default::default()
        };
        let event = serde_json::to_value(RoomArchiveSnapshot::from(&archive)).unwrap();
        assert_eq!(event["stats"]["roomId"], "7");
        assert!(event["events"].is_array());
    }

    #[test]
    fn diagnostic_export_rejects_sensitive_fields_and_writes_sanitized_json() {
        let directory =
            std::env::temp_dir().join(format!("edenvoice-diagnostic-test-{}", std::process::id()));
        let valid = r#"{"schema":"edenvoice.voice-diagnostics.v1","serverAddressHash":"abc","diagnostics":{"rttMs":42}}"#;
        let path = write_diagnostic_report(&directory, valid, 123)
            .expect("sanitized report should be written");
        let saved = fs::read_to_string(&path).expect("saved report should be readable");
        assert!(saved.contains("serverAddressHash"));
        let sensitive = r#"{"schema":"edenvoice.voice-diagnostics.v1","nickname":"secret"}"#;
        assert!(write_diagnostic_report(&directory, sensitive, 124).is_err());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn mini_overlay_requires_membership_in_a_real_channel() {
        let mut snapshot = CoreSnapshot {
            self_session_id: "self".into(),
            users: vec![UserSnapshot {
                session_id: "self".into(),
                channel_id: "42".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(joined_channel_id(&snapshot), None);

        snapshot.channels.push(ChannelSnapshot {
            id: "42".into(),
            ..Default::default()
        });
        assert_eq!(joined_channel_id(&snapshot), Some("42"));
    }

    #[test]
    fn mini_overlay_native_size_fits_avatar_and_list_modes() {
        let mut snapshot = CoreSnapshot {
            self_session_id: "self".into(),
            mini_overlay_mode: "avatars".into(),
            channels: vec![ChannelSnapshot {
                id: "42".into(),
                ..Default::default()
            }],
            users: ["self", "second", "third"]
                .into_iter()
                .map(|session_id| UserSnapshot {
                    session_id: session_id.into(),
                    channel_id: "42".into(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };

        assert_eq!(mini_overlay_size(&snapshot), LogicalSize::new(118.0, 50.0));
        snapshot.mini_overlay_mode = "list".into();
        assert_eq!(mini_overlay_size(&snapshot), LogicalSize::new(190.0, 114.0));
    }

    #[test]
    fn speaking_state_does_not_resynchronize_the_native_overlay_window() {
        let mut snapshot = CoreSnapshot {
            connection: "connected".into(),
            mini_overlay_enabled: true,
            self_session_id: "self".into(),
            channels: vec![ChannelSnapshot {
                id: "42".into(),
                ..Default::default()
            }],
            users: ["self", "speaker"]
                .into_iter()
                .map(|session_id| UserSnapshot {
                    session_id: session_id.into(),
                    channel_id: "42".into(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let target = mini_overlay_target(&snapshot);

        snapshot.users[1].speaking = true;

        assert_eq!(mini_overlay_target(&snapshot), target);
    }

    #[test]
    fn saved_mini_overlay_position_is_kept_on_its_display() {
        let displays = [(-1920, 0, 1920, 1080), (0, 0, 2560, 1440)];
        assert_eq!(
            clamp_window_position((-700, 240), (190, 114), &displays),
            (-700, 240)
        );
        assert_eq!(
            clamp_window_position((-40, 200), (190, 114), &displays),
            (-40, 200)
        );
    }

    #[test]
    fn saved_mini_overlay_position_is_clamped_after_display_changes() {
        let displays = [(0, 0, 1920, 1080)];
        assert_eq!(
            clamp_window_position((2400, 1200), (190, 114), &displays),
            (1730, 966)
        );
        assert_eq!(
            clamp_window_position((-400, -300), (190, 114), &displays),
            (0, 0)
        );
    }

    #[test]
    fn input_mode_snapshot_values_match_the_frontend_protocol() {
        assert_eq!(input_mode_wire(InputMode::VoiceActivity), "voice_activity");
        assert_eq!(input_mode_wire(InputMode::PushToTalk), "push_to_talk");
    }

    #[test]
    fn auto_connect_setting_defaults_on_and_accepts_the_react_command() {
        assert!(CoreSnapshot::default().auto_connect_last_server);
        assert!(CoreSnapshot::default().auto_connect_server.is_empty());
        let command: CoreCommand = serde_json::from_value(serde_json::json!({
            "type": "set_auto_connect_last_server",
            "enabled": false
        }))
        .unwrap();
        assert!(matches!(
            command,
            CoreCommand::SetAutoConnectLastServer { enabled: false }
        ));
        let selected: CoreCommand = serde_json::from_value(serde_json::json!({
            "type": "set_auto_connect",
            "enabled": true,
            "serverAddress": "voice.example:7001"
        }))
        .unwrap();
        assert!(matches!(
            selected,
            CoreCommand::SetAutoConnect {
                enabled: true,
                server_address
            } if server_address == "voice.example:7001"
        ));
    }

    #[test]
    fn react_audio_preferences_carry_noise_strength() {
        let command: CoreCommand = serde_json::from_str(
            r#"{"type":"update_audio_preferences","inputDevice":"","outputDevice":"","inputGain":1.0,"outputVolume":1.0,"inputMode":"voice_activity","noiseSuppression":"standard","noiseSuppressionStrength":"high","echoCancellation":true,"automaticGain":true}"#,
        )
        .expect("React audio preferences should deserialize");

        match command {
            CoreCommand::UpdateAudioPreferences {
                noise_suppression_strength,
                ..
            } => {
                assert_eq!(noise_suppression_strength, "high");
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn react_audio_preferences_accept_strong_deep_filter_mode() {
        let command: CoreCommand = serde_json::from_str(
            r#"{"type":"update_audio_preferences","inputDevice":"","outputDevice":"","inputGain":1.0,"outputVolume":1.0,"inputMode":"voice_activity","noiseSuppression":"deepfilternet_strong","noiseSuppressionStrength":"medium","echoCancellation":false,"automaticGain":false}"#,
        )
        .expect("strong DeepFilterNet preferences should deserialize");

        let mapped = map_command(&Application::new(), command)
            .expect("strong DeepFilterNet preferences should map");
        assert!(matches!(
            mapped,
            AppCommand::UpdateAudioPreferences(AudioPreferences {
                noise_suppression_mode: NoiseSuppressionMode::DeepFilterNetStrong,
                ..
            })
        ));
        assert_eq!(
            noise_suppression_wire(NoiseSuppressionMode::DeepFilterNetStrong),
            "deepfilternet_strong"
        );
    }
}
