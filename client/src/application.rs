use std::{collections::HashSet, time::Instant};

pub use crate::{
    config::{Language, ThemeMode},
    dsp::{InputMode, NoiseSuppressionMode, NoiseSuppressionStrength},
    model::{AudioState, ConnectionState, MicTestStatus},
};

use crate::{
    audio::{
        AudioController, AudioDeviceBackend, AudioEvent, AudioProcessingSettings, AudioSettings,
        AudioTransportSettings, CpalAudioBackend,
    },
    config::{ClientConfig, SavedServer, ShortcutBindings},
    control::{Command as ControlCommand, ControlHandle, Event as ControlEvent},
    devices::DeviceList,
    model::AppState,
    network::{
        AudioAdaptationController, AudioQualityPolicy, NetworkObservation, NetworkQualityEngine,
        ReceiverFeedbackWindow, RoomNetworkPolicy,
    },
    platform::hotkey::{GlobalPttHotkey, HotkeyEvent},
    protocol::control::{self, MessageType},
    security::MediaSecurity,
    transport::{AdaptiveControl, TransportEvent, TransportHealth},
};

#[derive(Clone, Debug, PartialEq)]
pub struct AudioPreferences {
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub input_gain: f32,
    pub output_volume: f32,
    pub input_mode: InputMode,
    pub echo_cancellation: bool,
    pub automatic_gain: bool,
    pub noise_suppression_mode: NoiseSuppressionMode,
    pub noise_suppression_strength: NoiseSuppressionStrength,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AppCommand {
    Connect {
        server_password: crate::control::ServerPassword,
        address: String,
        nickname: String,
        remember: bool,
        auto_connect: Option<bool>,
    },
    Disconnect,
    RetryAudio,
    JoinChannel(u64),
    JoinProtectedChannel { channel_id:u64, password:String },
    SetChannelPassword { channel_id:u64, password:String },
    LeaveChannel,
    SetMuted(bool),
    SetDeafened(bool),
    SetDisplayName(String),
    SetAvatar(String),
    SetShareGameActivity(bool),
    SendTextMessage(String),
    ScreenShare(crate::protocol::screenshare::Request),
    Watch(crate::protocol::watch::WatchRequest),
    RequestRoomArchive {
        channel_id: u64,
        all: bool,
    },
    CloseRoomArchive,
    SetUserVolume {
        session_id: u64,
        volume: f32,
    },
    SetUserLocalMute {
        session_id: u64,
        muted: bool,
    },
    AdminMute {
        session_id: u64,
        enabled: bool,
    },
    KickChannel(u64),
    KickServer(u64),
    BanUser {
        session_id: u64,
        reason: String,
    },
    ListBans {
        offset: usize,
        limit: usize,
    },
    Unban(String),
    MoveUser {
        session_id: u64,
        channel_id: u64,
    },
    SetChannelAdmin {
        session_id: u64,
        enabled: bool,
    },
    SetServerAdmin {
        session_id: u64,
        enabled: bool,
    },
    DeleteChannel(u64),
    CreateChannel(control::Channel),
    CreateProtectedChannel {channel:control::Channel,password:String},
    EditChannel(control::Channel),
    ProvisionMaster(String),
    TrustServer {
        endpoint: String,
        fingerprint: String,
    },
    SetMasterOutputVolume(f32),
    SetPushToTalkPressed(bool),
    UpdateShortcuts(ShortcutBindings),
    PlayOutputTest,
    StartMicTest,
    PlayMicTestOriginal,
    PlayMicTestProcessed,
    StopMicTest,
    PrepareLocalAudio,
    StopLocalAudio,
    UpdateAudioPreferences(AudioPreferences),
    RefreshDevices,
    SetTheme(ThemeMode),
    SetLanguage(Language),
    SetMiniOverlay {
        enabled: bool,
        mode: String,
    },
    SetMiniOverlayPosition {
        x: i32,
        y: i32,
    },
    SetAutoConnectLastServer(bool),
    SetAutoConnect {
        enabled: bool,
        server_address: String,
    },
    SetTransportMode(String),
    SetRoomNetworkPolicy(RoomNetworkPolicy),
    SetChannelNetworkPolicy {
        channel_id: u64,
        policy: RoomNetworkPolicy,
    },
    SetAudioQualityPolicy(AudioQualityPolicy),
    Shutdown,
}

#[derive(Debug)]
pub enum AppEvent {
    Control(ControlEvent),
    Audio(AudioEvent),
}

pub struct Dispatcher {
    control: ControlHandle,
    audio: AudioController,
}

impl Dispatcher {
    fn new() -> Self {
        Self {
            control: ControlHandle::spawn(),
            audio: AudioController::spawn(),
        }
    }

    fn send_control(&self, command: ControlCommand) -> Result<(), String> {
        self.control
            .commands
            .try_send(command)
            .map_err(|error| match error {
                crossbeam_channel::TrySendError::Full(_) => {
                    "control command queue is busy".to_owned()
                }
                crossbeam_channel::TrySendError::Disconnected(_) => {
                    "control service is unavailable".to_owned()
                }
            })
    }

    fn try_next_event(&self) -> Option<AppEvent> {
        self.control
            .events
            .try_recv()
            .map(AppEvent::Control)
            .ok()
            .or_else(|| self.audio.events.try_recv().map(AppEvent::Audio).ok())
    }

    fn start_audio(&self, settings: AudioSettings) {
        self.audio.start(settings);
    }

    fn stop_audio(&self) {
        self.audio.stop();
    }

    fn set_audio_muted(&self, muted: bool) {
        self.audio.set_muted(muted);
    }

    fn set_audio_deafened(&self, deafened: bool) {
        self.audio.set_deafened(deafened);
    }

    fn set_user_volume(&self, session_id: u64, volume: f32) {
        self.audio.set_user_volume(session_id, volume);
    }

    fn set_user_local_mute(&self, session_id: u64, muted: bool) {
        self.audio.set_user_local_mute(session_id, muted);
    }

    fn set_master_output_volume(&self, volume: f32) {
        self.audio.set_master_output_volume(volume);
    }

    fn set_push_to_talk_pressed(&self, pressed: bool) {
        self.audio.set_push_to_talk_pressed(pressed);
    }

    fn update_audio_processing(&self, settings: AudioProcessingSettings) {
        self.audio.update_processing(settings);
    }

    fn apply_audio_adaptation(&self, decision: crate::network::AudioAdaptationDecision) {
        self.audio.apply_adaptation(decision);
    }

    fn play_output_test(&self) {
        self.audio.play_output_test();
    }

    fn start_mic_test(&self) {
        self.audio.start_mic_test();
    }

    fn play_mic_test_original(&self) {
        self.audio.play_mic_test_original();
    }

    fn play_mic_test_processed(&self) {
        self.audio.play_mic_test_processed();
    }

    fn stop_mic_test(&self) {
        self.audio.stop_mic_test();
    }

    fn switch_audio_devices(&self, input: Option<String>, output: Option<String>) {
        self.audio.switch_devices(input, output);
    }
}

pub struct Application {
    game_catalog_cache: std::collections::HashMap<String, crate::game_catalog::Catalog>,
    game_catalog_key: String,
    volume_save_due: Option<Instant>,
    game_catalog_requested: Option<Instant>,
    config: ClientConfig,
    client_id: String,
    state: AppState,
    dispatcher: Dispatcher,
    error: Option<String>,
    voice_session: Option<(String, u64, usize)>,
    control_endpoint: Option<std::net::SocketAddr>,
    devices: DeviceList,
    global_ptt: Option<GlobalPttHotkey>,
    global_ptt_error: Option<String>,
    focused_ptt_pressed: bool,
    global_ptt_pressed: bool,
    active_audio_session: ActiveAudioSession,
    adaptive_control: Option<AdaptiveControl>,
    adaptive_events: Option<crossbeam_channel::Sender<TransportEvent>>,
    media_security: Option<MediaSecurity>,
    network_quality: NetworkQualityEngine,
    receiver_feedback: ReceiverFeedbackWindow,
    audio_adaptation: AudioAdaptationController,
    pending_avatar_downloads: HashSet<String>,
    pending_connection: Option<PendingConnection>,
    auto_connecting: bool,
    preserve_disconnect_error: bool,
    last_archive_voice_report: Option<Instant>,
    muted_speech_analysis_enabled: bool,
    last_muted_speech_notice: Option<Instant>,
    muted_speech_sequence: u64,
    pending_away_request: Option<(String, bool)>,
}

#[derive(Clone, Debug)]
struct PendingConnection {
    address: String,
    nickname: String,
    remember: bool,
    auto_connect: Option<bool>,
}

fn auto_connect_target(config: &ClientConfig) -> Option<PendingConnection> {
    if !config.auto_connect_last_server {
        return None;
    }
    if !config.auto_connect_server.trim().is_empty() {
        return config
            .saved_servers
            .iter()
            .find(|profile| profile.address == config.auto_connect_server)
            .and_then(|profile| {
                let nickname = if profile.nickname.trim().is_empty() {
                    config.nickname.clone()
                } else {
                    profile.nickname.clone()
                };
                (!nickname.trim().is_empty()).then(|| PendingConnection {
                    address: profile.address.clone(),
                    nickname,
                    remember: false,
                    auto_connect: None,
                })
            });
    }
    (!config.last_successful_server.trim().is_empty()
        && !config.last_successful_nickname.trim().is_empty())
    .then(|| PendingConnection {
        address: config.last_successful_server.clone(),
        nickname: config.last_successful_nickname.clone(),
        remember: false,
        auto_connect: None,
    })
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ActiveAudioSession {
    #[default]
    None,
    Local,
    Remote,
}

impl Application {
    pub fn new() -> Self { Self::new_with_auto_connect(true) }

    pub fn new_with_auto_connect(allow_auto_connect: bool) -> Self {
        let config = ClientConfig::load();
        let auto_connect_target = if allow_auto_connect { auto_connect_target(&config) } else { None };
        let client_id = crate::security::Identity::load_or_create()
            .map(|identity| identity.client_id())
            .unwrap_or_default();
        let room_network_policy = config.room_network_policy;
        let audio_quality_policy = config.audio_quality_policy;
        let (global_ptt, global_ptt_error) =
            register_global_ptt(config.input_mode, &config.shortcuts.push_to_talk);
        let state = AppState {
            transport_mode: room_policy_wire(room_network_policy).into(),
            audio_quality_mode: audio_quality_label(audio_quality_policy),
            ..Default::default()
        };
        let mut application = Self {
            game_catalog_cache: Default::default(),
            game_catalog_key: String::new(),
            volume_save_due: None,
            game_catalog_requested: None,
            config,
            client_id,
            state,
            dispatcher: Dispatcher::new(),
            error: None,
            voice_session: None,
            control_endpoint: None,
            devices: enumerate_audio_devices(),
            global_ptt,
            global_ptt_error,
            focused_ptt_pressed: false,
            global_ptt_pressed: false,
            active_audio_session: ActiveAudioSession::None,
            adaptive_control: None,
            adaptive_events: None,
            media_security: None,
            network_quality: NetworkQualityEngine::new(Instant::now()),
            receiver_feedback: ReceiverFeedbackWindow::default(),
            audio_adaptation: AudioAdaptationController::new(audio_quality_policy),
            pending_avatar_downloads: HashSet::new(),
            pending_connection: None,
            auto_connecting: false,
            preserve_disconnect_error: false,
            last_archive_voice_report: None,
            muted_speech_analysis_enabled: false,
            last_muted_speech_notice: None,
            muted_speech_sequence: 0,
            pending_away_request: None,
        };
        if let Some(target) = auto_connect_target {
            application.state.connection = ConnectionState::Connecting;
            application.pending_connection = Some(target.clone());
            application.auto_connecting = true;
            if let Err(error) = application
                .dispatcher
                .send_control(ControlCommand::Connect {
                    server_password: Default::default(),
                    address: target.address,
                    nickname: target.nickname,
                })
            {
                application.state.connection = ConnectionState::Error(error.clone());
                application.error = Some(error);
                application.pending_connection = None;
                application.auto_connecting = false;
            }
        }
        application
    }

    fn sync_muted_speech_analysis(&mut self) {
        let enabled = muted_speech_eligible(&self.state, self.config.input_mode);
        if enabled != self.muted_speech_analysis_enabled {
            self.muted_speech_analysis_enabled = enabled;
            self.dispatcher.audio.set_muted_speech_analysis(enabled);
        }
        if !enabled { self.state.muted_speech_notice = None; }
    }

    pub fn update(&mut self) {
        if self.volume_save_due.is_some_and(|due|Instant::now()>=due){self.volume_save_due=None;self.persist_config();}
        if self.state.connection == ConnectionState::Connected
            && self.state.game_catalog_required
            && self.state.game_catalog_version > 0
            && self.state.game_catalog.as_ref().map(|c| c.version)
                != Some(self.state.game_catalog_version)
            && self
                .game_catalog_requested
                .is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(10))
            && self
                .dispatcher
                .send_control(ControlCommand::GetGameCatalog)
                .is_ok()
        {
            self.game_catalog_requested = Some(Instant::now());
        }

        if let Some(event) = self.global_ptt.as_ref().and_then(GlobalPttHotkey::try_next) {
            self.global_ptt_pressed = event == HotkeyEvent::Pressed;
            self.sync_push_to_talk_state();
        }
        self.sync_muted_speech_analysis();
        while let Some(event) = self.dispatcher.try_next_event() {
            self.apply_event(event);
        }
        self.sync_muted_speech_analysis();
        let transport_events = self
            .adaptive_control
            .as_ref()
            .map(|control| {
                let mut values = Vec::new();
                while let Some(event) = control.try_event() {
                    values.push(event);
                }
                values
            })
            .unwrap_or_default();
        for event in transport_events {
            let command = match event {
                TransportEvent::LocalCandidates(candidates) => {
                    Some(ControlCommand::RegisterCandidates(
                        control::CandidateRegistration { candidates },
                    ))
                }
                TransportEvent::NetworkReport(report) => {
                    Some(ControlCommand::NetworkReport(report))
                }
                TransportEvent::RouteStatus(status) => {
                    self.state.actual_transport = status.actual_transport;
                    self.state.selected_route = status.selected_route;
                    self.state.route_endpoint = status.endpoint;
                    self.state.p2p_qualification = status.qualification;
                    None
                }
                TransportEvent::RekeyRequired => Some(ControlCommand::RequestMediaRekey),
                TransportEvent::Health(TransportHealth::Healthy) => None,
                TransportEvent::Health(TransportHealth::Recovering(reason)) => {
                    tracing::warn!(%reason, "voice transport is recovering");
                    None
                }
                TransportEvent::Health(TransportHealth::Failed(reason)) => {
                    self.error = Some(reason);
                    None
                }
            };
            if let Some(command) = command {
                let _ = self.dispatcher.send_control(command);
            }
        }
    }

    pub fn dispatch(&mut self, command: AppCommand) {
        clear_operation_error_for_user_action(&self.state.connection, &mut self.error);
        if matches!(&command, AppCommand::CreateProtectedChannel{..}|AppCommand::SetChannelPassword{..}|AppCommand::JoinProtectedChannel{..}) && !self.state.password_rooms_supported {
            self.error=Some("当前服务器不支持房间密码，请先升级服务端".into());return;
        }
        let result = match command {
            AppCommand::Connect {
                server_password,
                address,
                nickname,
                remember,
                auto_connect,
            } => {
                self.auto_connecting = false;
                self.preserve_disconnect_error = false;
                self.pending_connection = Some(PendingConnection {
                    address: address.clone(),
                    nickname: nickname.clone(),
                    remember,
                    auto_connect,
                });
                if auto_connect == Some(false) {
                    self.config.auto_connect_last_server = false;
                    self.config.auto_connect_server.clear();
                }
                if remember {
                    self.config
                        .remember_server(address.clone(), nickname.clone(), String::new());
                }
                if remember || auto_connect == Some(false) {
                    self.persist_config();
                }
                self.dispatcher
                    .send_control(ControlCommand::Connect { address, nickname, server_password })
            }
            AppCommand::Disconnect => {
                self.pending_connection = None;
                self.auto_connecting = false;
                self.preserve_disconnect_error = false;
                clear_pending_server_trust(&mut self.state);
                self.dispatcher.send_control(ControlCommand::Disconnect)
            }
            AppCommand::UpdateAudioPreferences(preferences) => {
                let previous = self.audio_preferences();
                self.config.input_device = preferences.input_device;
                self.config.output_device = preferences.output_device;
                self.config.input_gain = preferences.input_gain.clamp(0.0, 3.0);
                self.config.output_volume = preferences.output_volume.clamp(0.01, 3.0);
                self.config.input_mode = preferences.input_mode;
                self.config.echo_cancellation = preferences.echo_cancellation;
                self.config.automatic_gain = preferences.automatic_gain;
                self.config.noise_suppression_mode = preferences.noise_suppression_mode;
                self.config.noise_suppression_strength = preferences.noise_suppression_strength;
                if previous.input_mode != preferences.input_mode {
                    self.configure_global_ptt(preferences.input_mode);
                }
                self.persist_config();
                self.state.dsp_error = None;
                let devices_changed = previous.input_device != self.config.input_device
                    || previous.output_device != self.config.output_device;
                if self.state.audio_state == AudioState::Ready {
                    if devices_changed {
                        self.state.device_error = None;
                        self.dispatcher.switch_audio_devices(
                            self.config.input_device.clone(),
                            self.config.output_device.clone(),
                        );
                    }
                    self.dispatcher
                        .set_master_output_volume(self.config.output_volume);
                    self.dispatcher
                        .update_audio_processing(self.current_audio_processing_settings());
                } else if self.state.audio_state == AudioState::Starting {
                    match self.active_audio_session {
                        ActiveAudioSession::Local => self.start_local_audio(),
                        ActiveAudioSession::Remote => self.start_audio(),
                        ActiveAudioSession::None => {}
                    }
                } else if devices_changed {
                    self.state.device_error = None;
                    self.start_audio();
                }
                Ok(())
            }
            AppCommand::RefreshDevices => {
                self.devices = enumerate_audio_devices();
                Ok(())
            }
            AppCommand::SetShareGameActivity(enabled) => {
                self.config.share_game_activity = enabled;
                self.persist_config();
                Ok(())
            }
            AppCommand::SetTheme(theme) => {
                self.config.theme = theme;
                self.persist_config();
                Ok(())
            }
            AppCommand::SetLanguage(language) => {
                self.config.language = language;
                self.persist_config();
                Ok(())
            }
            AppCommand::SetMiniOverlay { enabled, mode } => {
                if mode != "avatars" && mode != "list" {
                    Err("invalid mini overlay mode".into())
                } else {
                    self.config.mini_overlay_enabled = enabled;
                    self.config.mini_overlay_mode = mode;
                    self.persist_config();
                    Ok(())
                }
            }
            AppCommand::SetMiniOverlayPosition { x, y } => {
                self.config.mini_overlay_position_x = Some(x);
                self.config.mini_overlay_position_y = Some(y);
                self.persist_config();
                Ok(())
            }
            AppCommand::SetAutoConnectLastServer(enabled) => {
                self.config.auto_connect_last_server = enabled;
                self.persist_config();
                Ok(())
            }
            AppCommand::SetAutoConnect {
                enabled,
                server_address,
            } => {
                let server_address = server_address.trim();
                if enabled
                    && !server_address.is_empty()
                    && !self
                        .config
                        .saved_servers
                        .iter()
                        .any(|profile| profile.address == server_address)
                {
                    Err("auto-connect server must be saved first".into())
                } else {
                    self.config.auto_connect_last_server = enabled;
                    self.config.auto_connect_server = server_address.to_owned();
                    self.persist_config();
                    Ok(())
                }
            }
            AppCommand::SetTransportMode(mode) => {
                self.state.transport_mode = mode.clone();
                self.dispatcher
                    .send_control(ControlCommand::SetTransportMode(mode))
            }
            AppCommand::SetRoomNetworkPolicy(policy) => {
                self.config.room_network_policy = policy;
                self.state.transport_mode = room_policy_wire(policy).into();
                self.persist_config();
                self.dispatcher
                    .send_control(ControlCommand::SetRoomNetworkPolicy {
                        policy: room_policy_wire(policy).into(),
                        channel_id: None,
                    })
            }
            AppCommand::SetChannelNetworkPolicy { channel_id, policy } => self
                .dispatcher
                .send_control(ControlCommand::SetRoomNetworkPolicy {
                    policy: room_policy_wire(policy).into(),
                    channel_id: Some(channel_id),
                }),
            AppCommand::SetAudioQualityPolicy(policy) => {
                self.config.audio_quality_policy = policy;
                self.persist_config();
                self.sync_channel_audio_quality();
                Ok(())
            }
            AppCommand::SetUserVolume { session_id, volume } => {
                if !volume.is_finite() { return; }
                let volume = volume.clamp(0.0, 3.0);
                self.state.user_volumes.insert(session_id, volume);
                self.dispatcher.set_user_volume(session_id, volume);
                if let Some(user) = self.state.users.get(&session_id)
                    && !user.client_id.is_empty()
                    && !self.game_catalog_key.is_empty()
                {
                    let key = serde_json::to_string(&(&self.game_catalog_key, &user.client_id))
                        .unwrap_or_default();
                    self.config.remembered_user_volumes.insert(key, volume);
                    self.volume_save_due =
                        Some(Instant::now() + std::time::Duration::from_millis(500));
                }
                Ok(())
            }
            AppCommand::SetUserLocalMute { session_id, muted } => {
                self.state.locally_muted_users.insert(session_id, muted);
                let admin_muted = self
                    .state
                    .users
                    .get(&session_id)
                    .is_some_and(|user| user.admin_muted);
                self.dispatcher
                    .set_user_local_mute(session_id, effective_remote_mute(muted, admin_muted));
                Ok(())
            }
            AppCommand::AdminMute {
                session_id,
                enabled,
            } => self.state.users.get(&session_id).map_or_else(
                || Err("unknown user".into()),
                |user| {
                    self.dispatcher.send_control(ControlCommand::AdminMute {
                        session_id,
                        client_id: user.client_id.clone(),
                        enabled,
                    })
                },
            ),
            AppCommand::KickChannel(session_id) => self.state.users.get(&session_id).map_or_else(
                || Err("unknown user".into()),
                |user| {
                    self.dispatcher.send_control(ControlCommand::KickChannel {
                        session_id,
                        client_id: user.client_id.clone(),
                    })
                },
            ),
            AppCommand::KickServer(session_id) => self.state.users.get(&session_id).map_or_else(
                || Err("unknown user".into()),
                |user| {
                    self.dispatcher.send_control(ControlCommand::KickServer {
                        session_id,
                        client_id: user.client_id.clone(),
                    })
                },
            ),
            AppCommand::BanUser { session_id, reason } => {
                self.state.users.get(&session_id).map_or_else(
                    || Err("unknown user".into()),
                    |user| {
                        self.dispatcher.send_control(ControlCommand::Ban {
                            client_id: user.client_id.clone(),
                            reason,
                        })
                    },
                )
            }
            AppCommand::ListBans { offset, limit } => self
                .dispatcher
                .send_control(ControlCommand::ListBans { offset, limit }),
            AppCommand::Unban(client_id) => self
                .dispatcher
                .send_control(ControlCommand::Unban(client_id)),
            AppCommand::MoveUser {
                session_id,
                channel_id,
            } => self.state.users.get(&session_id).map_or_else(
                || Err("unknown user".into()),
                |user| {
                    self.dispatcher.send_control(ControlCommand::MoveUser {
                        session_id,
                        client_id: user.client_id.clone(),
                        channel_id,
                    })
                },
            ),
            AppCommand::SetChannelAdmin {
                session_id,
                enabled,
            } => {
                channel_admin_target(&self.state, session_id).and_then(|(channel_id, client_id)| {
                    self.dispatcher
                        .send_control(ControlCommand::SetChannelAdmin {
                            channel_id,
                            client_id,
                            enabled,
                        })
                })
            }
            AppCommand::SetServerAdmin {
                session_id,
                enabled,
            } => self.state.users.get(&session_id).map_or_else(
                || Err("unknown user".into()),
                |user| {
                    self.dispatcher.send_control(ControlCommand::SetServerRole {
                        client_id: user.client_id.clone(),
                        role: if enabled { "ADMIN" } else { "USER" }.into(),
                    })
                },
            ),
            AppCommand::JoinProtectedChannel {channel_id,password} => self.dispatcher.send_control(ControlCommand::JoinProtectedChannel{channel_id,password}),
            AppCommand::SetChannelPassword {channel_id,password} => self.dispatcher.send_control(ControlCommand::SetChannelPassword{channel_id,password}),
            AppCommand::DeleteChannel(channel_id) => self
                .dispatcher
                .send_control(ControlCommand::DeleteChannel(channel_id)),
            AppCommand::CreateProtectedChannel{channel,password} => self.dispatcher.send_control(ControlCommand::CreateProtectedChannel{channel,password}),
            AppCommand::CreateChannel(channel) => self
                .dispatcher
                .send_control(ControlCommand::CreateChannel(channel)),
            AppCommand::EditChannel(channel) => self
                .dispatcher
                .send_control(ControlCommand::EditChannel(channel)),
            AppCommand::ProvisionMaster(token) => self
                .dispatcher
                .send_control(ControlCommand::ProvisionMaster(token)),
            AppCommand::TrustServer {
                endpoint,
                fingerprint,
            } => {
                clear_pending_server_trust(&mut self.state);
                self.dispatcher.send_control(ControlCommand::TrustServer {
                    endpoint,
                    fingerprint,
                })
            }
            AppCommand::SetMasterOutputVolume(volume) => {
                let volume = volume.clamp(0.01, 3.0);
                self.config.output_volume = volume;
                self.dispatcher.set_master_output_volume(volume);
                self.persist_config();
                Ok(())
            }
            AppCommand::SetMuted(muted) => {
                let undeafen_result = if !muted && self.state.deafened {
                    apply_local_deafened_state(&mut self.state, false);
                    self.dispatcher.set_audio_deafened(false);
                    self.dispatcher
                        .send_control(ControlCommand::SetDeafen(false))
                } else {
                    Ok(())
                };
                apply_local_muted_state(&mut self.state, muted);
                self.dispatcher.set_audio_muted(self.state.muted);
                undeafen_result
                    .and_then(|_| self.dispatcher.send_control(ControlCommand::SetMute(muted)))
            }
            AppCommand::SetDeafened(deafened) => {
                apply_local_deafened_state(&mut self.state, deafened);
                if deafened {
                    self.dispatcher.set_audio_muted(true);
                }
                self.dispatcher.set_audio_deafened(deafened);
                self.dispatcher
                    .send_control(ControlCommand::SetDeafen(deafened))
            }
            AppCommand::SetDisplayName(display_name) => {
                let display_name = display_name.trim().to_owned();
                if display_name.is_empty() || display_name.len() > 64 {
                    Err("display name must be 1-64 bytes".into())
                } else {
                    self.config.nickname = display_name.clone();
                    self.persist_config();
                    if matches!(self.state.connection, ConnectionState::Connected) {
                        self.dispatcher
                            .send_control(ControlCommand::SetDisplayName(display_name))
                    } else {
                        Ok(())
                    }
                }
            }
            AppCommand::SetAvatar(avatar) => {
                if !crate::avatar::is_supported_avatar(&avatar) {
                    Err("invalid avatar".into())
                } else {
                    match if avatar.starts_with(crate::avatar::CUSTOM_AVATAR_PREFIX) {
                        crate::avatar::cache_upload(&avatar).map_err(|error| error.to_string())
                    } else {
                        Ok(avatar.clone())
                    } {
                        Ok(reference) => {
                            self.config.avatar = reference;
                            self.persist_config();
                            self.dispatcher
                                .send_control(ControlCommand::SetAvatar(avatar))
                        }
                        Err(error) => Err(error),
                    }
                }
            }
            AppCommand::RetryAudio => {
                if self.state.connection != ConnectionState::Connected || self.voice_session.is_none() {
                    Err("连接恢复后才能重试语音".into())
                } else {
                    self.devices = enumerate_audio_devices();
                    self.state.device_error = None;
                    self.start_audio();
                    Ok(())
                }
            }
            AppCommand::SendTextMessage(text) => {
                let text = text.trim().to_owned();
                validate_chat_message(&self.state.connection, &text).and_then(|()| {
                    self.dispatcher
                        .send_control(ControlCommand::SendTextMessage(text))
                })
            }
            AppCommand::ScreenShare(request) => {
                if self.state.connection != ConnectionState::Connected || !self.state.screen_share_supported {
                    Err("服务器不支持屏幕共享".into())
                } else if request.sdp.len() > 48 * 1024 || request.request_id.len() > 64 || request.share_id.len() > 64 {
                    Err("屏幕共享请求过大".into())
                } else {
                    self.dispatcher.send_control(ControlCommand::ScreenShare(request))
                }
            }
            AppCommand::Watch(request) => {
                if self.state.connection != ConnectionState::Connected
                    || !self.state.watch_supported
                {
                    Err("This server does not support watch parties".into())
                } else {
                    self.dispatcher.send_control(ControlCommand::Watch(request))
                }
            }
            AppCommand::RequestRoomArchive { channel_id, all } => {
                if !self.state.archive_supported {
                    Err("This server does not support Eden Archive yet".into())
                } else {
                    self.state.room_archive = None;
                    let result = self
                        .dispatcher
                        .send_control(ControlCommand::RequestRoomArchive { channel_id, all });
                    self.state.archive_requested_channel = result.as_ref().ok().map(|_| channel_id);
                    result
                }
            }
            AppCommand::CloseRoomArchive => {
                self.state.close_room_archive();
                Ok(())
            }
            AppCommand::SetPushToTalkPressed(pressed) => {
                self.focused_ptt_pressed = pressed;
                self.sync_push_to_talk_state();
                Ok(())
            }
            AppCommand::UpdateShortcuts(shortcuts) => {
                if self.config.shortcuts != shortcuts {
                    self.config.shortcuts = shortcuts;
                    self.configure_global_ptt(self.config.input_mode);
                    self.persist_config();
                }
                Ok(())
            }
            AppCommand::PlayOutputTest => {
                // A device test that bypasses the room's speaker mute can sound
                // successful while all remote speech remains intentionally
                // suppressed. Make the action restore receive audio as well so
                // the test reflects the path used by the call.
                let unmute_result = if self.state.deafened {
                    apply_local_deafened_state(&mut self.state, false);
                    self.dispatcher.set_audio_deafened(false);
                    if matches!(self.state.connection, ConnectionState::Connected) {
                        self.dispatcher
                            .send_control(ControlCommand::SetDeafen(false))
                    } else {
                        Ok(())
                    }
                } else {
                    Ok(())
                };
                self.dispatcher.play_output_test();
                unmute_result
            }
            AppCommand::StartMicTest => {
                self.dispatcher.start_mic_test();
                Ok(())
            }
            AppCommand::PlayMicTestOriginal => {
                self.dispatcher.play_mic_test_original();
                Ok(())
            }
            AppCommand::PlayMicTestProcessed => {
                self.dispatcher.play_mic_test_processed();
                Ok(())
            }
            AppCommand::StopMicTest => {
                self.dispatcher.stop_mic_test();
                Ok(())
            }
            AppCommand::PrepareLocalAudio => {
                if self.active_audio_session == ActiveAudioSession::None {
                    self.start_local_audio();
                }
                Ok(())
            }
            AppCommand::StopLocalAudio => {
                if self.active_audio_session == ActiveAudioSession::Local {
                    self.active_audio_session = ActiveAudioSession::None;
                    self.dispatcher.stop_audio();
                }
                Ok(())
            }
            AppCommand::Shutdown => {
                self.persist_config();
                self.dispatcher.stop_audio();
                self.dispatcher.send_control(ControlCommand::Shutdown)
            }
            command => control_command(&command).map_or_else(
                || Err("application command has no control mapping".to_owned()),
                |command| self.dispatcher.send_control(command),
            ),
        };
        if let Err(error) = result {
            self.error = Some(error);
        }
        self.sync_muted_speech_analysis();
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    // Archive is an on-demand response, never part of the high-frequency UI
    // state stream. The desktop bridge consumes each response exactly once.
    pub fn query_chat(&self, request: control::ChatQuery) -> Result<(), String> {
        if self.state.connection != ConnectionState::Connected || !self.state.chat_query_supported {
            return Err("当前服务器不支持历史查询，请先升级服务端".into());
        }
        self.dispatcher.send_control(ControlCommand::QueryChat(request))
    }

    pub fn take_chat_query_results(&mut self) -> Vec<control::ChatQueryResult> {
        std::mem::take(&mut self.state.chat_query_results)
    }

    pub fn take_room_archive(&mut self) -> Option<control::RoomArchive> {
        self.state.take_room_archive()
    }

    pub fn request_room_social(&mut self, request: crate::protocol::room_social::Request) -> Result<(), String> {
        request.validate()?;
        if self.state.connection != ConnectionState::Connected || !self.state.room_social_supported {
            return Err("当前服务器不支持房间互动".into());
        }
        if request.action.starts_with("image_") && !self.state.room_images_supported {
            return Err("当前服务器未启用房间图片".into());
        }
        if self.state.self_session.and_then(|id| self.state.users.get(&id)).map(|user| user.channel_id) != Some(request.channel_id) {
            return Err("房间已改变，请重试".into());
        }
        let previous_away = self.state.self_away;
        if request.action == "away" { self.pending_away_request = Some((request.request_id.clone(), previous_away)); }
        if request.action == "away" && request.active == Some(true) {
            self.dispatch(AppCommand::SetMuted(true));
            self.state.self_away = true;
            self.state.muted_speech_notice = None;
        }
        self.sync_muted_speech_analysis();
        let result = self.dispatcher.send_control(ControlCommand::RoomSocial(request));
        if result.is_err() { self.state.self_away = previous_away; self.pending_away_request = None; }
        result
    }

    pub fn take_room_social_events(&mut self) -> Vec<crate::protocol::room_social::Event> {
        let events = std::mem::take(&mut self.state.room_social_events);
        if self.state.connection == ConnectionState::Connected { events } else { Vec::new() }
    }

    pub fn take_screen_share_messages(&mut self) -> Vec<crate::protocol::screenshare::Response> {
        let messages = std::mem::take(&mut self.state.screen_share_messages);
        if self.state.connection == ConnectionState::Connected { messages } else { Vec::new() }
    }

    pub fn take_watch_messages(&mut self) -> Vec<crate::protocol::watch::WatchState> {
        std::mem::take(&mut self.state.watch_messages)
    }

    pub fn mpv_path(&self) -> &str {
        &self.config.mpv_path
    }

    pub fn set_mpv_path(&mut self, path: String) -> Result<(), String> {
        let mut config = self.config.clone();
        config.mpv_path = path;
        config
            .save()
            .map_err(|e| format!("播放器设置保存失败：{e}"))?;
        self.config = config;
        Ok(())
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn server_address(&self) -> &str {
        self.pending_connection
            .as_ref()
            .map(|pending| pending.address.as_str())
            .unwrap_or(&self.config.last_server)
    }

    pub fn nickname(&self) -> &str {
        self.pending_connection
            .as_ref()
            .map(|pending| pending.nickname.as_str())
            .unwrap_or(&self.config.nickname)
    }

    pub fn avatar(&self) -> &str {
        &self.config.avatar
    }

    fn restore_member_volumes(&mut self) {
        for (&session_id, user) in &self.state.users {
            if user.client_id.is_empty() || self.game_catalog_key.is_empty() {
                continue;
            }
            let key = serde_json::to_string(&(&self.game_catalog_key, &user.client_id))
                .unwrap_or_default();
            if let Some(&volume) = self.config.remembered_user_volumes.get(&key)
                && volume.is_finite()
                && (0.0..=3.0).contains(&volume)
                && self.state.user_volumes.get(&session_id) != Some(&volume)
            {
                self.state.user_volumes.insert(session_id, volume);
                self.dispatcher.set_user_volume(session_id, volume);
            }
        }
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    pub fn saved_servers(&self) -> &[SavedServer] {
        &self.config.saved_servers
    }

    pub fn update_saved_server(&mut self, index: usize, profile: SavedServer) -> bool {
        let previous_address = self
            .config
            .saved_servers
            .get(index)
            .map(|saved| saved.address.clone())
            .unwrap_or_default();
        let was_active = self
            .config
            .saved_servers
            .get(index)
            .is_some_and(|saved| saved.address == self.config.last_server);
        let updated = self.config.update_saved_server(index, profile);
        if updated {
            if previous_address == self.config.auto_connect_server {
                self.config.auto_connect_server = self.config.saved_servers[index].address.clone();
            }
            if was_active {
                self.config.last_server = self.config.saved_servers[index].address.clone();
                self.config.nickname = self.config.saved_servers[index].nickname.clone();
            }
            self.persist_config();
        }
        updated
    }

    pub fn add_saved_server(&mut self, profile: SavedServer) -> bool {
        let added = self.config.add_saved_server(profile);
        if added {
            self.persist_config();
        }
        added
    }

    pub fn delete_saved_server(&mut self, index: usize) -> bool {
        let removed = self.config.delete_saved_server(index);
        if let Some(removed) = removed {
            if removed.address == self.config.auto_connect_server {
                self.config.auto_connect_server.clear();
                self.config.auto_connect_last_server = false;
            }
            if removed.address == self.config.last_server {
                self.config.last_server = self
                    .config
                    .saved_servers
                    .first()
                    .map(|profile| profile.address.clone())
                    .unwrap_or_default();
            }
            self.persist_config();
            true
        } else {
            false
        }
    }

    pub fn audio_preferences(&self) -> AudioPreferences {
        AudioPreferences {
            input_device: self.config.input_device.clone(),
            output_device: self.config.output_device.clone(),
            input_gain: self.config.input_gain,
            output_volume: self.config.output_volume,
            input_mode: self.config.input_mode,
            echo_cancellation: self.config.echo_cancellation,
            automatic_gain: self.config.automatic_gain,
            noise_suppression_mode: self.config.noise_suppression_mode,
            noise_suppression_strength: self.config.noise_suppression_strength,
        }
    }

    pub fn devices(&self) -> &DeviceList {
        &self.devices
    }

    pub fn theme(&self) -> ThemeMode {
        self.config.theme
    }

    pub fn language(&self) -> Language {
        self.config.language
    }

    pub fn share_game_activity(&self) -> bool {
        self.config.share_game_activity
    }
    pub fn publish_game_activity(&self, game_id: String) -> Result<(), String> {
        if self.state.connection != ConnectionState::Connected
            || !self.state.game_activity_supported
        {
            return Ok(());
        }
        self.dispatcher
            .send_control(ControlCommand::SetGameActivity(game_id))
    }
    pub fn mini_overlay_enabled(&self) -> bool {
        self.config.mini_overlay_enabled
    }

    pub fn mini_overlay_mode(&self) -> &str {
        &self.config.mini_overlay_mode
    }

    pub fn mini_overlay_position(&self) -> Option<(i32, i32)> {
        Some((
            self.config.mini_overlay_position_x?,
            self.config.mini_overlay_position_y?,
        ))
    }

    pub fn auto_connect_last_server(&self) -> bool {
        self.config.auto_connect_last_server
    }

    pub fn auto_connect_server(&self) -> &str {
        if self.config.auto_connect_server.is_empty() {
            &self.config.last_successful_server
        } else {
            &self.config.auto_connect_server
        }
    }

    pub fn auto_connecting(&self) -> bool {
        self.auto_connecting
    }

    pub fn shortcut_bindings(&self) -> &ShortcutBindings {
        &self.config.shortcuts
    }

    pub fn input_mode(&self) -> InputMode {
        self.config.input_mode
    }

    pub fn room_network_policy(&self) -> RoomNetworkPolicy {
        self.config.room_network_policy
    }

    pub fn global_ptt_error(&self) -> Option<&str> {
        self.global_ptt_error.as_deref()
    }

    fn persist_config(&mut self) {
        if let Err(error) = self.config.save() {
            self.error = Some(format!("settings could not be saved: {error}"));
        }
    }

    fn configure_global_ptt(&mut self, input_mode: InputMode) {
        self.global_ptt = None;
        self.global_ptt_error = None;
        self.global_ptt_pressed = false;
        self.focused_ptt_pressed = false;
        self.sync_push_to_talk_state();
        if input_mode == InputMode::PushToTalk {
            let (hotkey, error) =
                register_global_ptt(input_mode, &self.config.shortcuts.push_to_talk);
            self.global_ptt = hotkey;
            self.global_ptt_error = error;
        }
    }

    fn sync_push_to_talk_state(&mut self) {
        let pressed = self.focused_ptt_pressed || self.global_ptt_pressed;
        self.state.push_to_talk_pressed = pressed;
        self.dispatcher.set_push_to_talk_pressed(pressed);
    }

    fn apply_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Control(event) => self.apply_control_event(event),
            AppEvent::Audio(event) => self.apply_audio_event(event),
        }
    }

    fn apply_control_event(&mut self, event: ControlEvent) {
        match event {
            ControlEvent::ConnectedEndpoint(endpoint) => {
                self.control_endpoint = Some(endpoint);
            }
            ControlEvent::Connecting => {
                self.state.invalidate_room_social();
                self.state.close_room_archive();
                self.state.connection = ConnectionState::Connecting;
                self.state.rtt_ms = None;
                self.state.control_rtt_ms = None;
                self.error = None;
            }
            ControlEvent::Reconnecting { delay_seconds: _ } => {
                self.state.invalidate_room_social();
                self.state.close_room_archive();
                if !self.preserve_disconnect_error {
                    self.state.connection = ConnectionState::Reconnecting;
                    self.state.rtt_ms = None;
                    self.state.control_rtt_ms = None;
                    self.stop_remote_session();
                }
            }
            ControlEvent::Disconnected => {
                tracing::info!("disconnected");
                let preserved_error = self
                    .preserve_disconnect_error
                    .then(|| self.error.clone())
                    .flatten();
                self.stop_remote_session();
                let keep_muted = self.state.local_mute_intent;
                self.state = AppState {local_mute_intent: keep_muted, muted: keep_muted, ..AppState::default()};
                self.error = preserved_error;
                self.pending_connection = None;
                self.auto_connecting = false;
                self.preserve_disconnect_error = false;
            }
            ControlEvent::Error(error) => {
                self.state.close_room_archive();
                tracing::warn!(error = %error, "control error");
                self.stop_remote_session();
                clear_pending_server_trust(&mut self.state);
                let user_message = control_error_message(&error);
                self.error = Some(user_message.clone());
                self.state.connection = ConnectionState::Error(user_message);
                self.state.rtt_ms = None;
                self.state.control_rtt_ms = None;
                if self.auto_connecting {
                    self.auto_connecting = false;
                    self.pending_connection = None;
                    self.preserve_disconnect_error = true;
                    let _ = self.dispatcher.send_control(ControlCommand::Disconnect);
                }
            }
            ControlEvent::SecurityEstablished(security) => {
                self.media_security = Some(security);
                clear_pending_server_trust(&mut self.state);
            }
            ControlEvent::ServerTrustRequired {
                endpoint,
                fingerprint,
            } => {
                self.state.trust_endpoint = endpoint;
                self.state.trust_fingerprint = fingerprint;
            }
            ControlEvent::MediaRekeyed(epoch) => {
                if let Some(control) = &self.adaptive_control {
                    control.media_rekeyed();
                }
                tracing::info!(key_epoch = epoch, "media security rekeyed");
            }
            ControlEvent::ControlRtt(rtt_ms) => self.state.control_rtt_ms = Some(rtt_ms),
            ControlEvent::Message(message) => self.apply_control_message(message),
        }
    }

    fn apply_control_message(&mut self, message: control::Envelope) {
        let result: Result<(), serde_json::Error> = (|| {
            match message.kind {
                MessageType::Welcome => {
                    let welcome: control::Welcome = message.decode()?;
                    self.state.close_room_archive();
                    if let Some(pending) = self.pending_connection.take() {
                        let auto_connect = pending.auto_connect;
                        let connected_address = pending.address.clone();
                        self.config.record_successful_connection(
                            pending.address,
                            pending.nickname,
                            welcome.server_name.clone(),
                            pending.remember,
                        );
                        if let Some(enabled) = auto_connect {
                            self.config.auto_connect_last_server = enabled;
                            self.config.auto_connect_server = if enabled {
                                connected_address
                            } else {
                                String::new()
                            };
                        }
                        self.persist_config();
                    }
                    self.auto_connecting = false;
                    self.preserve_disconnect_error = false;
                    self.state.announcement = None;
                    self.state.announcement_revision = 0;
                    self.state.announcement_scope = welcome.server_fingerprint.clone();
                    self.state.self_session = Some(welcome.session_id);
                    self.state.server_role = welcome.server_role;
                    self.state.capabilities = welcome.capabilities;
                    self.state.network_policy_channel_ids = welcome.network_policy_channel_ids;
                    self.state.archive_supported = welcome
                        .server_features
                        .iter()
                        .any(|feature| feature == "eden_archive_v1");
                    self.game_catalog_requested = None;
                    self.game_catalog_key =
                        format!("{}|{}", self.server_address(), welcome.server_fingerprint);
                    self.state.game_catalog_required = welcome
                        .server_features
                        .iter()
                        .any(|f| f == "game_catalog_v1");
                    self.state.game_catalog = if self.state.game_catalog_required {
                        self.game_catalog_cache.get(&self.game_catalog_key).cloned()
                    } else {
                        None
                    };
                    self.state.game_activity_supported = welcome
                        .server_features
                        .iter()
                        .any(|f| f == "game_activity_v1");
                    self.state.chat_query_supported = welcome.server_features.iter().any(|f| f == "chat_query_v1");
                    self.state.chat_query_results.clear();
                    self.state.watch_supported = welcome
                        .server_features
                        .iter()
                        .any(|f| f == "watch_rooms_v1");
                    self.state.room_social_supported = welcome.server_features.iter().any(|f| f == "room_social_v1");
                    self.state.room_images_supported = welcome.server_features.iter().any(|f| f == "room_images_v1");
                    self.state.invalidate_room_social();
                    self.state.screen_share_supported = welcome.server_features.iter().any(|f| f == "screen_share_v1");
                    self.state.screen_share_messages.clear();
                    self.state.watch_messages.clear();
                    self.last_archive_voice_report = None;
                    self.state.server_name = welcome.server_name;
                    self.state.connection = ConnectionState::Connected;
                    if self.state.local_mute_intent { let _ = self.dispatcher.send_control(ControlCommand::SetMute(true)); }
                    self.error = None;
                    self.voice_session = Some((
                        welcome.voice_addr,
                        welcome.session_id,
                        welcome.media_queue_size,
                    ));
                    let (adaptive_control, adaptive_events) = AdaptiveControl::new();
                    self.adaptive_control = Some(adaptive_control);
                    self.adaptive_events = Some(adaptive_events);
                    self.start_audio();
                    if let Some(avatar) = avatar_payload_for_server(&self.config.avatar) {
                        let _ = self
                            .dispatcher
                            .send_control(ControlCommand::SetAvatar(avatar));
                    }
                    tracing::info!(session_id = welcome.session_id, "session welcomed");
                }
                MessageType::GameCatalog => {
                    let catalog: crate::game_catalog::Catalog = message.decode()?;
                    if self.state.game_catalog_required && catalog.valid() {
                        if self.game_catalog_cache.len() >= 32
                            && !self.game_catalog_cache.contains_key(&self.game_catalog_key)
                        {
                            self.game_catalog_cache.clear();
                        }
                        self.game_catalog_cache
                            .insert(self.game_catalog_key.clone(), catalog.clone());
                        self.state.game_catalog = Some(catalog);
                    }
                }
                MessageType::ServerState => {
                    let snapshot: control::ServerState = message.decode()?;
                    if snapshot.game_catalog_version != self.state.game_catalog_version {
                        self.game_catalog_requested = None;
                    }
                    self.state.apply_snapshot(snapshot);
                    self.restore_member_volumes();
                    if let Some(c) = self.state.game_catalog.as_ref() {
                        if self.game_catalog_cache.len() >= 32
                            && !self.game_catalog_cache.contains_key(&self.game_catalog_key)
                        {
                            self.game_catalog_cache.clear();
                        }
                        self.game_catalog_cache
                            .insert(self.game_catalog_key.clone(), c.clone());
                    }
                    self.sync_channel_audio_quality();
                    self.request_missing_avatars();
                    if let Some(session_id) = self.state.self_session
                        && let Some(user) = self.state.users.get(&session_id)
                    {
                        let (muted, deafened, admin_muted) =
                            (user.muted, user.deafened, user.admin_muted);
                        apply_authoritative_self_state(
                            &mut self.state,
                            muted,
                            deafened,
                            admin_muted,
                        );
                        self.dispatcher.set_audio_muted(self.state.muted);
                        self.dispatcher.set_audio_deafened(deafened);
                    }
                    for (&session_id, user) in &self.state.users {
                        if self.state.self_session != Some(session_id) {
                            self.dispatcher.set_user_local_mute(
                                session_id,
                                effective_remote_mute(
                                    self.state.is_user_locally_muted(session_id),
                                    user.admin_muted,
                                ),
                            );
                        }
                    }
                }
                MessageType::UserJoined => {
                    let event: control::UserEvent = message.decode()?;
                    self.state.users.insert(event.user.session_id, event.user);
                    self.restore_member_volumes();
                    self.request_missing_avatars();
                }
                MessageType::UserLeft => {
                    let event: control::UserLeft = message.decode()?;
                    if let Some(transport) = &self.adaptive_control {
                        transport.remove_peer(event.session_id);
                    }
                    self.state.remove_user(event.session_id);
                }
                MessageType::UserMoved => {
                    let event: control::UserMoved = message.decode()?;
                    self.state.move_user(event.session_id, event.channel_id);
                    if self.state.self_session == Some(event.session_id) {
                        if let Some(transport) = &self.adaptive_control {
                            transport.clear_peers();
                        }
                        tracing::info!(channel_id = event.channel_id, "joined channel");
                        self.sync_channel_audio_quality();
                    } else {
                        let own_channel = self
                            .state
                            .self_session
                            .and_then(|id| self.state.users.get(&id))
                            .map_or(0, |user| user.channel_id);
                        if event.channel_id != own_channel
                            && let Some(transport) = &self.adaptive_control
                        {
                            transport.remove_peer(event.session_id);
                        }
                    }
                }
                MessageType::UserStateChanged => {
                    let event: control::UserStateChanged = message.decode()?;
                    if let Some(user) = self.state.users.get_mut(&event.session_id) {
                        user.game_activity = event.game_activity.clone();
                        user.muted = event.muted;
                        user.deafened = event.deafened;
                        user.admin_muted = event.admin_muted;
                        user.server_muted = event.server_muted;
                        user.channel_muted = event.channel_muted;
                    }
                    if self.state.self_session == Some(event.session_id) {
                        apply_authoritative_self_state(
                            &mut self.state,
                            event.muted,
                            event.deafened,
                            event.admin_muted,
                        );
                        self.dispatcher.set_audio_muted(self.state.muted);
                        self.dispatcher.set_audio_deafened(event.deafened);
                    } else {
                        self.dispatcher.set_user_local_mute(
                            event.session_id,
                            effective_remote_mute(
                                self.state.is_user_locally_muted(event.session_id),
                                event.admin_muted,
                            ),
                        );
                    }
                }
                MessageType::Pong => {
                    let _: control::Ping = message.decode()?;
                }
                MessageType::Error => {
                    let server_error: control::ServerError = message.decode()?;
                    tracing::warn!(code = %server_error.code, message = %server_error.message, "server rejected control request");
                    self.error = Some(control::server_error_message(&server_error));
                }
                MessageType::PeerCandidates => {
                    let peers: control::PeerCandidates = message.decode()?;
                    if let Some(transport) = &self.adaptive_control {
                        transport.set_peer_candidates(peers);
                    }
                }
                MessageType::RouteUpdate => {
                    let route: control::RouteUpdate = message.decode()?;
                    self.state.room_topology = route.topology.clone();
                    self.state.current_transport = route.transport.clone();
                    self.state.route_reason = route.reason.clone();
                    self.state.transport_mode = route.room_network_policy.clone();
                    self.state.qualification_duration_ms = route.qualification_ms;
                    self.state.migration_cooldown_ms = route.migration_cooldown_ms;
                    if let Some(transport) = &self.adaptive_control {
                        transport.set_route(route);
                    }
                }
                MessageType::AudioQoeFeedback => {
                    let feedback: control::AudioQoeFeedback = message.decode()?;
                    let now = Instant::now();
                    let observation = self.receiver_feedback.observe(
                        feedback.peer_id,
                        NetworkObservation {
                            observed_packets: feedback.observed_packets,
                            path_metrics_available: feedback.path_metrics_available.unwrap_or(true),
                            rtt_ms: feedback.rtt_ms,
                            jitter_ms: feedback.jitter_ms,
                            transport_loss_percent: feedback.transport_loss_percent,
                            late_loss_percent: feedback.late_loss_percent,
                            effective_audio_loss_percent: feedback.effective_audio_loss_percent,
                            burst_loss_percent: feedback.burst_loss_percent,
                            max_consecutive_loss: feedback.max_consecutive_loss,
                            packet_reorder_percent: feedback.packet_reorder_percent,
                            jitter_buffer_delay_ms: feedback.jitter_buffer_delay_ms,
                            jitter_buffer_underruns: feedback.jitter_buffer_underruns,
                            plc_frames: feedback.plc_frames,
                            fec_recovered_packets: feedback.fec_recovered_packets,
                            playout_resync_count: feedback.playout_resync_count,
                            route_healthy: true,
                            reconnect_events: 0,
                        },
                        now,
                    );
                    let assessment = self.network_quality.evaluate(observation, now);
                    self.state.network_quality = format!("{:?}", assessment.quality).to_lowercase();
                    self.state.network_trend = format!("{:?}", assessment.trend).to_lowercase();
                    self.dispatcher
                        .apply_audio_adaptation(self.audio_adaptation.decide(assessment.quality));
                }
                MessageType::Capabilities => {
                    let capabilities: control::Capabilities = message.decode()?;
                    self.state.server_role = capabilities.server_role;
                    self.state.capabilities = capabilities.actions;
                    self.state.network_policy_channel_ids = capabilities.network_policy_channel_ids;
                }
                MessageType::BanList => {
                    let list: control::BanList = message.decode()?;
                    apply_ban_list(&mut self.state, list);
                }
                MessageType::ChatHeads => {
                    #[derive(serde::Deserialize)] struct Heads { heads: std::collections::BTreeMap<u64,String> }
                    for (channel,id) in message.decode::<Heads>()?.heads {
                        if id.parse::<u64>().unwrap_or(0)>self.state.chat_heads.get(&channel).and_then(|known|known.parse::<u64>().ok()).unwrap_or(0) {self.state.chat_heads.insert(channel,id);}
                    }
                }
                MessageType::ChatQueryResult if self.state.chat_query_results.len() < 8 => {
                    self.state.chat_query_results.push(message.decode()?);
                }
                MessageType::ChatHistory => {
                    let history: control::ChatHistory = message.decode()?;
                    apply_chat_history(&mut self.state, history);
                    self.request_missing_avatars();
                }
                MessageType::TextMessage => {
                    let item: control::TextMessage = message.decode()?;
                    append_chat_message(&mut self.state, item);
                    self.request_missing_avatars();
                }
                MessageType::RoomArchive if self.state.archive_requested_channel.is_some() => {
                    let archive: control::RoomArchive = message.decode()?;
                    if self.state.receive_room_archive(archive) {
                        self.request_missing_avatars();
                    }
                }
                MessageType::RoomSocialEvent if self.state.room_social_supported && self.state.room_social_events.len() < 32 => {
                    let event: crate::protocol::room_social::Event = message.decode()?;
                    if event.is_bounded() && self.state.self_session.and_then(|id| self.state.users.get(&id)).is_some_and(|user| user.channel_id == event.channel_id) {
                        if self.pending_away_request.as_ref().is_some_and(|(id, _)| id == &event.request_id) {
                            if event.kind == "error" { self.state.self_away = self.pending_away_request.as_ref().unwrap().1; }
                            self.pending_away_request = None;
                        }
                        if event.kind == "status" {
                            self.state.self_away = event.away.iter().any(|away| Some(away.session_id) == self.state.self_session);
                        } else if event.kind == "away" && Some(event.session_id) == self.state.self_session {
                            self.state.self_away = event.active;
                        }
                        if self.state.self_away { self.state.muted_speech_notice = None; }
                        self.state.room_social_events.push(event);
                    }
                }
                MessageType::ScreenShareState if self.state.screen_share_supported && self.state.screen_share_messages.len() < 64 => {
                    self.state.screen_share_messages.push(message.decode()?);
                }
                MessageType::WatchState
                    if self.state.watch_supported && self.state.watch_messages.len() < 64 =>
                {
                    self.state.watch_messages.push(message.decode()?);
                }
                MessageType::AvatarData => {
                    let avatar: control::AvatarData = message.decode()?;
                    match crate::avatar::store_download(&avatar.avatar, &avatar.data) {
                        Ok(()) => {
                            self.pending_avatar_downloads.remove(&avatar.avatar);
                            self.state.avatar_cache_revision =
                                self.state.avatar_cache_revision.wrapping_add(1);
                        }
                        Err(error) => {
                            self.pending_avatar_downloads.remove(&avatar.avatar);
                            self.error = Some(format!("头像下载校验失败：{error}"));
                        }
                    }
                }
                _ => {}
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.error = Some(format!("protocol error: {error}"));
        }
    }

    fn request_missing_avatars(&mut self) {
        let missing = self
            .state
            .users
            .values()
            .map(|user| user.avatar.as_str())
            .chain(
                self.state
                    .chat_messages
                    .iter()
                    .map(|message| message.sender_avatar.as_str()),
            )
            .chain(
                self.state
                    .room_archive
                    .iter()
                    .flat_map(|archive| archive.participant_profiles.values())
                    .map(|profile| profile.avatar.as_str()),
            )
            .filter(|avatar| crate::avatar::is_custom_avatar(avatar))
            .filter(|avatar| !crate::avatar::has_cached(avatar))
            .filter(|avatar| !self.pending_avatar_downloads.contains(*avatar))
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        for avatar in missing {
            if self
                .dispatcher
                .send_control(ControlCommand::GetAvatar(avatar.clone()))
                .is_ok()
            {
                self.pending_avatar_downloads.insert(avatar);
            }
        }
    }

    fn apply_audio_event(&mut self, event: AudioEvent) {
        match event {
            AudioEvent::Ready => {
                self.state.audio_state = AudioState::Ready;
                self.state.rtt_ms = None;
                self.state.jitter_ms = 0.0;
                self.state.effective_audio_loss_percent = 0.0;
                self.state.jitter_buffer_delay_ms = 0.0;
                self.state.jitter_buffer_underruns = 0;
                self.state.plc_frames = 0;
                self.state.fec_recovered_packets = 0;
                for (&session_id, &volume) in &self.state.user_volumes {
                    self.dispatcher.set_user_volume(session_id, volume);
                }
                for (&session_id, &muted) in &self.state.locally_muted_users {
                    self.dispatcher.set_user_local_mute(session_id, muted);
                }
                self.dispatcher
                    .set_master_output_volume(self.config.output_volume);
                self.dispatcher
                    .set_push_to_talk_pressed(self.state.push_to_talk_pressed);
                tracing::info!("audio initialized");
            }
            AudioEvent::Stopped => {
                self.state.audio_timing = Default::default();
                self.state.audio_state = AudioState::Stopped;
                self.state.rtt_ms = None;
            }
            AudioEvent::MutedSpeech(captured_at) => {
                let now = Instant::now();
                if muted_speech_eligible(&self.state, self.config.input_mode)
                    && now.saturating_duration_since(captured_at) <= std::time::Duration::from_millis(500)
                    && self.last_muted_speech_notice.is_none_or(|last| now.saturating_duration_since(last) >= std::time::Duration::from_secs(30))
                {
                    self.last_muted_speech_notice = Some(now);
                    self.muted_speech_sequence = self.muted_speech_sequence.wrapping_add(1);
                    self.state.muted_speech_notice = Some(crate::audio::MutedSpeechNotice {
                        sequence: self.muted_speech_sequence.to_string(),
                        at_unix_ms: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
                    });
                }
            }
            AudioEvent::InputLevel(level) => self.state.input_level = level,
            AudioEvent::InputPeak(level) => self.state.input_peak = level,
            AudioEvent::OutputLevel(level) => self.state.output_level = level,
            AudioEvent::Talking(session_id) => {
                self.state.talking.insert(session_id, Instant::now());
                if Some(session_id) == self.state.self_session
                    && self.state.archive_supported
                    && self
                        .last_archive_voice_report
                        .is_none_or(|last| last.elapsed() >= std::time::Duration::from_secs(1))
                {
                    self.last_archive_voice_report = Some(Instant::now());
                    let _ = self.dispatcher.send_control(ControlCommand::VoiceActivity);
                }
            }
            AudioEvent::Stats {
                sent,
                received,
                lost,
                reordered,
                jitter_depth,
                opus_bitrate_bps,
            } => {
                self.state.packets_sent = sent;
                self.state.packets_received = received;
                self.state.packets_lost = lost;
                self.state.packets_reordered = reordered;
                self.state.jitter_depth = jitter_depth;
                self.state.opus_bitrate_bps = opus_bitrate_bps;
            }
            AudioEvent::QoeFeedback(feedback) => {
                apply_qoe_diagnostics(&mut self.state, &feedback);
                let _ = self
                    .dispatcher
                    .send_control(ControlCommand::AudioQoeFeedback(feedback));
            }
            AudioEvent::VoicePathMetrics(metrics) => {
                apply_voice_path_diagnostics(&mut self.state, metrics);
            }
            AudioEvent::Timing(timing) => {
                self.state.audio_timing = timing;
            }
            AudioEvent::QueueStats {
                capture_overruns,
                playback_underruns,
            } => {
                self.state.capture_overruns = capture_overruns;
                self.state.playback_underruns = playback_underruns;
            }
            AudioEvent::DspStatus {
                mode,
                echo_cancellation,
                automatic_gain,
                average_ms,
                peak_ms,
                deadline_misses,
            } => {
                self.state.active_noise_suppression = mode;
                self.state.active_echo_cancellation = echo_cancellation;
                self.state.active_automatic_gain = automatic_gain;
                self.state.dsp_average_ms = average_ms;
                self.state.dsp_peak_ms = peak_ms;
                self.state.dsp_deadline_misses = deadline_misses;
            }
            AudioEvent::DspError(error) => {
                tracing::warn!(error = %error, "DSP error");
                self.state.dsp_error = Some(error);
            }
            AudioEvent::DspLoading(mode) => {
                self.state.dsp_loading = mode;
            }
            AudioEvent::DspPerformanceWarning(warning) => {
                self.state.dsp_performance_warning = warning;
            }
            AudioEvent::MicTestStatus(status) => {
                self.state.mic_test_status = status;
            }
            AudioEvent::DeviceStatus {
                input,
                output,
                input_sample_rate,
                output_sample_rate,
                fallback,
            } => {
                self.devices = enumerate_audio_devices();
                self.state.active_input_device = Some(input);
                self.state.active_output_device = Some(output);
                self.state.active_input_sample_rate = Some(input_sample_rate);
                self.state.active_output_sample_rate = Some(output_sample_rate);
                self.state.device_fallback_active = fallback;
                self.state.device_error = None;
            }
            AudioEvent::DeviceError {
                direction: _,
                message,
            } => {
                tracing::warn!(error = %message, "audio device error");
                self.state.device_error = Some(message);
            }
            AudioEvent::Error(error) => {
                tracing::warn!(error = %error, "audio error");
                self.active_audio_session = ActiveAudioSession::None;
                self.state.audio_state = AudioState::Error(error.clone());
                self.error = Some(error);
            }
        }
    }

    fn start_audio(&mut self) {
        let Some((voice_address, session_id, media_queue_size)) = self.voice_session.clone() else {
            return;
        };
        self.dispatcher.set_audio_muted(self.state.muted);
        self.dispatcher.set_audio_deafened(self.state.deafened);
        self.state.audio_state = AudioState::Starting;
        self.active_audio_session = ActiveAudioSession::Remote;
        self.dispatcher.start_audio(
            self.audio_settings(AudioTransportSettings::Adaptive {
                control_address: self
                    .control_endpoint
                    .map(|endpoint| endpoint.to_string())
                    .unwrap_or_else(|| self.config.last_server.clone()),
                voice_address,
                session_id,
                control: self
                    .adaptive_control
                    .clone()
                    .expect("adaptive control initialized"),
                events: self
                    .adaptive_events
                    .clone()
                    .expect("adaptive event channel initialized"),
                media_queue_size,
                security: self.media_security.clone(),
            }),
        );
    }

    fn start_local_audio(&mut self) {
        self.state.audio_state = AudioState::Starting;
        self.active_audio_session = ActiveAudioSession::Local;
        self.dispatcher
            .start_audio(self.audio_settings(AudioTransportSettings::Local));
    }

    fn audio_settings(&self, transport: AudioTransportSettings) -> AudioSettings {
        AudioSettings {
            transport,
            input_device: self.config.input_device.clone(),
            output_device: self.config.output_device.clone(),
            input_gain: self.config.input_gain,
            output_volume: self.config.output_volume,
            input_mode: self.config.input_mode,
            echo_cancellation: self.config.echo_cancellation,
            automatic_gain: self.config.automatic_gain,
            noise_suppression_mode: self.config.noise_suppression_mode,
            noise_suppression_strength: self.config.noise_suppression_strength,
        }
    }

    fn current_audio_processing_settings(&self) -> AudioProcessingSettings {
        AudioProcessingSettings {
            input_gain: self.config.input_gain,
            input_mode: self.config.input_mode,
            echo_cancellation: self.config.echo_cancellation,
            automatic_gain: self.config.automatic_gain,
            noise_suppression_mode: self.config.noise_suppression_mode,
            noise_suppression_strength: self.config.noise_suppression_strength,
        }
    }

    fn sync_channel_audio_quality(&mut self) {
        let policy = self
            .state
            .self_session
            .and_then(|session_id| self.state.users.get(&session_id))
            .and_then(|user| {
                self.state
                    .channels
                    .iter()
                    .find(|channel| channel.id == user.channel_id)
            })
            .map(|channel| audio_quality_policy_from_wire(&channel.audio_quality_policy))
            .unwrap_or(self.config.audio_quality_policy);
        self.state.audio_quality_mode = audio_quality_label(policy);
        self.audio_adaptation.set_policy(policy);
        let assessment = self.network_quality.evaluate(
            NetworkObservation {
                observed_packets: Some(0),
                ..Default::default()
            },
            Instant::now(),
        );
        self.dispatcher
            .apply_audio_adaptation(self.audio_adaptation.decide(assessment.quality));
    }

    fn stop_remote_session(&mut self) {
        self.control_endpoint = None;
        self.receiver_feedback = ReceiverFeedbackWindow::default();
        self.network_quality = NetworkQualityEngine::new(Instant::now());
        self.active_audio_session = ActiveAudioSession::None;
        self.dispatcher.stop_audio();
        clear_remote_session_handles(
            &mut self.voice_session,
            &mut self.adaptive_control,
            &mut self.adaptive_events,
            &mut self.media_security,
        );
    }
}

impl Default for Application {
    fn default() -> Self {
        Self::new()
    }
}

fn enumerate_audio_devices() -> DeviceList {
    let backend = CpalAudioBackend;
    DeviceList {
        inputs: backend.list_input_devices(),
        outputs: backend.list_output_devices(),
    }
}

fn register_global_ptt(
    input_mode: InputMode,
    ptt_key: &str,
) -> (Option<GlobalPttHotkey>, Option<String>) {
    if input_mode != InputMode::PushToTalk {
        return (None, None);
    }
    match GlobalPttHotkey::register(ptt_key) {
        Ok(hotkey) => (Some(hotkey), None),
        Err(error) => (None, Some(error)),
    }
}

fn room_policy_wire(policy: RoomNetworkPolicy) -> &'static str {
    match policy {
        RoomNetworkPolicy::Auto => "auto",
        RoomNetworkPolicy::ForceSfu => "force_sfu",
        RoomNetworkPolicy::ForceP2p => "force_p2p",
    }
}

fn audio_quality_label(policy: AudioQualityPolicy) -> String {
    match policy {
        AudioQualityPolicy::Auto => "auto".into(),
        AudioQualityPolicy::Fixed { target_bitrate_bps } => {
            format!("fixed_{}k", target_bitrate_bps / 1_000)
        }
    }
}

fn audio_quality_policy_from_wire(policy: &str) -> AudioQualityPolicy {
    const QUALITY_BITRATES_KBPS: [i32; 10] = [16, 24, 32, 40, 48, 56, 64, 72, 80, 96];
    if let Some(level) = policy
        .strip_prefix("quality_")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|level| (1..=QUALITY_BITRATES_KBPS.len()).contains(level))
    {
        return AudioQualityPolicy::Fixed {
            target_bitrate_bps: QUALITY_BITRATES_KBPS[level - 1] * 1_000,
        };
    }
    policy
        .strip_prefix("fixed_")
        .and_then(|value| value.strip_suffix('k'))
        .and_then(|value| value.parse::<i32>().ok())
        .filter(|kbps| matches!(kbps, 32 | 48 | 64 | 96))
        .map_or(AudioQualityPolicy::Auto, |kbps| AudioQualityPolicy::Fixed {
            target_bitrate_bps: kbps * 1_000,
        })
}

fn avatar_payload_for_server(avatar: &str) -> Option<String> {
    if avatar.is_empty() {
        None
    } else if crate::avatar::is_custom_avatar(avatar) {
        crate::avatar::data_for_reference(avatar).ok()
    } else {
        Some(avatar.to_owned())
    }
}

fn control_command(command: &AppCommand) -> Option<ControlCommand> {
    match command {
        AppCommand::Disconnect => Some(ControlCommand::Disconnect),
        AppCommand::JoinChannel(channel_id) => Some(ControlCommand::JoinChannel(*channel_id)),
        AppCommand::LeaveChannel => Some(ControlCommand::LeaveChannel),
        AppCommand::SetMuted(muted) => Some(ControlCommand::SetMute(*muted)),
        AppCommand::SetDeafened(deafened) => Some(ControlCommand::SetDeafen(*deafened)),
        AppCommand::SetDisplayName(display_name) => {
            Some(ControlCommand::SetDisplayName(display_name.clone()))
        }
        AppCommand::SetAvatar(avatar) => Some(ControlCommand::SetAvatar(avatar.clone())),
        AppCommand::SendTextMessage(text) => Some(ControlCommand::SendTextMessage(text.clone())),
        AppCommand::ScreenShare(request) => Some(ControlCommand::ScreenShare(request.clone())),
        AppCommand::Watch(request) => Some(ControlCommand::Watch(request.clone())),
        AppCommand::RequestRoomArchive { channel_id, all } => {
            Some(ControlCommand::RequestRoomArchive {
                channel_id: *channel_id,
                all: *all,
            })
        }
        AppCommand::Connect { .. }
        | AppCommand::RetryAudio
        | AppCommand::CloseRoomArchive
        | AppCommand::UpdateAudioPreferences(_)
        | AppCommand::RefreshDevices
        | AppCommand::SetShareGameActivity(_)
        | AppCommand::SetTheme(_)
        | AppCommand::SetLanguage(_)
        | AppCommand::SetMiniOverlay { .. }
        | AppCommand::SetMiniOverlayPosition { .. }
        | AppCommand::SetAutoConnectLastServer(_)
        | AppCommand::SetAutoConnect { .. }
        | AppCommand::SetTransportMode(_)
        | AppCommand::SetRoomNetworkPolicy(_)
        | AppCommand::SetChannelNetworkPolicy { .. }
        | AppCommand::SetAudioQualityPolicy(_)
        | AppCommand::SetUserVolume { .. }
        | AppCommand::SetUserLocalMute { .. }
        | AppCommand::AdminMute { .. }
        | AppCommand::KickChannel(_)
        | AppCommand::KickServer(_)
        | AppCommand::BanUser { .. }
        | AppCommand::ListBans { .. }
        | AppCommand::Unban(_)
        | AppCommand::MoveUser { .. }
        | AppCommand::SetChannelAdmin { .. }
        | AppCommand::SetServerAdmin { .. }
        | AppCommand::DeleteChannel(_)
        | AppCommand::JoinProtectedChannel {..}
        | AppCommand::SetChannelPassword {..}
        | AppCommand::CreateProtectedChannel{..}
        | AppCommand::CreateChannel(_)
        | AppCommand::EditChannel(_)
        | AppCommand::ProvisionMaster(_)
        | AppCommand::TrustServer { .. }
        | AppCommand::SetMasterOutputVolume(_)
        | AppCommand::SetPushToTalkPressed(_)
        | AppCommand::UpdateShortcuts(_)
        | AppCommand::PlayOutputTest
        | AppCommand::StartMicTest
        | AppCommand::PlayMicTestOriginal
        | AppCommand::PlayMicTestProcessed
        | AppCommand::StopMicTest
        | AppCommand::PrepareLocalAudio
        | AppCommand::StopLocalAudio
        | AppCommand::Shutdown => None,
    }
}

fn validate_chat_message(connection: &ConnectionState, text: &str) -> Result<(), String> {
    if *connection != ConnectionState::Connected {
        Err("连接尚未恢复，消息未发送，请稍后重试".into())
    } else if text.is_empty() || text.len() > 2048 {
        Err("message must be 1-2048 bytes".into())
    } else {
        Ok(())
    }
}

fn muted_speech_eligible(state: &AppState, input_mode: InputMode) -> bool {
    state.connection == ConnectionState::Connected
        && state.audio_state == AudioState::Ready
        && state.self_session.and_then(|id| state.users.get(&id)).is_some_and(|user| user.channel_id != 0)
        && state.local_mute_intent && state.muted && !state.admin_muted && !state.deafened && !state.self_away
        && state.mic_test_status == MicTestStatus::Idle
        && (input_mode != InputMode::PushToTalk || state.push_to_talk_pressed)
}

fn apply_local_muted_state(state: &mut AppState, muted: bool) {
    state.local_mute_intent = muted;
    if !muted {
        state.self_away = false;
        state.muted_speech_notice = None;
        state.deafened = false;
    }
    state.muted = muted || state.admin_muted;
}

fn apply_authoritative_self_state(
    state: &mut AppState,
    muted: bool,
    deafened: bool,
    admin_muted: bool,
) {
    if !muted && !admin_muted { state.self_away = false; state.muted_speech_notice = None; }
    state.muted = muted || admin_muted || state.local_mute_intent;
    state.deafened = deafened;
    state.admin_muted = admin_muted;
}

fn effective_remote_mute(locally_muted: bool, admin_muted: bool) -> bool {
    locally_muted || admin_muted
}

fn channel_admin_target(state: &AppState, session_id: u64) -> Result<(u64, String), String> {
    let user = state
        .users
        .get(&session_id)
        .ok_or_else(|| "unknown user".to_owned())?;
    if user.channel_id == 0 {
        return Err("user is not currently in a channel".to_owned());
    }
    Ok((user.channel_id, user.client_id.clone()))
}

fn apply_local_deafened_state(state: &mut AppState, deafened: bool) {
    state.deafened = deafened;
    if deafened {
        state.local_mute_intent = true;
        state.muted = true;
    }
}

fn apply_ban_list(state: &mut AppState, list: control::BanList) {
    state.bans = list.bans;
    state.ban_list_offset = list.offset;
    state.ban_list_total = list.total;
}

fn apply_chat_history(state: &mut AppState, history: control::ChatHistory) {
    state
        .chat_messages
        .retain(|item| item.channel_id != history.channel_id);
    state.chat_messages.extend(history.messages);
    trim_chat_messages(state);
}

fn trim_chat_messages(state: &mut AppState) {
    state.chat_messages.sort_by_key(|item| item.id);
    state.chat_messages.dedup_by_key(|item| item.id);
    state.chat_messages.reverse();
    let mut counts = std::collections::HashMap::<u64, usize>::new();
    state.chat_messages.retain(|item| {
        let count = counts.entry(item.channel_id).or_default();
        *count += 1;
        *count <= 100
    });
    state.chat_messages.truncate(500);
    state.chat_messages.reverse();
}

fn append_chat_message(state: &mut AppState, item: control::TextMessage) {
    if state.chat_messages.iter().any(|known| known.id == item.id) {
        return;
    }
    state.chat_messages.push(item);
    trim_chat_messages(state);
}

fn clear_pending_server_trust(state: &mut AppState) {
    state.trust_endpoint.clear();
    state.trust_fingerprint.clear();
}

fn clear_remote_session_handles(
    voice_session: &mut Option<(String, u64, usize)>,
    adaptive_control: &mut Option<AdaptiveControl>,
    adaptive_events: &mut Option<crossbeam_channel::Sender<TransportEvent>>,
    media_security: &mut Option<MediaSecurity>,
) {
    *voice_session = None;
    *adaptive_control = None;
    *adaptive_events = None;
    *media_security = None;
}

fn control_error_message(error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    if error.contains("Server Identity Changed") {
        "服务器身份已变化，已阻止连接。请联系服务器管理员确认新指纹。".into()
    } else if lower.contains("server trust confirmation timed out") {
        "服务器指纹确认已超时，请重新连接后确认。".into()
    } else if lower.contains("connection refused") {
        "无法连接服务器，请确认地址、端口和防火墙设置。".into()
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "连接服务器超时，请检查网络和端口。".into()
    } else if lower.contains("tls 1.3") {
        "无法建立 TLS 1.3 安全连接，请确认服务器在线且版本为 v0.5。".into()
    } else if lower.contains("identity") || lower.contains("fingerprint") {
        "无法验证服务器或客户端身份，请确认服务器地址和身份文件。".into()
    } else {
        error.to_owned()
    }
}

fn clear_operation_error_for_user_action(connection: &ConnectionState, error: &mut Option<String>) {
    if matches!(connection, ConnectionState::Connected) {
        *error = None;
    }
}

fn apply_qoe_diagnostics(
    state: &mut AppState,
    feedback: &crate::protocol::control::AudioQoeFeedback,
) {
    if feedback.path_metrics_available.unwrap_or(true) {
        state.jitter_ms = feedback.jitter_ms.max(0.0);
    }
    state.effective_audio_loss_percent = feedback.effective_audio_loss_percent.clamp(0.0, 100.0);
    state.jitter_buffer_delay_ms = feedback.jitter_buffer_delay_ms.max(0.0);
    state.jitter_buffer_underruns = state
        .jitter_buffer_underruns
        .saturating_add(feedback.jitter_buffer_underruns);
    state.plc_frames = state.plc_frames.saturating_add(feedback.plc_frames);
    state.fec_recovered_packets = state
        .fec_recovered_packets
        .saturating_add(feedback.fec_recovered_packets);
}

fn apply_voice_path_diagnostics(
    state: &mut AppState,
    metrics: Option<crate::transport::VoicePathMetrics>,
) {
    state.rtt_ms = metrics.map(|metrics| metrics.rtt_ms.round().max(0.0) as u64);
    if let Some(metrics) = metrics {
        state.jitter_ms = metrics.jitter_ms.max(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_requires_a_live_connection_and_counts_utf8_bytes() {
        for connection in [ConnectionState::Disconnected, ConnectionState::Connecting, ConnectionState::Reconnecting, ConnectionState::Error("offline".into())] {
            assert!(validate_chat_message(&connection, "保留草稿").is_err());
        }
        assert!(validate_chat_message(&ConnectionState::Connected, &"汉".repeat(682)).is_ok());
        assert!(validate_chat_message(&ConnectionState::Connected, &"汉".repeat(683)).is_err());
        assert!(validate_chat_message(&ConnectionState::Connected, &"a".repeat(2048)).is_ok());
        assert!(validate_chat_message(&ConnectionState::Connected, "").is_err());
    }

    #[test]
    fn archive_response_is_consumed_once_and_close_rejects_late_responses() {
        let response = || control::RoomArchive {
            stats: control::ArchiveRoomStats {
                room_id: 7,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut state = AppState::default();
        assert!(!state.receive_room_archive(response()));
        state.archive_requested_channel = Some(7);
        assert!(state.receive_room_archive(response()));
        assert!(state.take_room_archive().is_some());
        assert!(state.take_room_archive().is_none());
        assert!(state.receive_room_archive(response()));
        state.close_room_archive();
        assert!(state.take_room_archive().is_none());
        assert!(!state.receive_room_archive(response()));
        state.archive_requested_channel = Some(8);
        assert!(!state.receive_room_archive(response()));
        assert!(state.take_room_archive().is_none());
        assert!(control_command(&AppCommand::CloseRoomArchive).is_none());
    }

    #[test]
    fn startup_auto_connect_requires_a_connection_recorded_as_successful_by_this_version() {
        let mut config = ClientConfig::default();
        config.last_server = "legacy.example:7001".into();
        config.nickname = "Legacy User".into();
        assert!(auto_connect_target(&config).is_none());

        config.record_successful_connection(
            "voice.example:7001".into(),
            "Alice".into(),
            String::new(),
            false,
        );
        let target = auto_connect_target(&config).unwrap();
        assert_eq!(target.address, "voice.example:7001");
        assert_eq!(target.nickname, "Alice");

        config.remember_server("games.example:7001".into(), "Bob".into(), "Games".into());
        config.auto_connect_server = "games.example:7001".into();
        let selected = auto_connect_target(&config).unwrap();
        assert_eq!(selected.address, "games.example:7001");
        assert_eq!(selected.nickname, "Bob");

        config.auto_connect_server = "missing.example:7001".into();
        assert!(auto_connect_target(&config).is_none());

        config.auto_connect_last_server = false;
        assert!(auto_connect_target(&config).is_none());
    }

    #[test]
    fn qoe_diagnostics_keep_latest_quality_and_accumulate_recovery_counters() {
        let mut state = AppState::default();
        let feedback = crate::protocol::control::AudioQoeFeedback {
            jitter_ms: 7.5,
            effective_audio_loss_percent: 2.5,
            jitter_buffer_delay_ms: 60.0,
            jitter_buffer_underruns: 2,
            plc_frames: 3,
            fec_recovered_packets: 4,
            ..Default::default()
        };
        apply_qoe_diagnostics(&mut state, &feedback);
        apply_qoe_diagnostics(&mut state, &feedback);
        assert_eq!(state.jitter_ms, 7.5);
        assert_eq!(state.effective_audio_loss_percent, 2.5);
        assert_eq!(state.jitter_buffer_delay_ms, 60.0);
        assert_eq!(state.jitter_buffer_underruns, 4);
        assert_eq!(state.plc_frames, 6);
        assert_eq!(state.fec_recovered_packets, 8);
    }

    #[test]
    fn unknown_qoe_probe_metrics_do_not_overwrite_measured_diagnostics() {
        let mut state = AppState {
            jitter_ms: 12.0,
            ..Default::default()
        };
        apply_qoe_diagnostics(
            &mut state,
            &control::AudioQoeFeedback {
                path_metrics_available: Some(false),
                plc_frames: 2,
                ..Default::default()
            },
        );
        assert_eq!(state.jitter_ms, 12.0);
        assert_eq!(state.plc_frames, 2);
    }

    #[test]
    fn voice_path_diagnostics_include_probe_rtt_and_jitter_without_remote_audio() {
        let mut state = AppState::default();
        apply_voice_path_diagnostics(
            &mut state,
            Some(crate::transport::VoicePathMetrics {
                rtt_ms: 133.6,
                jitter_ms: 8.25,
            }),
        );

        assert_eq!(state.rtt_ms, Some(134));
        assert_eq!(state.jitter_ms, 8.25);
    }

    #[test]
    fn control_failures_are_actionable_without_losing_diagnostics() {
        assert_eq!(
            control_error_message("Server Identity Changed"),
            "服务器身份已变化，已阻止连接。请联系服务器管理员确认新指纹。"
        );
        assert_eq!(
            control_error_message("TLS 1.3 control connection failed: connection refused"),
            "无法连接服务器，请确认地址、端口和防火墙设置。"
        );
        assert_eq!(
            control_error_message("server trust confirmation timed out"),
            "服务器指纹确认已超时，请重新连接后确认。"
        );
        assert_eq!(
            control_error_message("custom diagnostic"),
            "custom diagnostic"
        );
    }

    #[test]
    fn terminal_handshake_outcomes_remove_the_trust_overlay_state() {
        let mut state = AppState {
            trust_endpoint: "voice.example:7001".into(),
            trust_fingerprint: "fingerprint".into(),
            ..AppState::default()
        };
        clear_pending_server_trust(&mut state);
        assert!(state.trust_endpoint.is_empty());
        assert!(state.trust_fingerprint.is_empty());
    }

    #[test]
    fn next_connected_user_action_clears_the_previous_operation_error() {
        let mut error = Some("没有权限执行此操作。".into());
        clear_operation_error_for_user_action(&ConnectionState::Connected, &mut error);
        assert!(error.is_none());

        let mut fatal_error = Some("服务器身份已变化".into());
        clear_operation_error_for_user_action(
            &ConnectionState::Error("服务器身份已变化".into()),
            &mut fatal_error,
        );
        assert!(fatal_error.is_some());
    }

    #[test]
    fn disconnect_releases_all_remote_session_handles_and_media_keys() {
        let (adaptive, adaptive_events) = AdaptiveControl::new();
        let ephemeral = crate::security::EphemeralKey::generate();
        let security = MediaSecurity::new(crate::security::SessionSecrets::default(), &ephemeral);
        let outbound = security.outbound.clone();
        let mut voice_session = Some(("127.0.0.1:7002".into(), 7, 256));
        let mut adaptive_control = Some(adaptive);
        let mut adaptive_events = Some(adaptive_events);
        let mut media_security = Some(security);

        clear_remote_session_handles(
            &mut voice_session,
            &mut adaptive_control,
            &mut adaptive_events,
            &mut media_security,
        );

        assert!(voice_session.is_none());
        assert!(adaptive_control.is_none());
        assert!(adaptive_events.is_none());
        assert!(media_security.is_none());
        assert_eq!(std::sync::Arc::strong_count(&outbound), 1);
    }

    #[test]
    fn audio_preferences_have_safe_gain_bounds() {
        let mut config = ClientConfig::default();
        let preferences = AudioPreferences {
            input_device: Some("Input".into()),
            output_device: Some("Output".into()),
            input_gain: 50.0,
            output_volume: -2.0,
            input_mode: InputMode::VoiceActivity,
            echo_cancellation: true,
            automatic_gain: true,
            noise_suppression_mode: NoiseSuppressionMode::Standard,
            noise_suppression_strength: NoiseSuppressionStrength::Medium,
        };
        config.input_device = preferences.input_device;
        config.output_device = preferences.output_device;
        config.input_gain = preferences.input_gain.clamp(0.0, 3.0);
        config.output_volume = preferences.output_volume.clamp(0.01, 3.0);
        assert_eq!(config.input_gain, 3.0);
        assert_eq!(config.output_volume, 0.01);
    }

    #[test]
    fn local_mute_and_deafen_state_updates_optimistically() {
        let mut state = AppState::default();
        apply_local_muted_state(&mut state, true);
        assert!(state.muted);

        apply_local_muted_state(&mut state, false);
        apply_local_deafened_state(&mut state, true);
        assert!(state.deafened);
        assert!(state.muted, "deafening also mutes the microphone");

        apply_local_deafened_state(&mut state, false);
        assert!(!state.deafened);
        assert!(state.muted, "undeafening must not implicitly unmute");

        apply_local_deafened_state(&mut state, true);
        apply_local_muted_state(&mut state, false);
        assert!(
            !state.deafened,
            "opening the microphone also restores audio"
        );
        assert!(!state.muted);
    }

    #[test]
    fn muted_speech_requires_intent_and_connected_room_without_suppression() {
        let mut state = AppState {
            connection: ConnectionState::Connected,
            audio_state: AudioState::Ready,
            self_session: Some(7),
            muted: true,
            local_mute_intent: true,
            ..AppState::default()
        };
        state.users.insert(7, control::User {session_id:7,channel_id:3,..Default::default()});
        assert!(muted_speech_eligible(&state, InputMode::VoiceActivity));
        assert!(!muted_speech_eligible(&state, InputMode::PushToTalk));
        state.push_to_talk_pressed = true;
        assert!(muted_speech_eligible(&state, InputMode::PushToTalk));
        for blocked in ["away", "admin", "deafened", "server_only", "unmuted", "lobby", "disconnected", "mic_test"] {
            state.self_away = blocked == "away";
            state.admin_muted = blocked == "admin";
            state.deafened = blocked == "deafened";
            state.local_mute_intent = blocked != "server_only";
            state.muted = blocked != "unmuted";
            state.users.get_mut(&7).unwrap().channel_id = if blocked == "lobby" {0} else {3};
            state.connection = if blocked == "disconnected" {ConnectionState::Disconnected} else {ConnectionState::Connected};
            state.mic_test_status = if blocked == "mic_test" {MicTestStatus::Recording} else {MicTestStatus::Idle};
            assert!(!muted_speech_eligible(&state, InputMode::VoiceActivity), "{blocked}");
        }
    }

    #[test]
    fn away_mute_survives_server_snapshot_until_explicit_unmute() {
        let mut state = AppState::default();
        apply_local_muted_state(&mut state, true);
        state.self_away = true;
        apply_authoritative_self_state(&mut state, false, false, false);
        assert!(state.muted, "rejoin or stale snapshot must not open the microphone");
        state.self_away = true;
        apply_local_muted_state(&mut state, false);
        assert!(!state.self_away);
        assert!(!state.muted);
        assert!(!state.local_mute_intent);
    }

    #[test]
    fn administrative_mute_cannot_be_optimistically_overridden() {
        let mut state = AppState::default();
        apply_authoritative_self_state(&mut state, false, false, true);

        apply_local_muted_state(&mut state, false);
        assert!(state.muted, "local unmute must not reopen capture");
        assert!(state.admin_muted);

        apply_authoritative_self_state(&mut state, false, false, false);
        assert!(!state.muted);
        assert!(!state.admin_muted);
    }

    #[test]
    fn remote_administrative_mute_preserves_local_mute_preference() {
        assert!(effective_remote_mute(false, true));
        assert!(effective_remote_mute(true, false));
        assert!(!effective_remote_mute(false, false));
    }

    #[test]
    fn app_commands_map_to_control_commands() {
        assert_eq!(
            control_command(&AppCommand::JoinChannel(7)),
            Some(ControlCommand::JoinChannel(7))
        );
        assert_eq!(
            control_command(&AppCommand::SetMuted(true)),
            Some(ControlCommand::SetMute(true))
        );
        assert_eq!(
            control_command(&AppCommand::SetDeafened(false)),
            Some(ControlCommand::SetDeafen(false))
        );
        assert_eq!(
            control_command(&AppCommand::SetDisplayName("Renamed".into())),
            Some(ControlCommand::SetDisplayName("Renamed".into()))
        );
        assert_eq!(
            control_command(&AppCommand::UpdateAudioPreferences(AudioPreferences {
                input_device: None,
                output_device: None,
                input_gain: 1.0,
                output_volume: 0.8,
                input_mode: InputMode::VoiceActivity,
                echo_cancellation: true,
                automatic_gain: true,
                noise_suppression_mode: NoiseSuppressionMode::Standard,
                noise_suppression_strength: NoiseSuppressionStrength::Medium,
            })),
            None
        );
    }

    #[test]
    fn channel_admin_scope_comes_from_the_target_user_channel() {
        let mut state = AppState {
            self_session: Some(1),
            ..Default::default()
        };
        state.users.insert(
            1,
            control::User {
                session_id: 1,
                client_id: "operator".into(),
                channel_id: 7,
                ..Default::default()
            },
        );
        state.users.insert(
            2,
            control::User {
                session_id: 2,
                client_id: "target".into(),
                channel_id: 9,
                ..Default::default()
            },
        );

        assert_eq!(channel_admin_target(&state, 2), Ok((9, "target".into())));
        assert_eq!(channel_admin_target(&state, 99), Err("unknown user".into()));

        state.users.get_mut(&2).unwrap().channel_id = 0;
        assert_eq!(
            channel_admin_target(&state, 2),
            Err("user is not currently in a channel".into())
        );
    }

    #[test]
    fn ban_list_messages_update_administration_state() {
        let mut state = AppState::default();
        apply_ban_list(
            &mut state,
            control::BanList {
                bans: vec![control::BanRecord {
                    client_id: "client-42".into(),
                    display_name: "Blocked user".into(),
                    reason: "abuse".into(),
                    created_at: "2026-08-25T00:00:00Z".into(),
                    expires_at: None,
                    operator_client_id: "admin".into(),
                }],
                offset: 8,
                total: 9,
            },
        );
        assert_eq!(state.ban_list_offset, 8);
        assert_eq!(state.ban_list_total, 9);
        assert_eq!(state.bans[0].client_id, "client-42");
    }

    #[test]
    fn chat_history_replaces_only_its_channel_and_live_messages_deduplicate() {
        let message = |id, channel_id| control::TextMessage {
            id,
            channel_id,
            sender_session_id: 7,
            sender_client_id: "client".into(),
            sender_name: "Alice".into(),
            sender_avatar: "L".into(),
            text: format!("message-{id}"),
            sent_at_unix_ms: 1,
            image: None,
        };
        let mut state = AppState {
            chat_messages: vec![message(1, 1), message(2, 2)],
            ..Default::default()
        };
        apply_chat_history(
            &mut state,
            control::ChatHistory {
                channel_id: 1,
                messages: vec![message(3, 1)],
            },
        );
        assert_eq!(
            state
                .chat_messages
                .iter()
                .map(|item| item.id)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        append_chat_message(&mut state, message(3, 1));
        append_chat_message(&mut state, message(4, 1));
        assert_eq!(
            state
                .chat_messages
                .iter()
                .map(|item| item.id)
                .collect::<Vec<_>>(),
            vec![2, 3, 4]
        );
    }

    #[test]
    fn chat_cache_keeps_only_latest_hundred_per_room_for_history_and_live_messages() {
        let message = |id, channel_id| control::TextMessage {
            id,
            channel_id,
            sender_session_id: 1,
            sender_client_id: "stable".into(),
            sender_name: "Alice".into(),
            sender_avatar: String::new(),
            text: id.to_string(),
            sent_at_unix_ms: 1,
            image: None,
        };
        let mut state = AppState::default();
        apply_chat_history(
            &mut state,
            control::ChatHistory {
                channel_id: 1,
                messages: (1..=150).rev().map(|id| message(id, 1)).collect(),
            },
        );
        assert_eq!(state.chat_messages.len(), 100);
        assert_eq!(state.chat_messages[0].id, 51);
        for id in 151..=260 {
            append_chat_message(&mut state, message(id, 2));
        }
        append_chat_message(&mut state, message(261, 1));
        assert_eq!(
            state
                .chat_messages
                .iter()
                .filter(|m| m.channel_id == 1)
                .count(),
            100
        );
        assert_eq!(
            state
                .chat_messages
                .iter()
                .filter(|m| m.channel_id == 2)
                .count(),
            100
        );
        assert_eq!(state.chat_messages[0].id, 52);
        assert_eq!(state.chat_messages.last().unwrap().id, 261);
        append_chat_message(&mut state, message(1, 1));
        assert_eq!(state.chat_messages[0].id, 52);
    }

    #[test]
    fn room_audio_quality_wire_values_map_to_adaptation_policies() {
        assert_eq!(
            audio_quality_policy_from_wire("quality_1"),
            AudioQualityPolicy::Fixed {
                target_bitrate_bps: 16_000
            }
        );
        assert_eq!(
            audio_quality_policy_from_wire("quality_6"),
            AudioQualityPolicy::Fixed {
                target_bitrate_bps: 56_000
            }
        );
        assert_eq!(
            audio_quality_policy_from_wire("quality_10"),
            AudioQualityPolicy::Fixed {
                target_bitrate_bps: 96_000
            }
        );
        assert_eq!(
            audio_quality_policy_from_wire("fixed_96k"),
            AudioQualityPolicy::Fixed {
                target_bitrate_bps: 96_000
            }
        );
        assert_eq!(
            audio_quality_policy_from_wire("fixed_32k"),
            AudioQualityPolicy::Fixed {
                target_bitrate_bps: 32_000
            }
        );
        assert_eq!(
            audio_quality_policy_from_wire("quality_0"),
            AudioQualityPolicy::Auto
        );
        assert_eq!(
            audio_quality_policy_from_wire("quality_11"),
            AudioQualityPolicy::Auto
        );
        assert_eq!(
            audio_quality_policy_from_wire("unsupported"),
            AudioQualityPolicy::Auto
        );
    }

    #[test]
    fn reconnect_avatar_payload_preserves_builtin_and_rejects_missing_custom_cache() {
        assert_eq!(avatar_payload_for_server("L"), Some("L".into()));
        assert_eq!(avatar_payload_for_server(""), None);
        assert_eq!(
            avatar_payload_for_server(&format!("custom:{}", "0".repeat(64))),
            None
        );
    }
}
