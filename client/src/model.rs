use std::{collections::BTreeMap, time::Instant};

use crate::dsp::NoiseSuppressionMode;
use crate::protocol::control::{BanRecord, Channel, RoomArchive, ServerState, TextMessage, User};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Error(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AudioState {
    Stopped,
    Starting,
    Ready,
    Error(String),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MicTestStatus {
    #[default]
    Idle,
    Recording,
    PreviewReady,
    PlayingOriginal,
    PlayingProcessed,
}

pub struct AppState {
    pub chat_heads: BTreeMap<u64,String>,
    pub chat_query_supported: bool,
    pub chat_query_results: Vec<crate::protocol::control::ChatQueryResult>,
    pub connection: ConnectionState,
    pub server_name: String,
    pub channels: Vec<Channel>,
    pub users: BTreeMap<u64, User>,
    pub avatar_cache_revision: u64,
    pub chat_messages: Vec<TextMessage>,
    pub room_archive: Option<RoomArchive>,
    pub archive_requested_channel: Option<u64>,
    pub archive_supported: bool,
    pub self_away: bool,
    pub muted_speech_notice: Option<crate::audio::MutedSpeechNotice>,
    pub room_social_epoch: u64,
    pub room_social_supported: bool,
    pub room_images_supported: bool,
    pub room_social_events: Vec<crate::protocol::room_social::Event>,
    pub screen_share_supported: bool,
    pub screen_share_messages: Vec<crate::protocol::screenshare::Response>,
    pub watch_supported: bool,
    pub game_activity_supported: bool,
    pub game_catalog_required: bool,
    pub game_catalog_version: u64,
    pub game_catalog: Option<crate::game_catalog::Catalog>,
    pub announcement_scope: String,
    pub announcement_revision: u64,
    pub announcement: Option<serde_json::Value>,
    pub client_updates: Vec<serde_json::Value>,
    pub password_rooms_supported: bool,
    pub locked_channel_ids: Vec<u64>,
    pub room_scenes: std::collections::BTreeMap<u64, crate::protocol::control::RoomScene>,
    pub watch_messages: Vec<crate::protocol::watch::WatchState>,
    pub self_session: Option<u64>,
    pub server_role: String,
    pub capabilities: Vec<String>,
    pub network_policy_channel_ids: Vec<u64>,
    pub bans: Vec<BanRecord>,
    pub ban_list_offset: usize,
    pub ban_list_total: usize,
    pub trust_endpoint: String,
    pub trust_fingerprint: String,
    pub local_mute_intent: bool,
    pub muted: bool,
    pub admin_muted: bool,
    pub deafened: bool,
    pub audio_state: AudioState,
    pub input_level: f32,
    pub input_peak: f32,
    pub output_level: f32,
    pub talking: BTreeMap<u64, Instant>,
    pub rtt_ms: Option<u64>,
    pub control_rtt_ms: Option<u64>,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub packets_lost: u64,
    pub packets_reordered: u64,
    pub jitter_depth: usize,
    pub opus_bitrate_bps: i32,
    pub jitter_ms: f64,
    pub effective_audio_loss_percent: f64,
    pub jitter_buffer_delay_ms: f64,
    pub jitter_buffer_underruns: u64,
    pub plc_frames: u64,
    pub fec_recovered_packets: u64,
    pub audio_timing: crate::audio::AudioTiming,
    pub capture_overruns: u64,
    pub playback_underruns: u64,
    pub user_volumes: BTreeMap<u64, f32>,
    pub locally_muted_users: BTreeMap<u64, bool>,
    pub push_to_talk_pressed: bool,
    pub active_noise_suppression: NoiseSuppressionMode,
    pub active_echo_cancellation: bool,
    pub active_automatic_gain: bool,
    pub dsp_average_ms: f32,
    pub dsp_peak_ms: f32,
    pub dsp_deadline_misses: u64,
    pub dsp_error: Option<String>,
    pub dsp_loading: Option<NoiseSuppressionMode>,
    pub dsp_performance_warning: Option<String>,
    pub mic_test_status: MicTestStatus,
    pub active_input_device: Option<String>,
    pub active_output_device: Option<String>,
    pub active_input_sample_rate: Option<u32>,
    pub active_output_sample_rate: Option<u32>,
    pub device_fallback_active: bool,
    pub device_error: Option<String>,
    pub room_topology: String,
    pub current_transport: String,
    pub route_reason: String,
    pub qualification_duration_ms: u64,
    pub migration_cooldown_ms: u64,
    pub transport_mode: String,
    pub audio_quality_mode: String,
    pub network_quality: String,
    pub network_trend: String,
    pub actual_transport: String,
    pub selected_route: String,
    pub route_endpoint: String,
    pub p2p_qualification: String,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            connection: ConnectionState::Disconnected,
            server_name: String::new(),
            channels: vec![],
            users: BTreeMap::new(),
            avatar_cache_revision: 0,
            chat_messages: Vec::new(),
            chat_heads: Default::default(),
            chat_query_supported: false,
            chat_query_results: Vec::new(),
            room_archive: None,
            archive_requested_channel: None,
            archive_supported: false,
            self_away: false,
            muted_speech_notice: None,
            room_social_epoch: 0,
            room_social_supported: false,
            room_images_supported: false,
            room_social_events: Vec::new(),
            screen_share_supported: false,
            screen_share_messages: Vec::new(),
            watch_supported: false,
            game_activity_supported: false,
            game_catalog_required: false,
            game_catalog_version: 0,
            game_catalog: None,
            announcement_scope: String::new(),
            announcement_revision: 0,
            announcement: None,
            client_updates: Vec::new(),
            password_rooms_supported: false,
            locked_channel_ids: Default::default(),
            room_scenes: Default::default(),
            watch_messages: Vec::new(),
            self_session: None,
            server_role: String::new(),
            capabilities: Vec::new(),
            network_policy_channel_ids: Vec::new(),
            bans: Vec::new(),
            ban_list_offset: 0,
            ban_list_total: 0,
            trust_endpoint: String::new(),
            trust_fingerprint: String::new(),
            local_mute_intent: false,
            muted: false,
            admin_muted: false,
            deafened: false,
            audio_state: AudioState::Stopped,
            input_level: 0.0,
            input_peak: 0.0,
            output_level: 0.0,
            talking: BTreeMap::new(),
            rtt_ms: None,
            control_rtt_ms: None,
            packets_sent: 0,
            packets_received: 0,
            packets_lost: 0,
            packets_reordered: 0,
            jitter_depth: 0,
            opus_bitrate_bps: 32_000,
            jitter_ms: 0.0,
            effective_audio_loss_percent: 0.0,
            jitter_buffer_delay_ms: 0.0,
            jitter_buffer_underruns: 0,
            plc_frames: 0,
            fec_recovered_packets: 0,
            audio_timing: crate::audio::AudioTiming::default(),
            capture_overruns: 0,
            playback_underruns: 0,
            user_volumes: BTreeMap::new(),
            locally_muted_users: BTreeMap::new(),
            push_to_talk_pressed: false,
            active_noise_suppression: NoiseSuppressionMode::Off,
            active_echo_cancellation: false,
            active_automatic_gain: false,
            dsp_average_ms: 0.0,
            dsp_peak_ms: 0.0,
            dsp_deadline_misses: 0,
            dsp_error: None,
            dsp_loading: None,
            dsp_performance_warning: None,
            mic_test_status: MicTestStatus::Idle,
            active_input_device: None,
            active_output_device: None,
            active_input_sample_rate: None,
            active_output_sample_rate: None,
            device_fallback_active: false,
            device_error: None,
            room_topology: "disconnected".into(),
            current_transport: "none".into(),
            route_reason: String::new(),
            qualification_duration_ms: 0,
            migration_cooldown_ms: 0,
            transport_mode: "auto".into(),
            audio_quality_mode: "auto".into(),
            network_quality: "good".into(),
            network_trend: "stable".into(),
            actual_transport: "SFU".into(),
            selected_route: "Packet SFU".into(),
            route_endpoint: String::new(),
            p2p_qualification: "Waiting for peers".into(),
        }
    }
}

impl AppState {
    pub fn close_room_archive(&mut self) {
        self.archive_requested_channel = None;
        self.room_archive = None;
    }

    pub fn receive_room_archive(&mut self, archive: RoomArchive) -> bool {
        if self.archive_requested_channel != Some(archive.stats.room_id) {
            return false;
        }
        self.room_archive = Some(archive);
        true
    }

    pub fn take_room_archive(&mut self) -> Option<RoomArchive> {
        self.room_archive.take()
    }

    pub fn invalidate_room_social(&mut self) {
        self.self_away = false;
        self.muted_speech_notice = None;
        self.room_social_epoch = self.room_social_epoch.wrapping_add(1);
        self.room_social_events.clear();
    }

    pub fn apply_snapshot(&mut self, state: ServerState) {
        let old_channel = self.self_session.and_then(|id| self.users.get(&id)).map(|user| user.channel_id);
        let new_channel = state.users.iter().find(|user| Some(user.session_id) == self.self_session).map(|user| user.channel_id);
        if old_channel != new_channel { self.invalidate_room_social(); }
        if let Some(c) = state.game_catalog
            && c.valid()
        {
            self.game_catalog = Some(c)
        }
        self.game_catalog_version = state.game_catalog_version;
        if let Ok(revision) = state.announcement_revision.parse::<u64>() {
            if revision > 0 && revision >= self.announcement_revision {
                self.announcement_revision = revision;
                self.announcement = state.announcement;
            }
        }
        self.client_updates = state.client_updates;
        self.chat_heads.retain(|channel,_|state.channels.iter().any(|item|item.id==*channel));
        for (channel,id) in state.chat_heads {
            if id.parse::<u64>().unwrap_or(0)>self.chat_heads.get(&channel).and_then(|known|known.parse::<u64>().ok()).unwrap_or(0) {self.chat_heads.insert(channel,id);}
        }
        self.password_rooms_supported = state.password_rooms_supported;
        self.locked_channel_ids = state.locked_channel_ids;
        self.room_scenes = state.room_scenes;
        self.server_name = state.server_name;
        self.channels = state.channels;
        self.channels.sort_by_key(|c| (c.sort_order, c.id));
        self.users = state.users.into_iter().map(|u| (u.session_id, u)).collect();
    }
    pub fn move_user(&mut self, id: u64, channel: u64) {
        if self.self_session == Some(id) && self.users.get(&id).is_some_and(|user| user.channel_id != channel) {
            self.invalidate_room_social();
        }
        if let Some(user) = self.users.get_mut(&id) {
            user.channel_id = channel;
        }
    }
    pub fn remove_user(&mut self, id: u64) {
        if self.self_session == Some(id) { self.invalidate_room_social(); }
        self.users.remove(&id);
        self.talking.remove(&id);
        self.user_volumes.remove(&id);
        self.locally_muted_users.remove(&id);
    }
    pub fn is_talking(&self, id: u64) -> bool {
        self.talking
            .get(&id)
            .is_some_and(|last| last.elapsed().as_millis() < 300)
    }

    pub fn user_volume(&self, id: u64) -> f32 {
        self.user_volumes.get(&id).copied().unwrap_or(1.0)
    }

    pub fn is_user_locally_muted(&self, id: u64) -> bool {
        self.locally_muted_users.get(&id).copied().unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn room_social_pending_events_are_invalidated_even_when_returning_to_same_room() {
        let mut state = AppState {self_session:Some(7), ..Default::default()};
        state.users.insert(7, User {session_id:7,channel_id:3,..Default::default()});
        state.room_social_events.push(Default::default());
        let epoch = state.room_social_epoch;
        state.move_user(7, 4);
        state.move_user(7, 3);
        assert_eq!(state.room_social_epoch, epoch + 2);
        assert!(state.room_social_events.is_empty());
        state.move_user(7, 3);
        assert_eq!(state.room_social_epoch, epoch + 2);
    }
    #[test]
    fn announcement_withdrawal_rejects_late_snapshots() {
        let mut app = AppState::default();
        let snapshot = |revision: &str, announcement: serde_json::Value| {
            serde_json::from_value::<ServerState>(serde_json::json!({
                "server_name":"Test", "channels":[], "users":[],
                "announcement_revision":revision, "announcement":announcement
            })).unwrap()
        };
        let publication = serde_json::json!({"revision":"3","title":"Hi","body":"Hello","published_at":1});
        app.apply_snapshot(snapshot("3", publication.clone()));
        assert!(app.announcement.is_some());
        app.apply_snapshot(snapshot("4", serde_json::Value::Null));
        app.apply_snapshot(snapshot("3", publication));
        assert!(app.announcement.is_none());
        let latest = serde_json::json!({"revision":"5","title":"Latest","body":"Hello","published_at":2});
        app.apply_snapshot(snapshot("5", latest.clone()));
        app.apply_snapshot(snapshot("4", serde_json::Value::Null));
        assert_eq!(app.announcement, Some(latest));
        assert_eq!(app.announcement_revision, 5);
    }
    #[test]
    fn snapshot_and_events_update_state() {
        let mut app = AppState::default();
        app.apply_snapshot(ServerState {
            chat_heads: Default::default(),
            announcement_revision: String::new(),
            announcement: None,
            game_catalog_version: 0,
            game_catalog: None,
            client_updates: Vec::new(),
            password_rooms_supported: false,
            locked_channel_ids: Default::default(),
            room_scenes: Default::default(),
            server_name: "Test".into(),
            channels: vec![
                Channel {
                    id: 2,
                    parent_id: 0,
                    name: "B".into(),
                    sort_order: 2,
                    user_limit: 0,
                    network_policy: "force_sfu".into(),
                    audio_quality_policy: "quality_7".into(),
                    temporary: false,
                },
                Channel {
                    id: 1,
                    parent_id: 0,
                    name: "A".into(),
                    sort_order: 1,
                    user_limit: 0,
                    network_policy: "auto".into(),
                    audio_quality_policy: "auto".into(),
                    temporary: false,
                },
            ],
            users: vec![User {
                session_id: 9,
                nickname: "Alice".into(),
                ..Default::default()
            }],
        });
        assert_eq!(app.channels[0].id, 1);
        app.move_user(9, 2);
        assert_eq!(app.users[&9].channel_id, 2);
        app.remove_user(9);
        assert!(app.users.is_empty());
    }
}
