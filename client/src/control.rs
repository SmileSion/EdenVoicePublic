use std::{
    io::{BufRead, BufReader, ErrorKind, Read, Write},
    net::{Shutdown, TcpStream, ToSocketAddrs},
    thread,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
use crossbeam_channel::{Receiver, Sender, bounded};
#[cfg(not(test))]
use sha2::{Digest, Sha256};

use crate::protocol::control::{
    AudioQoeFeedback, AvatarRequest, BanListRequest, CandidateRegistration, ChannelMutationRequest,
    ChannelRequest, ChannelRoleRequest, DisplayNameRequest, Envelope, Hello, MediaRekey,
    MessageType, NetworkReport, Ping, ProvisionMasterRequest, RoomArchiveRequest,
    RoomNetworkPolicyRequest, SendTextMessageRequest, ServerRoleRequest, StateRequest,
    TargetRequest, TransportModeRequest,
};
#[cfg(not(test))]
use crate::protocol::control::{IdentityChallenge, IdentityResponse};
use crate::security::{self, EphemeralKey, Identity};

const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(20);
const CONTROL_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

const MAX_INITIAL_CONNECTION_ATTEMPTS: usize = 3;

#[derive(Clone, Default, PartialEq)]
pub struct ServerPassword(zeroize::Zeroizing<String>);
impl From<String> for ServerPassword { fn from(value:String)->Self { Self(zeroize::Zeroizing::new(value)) } }
impl std::fmt::Debug for ServerPassword { fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result { f.write_str("[REDACTED]") } }
impl ServerPassword { fn as_str(&self)->&str { &self.0 } }

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Connect {
        server_password: ServerPassword,
        address: String,
        nickname: String,
    },
    Disconnect,
    JoinChannel(u64),
    JoinProtectedChannel { channel_id:u64, password:String },
    SetChannelPassword { channel_id:u64, password:String },
    LeaveChannel,
    SetMute(bool),
    SetDeafen(bool),
    SetDisplayName(String),
    SetAvatar(String),
    SetGameActivity(String),
    GetGameCatalog,
    GetAvatar(String),
    SendTextMessage(String),
    QueryChat(crate::protocol::control::ChatQuery),
    RoomSocial(crate::protocol::room_social::Request),
    ScreenShare(crate::protocol::screenshare::Request),
    Watch(crate::protocol::watch::WatchRequest),
    RequestRoomArchive {
        channel_id: u64,
        all: bool,
    },
    VoiceActivity,
    RegisterCandidates(CandidateRegistration),
    NetworkReport(NetworkReport),
    SetTransportMode(String),
    SetRoomNetworkPolicy {
        policy: String,
        channel_id: Option<u64>,
    },
    AudioQoeFeedback(AudioQoeFeedback),
    ProvisionMaster(String),
    CreateChannel(crate::protocol::control::Channel),
    CreateProtectedChannel {channel:crate::protocol::control::Channel,password:String},
    EditChannel(crate::protocol::control::Channel),
    DeleteChannel(u64),
    SetChannelAdmin {
        channel_id: u64,
        client_id: String,
        enabled: bool,
    },
    SetServerRole {
        client_id: String,
        role: String,
    },
    AdminMute {
        session_id: u64,
        client_id: String,
        enabled: bool,
    },
    MoveUser {
        session_id: u64,
        client_id: String,
        channel_id: u64,
    },
    KickChannel {
        session_id: u64,
        client_id: String,
    },
    KickServer {
        session_id: u64,
        client_id: String,
    },
    Ban {
        client_id: String,
        reason: String,
    },
    Unban(String),
    ListBans {
        offset: usize,
        limit: usize,
    },
    TrustServer {
        endpoint: String,
        fingerprint: String,
    },
    RequestMediaRekey,
    Shutdown,
}

#[derive(Debug)]
pub enum Event {
    Connecting,
    Reconnecting {
        delay_seconds: u64,
    },
    Disconnected,
    SecurityEstablished(security::MediaSecurity),
    ServerTrustRequired {
        endpoint: String,
        fingerprint: String,
    },
    MediaRekeyed(u16),
    ControlRtt(u64),
    ConnectedEndpoint(std::net::SocketAddr),
    Message(Envelope),
    Error(String),
}

pub struct ControlHandle {
    pub commands: Sender<Command>,
    pub events: Receiver<Event>,
}

impl ControlHandle {
    pub fn spawn() -> Self {
        // Keep UI stalls or a noisy peer from growing control memory without bound.
        // The worker deliberately applies backpressure because control messages are
        // ordered state transitions and cannot be silently coalesced here.
        let (command_tx, command_rx) = bounded(256);
        let (event_tx, event_rx) = bounded(512);
        let worker_events = event_tx.clone();
        if let Err(error) = thread::Builder::new()
            .name("control-client".into())
            .spawn(move || worker(command_rx, worker_events))
        {
            let _ = event_tx.send(Event::Error(format!(
                "control thread creation failed: {error}"
            )));
        }
        Self {
            commands: command_tx,
            events: event_rx,
        }
    }
}

fn worker(commands: Receiver<Command>, events: Sender<Event>) {
    while let Ok(command) = commands.recv() {
        match command {
            Command::Connect { address, nickname, server_password } => {
                tracing::info!(server = %address, "connecting");
                connection_loop(&commands, &events, address, nickname, server_password)
            }
            Command::Shutdown => return,
            _ => {}
        }
    }
}

fn connection_loop(
    commands: &Receiver<Command>,
    events: &Sender<Event>,
    address: String,
    nickname: String,
    server_password: ServerPassword,
) {
    let backoff = [1, 2, 5, 10];
    let mut attempt = 0usize;
    let mut ever_connected = false;
    let mut desired = DesiredState::default();
    loop {
        let _ = events.send(if attempt == 0 {
            Event::Connecting
        } else {
            Event::Reconnecting {
                delay_seconds: backoff[(attempt - 1).min(backoff.len() - 1)],
            }
        });
        if attempt > 0 {
            let delay = Duration::from_secs(backoff[(attempt - 1).min(backoff.len() - 1)]);
            match wait_for_retry(commands, delay, &mut desired) {
                RetryWait::Elapsed => {}
                RetryWait::Disconnect => {
                    let _ = events.send(Event::Disconnected);
                    return;
                }
                RetryWait::Shutdown => return,
            }
        }
        match run_session(commands, events, &address, &nickname, &mut desired, server_password.as_str()) {
            SessionEnd::UserDisconnect => {
                let _ = events.send(Event::Disconnected);
                return;
            }
            SessionEnd::Shutdown => return,
            SessionEnd::Failed(error) => {
                tracing::warn!(error = %error, attempt, "reconnecting");
                let _ = events.send(Event::Error(error));
                attempt += 1;
                if initial_connection_exhausted(attempt, ever_connected) {
                    tracing::warn!(attempts = attempt, "initial connection attempts exhausted");
                    return;
                }
            }
            SessionEnd::ConnectionLost(error) => {
                tracing::warn!(error = %error, "connection lost");
                let _ = events.send(Event::Error(error));
                ever_connected = true;
                attempt = 1;
            }
            SessionEnd::Terminal(error) => {
                tracing::info!(reason = %error, "server ended the session without automatic reconnect");
                let _ = events.send(Event::Error(error));
                return;
            }
        }
    }
}

fn initial_connection_exhausted(attempts: usize, ever_connected: bool) -> bool {
    !ever_connected && attempts >= MAX_INITIAL_CONNECTION_ATTEMPTS
}

#[derive(Debug, Eq, PartialEq)]
enum SessionEnd {
    UserDisconnect,
    Shutdown,
    Failed(String),
    ConnectionLost(String),
    Terminal(String),
}

#[derive(Debug, Eq, PartialEq)]
enum HandshakeFailure {
    Retryable(String),
    Terminal(String),
    UserDisconnect,
    Shutdown,
}

fn handshake_failure_session_end(failure: HandshakeFailure) -> SessionEnd {
    match failure {
        HandshakeFailure::Retryable(error) => SessionEnd::Failed(error),
        HandshakeFailure::Terminal(error) => SessionEnd::Terminal(error),
        HandshakeFailure::UserDisconnect => SessionEnd::UserDisconnect,
        HandshakeFailure::Shutdown => SessionEnd::Shutdown,
    }
}

fn handshake_server_failure(error: crate::protocol::control::ServerError) -> HandshakeFailure {
    let terminal = matches!(
        error.code.as_str(),
        "banned"
            | "secure_mode_required"
            | "identity_authentication_failed"
            | "identity_admission_failed"
            | "invalid_hello"
            | "invalid_nickname"
            | "server_password_required" | "server_password_invalid" | "server_password_rate_limited"
    );
    let message = crate::protocol::control::server_error_message(&error);
    if terminal {
        HandshakeFailure::Terminal(message)
    } else {
        HandshakeFailure::Retryable(message)
    }
}

#[allow(dead_code)]
enum ControlStream {
    Plain(TcpStream),
    Secure(Box<security::SecureStream>),
}

impl ControlStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        match self {
            Self::Plain(stream) => stream.set_read_timeout(timeout),
            Self::Secure(stream) => stream.sock.set_read_timeout(timeout),
        }
    }

    fn shutdown(&self) {
        let stream = match self {
            Self::Plain(stream) => stream,
            Self::Secure(stream) => &stream.sock,
        };
        let _ = stream.shutdown(Shutdown::Both);
    }
}

impl Read for ControlStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buffer),
            Self::Secure(stream) => stream.read(buffer),
        }
    }
}

impl Write for ControlStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(buffer),
            Self::Secure(stream) => stream.write(buffer),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            Self::Secure(stream) => stream.flush(),
        }
    }
}

#[derive(Default)]
struct DesiredState {
    channel: Option<u64>,
    channel_passwords: std::collections::HashMap<u64,String>,
    muted: bool,
    deafened: bool,
    display_name: Option<String>,
    avatar: Option<String>,
    room_network_policy: Option<String>,
}

fn run_session(
    commands: &Receiver<Command>,
    events: &Sender<Event>,
    address: &str,
    nickname: &str,
    desired: &mut DesiredState,
    server_password: &str,
) -> SessionEnd {
    run_session_with_timeout(
        commands,
        events,
        address,
        nickname,
        desired,
        server_password,
        HEARTBEAT_TIMEOUT,
    )
}

fn run_session_with_timeout(
    commands: &Receiver<Command>,
    events: &Sender<Event>,
    address: &str,
    nickname: &str,
    desired: &mut DesiredState,
    server_password: &str,
    heartbeat_timeout: Duration,
) -> SessionEnd {
    #[cfg(test)]
    let _ = server_password;
    let addresses = match address.to_socket_addrs() {
        Ok(values) => values,
        Err(error) => return SessionEnd::Failed(format!("invalid server address: {error}")),
    };
    let mut connected = None;
    let mut last_error = None;
    for endpoint in addresses {
        match TcpStream::connect_timeout(&endpoint, Duration::from_secs(5)) {
            Ok(stream) => {
                connected = Some(stream);
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    let stream = match connected {
        Some(stream) => stream,
        None => {
            return SessionEnd::Failed(format!(
                "connection failed: {}",
                last_error
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "address did not resolve".into())
            ));
        }
    };
    if let Ok(endpoint) = stream.peer_addr() {
        let _ = events.send(Event::ConnectedEndpoint(endpoint));
    }
    tracing::info!(server = address, "control connected");
    if let Err(error) = stream.set_read_timeout(Some(Duration::from_secs(5))) {
        return SessionEnd::Failed(error.to_string());
    }
    if let Err(error) = stream.set_write_timeout(Some(CONTROL_WRITE_TIMEOUT)) {
        return SessionEnd::Failed(error.to_string());
    }
    let identity = match Identity::load_or_create() {
        Ok(value) => value,
        Err(error) => return SessionEnd::Failed(format!("client identity failed: {error}")),
    };
    let ephemeral = EphemeralKey::generate();
    let client_id = identity.client_id();
    let client_public = STANDARD_NO_PAD.encode(identity.public_key());
    let client_ephemeral = STANDARD_NO_PAD.encode(ephemeral.public);
    #[cfg(test)]
    let control_stream = ControlStream::Plain(stream);
    #[cfg(not(test))]
    let control_stream = {
        let host = address
            .rsplit_once(':')
            .map(|(host, _)| host.trim_matches(['[', ']']))
            .unwrap_or(address);
        match security::tls_connect(stream, host) {
            Ok(value) => ControlStream::Secure(Box::new(value)),
            Err(error) => {
                return SessionEnd::Failed(format!("TLS 1.3 control connection failed: {error}"));
            }
        }
    };
    let mut reader = BufReader::new(control_stream);
    let hello = match Envelope::new(
        MessageType::Hello,
        Hello {
            client_features: vec!["chat_query_v1"],
            client_version: env!("CARGO_PKG_VERSION"),
            client_platform: std::env::consts::OS,
            nickname,
            protocol_version: security::PROTOCOL_VERSION,
            security_capabilities: security::SECURITY_CAPABILITIES.to_vec(),
            client_id: &client_id,
            client_public_key: &client_public,
            client_ephemeral_key: &client_ephemeral,
        },
    ) {
        Ok(value) => value,
        Err(error) => return SessionEnd::Failed(error.to_string()),
    };
    if let Err(error) = send(reader.get_mut(), &hello) {
        return SessionEnd::Failed(format!("HELLO failed: {error}"));
    }
    #[cfg(not(test))]
    let media_security = match complete_identity_handshake(
        &mut reader,
        address,
        &identity,
        &ephemeral,
        server_password,
        commands,
        events,
    ) {
        Ok(media_security) => {
            let _ = events.send(Event::SecurityEstablished(media_security.clone()));
            Some(media_security)
        }
        Err(error) => return handshake_failure_session_end(error),
    };
    #[cfg(test)]
    let media_security: Option<security::MediaSecurity> = None;
    let mut line = Vec::with_capacity(4096);
    if let Err(error) = reader
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(100)))
    {
        return SessionEnd::Failed(error.to_string());
    }
    let mut last_ping = Instant::now();
    let mut next_ping_nonce = 0_u64;
    let mut pending_ping = None::<(u64, Instant)>;
    let mut established = false;
    let mut game_activity_supported = false;
    let mut game_catalog_supported = false;
    loop {
        if pending_ping.is_some_and(|(_, sent_at)| sent_at.elapsed() >= heartbeat_timeout) {
            reader.get_ref().shutdown();
            return session_failure(established, "control heartbeat timed out".into());
        }
        // Revisit heartbeat/read deadlines even under sustained command traffic.
        for command in commands.try_iter().take(64) {
            let result = match command {
                Command::Disconnect => {
                    reader.get_ref().shutdown();
                    return SessionEnd::UserDisconnect;
                }
                Command::Shutdown => return SessionEnd::Shutdown,
                Command::JoinProtectedChannel {channel_id,password} => {
                    desired.channel=Some(channel_id);
                    desired.channel_passwords.insert(channel_id,password.clone());
                    Envelope::new(MessageType::JoinChannel, serde_json::json!({"channel_id":channel_id,"password":password}))
                }
                Command::SetChannelPassword {channel_id,password} => Envelope::new(MessageType::SetChannelPassword,serde_json::json!({"channel_id":channel_id,"password":password})),
                Command::JoinChannel(channel_id) => {
                    desired.channel = Some(channel_id);
                    Envelope::new(MessageType::JoinChannel, ChannelRequest { channel_id })
                }
                Command::LeaveChannel => {
                    desired.channel = None;
                    Ok(Envelope {
                        kind: MessageType::LeaveChannel,
                        data: Default::default(),
                    })
                }
                Command::SetMute(enabled) => {
                    desired.muted = enabled;
                    Envelope::new(MessageType::SetMute, StateRequest { enabled })
                }
                Command::SetDeafen(enabled) => {
                    desired.deafened = enabled;
                    if enabled {
                        desired.muted = true;
                    }
                    Envelope::new(MessageType::SetDeafen, StateRequest { enabled })
                }
                Command::SetDisplayName(display_name) => {
                    desired.display_name = Some(display_name.clone());
                    Envelope::new(
                        MessageType::SetDisplayName,
                        DisplayNameRequest { display_name },
                    )
                }
                Command::GetGameCatalog => {
                    if !game_catalog_supported {
                        continue;
                    }
                    Envelope::new(MessageType::GetGameCatalog, serde_json::json!({}))
                }
                Command::SetGameActivity(game_id) => {
                    // Do not replay a previous connection's activity into an old server.
                    if !game_activity_supported {
                        continue;
                    }
                    Envelope::new(
                        MessageType::SetGameActivity,
                        crate::protocol::control::GameActivityRequest { game_id },
                    )
                }
                Command::SetAvatar(avatar) => {
                    desired.avatar = Some(avatar.clone());
                    Envelope::new(MessageType::SetAvatar, AvatarRequest { avatar })
                }
                Command::GetAvatar(avatar) => {
                    Envelope::new(MessageType::GetAvatar, AvatarRequest { avatar })
                }
                Command::QueryChat(request) => Envelope::new(MessageType::QueryChat, request),
                Command::SendTextMessage(text) => Envelope::new(
                    MessageType::SendTextMessage,
                    SendTextMessageRequest { text },
                ),
                Command::RoomSocial(request) => Envelope::new(MessageType::RoomSocialRequest, request),
                Command::ScreenShare(request) => Envelope::new(MessageType::ScreenShareRequest, request),
                Command::Watch(request) => Envelope::new(MessageType::WatchRequest, request),
                Command::RequestRoomArchive { channel_id, all } => Envelope::new(
                    MessageType::GetRoomArchive,
                    RoomArchiveRequest {
                        channel_id,
                        all,
                        limit: 100,
                    },
                ),
                Command::VoiceActivity => Ok(Envelope {
                    kind: MessageType::VoiceActivity,
                    data: Default::default(),
                }),
                Command::RegisterCandidates(value) => {
                    Envelope::new(MessageType::RegisterCandidates, value)
                }
                Command::NetworkReport(value) => Envelope::new(MessageType::NetworkReport, value),
                Command::SetTransportMode(mode) => {
                    Envelope::new(MessageType::SetTransportMode, TransportModeRequest { mode })
                }
                Command::SetRoomNetworkPolicy { policy, channel_id } => {
                    if channel_id.is_none() {
                        desired.room_network_policy = Some(policy.clone());
                    }
                    Envelope::new(
                        MessageType::SetRoomNetworkPolicy,
                        RoomNetworkPolicyRequest { policy, channel_id },
                    )
                }
                Command::AudioQoeFeedback(feedback) => {
                    Envelope::new(MessageType::AudioQoeFeedback, feedback)
                }
                Command::ProvisionMaster(token) => Envelope::new(
                    MessageType::ProvisionMaster,
                    ProvisionMasterRequest { token },
                ),
                Command::CreateProtectedChannel{channel,password} => Envelope::new(MessageType::CreateChannel,serde_json::json!({"channel":channel,"password":password})),
                Command::CreateChannel(channel) => Envelope::new(
                    MessageType::CreateChannel,
                    ChannelMutationRequest { channel },
                ),
                Command::EditChannel(channel) => {
                    Envelope::new(MessageType::EditChannel, ChannelMutationRequest { channel })
                }
                Command::DeleteChannel(channel_id) => {
                    Envelope::new(MessageType::DeleteChannel, ChannelRequest { channel_id })
                }
                Command::SetChannelAdmin {
                    channel_id,
                    client_id,
                    enabled,
                } => Envelope::new(
                    MessageType::SetChannelAdmin,
                    ChannelRoleRequest {
                        channel_id,
                        client_id,
                        enabled,
                    },
                ),
                Command::SetServerRole { client_id, role } => Envelope::new(
                    MessageType::SetServerRole,
                    ServerRoleRequest { client_id, role },
                ),
                Command::AdminMute {
                    session_id,
                    client_id,
                    enabled,
                } => Envelope::new(
                    MessageType::AdminMute,
                    TargetRequest {
                        client_id,
                        session_id: Some(session_id),
                        channel_id: None,
                        enabled: Some(enabled),
                        reason: None,
                    },
                ),
                Command::MoveUser {
                    session_id,
                    client_id,
                    channel_id,
                } => Envelope::new(
                    MessageType::MoveUser,
                    TargetRequest {
                        client_id,
                        session_id: Some(session_id),
                        channel_id: Some(channel_id),
                        enabled: None,
                        reason: None,
                    },
                ),
                Command::KickChannel {
                    session_id,
                    client_id,
                } => Envelope::new(
                    MessageType::KickChannel,
                    TargetRequest {
                        client_id,
                        session_id: Some(session_id),
                        channel_id: None,
                        enabled: None,
                        reason: None,
                    },
                ),
                Command::KickServer {
                    session_id,
                    client_id,
                } => Envelope::new(
                    MessageType::KickServer,
                    TargetRequest {
                        client_id,
                        session_id: Some(session_id),
                        channel_id: None,
                        enabled: None,
                        reason: None,
                    },
                ),
                Command::Ban { client_id, reason } => Envelope::new(
                    MessageType::Ban,
                    TargetRequest {
                        client_id,
                        session_id: None,
                        channel_id: None,
                        enabled: None,
                        reason: Some(reason),
                    },
                ),
                Command::Unban(client_id) => Envelope::new(
                    MessageType::Unban,
                    TargetRequest {
                        client_id,
                        session_id: None,
                        channel_id: None,
                        enabled: None,
                        reason: None,
                    },
                ),
                Command::ListBans { offset, limit } => {
                    Envelope::new(MessageType::ListBans, BanListRequest { offset, limit })
                }
                Command::TrustServer { .. } => continue,
                Command::RequestMediaRekey => Ok(Envelope {
                    kind: MessageType::RequestMediaRekey,
                    data: Default::default(),
                }),
                Command::Connect { .. } => continue,
            };
            match result.and_then(|message| send(reader.get_mut(), &message)) {
                Ok(()) => {}
                Err(error) => {
                    return session_failure(established, format!("control write failed: {error}"));
                }
            }
        }
        if pending_ping.is_none() && last_ping.elapsed() >= Duration::from_secs(10) {
            next_ping_nonce = next_ping_nonce.wrapping_add(1);
            let nonce = next_ping_nonce;
            let message = match Envelope::new(MessageType::Ping, Ping { nonce }) {
                Ok(value) => value,
                Err(error) => return session_failure(established, error.to_string()),
            };
            if let Err(error) = send(reader.get_mut(), &message) {
                return session_failure(established, error.to_string());
            }
            pending_ping = Some((nonce, Instant::now()));
            last_ping = Instant::now();
        }
        match read_limited_line(&mut reader, &mut line) {
            Ok(0) => return session_failure(established, "server closed the connection".into()),
            Ok(_) => match serde_json::from_slice::<Envelope>(&line) {
                Ok(message) => {
                    if let Some(error) = terminal_server_error(&message) {
                        return SessionEnd::Terminal(error);
                    }
                    if message.kind == MessageType::Pong {
                        let pong: Ping = match message.decode() {
                            Ok(value) => value,
                            Err(error) => {
                                return session_failure(established, error.to_string());
                            }
                        };
                        if pending_ping.is_some_and(|(nonce, _)| nonce == pong.nonce)
                            && let Some((_, sent_at)) = pending_ping.take()
                        {
                            let _ = events.send(Event::ControlRtt(
                                sent_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                            ));
                        }
                        continue;
                    }
                    if message.kind == MessageType::MediaRekeyPrepare {
                        let rekey: MediaRekey = match message.decode() {
                            Ok(value) => value,
                            Err(error) => return session_failure(established, error.to_string()),
                        };
                        let salt = match STANDARD_NO_PAD.decode(&rekey.salt) {
                            Ok(value) => value,
                            Err(_) => {
                                return session_failure(
                                    established,
                                    "invalid media rekey salt".into(),
                                );
                            }
                        };
                        let Some(security) = media_security.as_ref() else {
                            return session_failure(
                                established,
                                "media security is unavailable".into(),
                            );
                        };
                        if let Err(error) = security.prepare_rekey(&salt, rekey.epoch) {
                            return session_failure(established, error.to_string());
                        }
                        if let Ok(ack) = Envelope::new(
                            MessageType::MediaRekeyAck,
                            MediaRekey {
                                epoch: rekey.epoch,
                                salt: String::new(),
                            },
                        ) {
                            let _ = send(reader.get_mut(), &ack);
                        }
                        continue;
                    }
                    if message.kind == MessageType::MediaRekeyCommit {
                        let rekey: MediaRekey = match message.decode() {
                            Ok(value) => value,
                            Err(error) => return session_failure(established, error.to_string()),
                        };
                        let Some(security) = media_security.as_ref() else {
                            return session_failure(
                                established,
                                "media security is unavailable".into(),
                            );
                        };
                        if let Err(error) = security.activate_rekey(rekey.epoch) {
                            return session_failure(established, error.to_string());
                        }
                        let _ = events.send(Event::MediaRekeyed(rekey.epoch));
                        continue;
                    }
                    if message.kind == MessageType::Welcome {
                        game_catalog_supported = message
                            .data
                            .get("server_features")
                            .and_then(|value| value.as_array())
                            .is_some_and(|features| {
                                features
                                    .iter()
                                    .any(|feature| feature.as_str() == Some("game_catalog_v1"))
                            });
                        game_activity_supported = message
                            .data
                            .get("server_features")
                            .and_then(|value| value.as_array())
                            .is_some_and(|features| {
                                features
                                    .iter()
                                    .any(|feature| feature.as_str() == Some("game_activity_v1"))
                            });
                        established = true;
                        if let Some(channel_id) = desired.channel
                            && let Ok(join) = Envelope::new(
                                MessageType::JoinChannel,
                                serde_json::json!({"channel_id":channel_id,"password":desired.channel_passwords.get(&channel_id)}),
                            )
                        {
                            let _ = send(reader.get_mut(), &join);
                        }
                        if desired.muted
                            && let Ok(state) =
                                Envelope::new(MessageType::SetMute, StateRequest { enabled: true })
                        {
                            let _ = send(reader.get_mut(), &state);
                        }
                        if desired.deafened
                            && let Ok(state) = Envelope::new(
                                MessageType::SetDeafen,
                                StateRequest { enabled: true },
                            )
                        {
                            let _ = send(reader.get_mut(), &state);
                        }
                        if let Some(display_name) = desired.display_name.as_ref()
                            && let Ok(message) = Envelope::new(
                                MessageType::SetDisplayName,
                                DisplayNameRequest {
                                    display_name: display_name.clone(),
                                },
                            )
                        {
                            let _ = send(reader.get_mut(), &message);
                        }
                        if let Some(avatar) = desired.avatar.as_ref()
                            && let Ok(message) = Envelope::new(
                                MessageType::SetAvatar,
                                AvatarRequest {
                                    avatar: avatar.clone(),
                                },
                            )
                        {
                            let _ = send(reader.get_mut(), &message);
                        }
                        if let Some(policy) = desired.room_network_policy.as_ref()
                            && let Ok(message) = Envelope::new(
                                MessageType::SetRoomNetworkPolicy,
                                RoomNetworkPolicyRequest {
                                    policy: policy.clone(),
                                    channel_id: None,
                                },
                            )
                        {
                            let _ = send(reader.get_mut(), &message);
                        }
                        // Populate RTT immediately after the handshake and desired-state
                        // restoration. Sending Ping before Welcome would reorder the
                        // reconnect protocol ahead of JoinChannel/SetMute restoration.
                        last_ping = Instant::now() - Duration::from_secs(10);
                    }
                    let _ = events.send(Event::Message(message));
                }
                Err(error) => {
                    return session_failure(
                        established,
                        format!("invalid server message: {error}"),
                    );
                }
            },
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => {
                return session_failure(established, format!("control read failed: {error}"));
            }
        }
    }
}

fn session_failure(established: bool, error: String) -> SessionEnd {
    if established {
        SessionEnd::ConnectionLost(error)
    } else {
        SessionEnd::Failed(error)
    }
}

fn terminal_server_error(message: &Envelope) -> Option<String> {
    if message.kind != MessageType::Error {
        return None;
    }
    let error: crate::protocol::control::ServerError = message.decode().ok()?;
    matches!(error.code.as_str(), "banned" | "kicked_from_server" | "server_password_required" | "server_password_invalid" | "server_password_rate_limited")
        .then(|| crate::protocol::control::server_error_message(&error))
}

fn send<W: Write>(stream: &mut W, envelope: &Envelope) -> serde_json::Result<()> {
    serde_json::to_writer(&mut *stream, envelope)?;
    stream.write_all(b"\n").map_err(serde_json::Error::io)
}

#[cfg(not(test))]
fn complete_identity_handshake(
    reader: &mut BufReader<ControlStream>,
    endpoint: &str,
    identity: &Identity,
    ephemeral: &EphemeralKey,
    server_password: &str,
    commands: &Receiver<Command>,
    events: &Sender<Event>,
) -> Result<security::MediaSecurity, HandshakeFailure> {
    let mut line = Vec::with_capacity(4096);
    read_limited_line(reader, &mut line).map_err(|error| {
        HandshakeFailure::Retryable(format!("identity challenge read failed: {error}"))
    })?;
    let envelope: Envelope = serde_json::from_slice(&line).map_err(|error| {
        HandshakeFailure::Terminal(format!("invalid identity challenge: {error}"))
    })?;
    if envelope.kind == MessageType::Error {
        let server_error: crate::protocol::control::ServerError =
            envelope.decode().map_err(|error| {
                HandshakeFailure::Terminal(format!("invalid server error response: {error}"))
            })?;
        return Err(handshake_server_failure(server_error));
    }
    if envelope.kind != MessageType::IdentityChallenge {
        return Err(HandshakeFailure::Terminal(
            "server did not provide an identity challenge".into(),
        ));
    }
    let challenge: IdentityChallenge = envelope.decode().map_err(|error| {
        HandshakeFailure::Terminal(format!("invalid identity challenge: {error}"))
    })?;
    for required in security::SECURITY_CAPABILITIES {
        if !challenge
            .required_capabilities
            .iter()
            .any(|value| value == required)
        {
            return Err(HandshakeFailure::Terminal(format!(
                "server omitted required security capability {required}"
            )));
        }
    }
    let challenge_bytes = STANDARD_NO_PAD
        .decode(&challenge.challenge)
        .map_err(|_| HandshakeFailure::Terminal("invalid challenge encoding".into()))?;
    let server_public = STANDARD_NO_PAD
        .decode(&challenge.server_public_key)
        .map_err(|_| HandshakeFailure::Terminal("invalid server identity encoding".into()))?;
    let server_ephemeral: [u8; 32] = STANDARD_NO_PAD
        .decode(&challenge.server_ephemeral_key)
        .map_err(|_| HandshakeFailure::Terminal("invalid server ephemeral key encoding".into()))?
        .try_into()
        .map_err(|_| HandshakeFailure::Terminal("invalid server ephemeral key".into()))?;
    let calculated_fingerprint =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(&server_public));
    if calculated_fingerprint != challenge.server_fingerprint {
        return Err(HandshakeFailure::Terminal(
            "server identity fingerprint does not match its public key".into(),
        ));
    }
    let tls_public = match reader.get_ref() {
        ControlStream::Secure(stream) => security::peer_identity_public_key(stream)
            .map_err(|error| HandshakeFailure::Terminal(error.to_string()))?,
        ControlStream::Plain(_) => {
            return Err(HandshakeFailure::Terminal(
                "secure mode requires TLS".into(),
            ));
        }
    };
    if tls_public != server_public {
        return Err(HandshakeFailure::Terminal(
            "TLS certificate is not bound to the EdenVoice server identity".into(),
        ));
    }
    match security::server_trust_status(endpoint, &challenge.server_fingerprint)
        .map_err(|error| HandshakeFailure::Terminal(error.to_string()))?
    {
        security::TrustStatus::Trusted => {}
        security::TrustStatus::Unknown => {
            let _ = events.send(Event::ServerTrustRequired {
                endpoint: endpoint.to_owned(),
                fingerprint: challenge.server_fingerprint.clone(),
            });
            loop {
                match commands.recv_timeout(Duration::from_secs(60)) {
                    Ok(Command::TrustServer {
                        endpoint: approved_endpoint,
                        fingerprint,
                    }) if approved_endpoint == endpoint
                        && fingerprint == challenge.server_fingerprint =>
                    {
                        security::record_server_trust(endpoint, &fingerprint)
                            .map_err(|error| HandshakeFailure::Terminal(error.to_string()))?;
                        break;
                    }
                    Ok(Command::Disconnect) => return Err(HandshakeFailure::UserDisconnect),
                    Ok(Command::Shutdown) => return Err(HandshakeFailure::Shutdown),
                    Ok(_) => {}
                    Err(_) => {
                        return Err(HandshakeFailure::Terminal(
                            "server trust confirmation timed out".into(),
                        ));
                    }
                }
            }
        }
    }
    let transcript = security::auth_transcript(
        &challenge_bytes,
        &server_public,
        &ephemeral.public,
        &server_ephemeral,
    );
    let signature = STANDARD_NO_PAD.encode(identity.sign(&transcript));
    let secrets = ephemeral
        .derive(server_ephemeral, &transcript)
        .map_err(|error| HandshakeFailure::Terminal(error.to_string()))?;
    let response = Envelope::new(
        MessageType::IdentityResponse,
        IdentityResponse { signature, server_password: if challenge.server_password_required { server_password.to_owned() } else { String::new() } },
    )
    .map_err(|error| HandshakeFailure::Terminal(error.to_string()))?;
    send(reader.get_mut(), &response).map_err(|error| {
        HandshakeFailure::Retryable(format!("identity response failed: {error}"))
    })?;
    Ok(security::MediaSecurity::new(secrets, ephemeral))
}

fn read_limited_line<R: BufRead>(reader: &mut R, output: &mut Vec<u8>) -> std::io::Result<usize> {
    const MAX: usize = 64 * 1024;
    // A timeout may leave a partial message; discard only a delivered line.
    if output.last() == Some(&b'\n') {
        output.clear();
    }
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if output.is_empty() {
                Ok(0)
            } else {
                Err(std::io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "incomplete control message",
                ))
            };
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if output.len() + take > MAX {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "control message exceeds 64 KiB",
            ));
        }
        output.extend_from_slice(&available[..take]);
        reader.consume(take);
        if output.last() == Some(&b'\n') {
            return Ok(output.len());
        }
    }
}
enum RetryWait {
    Elapsed,
    Disconnect,
    Shutdown,
}
fn wait_for_retry(
    commands: &Receiver<Command>,
    delay: Duration,
    desired: &mut DesiredState,
) -> RetryWait {
    let deadline = Instant::now() + delay;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return RetryWait::Elapsed;
        }
        match commands.recv_timeout(remaining) {
            Ok(Command::Disconnect) => return RetryWait::Disconnect,
            Ok(Command::Shutdown) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return RetryWait::Shutdown;
            }
            Ok(Command::JoinProtectedChannel {channel_id,password}) => {desired.channel=Some(channel_id);desired.channel_passwords.insert(channel_id,password);},
            Ok(Command::SetChannelPassword {..}) => {},
            Ok(Command::JoinChannel(channel)) => desired.channel = Some(channel),
            Ok(Command::LeaveChannel) => desired.channel = None,
            Ok(Command::SetMute(value)) => desired.muted = value,
            Ok(Command::SetDeafen(value)) => {
                desired.deafened = value;
                if value {
                    desired.muted = true;
                }
            }
            Ok(Command::SetRoomNetworkPolicy { policy, channel_id }) => {
                if channel_id.is_none() {
                    desired.room_network_policy = Some(policy);
                }
            }
            Ok(Command::Connect { .. }) => {}
            Ok(Command::RegisterCandidates(_))
            | Ok(Command::NetworkReport(_))
            | Ok(Command::SetTransportMode(_))
            | Ok(Command::AudioQoeFeedback(_)) => {}
            Ok(Command::SetDisplayName(display_name)) => desired.display_name = Some(display_name),
            Ok(Command::SetAvatar(avatar)) => desired.avatar = Some(avatar),
            Ok(Command::GetGameCatalog)
            | Ok(Command::SetGameActivity(_))
            | Ok(Command::QueryChat(_))
            | Ok(Command::SendTextMessage(_))
            | Ok(Command::RoomSocial(_))
            | Ok(Command::ScreenShare(_))
            | Ok(Command::Watch(_))
            | Ok(Command::GetAvatar(_))
            | Ok(Command::RequestRoomArchive { .. }) => {}
            Ok(Command::VoiceActivity) => {}
            Ok(Command::ProvisionMaster(_))
            | Ok(Command::CreateProtectedChannel{..})
            | Ok(Command::CreateChannel(_))
            | Ok(Command::EditChannel(_))
            | Ok(Command::DeleteChannel(_))
            | Ok(Command::SetChannelAdmin { .. })
            | Ok(Command::SetServerRole { .. })
            | Ok(Command::AdminMute { .. })
            | Ok(Command::MoveUser { .. })
            | Ok(Command::KickChannel { .. })
            | Ok(Command::KickServer { .. })
            | Ok(Command::Ban { .. })
            | Ok(Command::Unban(_))
            | Ok(Command::ListBans { .. }) => {}
            Ok(Command::TrustServer { .. }) => {}
            Ok(Command::RequestMediaRekey) => {}
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => return RetryWait::Elapsed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;
    use std::io::{BufReader, Cursor};
    use std::net::TcpListener;

    #[test]
    fn unresponsive_control_peer_triggers_connection_lost_without_tcp_close() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            send(
                &mut stream,
                &Envelope::new(MessageType::Welcome, serde_json::json!({})).unwrap(),
            )
            .unwrap();
            line.clear();
            reader.read_line(&mut line).unwrap();
            let ping: Envelope = serde_json::from_str(&line).unwrap();
            assert_eq!(ping.kind, MessageType::Ping);
            let ping: Ping = ping.decode().unwrap();
            // An unrelated/stale Pong must not extend the outstanding deadline.
            send(
                &mut stream,
                &Envelope::new(
                    MessageType::Pong,
                    Ping {
                        nonce: ping.nonce + 1,
                    },
                )
                .unwrap(),
            )
            .unwrap();
            line.clear();
            assert_eq!(
                reader.read_line(&mut line).unwrap(),
                0,
                "client must close the stale session"
            );
        });
        let (_commands, commands_rx) = unbounded();
        let (events, _events_rx) = unbounded();
        let started = Instant::now();
        let end = run_session_with_timeout(
            &commands_rx,
            &events,
            &address,
            "Alice",
            &mut DesiredState::default(),
            "",
            Duration::from_millis(250),
        );
        assert!(
            matches!(end, SessionEnd::ConnectionLost(ref message) if message.contains("heartbeat timed out"))
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        server.join().unwrap();
    }

    #[test]
    fn retry_wait_preserves_user_intent() {
        let (tx, rx) = unbounded();
        tx.send(Command::JoinChannel(7)).unwrap();
        tx.send(Command::SetDeafen(true)).unwrap();
        let mut desired = DesiredState::default();
        assert!(matches!(
            wait_for_retry(&rx, Duration::from_millis(1), &mut desired),
            RetryWait::Elapsed
        ));
        assert_eq!(desired.channel, Some(7));
        assert!(desired.muted && desired.deafened);
    }

    #[test]
    fn retry_wait_honors_disconnect() {
        let (tx, rx) = unbounded();
        tx.send(Command::Disconnect).unwrap();
        assert!(matches!(
            wait_for_retry(&rx, Duration::from_secs(1), &mut DesiredState::default()),
            RetryWait::Disconnect
        ));
    }

    #[test]
    fn initial_connection_retries_are_bounded_but_reconnects_remain_persistent() {
        assert!(!initial_connection_exhausted(2, false));
        assert!(initial_connection_exhausted(3, false));
        assert!(!initial_connection_exhausted(30, true));
    }

    #[test]
    fn kick_and_ban_errors_end_the_session_but_permission_errors_do_not() {
        for code in ["kicked_from_server", "banned"] {
            let message = Envelope::new(
                MessageType::Error,
                crate::protocol::control::ServerError {
                    code: code.into(),
                    message: String::new(),
                },
            )
            .unwrap();
            assert!(terminal_server_error(&message).is_some());
        }

        let permission_error = Envelope::new(
            MessageType::Error,
            crate::protocol::control::ServerError {
                code: "permission_denied".into(),
                message: String::new(),
            },
        )
        .unwrap();
        assert!(terminal_server_error(&permission_error).is_none());
    }

    #[test]
    fn non_recoverable_handshake_rejections_do_not_enter_the_retry_loop() {
        for code in [
            "banned",
            "secure_mode_required",
            "identity_authentication_failed",
            "identity_admission_failed",
            "invalid_hello",
            "invalid_nickname",
        ] {
            assert!(matches!(
                handshake_server_failure(crate::protocol::control::ServerError {
                    code: code.into(),
                    message: String::new(),
                }),
                HandshakeFailure::Terminal(_)
            ));
        }
        assert!(matches!(
            handshake_server_failure(crate::protocol::control::ServerError {
                code: "temporary_server_failure".into(),
                message: "try again".into(),
            }),
            HandshakeFailure::Retryable(_)
        ));
        assert_eq!(
            handshake_failure_session_end(HandshakeFailure::UserDisconnect),
            SessionEnd::UserDisconnect
        );
        assert_eq!(
            handshake_failure_session_end(HandshakeFailure::Shutdown),
            SessionEnd::Shutdown
        );
    }

    #[test]
    fn limited_reader_accepts_line_and_rejects_oversize() {
        let mut output = Vec::new();
        let mut valid = BufReader::with_capacity(2, Cursor::new(b"{}\nnext\n"));
        assert_eq!(read_limited_line(&mut valid, &mut output).unwrap(), 3);
        assert_eq!(output, b"{}\n");
        let mut oversized = BufReader::new(Cursor::new(vec![b'a'; 64 * 1024 + 1]));
        assert_eq!(
            read_limited_line(&mut oversized, &mut output)
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidData
        );
    }

    #[test]
    fn reconnect_repeats_hello_and_restores_channel() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let (server_done_tx, server_done_rx) = unbounded();
        thread::spawn(move || {
            for connection_index in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let read_stream = stream.try_clone().unwrap();
                let mut reader = BufReader::new(read_stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let hello: Envelope = serde_json::from_str(&line).unwrap();
                assert_eq!(hello.kind, MessageType::Hello);
                if connection_index == 0 {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    let join: Envelope = serde_json::from_str(&line).unwrap();
                    assert_eq!(join.kind, MessageType::JoinChannel);
                }
                let welcome = Envelope::new(
                    MessageType::Welcome,
                    crate::protocol::control::Welcome {
                        session_id: connection_index as u64 + 1,
                        server_name: "Test".into(),
                        voice_addr: "127.0.0.1:9".into(),
                        media_queue_size: 256,
                        client_id: String::new(),
                        server_fingerprint: String::new(),
                        media_key_epoch: 0,
                        server_role: String::new(),
                        capabilities: vec![],
                        network_policy_channel_ids: vec![],
                        server_features: vec![],
                    },
                )
                .unwrap();
                send(&mut stream, &welcome).unwrap();
                if connection_index == 1 {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    let restored: Envelope = serde_json::from_str(&line).unwrap();
                    assert_eq!(restored.kind, MessageType::JoinChannel);
                    let request: ChannelRequestOwned = restored.decode().unwrap();
                    assert_eq!(request.channel_id, 7);
                    server_done_tx.send(()).unwrap();
                    thread::sleep(Duration::from_millis(100));
                }
            }
        });
        let (command_tx, command_rx) = unbounded();
        let (event_tx, _event_rx) = unbounded();
        command_tx.send(Command::JoinChannel(7)).unwrap();
        let handle =
            thread::spawn(move || connection_loop(&command_rx, &event_tx, address, "Alice".into(), Default::default()));
        server_done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        command_tx.send(Command::Disconnect).unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn connection_loop_stops_after_each_terminal_server_error() {
        for (code, expected_message) in [
            ("kicked_from_server", "管理员踢出服务器"),
            ("banned", "服务器封禁"),
            ("server_password_required", "需要密码"),
            ("server_password_invalid", "密码不正确"),
            ("server_password_rate_limited", "一分钟后"),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap().to_string();
            thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let hello: Envelope = serde_json::from_str(&line).unwrap();
                assert_eq!(hello.kind, MessageType::Hello);

                let welcome = Envelope::new(
                    MessageType::Welcome,
                    crate::protocol::control::Welcome {
                        session_id: 1,
                        server_name: "Test".into(),
                        voice_addr: "127.0.0.1:9".into(),
                        media_queue_size: 256,
                        client_id: String::new(),
                        server_fingerprint: String::new(),
                        media_key_epoch: 0,
                        server_role: String::new(),
                        capabilities: vec![],
                        network_policy_channel_ids: vec![],
                        server_features: vec![],
                    },
                )
                .unwrap();
                send(&mut stream, &welcome).unwrap();
                let terminal = Envelope::new(
                    MessageType::Error,
                    crate::protocol::control::ServerError {
                        code: code.into(),
                        message: String::new(),
                    },
                )
                .unwrap();
                send(&mut stream, &terminal).unwrap();
                thread::sleep(Duration::from_millis(100));
            });

            let (_command_tx, command_rx) = unbounded();
            let (event_tx, event_rx) = unbounded();
            let (done_tx, done_rx) = unbounded();
            thread::spawn(move || {
                connection_loop(&command_rx, &event_tx, address, "Alice".into(), Default::default());
                done_tx.send(()).unwrap();
            });

            done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            let events: Vec<_> = event_rx.try_iter().collect();
            assert!(events.iter().any(|event| matches!(
                event,
                Event::Error(message) if message.contains(expected_message)
            )));
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, Event::Reconnecting { .. }))
            );
        }
    }

    #[derive(serde::Deserialize)]
    struct ChannelRequestOwned {
        channel_id: u64,
    }
}

#[cfg(test)]
mod fragment_regression_tests {
    use super::*;
    #[test]
    fn consecutive_lines_and_partial_eof_are_handled_once() {
        let mut reader = BufReader::with_capacity(2, std::io::Cursor::new(b"{}\n[]\n{\"x\":1}"));
        let mut output = Vec::new();
        assert_eq!(read_limited_line(&mut reader, &mut output).unwrap(), 3);
        assert_eq!(output, b"{}\n");
        assert_eq!(read_limited_line(&mut reader, &mut output).unwrap(), 3);
        assert_eq!(output, b"[]\n");
        assert_eq!(
            read_limited_line(&mut reader, &mut output)
                .unwrap_err()
                .kind(),
            ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn fragment_timeout_preserves_json_prefix() {
        struct Fragmented(u8);
        impl Read for Fragmented {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let part: &[u8] = match self.0 {
                    0 => b"{\"type\":\"pong\",",
                    1 => {
                        self.0 += 1;
                        return Err(std::io::Error::from(ErrorKind::TimedOut));
                    }
                    2 => b"\"data\":{\"nonce\":1}}\n",
                    _ => return Ok(0),
                };
                self.0 += 1;
                buffer[..part.len()].copy_from_slice(part);
                Ok(part.len())
            }
        }
        let mut reader = BufReader::new(Fragmented(0));
        let mut line = Vec::new();
        assert_eq!(
            read_limited_line(&mut reader, &mut line)
                .unwrap_err()
                .kind(),
            ErrorKind::TimedOut
        );
        assert_eq!(line, b"{\"type\":\"pong\",");
        read_limited_line(&mut reader, &mut line).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&line).unwrap()["data"]["nonce"],
            1
        );
    }
}
