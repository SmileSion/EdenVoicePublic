use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    io,
    net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const RECENT_AUDIO_TIMESTAMPS_PER_PEER: usize = 64;
const AUDIO_DUPLICATE_WINDOW: Duration = Duration::from_millis(500);

#[derive(Default)]
struct AudioDeduplicator {
    recent: HashMap<u64, VecDeque<(u32, Instant)>>,
}

impl AudioDeduplicator {
    fn accept(&mut self, session_id: u64, timestamp: u32) -> bool {
        self.accept_at(session_id, timestamp, Instant::now())
    }

    fn accept_at(&mut self, session_id: u64, timestamp: u32, now: Instant) -> bool {
        let timestamps = self.recent.entry(session_id).or_default();
        while timestamps.front().is_some_and(|(_, received_at)| {
            now.duration_since(*received_at) >= AUDIO_DUPLICATE_WINDOW
        }) {
            timestamps.pop_front();
        }
        if timestamps
            .iter()
            .any(|(recent_timestamp, _)| *recent_timestamp == timestamp)
        {
            return false;
        }
        if timestamps.len() == RECENT_AUDIO_TIMESTAMPS_PER_PEER {
            timestamps.pop_front();
        }
        timestamps.push_back((timestamp, now));
        true
    }

    fn retain_peers(&mut self, peers: &BTreeMap<u64, PeerCandidates>) {
        self.recent.retain(|peer_id, _| peers.contains_key(peer_id));
    }
}

use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
use crossbeam_channel::{Receiver, Sender, bounded};

use crate::{
    protocol::{
        control::{Candidate, NetworkReport, PeerCandidates, RouteUpdate},
        voice::{HEADER_SIZE, MAX_PACKET_SIZE, PacketType, VoicePacket},
    },
    security::{MediaSecurity, PairwiseSecurity},
    transport::{
        QueuedAudio, ReceivedAudio, RouteKind, RouteManager, RouteThresholds,
        TimedVoicePathMetrics, VoicePathMetrics, VoiceTransport, fresh_voice_path_metrics,
        receive_audio, receive_fresh_packet, send_latest_packet,
    },
};

#[derive(Clone, Debug)]
pub enum TransportEvent {
    LocalCandidates(Vec<Candidate>),
    NetworkReport(NetworkReport),
    RouteStatus(RouteStatus),
    RekeyRequired,
    Health(TransportHealth),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransportHealth {
    Healthy,
    Recovering(String),
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteStatus {
    pub actual_transport: String,
    pub selected_route: String,
    pub endpoint: String,
    pub qualification: String,
}

#[derive(Debug, Default)]
struct Desired {
    peers: BTreeMap<u64, PeerCandidates>,
    route: Option<RouteUpdate>,
}

#[derive(Clone)]
pub struct AdaptiveControl {
    desired: Arc<Mutex<Desired>>,
    events: Receiver<TransportEvent>,
    rekey_state: Arc<AtomicU8>,
    media: Arc<SessionMediaState>,
}

// One session-owned handle keeps cloned settings small while device restarts
// retain nonce allocation, timestamps and both directional crypto contexts.
#[derive(Default)]
struct SessionMediaState {
    sequence: Arc<AtomicU32>,
    timestamp: Arc<AtomicU32>,
    pairwise: Arc<Mutex<HashMap<u64, PairwiseSecurity>>>,
    receive_pairwise: Arc<Mutex<HashMap<u64, PairwiseSecurity>>>,
}

impl std::fmt::Debug for AdaptiveControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdaptiveControl").finish_non_exhaustive()
    }
}

impl AdaptiveControl {
    pub fn new() -> (Self, Sender<TransportEvent>) {
        let (events_tx, events_rx) = bounded(256);
        (
            Self {
                desired: Arc::new(Mutex::new(Desired::default())),
                events: events_rx,
                rekey_state: Arc::new(AtomicU8::new(0)),
                media: Arc::new(SessionMediaState::default()),
            },
            events_tx,
        )
    }

    pub fn set_peer_candidates(&self, value: PeerCandidates) {
        if let Ok(mut desired) = self.desired.lock() {
            desired.peers.insert(value.peer_id, value);
        }
    }

    pub fn set_route(&self, value: RouteUpdate) {
        if let Ok(mut desired) = self.desired.lock() {
            desired.route = Some(value);
        }
    }
    pub fn remove_peer(&self, peer_id: u64) {
        if let Ok(mut desired) = self.desired.lock() {
            desired.peers.remove(&peer_id);
        }
    }
    pub fn clear_peers(&self) {
        if let Ok(mut desired) = self.desired.lock() {
            desired.peers.clear();
        }
    }

    pub fn try_event(&self) -> Option<TransportEvent> {
        self.events.try_recv().ok()
    }
    pub fn media_rekeyed(&self) {
        self.rekey_state.store(2, Ordering::Release);
    }
}

pub struct AdaptiveTransport {
    socket: Arc<Mutex<UdpSocket>>,
    server: SocketAddr,
    session_id: u64,
    sequence: Arc<AtomicU32>,
    timestamp: Arc<AtomicU32>,
    incoming: Receiver<QueuedAudio>,
    alive: Arc<AtomicBool>,
    manager: Arc<Mutex<RouteManager>>,
    desired: Arc<Mutex<Desired>>,
    packet_buffer: Mutex<Vec<u8>>,
    security: Option<MediaSecurity>,
    pairwise: Arc<Mutex<HashMap<u64, PairwiseSecurity>>>,
    events: Sender<TransportEvent>,
    rekey_state: Arc<AtomicU8>,
    consecutive_send_failures: Arc<AtomicU8>,
    repair_requested: Arc<MaintenanceSignal>,
    voice_path_metrics: Arc<Mutex<Option<TimedVoicePathMetrics>>>,
    peer_path_metrics: Arc<Mutex<HashMap<u64, TimedVoicePathMetrics>>>,
}

impl AdaptiveTransport {
    pub fn connect(
        control_address: &str,
        advertised_voice_address: &str,
        session_id: u64,
        control: AdaptiveControl,
        events: Sender<TransportEvent>,
        media_queue_size: usize,
        security: Option<MediaSecurity>,
    ) -> io::Result<Self> {
        let server = resolve_server(control_address, advertised_voice_address)?;
        let socket = UdpSocket::bind(if server.is_ipv6() {
            "[::]:0"
        } else {
            "0.0.0.0:0"
        })?;
        socket.set_nonblocking(true)?;
        let sequence = control.media.sequence.clone();
        let registration_sequence = sequence.fetch_add(1, Ordering::Relaxed);
        guard_shared_media_sequence(registration_sequence, &control.rekey_state, &events)?;
        send_registration(
            &socket,
            server,
            session_id,
            registration_sequence,
            security.as_ref(),
        )?;

        let local_port = socket.local_addr()?.port();
        let candidates = local_candidates(server, local_port);
        if !candidates.is_empty() {
            let _ = events.try_send(TransportEvent::LocalCandidates(candidates.clone()));
        }

        let receive_socket = socket.try_clone()?;
        let probe_socket = socket.try_clone()?;
        let socket = Arc::new(Mutex::new(socket));
        // This queue only decouples UDP decryption from the audio worker; the
        // per-speaker jitter buffers own actual playout delay. Keeping seconds
        // of encrypted media here would replay stale speech after a suspend or
        // scheduler stall.
        let (incoming_tx, incoming_rx) = bounded(media_queue_size.clamp(16, 64));
        let incoming_stale = incoming_rx.clone();
        let alive = Arc::new(AtomicBool::new(true));
        let manager = Arc::new(Mutex::new(RouteManager::new(Duration::from_secs(20))));
        let thread_manager = Arc::clone(&manager);
        let thread_desired = Arc::clone(&control.desired);
        let thread_alive = Arc::clone(&alive);
        let receiver_security = security.clone();
        let pairwise = control.media.pairwise.clone();
        // Directional contexts must not contend: a busy send cipher is not
        // evidence that an authenticated incoming packet is invalid. Only seal
        // uses the send map; only open uses the receive map.
        let receiver_pairwise = control.media.receive_pairwise.clone();
        let sender_pairwise_cleanup = pairwise.clone();
        let transport_events = events.clone();
        let receiver_sequence = Arc::clone(&sequence);
        let shared_socket = Arc::clone(&socket);
        let consecutive_send_failures = Arc::new(AtomicU8::new(0));
        let receiver_send_failures = Arc::clone(&consecutive_send_failures);
        let repair_requested = Arc::new(MaintenanceSignal::new());
        let receiver_repair_requested = Arc::clone(&repair_requested);
        let receiver_rekey_state = Arc::clone(&control.rekey_state);
        let voice_path_metrics = Arc::new(Mutex::new(None));
        let receiver_voice_path_metrics = Arc::clone(&voice_path_metrics);
        let peer_path_metrics = Arc::new(Mutex::new(HashMap::new()));
        let receiver_peer_path_metrics = Arc::clone(&peer_path_metrics);
        thread::Builder::new()
            .name("adaptive-voice-receiver".into())
            .spawn(move || {
                receiver_loop(
                    receive_socket,
                    probe_socket,
                    server,
                    session_id,
                    incoming_tx,
                    incoming_stale,
                    thread_manager,
                    thread_desired,
                    events,
                    thread_alive,
                    candidates,
                    receiver_security,
                    receiver_pairwise,
                    sender_pairwise_cleanup,
                    shared_socket,
                    receiver_sequence,
                    receiver_rekey_state,
                    receiver_send_failures,
                    receiver_repair_requested,
                    receiver_voice_path_metrics,
                    receiver_peer_path_metrics,
                )
            })?;
        let rekey_state = Arc::clone(&control.rekey_state);
        Ok(Self {
            socket,
            server,
            session_id,
            sequence,
            timestamp: control.media.timestamp.clone(),
            incoming: incoming_rx,
            alive,
            manager,
            desired: control.desired,
            packet_buffer: Mutex::new(Vec::with_capacity(MAX_PACKET_SIZE)),
            security,
            pairwise,
            events: transport_events,
            rekey_state,
            consecutive_send_failures,
            repair_requested,
            voice_path_metrics,
            peer_path_metrics,
        })
    }

    fn guard_media_sequence(&self, sequence: u32) -> io::Result<()> {
        guard_shared_media_sequence(sequence, &self.rekey_state, &self.events)
    }
}

impl VoiceTransport for AdaptiveTransport {
    fn send_audio(&self, opus_payload: &[u8]) -> io::Result<()> {
        let mut packet = self
            .packet_buffer
            .try_lock()
            .map_err(|_| io::Error::other("voice packet buffer unavailable"))?;
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        self.guard_media_sequence(sequence)?;
        let timestamp = self.timestamp.fetch_add(960, Ordering::Relaxed);
        if self.security.is_none() {
            encode_media_packet(
                PacketType::Audio,
                self.session_id,
                sequence,
                timestamp,
                opus_payload,
                self.security.as_ref(),
                &mut packet,
            )?;
        }
        // Route discovery runs on the UDP receiver thread. It must never make
        // the 20 ms audio worker wait for its bookkeeping locks; if a snapshot
        // is momentarily unavailable, this frame safely uses the server path.
        let (transport, expected_peers) = self
            .desired
            .try_lock()
            .ok()
            .map(|desired| {
                (
                    desired
                        .route
                        .as_ref()
                        .map(|route| route.transport.clone())
                        .unwrap_or_else(|| "sfu".into()),
                    desired.peers.keys().copied().collect::<Vec<_>>(),
                )
            })
            .unwrap_or_else(|| ("sfu".into(), Vec::new()));
        if transport == "p2p" || transport == "hybrid" {
            let QualifiedTargets {
                peer_count,
                targets,
                protect,
            } = try_qualified_targets(&self.manager, &expected_peers).unwrap_or_default();
            if !targets.is_empty() {
                let direct_peers = self.desired.try_lock().ok().map(|desired| {
                    targets
                        .iter()
                        .filter_map(|(peer_id, _)| {
                            desired
                                .peers
                                .get(peer_id)
                                .cloned()
                                .map(|peer| (*peer_id, peer))
                        })
                        .collect::<HashMap<_, _>>()
                });
                let mut sent = 0usize;
                for (peer_id, target) in &targets {
                    if let Some(security) = self.security.as_ref() {
                        let peer = direct_peers.as_ref().and_then(|peers| peers.get(peer_id));
                        let Some(peer) = peer else {
                            continue;
                        };
                        if ensure_pairwise(&self.pairwise, security, self.session_id, peer).is_err()
                        {
                            continue;
                        }
                        if encode_pairwise_packet(
                            &self.pairwise,
                            *peer_id,
                            self.session_id,
                            sequence,
                            timestamp,
                            opus_payload,
                            &mut packet,
                        )
                        .is_ok()
                            && self
                                .socket
                                .try_lock()
                                .is_ok_and(|socket| socket.send_to(&packet, target).is_ok())
                        {
                            sent += 1;
                        }
                    } else if self
                        .socket
                        .try_lock()
                        .is_ok_and(|socket| socket.send_to(&packet, target).is_ok())
                    {
                        sent += 1;
                    }
                }
                if transport == "p2p" && sent == peer_count && !protect {
                    return Ok(());
                }
            }
        }
        // Pure P2P needs no SFU ciphertext or shared server cipher lock.
        if self.security.is_some() {
            encode_media_packet(
                PacketType::Audio,
                self.session_id,
                sequence,
                timestamp,
                opus_payload,
                self.security.as_ref(),
                &mut packet,
            )?;
        }
        let result = self
            .socket
            .try_lock()
            .map_err(|_| io::Error::new(io::ErrorKind::WouldBlock, "adaptive UDP socket busy"))?
            .send_to(&packet, self.server)
            .map(|_| ());
        match &result {
            Ok(()) => {
                let previous = self.consecutive_send_failures.swap(0, Ordering::AcqRel);
                if previous > 0 {
                    let _ = self
                        .events
                        .try_send(TransportEvent::Health(TransportHealth::Healthy));
                }
            }
            // Winsock can report an ICMP error from an earlier peer probe on
            // this shared socket. It does not mean the SFU socket is broken.
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionReset | io::ErrorKind::WouldBlock
                ) => {}
            Err(error) => {
                let failures = self
                    .consecutive_send_failures
                    .fetch_add(1, Ordering::AcqRel)
                    .saturating_add(1);
                if failures == 3 {
                    self.repair_requested.request_repair();
                    let _ =
                        self.events
                            .try_send(TransportEvent::Health(TransportHealth::Recovering(
                                format!("UDP send failed {failures} consecutive times: {error}"),
                            )));
                }
            }
        }
        result
    }

    fn try_receive(&self) -> Option<VoicePacket> {
        receive_fresh_packet(&self.incoming)
    }

    fn try_receive_audio(&self) -> Option<ReceivedAudio> {
        receive_audio(&self.incoming)
    }

    fn session_id(&self) -> u64 {
        self.session_id
    }

    fn network_metrics(&self, peer_id: u64) -> Option<(f64, f64)> {
        let metrics = self.peer_path_metrics.try_lock().ok()?;
        let sample = metrics.get(&peer_id)?;
        (sample.observed_at.elapsed() < Duration::from_secs(3))
            .then_some((sample.value.rtt_ms, sample.value.jitter_ms))
    }

    fn voice_path_metrics(&self) -> Option<VoicePathMetrics> {
        fresh_voice_path_metrics(&self.voice_path_metrics)
    }
}

#[derive(Debug, Default, PartialEq)]
struct QualifiedTargets {
    peer_count: usize,
    targets: Vec<(u64, SocketAddr)>,
    protect: bool,
}

fn try_qualified_targets(
    manager: &Arc<Mutex<RouteManager>>,
    expected_peers: &[u64],
) -> Option<QualifiedTargets> {
    let manager = manager.try_lock().ok()?;
    let mut protect = false;
    let now = Instant::now();
    let targets = expected_peers
        .iter()
        .filter_map(|peer| {
            manager.best(*peer, true).map(|route| {
                protect |= route.needs_sfu_protection(now);
                (*peer, route.endpoint)
            })
        })
        .collect();
    // A peer with no usable candidate is deliberately included in the count.
    // Pure P2P may suppress the warm SFU copy only after every server-advertised
    // room peer has a qualified route and received this frame.
    Some(QualifiedTargets {
        peer_count: expected_peers.len(),
        targets,
        protect,
    })
}

impl Drop for AdaptiveTransport {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
        self.repair_requested.wake();
    }
}

#[derive(Default)]
struct ProbeStats {
    sent: u64,
    received: u64,
    last_rtt: f64,
    jitter: f64,
    reported: bool,
    last_reported: Option<Instant>,
    last_received: Option<Instant>,
}

struct ProbeChallenge {
    sent_at: Instant,
    target: SocketAddr,
}

struct ServerProbeState {
    pending: HashMap<u64, Instant>,
    stats: ProbeStats,
    last_sent: Instant,
}

impl Default for ServerProbeState {
    fn default() -> Self {
        Self {
            pending: HashMap::new(),
            stats: ProbeStats::default(),
            last_sent: Instant::now() - Duration::from_secs(1),
        }
    }
}

const MAX_PROBES_PER_PEER_CYCLE: usize = 8;
// A VPN/NAT change can silently change our public endpoint while send_to
// continues to succeed. The SFU rejects probes from unregistered endpoints,
// so probing alone cannot restore receive-only calls. Renewal and route/interface
// discovery run on a separate worker: OS/network stalls must not delay UDP reads.
const REGISTRATION_INTERVAL: Duration = Duration::from_millis(250);

// A coalesced notification avoids polling while retaining an atomic repair
// request: requests arriving just before recv_timeout must not be lost.
struct MaintenanceSignal {
    requested: AtomicBool,
    wake_tx: Sender<()>,
    wake_rx: Receiver<()>,
}

impl MaintenanceSignal {
    fn new() -> Self {
        let (wake_tx, wake_rx) = bounded(1);
        Self {
            requested: AtomicBool::new(false),
            wake_tx,
            wake_rx,
        }
    }

    fn wake(&self) {
        let _ = self.wake_tx.try_send(());
    }

    fn request_repair(&self) {
        // Repeated failures do not keep waking a worker in repair backoff.
        if !self.requested.swap(true, Ordering::AcqRel) {
            self.wake();
        }
    }
}

struct PreparedSockets {
    sender: UdpSocket,
    receiver: UdpSocket,
    probe: UdpSocket,
    candidates: Vec<Candidate>,
}

struct SocketReplacement {
    sockets: PreparedSockets,
    installed: Sender<()>,
}

#[allow(clippy::too_many_arguments)]
fn maintenance_loop(
    mut socket: UdpSocket,
    server: SocketAddr,
    session_id: u64,
    events: Sender<TransportEvent>,
    alive: Arc<AtomicBool>,
    mut candidates: Vec<Candidate>,
    security: Option<MediaSecurity>,
    sequence: Arc<AtomicU32>,
    rekey_state: Arc<AtomicU8>,
    repair_requested: Arc<MaintenanceSignal>,
    replacements: Sender<SocketReplacement>,
) {
    let mut last_registration = Instant::now();
    let mut last_route_check = Instant::now();
    let mut local_route_ip = route_local_ip(server);
    let mut last_candidate_refresh = Instant::now();
    let mut next_repair_at = Instant::now();
    // Do not let successful keepalives reset the audio sender's error counter.
    let mut registration_failures = 0u8;
    while alive.load(Ordering::Acquire) {
        if last_route_check.elapsed() >= REGISTRATION_INTERVAL {
            if let Some(current_ip) = route_local_ip(server) {
                if local_route_ip.is_some_and(|previous| previous != current_ip) {
                    repair_requested.request_repair();
                }
                local_route_ip = Some(current_ip);
            }
            last_route_check = Instant::now();
        }
        if Instant::now() >= next_repair_at
            && repair_requested.requested.swap(false, Ordering::AcqRel)
        {
            match rebuild_udp_sockets(
                server,
                session_id,
                security.as_ref(),
                &sequence,
                &rekey_state,
                &events,
            ) {
                Ok((prepared, maintenance_socket)) => {
                    let refreshed = prepared.candidates.clone();
                    let (installed_tx, installed_rx) = bounded(1);
                    if !alive.load(Ordering::Acquire)
                        || replacements
                            .send(SocketReplacement {
                                sockets: prepared,
                                installed: installed_tx,
                            })
                            .is_err()
                    {
                        return;
                    }
                    // Nudge the old endpoint during handoff. This local empty
                    // datagram is
                    // rejected by the packet decoder; it never leaves the host.
                    if let Ok(mut local) = socket.local_addr() {
                        if local.ip().is_unspecified() {
                            local.set_ip(if local.is_ipv6() {
                                std::net::Ipv6Addr::LOCALHOST.into()
                            } else {
                                std::net::Ipv4Addr::LOCALHOST.into()
                            });
                        }
                        let _ = socket.send_to(&[], local);
                    }
                    // Never register the old endpoint after registering the new
                    // one. Wait for the receiver's atomic handoff before renewing.
                    loop {
                        if !alive.load(Ordering::Acquire) {
                            return;
                        }
                        crossbeam_channel::select! {
                            recv(installed_rx) -> installed => {
                                if installed.is_err() {
                                    return;
                                }
                                break;
                            }
                            recv(repair_requested.wake_rx) -> _ => {}
                        }
                    }
                    socket = maintenance_socket;
                    candidates = refreshed;
                    registration_failures = 0;
                    last_registration = Instant::now();
                    last_candidate_refresh = Instant::now();
                }
                Err(error) => {
                    tracing::debug!(error = %error, "adaptive UDP recovery will retry");
                    let _ = events.try_send(TransportEvent::Health(TransportHealth::Recovering(
                        format!("UDP transport recovery failed: {error}"),
                    )));
                    next_repair_at = Instant::now() + Duration::from_millis(500);
                    repair_requested.request_repair();
                }
            }
        }
        if !alive.load(Ordering::Acquire) {
            return;
        }
        if last_registration.elapsed() >= REGISTRATION_INTERVAL {
            let registration_sequence = if security.is_some() {
                let value = sequence.fetch_add(1, Ordering::Relaxed);
                if guard_shared_media_sequence(value, &rekey_state, &events).is_err() {
                    last_registration = Instant::now();
                    continue;
                }
                value
            } else {
                0
            };
            // Own socket clone: no shared audio/receiver lock spans an OS call.
            match send_registration(
                &socket,
                server,
                session_id,
                registration_sequence,
                security.as_ref(),
            ) {
                Ok(()) => registration_failures = 0,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionReset | io::ErrorKind::WouldBlock
                    ) => {}
                Err(error) => {
                    registration_failures = registration_failures.saturating_add(1);
                    tracing::debug!(error = %error, "UDP registration keepalive failed");
                    if registration_failures >= 3 {
                        let _ = events.try_send(TransportEvent::Health(TransportHealth::Recovering(
                            format!("UDP keepalive failed {registration_failures} consecutive times: {error}"),
                        )));
                        repair_requested.request_repair();
                    }
                }
            }
            last_registration = Instant::now();
        }
        if last_candidate_refresh.elapsed() >= Duration::from_secs(2) {
            if let Ok(local_addr) = socket.local_addr() {
                let refreshed = local_candidates(server, local_addr.port());
                if !same_candidates(&candidates, &refreshed)
                    && events
                        .try_send(TransportEvent::LocalCandidates(refreshed.clone()))
                        .is_ok()
                {
                    candidates = refreshed;
                }
            }
            last_candidate_refresh = Instant::now();
        }
        let mut deadline = (last_registration + REGISTRATION_INTERVAL)
            .min(last_route_check + REGISTRATION_INTERVAL)
            .min(last_candidate_refresh + Duration::from_secs(2));
        if repair_requested.requested.load(Ordering::Acquire) {
            deadline = deadline.min(next_repair_at);
        }
        // Drop may have happened during a maintenance OS call. A queued wake
        // also covers the race between this alive check and entering the wait.
        if alive.load(Ordering::Acquire) {
            let _ = repair_requested
                .wake_rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn receiver_loop(
    mut socket: UdpSocket,
    mut probe_socket: UdpSocket,
    server: SocketAddr,
    session_id: u64,
    incoming: Sender<QueuedAudio>,
    incoming_stale: Receiver<QueuedAudio>,
    manager: Arc<Mutex<RouteManager>>,
    desired: Arc<Mutex<Desired>>,
    events: Sender<TransportEvent>,
    alive: Arc<AtomicBool>,
    local_candidate_snapshot: Vec<Candidate>,
    security: Option<MediaSecurity>,
    pairwise: Arc<Mutex<HashMap<u64, PairwiseSecurity>>>,
    sender_pairwise_cleanup: Arc<Mutex<HashMap<u64, PairwiseSecurity>>>,
    shared_socket: Arc<Mutex<UdpSocket>>,
    sequence: Arc<AtomicU32>,
    rekey_state: Arc<AtomicU8>,
    consecutive_send_failures: Arc<AtomicU8>,
    repair_requested: Arc<MaintenanceSignal>,
    voice_path_metrics: Arc<Mutex<Option<TimedVoicePathMetrics>>>,
    peer_path_metrics: Arc<Mutex<HashMap<u64, TimedVoicePathMetrics>>>,
) {
    let (replacement_tx, replacement_rx) = bounded(1);
    let maintenance = socket.try_clone().and_then(|maintenance_socket| {
        let events = events.clone();
        let alive = alive.clone();
        let security = security.clone();
        let sequence = sequence.clone();
        let rekey_state = rekey_state.clone();
        let repair_requested = repair_requested.clone();
        thread::Builder::new()
            .name("adaptive-voice-maintenance".into())
            .spawn(move || {
                maintenance_loop(
                    maintenance_socket,
                    server,
                    session_id,
                    events,
                    alive,
                    local_candidate_snapshot,
                    security,
                    sequence,
                    rekey_state,
                    repair_requested,
                    replacement_tx,
                )
            })
    });
    if let Err(error) = maintenance {
        alive.store(false, Ordering::Release);
        let _ = events.try_send(TransportEvent::Health(TransportHealth::Failed(format!(
            "UDP maintenance thread creation failed: {error}"
        ))));
        return;
    }
    let mut buffer = [0_u8; MAX_PACKET_SIZE + 1];
    let mut stats = HashMap::<(u64, SocketAddr), ProbeStats>::new();
    let mut challenges = HashMap::<(u64, u64), ProbeChallenge>::new();
    let mut next_challenge = unix_micros();
    let mut generations = HashMap::<u64, u64>::new();
    let mut probe_cursors = HashMap::<u64, usize>::new();
    let mut last_probe = Instant::now() - Duration::from_secs(1);
    let mut probe_interval = Duration::from_millis(75);
    let mut last_candidate_refresh = Instant::now();
    let mut pending_replacement: Option<SocketReplacement> = None;
    let mut last_route_status = None;
    let mut server_probe = ServerProbeState::default();
    let mut audio_deduplicator = AudioDeduplicator::default();
    while alive.load(Ordering::Relaxed) {
        if pending_replacement.is_none() {
            pending_replacement = replacement_rx.try_recv().ok();
        }
        if pending_replacement.is_some() {
            // Maintenance prepares everything before publication. Never wait
            // for an audio send while installing a replacement socket.
            if let Ok(mut sender) = shared_socket.try_lock() {
                let replacement = pending_replacement.take().unwrap();
                let PreparedSockets {
                    sender: new_sender,
                    receiver,
                    probe,
                    candidates,
                } = replacement.sockets;
                // Receive failures accumulated while the old socket was being
                // replaced must not immediately trigger another replacement.
                repair_requested.requested.store(false, Ordering::Release);
                *sender = new_sender;
                drop(sender);
                socket = receiver;
                probe_socket = probe;
                consecutive_send_failures.store(0, Ordering::Release);
                let _ = events.try_send(TransportEvent::LocalCandidates(candidates));
                // Direct-route qualifications belong to the old socket.
                // Tell the server to use SFU while new paths qualify.
                if let Ok(mut routes) = manager.lock() {
                    for peer_id in routes.peers() {
                        let _ = events.try_send(TransportEvent::NetworkReport(NetworkReport {
                            peer_id,
                            qualified: false,
                            rtt_ms: 0.0,
                            packet_loss_percent: 100.0,
                            jitter_ms: 0.0,
                        }));
                    }
                    routes.retain_peers(&[]);
                }
                stats.clear();
                challenges.clear();
                generations.clear();
                probe_cursors.clear();
                server_probe = ServerProbeState::default();
                if let Ok(mut metrics) = voice_path_metrics.lock() {
                    *metrics = None;
                }
                if let Ok(mut metrics) = peer_path_metrics.lock() {
                    metrics.clear();
                }
                last_route_status = None;
                last_probe = Instant::now() - Duration::from_secs(1);
                let _ = replacement.installed.try_send(());
                let _ = events.try_send(TransportEvent::Health(TransportHealth::Healthy));
            }
        }
        if server_probe.last_sent.elapsed() >= Duration::from_secs(1) {
            send_server_probe(
                &probe_socket,
                server,
                session_id,
                &mut next_challenge,
                &mut server_probe,
                security.as_ref(),
                &sequence,
                &rekey_state,
                &events,
            );
        }
        if last_probe.elapsed() >= probe_interval {
            let route_status = send_probes(
                &probe_socket,
                server,
                session_id,
                &manager,
                &desired,
                &mut stats,
                &mut challenges,
                &mut next_challenge,
                &mut generations,
                &mut probe_cursors,
                &events,
            );
            if let Some(route_status) = route_status {
                let direct_ready =
                    matches!(route_status.actual_transport.as_str(), "P2P" | "Hybrid");
                if last_route_status.as_ref() != Some(&route_status) {
                    let _ = events.try_send(TransportEvent::RouteStatus(route_status.clone()));
                    last_route_status = Some(route_status);
                }
                publish_voice_path_metrics(
                    last_route_status.as_ref(),
                    &manager,
                    &server_probe.stats,
                    &voice_path_metrics,
                );
                probe_interval = if direct_ready {
                    Duration::from_millis(200)
                } else {
                    Duration::from_millis(75)
                };
            }
            last_probe = Instant::now();
        }
        if last_candidate_refresh.elapsed() >= Duration::from_secs(2) {
            if let (Ok(desired), Ok(mut contexts)) = (desired.try_lock(), pairwise.try_lock()) {
                contexts.retain(|peer_id, _| desired.peers.contains_key(peer_id));
                if let Ok(mut outbound) = sender_pairwise_cleanup.try_lock() {
                    outbound.retain(|peer_id, _| desired.peers.contains_key(peer_id));
                }
                audio_deduplicator.retain_peers(&desired.peers);
                if let Ok(mut metrics) = peer_path_metrics.lock() {
                    metrics.retain(|peer_id, _| desired.peers.contains_key(peer_id));
                }
            }
            last_candidate_refresh = Instant::now();
        }
        match receive_datagram(&socket, &mut buffer, server) {
            Ok((length, source)) => {
                // Capture application ingress before decoding, authentication,
                // or route locks can delay this datagram's delivery.
                let received_at = Instant::now();
                let Ok(mut packet) = VoicePacket::decode(&buffer[..length]) else {
                    continue;
                };
                match packet.kind {
                    PacketType::Probe => {
                        if source != server
                            && authorize_peer_source(&desired, packet.session_id, source)
                            && let Ok(bytes) = (VoicePacket {
                                kind: PacketType::ProbeAck,
                                session_id,
                                ..packet
                            })
                            .encode()
                        {
                            let _ = socket.send_to(&bytes, source);
                        }
                    }
                    PacketType::ProbeAck => {
                        if source == server && packet.session_id == session_id {
                            if let Some(security) = security.as_ref() {
                                let aad = &buffer[..HEADER_SIZE];
                                let plaintext =
                                    security.inbound.lock().ok().and_then(|mut cipher| {
                                        cipher
                                            .open(
                                                aad,
                                                packet.session_id,
                                                packet.key_epoch,
                                                packet.sequence,
                                                &packet.payload,
                                            )
                                            .ok()
                                    });
                                let Some(plaintext) = plaintext else {
                                    continue;
                                };
                                packet.payload = plaintext;
                            }
                            observe_server_ack(packet, &mut server_probe);
                            publish_voice_path_metrics(
                                last_route_status.as_ref(),
                                &manager,
                                &server_probe.stats,
                                &voice_path_metrics,
                            );
                        } else {
                            observe_ack(
                                packet,
                                source,
                                &manager,
                                &desired,
                                &events,
                                &mut stats,
                                &mut challenges,
                            );
                            publish_voice_path_metrics(
                                last_route_status.as_ref(),
                                &manager,
                                &server_probe.stats,
                                &voice_path_metrics,
                            );
                        }
                    }
                    PacketType::Audio => {
                        // The server path needs no P2P route lookup. Avoid
                        // delaying all SFU speakers behind route maintenance.
                        if source != server
                            && !manager.lock().is_ok_and(|routes| {
                                routes.accepts_media_from(packet.session_id, source)
                            })
                        {
                            continue;
                        }
                        if let Some(security) = security.as_ref() {
                            let aad = &buffer[..HEADER_SIZE];
                            let plaintext = if source == server {
                                security.inbound.lock().ok().and_then(|mut cipher| {
                                    cipher
                                        .open(
                                            aad,
                                            packet.session_id,
                                            packet.key_epoch,
                                            packet.sequence,
                                            &packet.payload,
                                        )
                                        .ok()
                                })
                            } else {
                                let peer = desired.lock().ok().and_then(|desired| {
                                    desired.peers.get(&packet.session_id).cloned()
                                });
                                peer.and_then(|peer| {
                                    ensure_pairwise(&pairwise, security, session_id, &peer).ok()
                                })
                                .and_then(|_| pairwise.lock().ok())
                                .and_then(|mut contexts| {
                                    contexts.get_mut(&packet.session_id).and_then(|context| {
                                        context
                                            .open(
                                                aad,
                                                packet.session_id,
                                                packet.key_epoch,
                                                packet.sequence,
                                                &packet.payload,
                                            )
                                            .ok()
                                    })
                                })
                            };
                            let Some(plaintext) = plaintext else {
                                continue;
                            };
                            packet.payload = plaintext;
                        }
                        // Hybrid sends the same logical frame over direct and SFU paths. Drop the
                        // second authenticated copy before it can occupy the small global media
                        // queue and evict a different speaker's unique frame. This bounded recent
                        // set is intentionally not a high-water mark: unique reordered frames and
                        // wrapping timestamps remain valid.
                        if !audio_deduplicator.accept(packet.session_id, packet.timestamp) {
                            continue;
                        }
                        record_peer_path_metrics(
                            packet.session_id,
                            source,
                            server,
                            &server_probe.stats,
                            &manager,
                            &peer_path_metrics,
                        );
                        send_latest_packet(&incoming, &incoming_stale, packet, received_at);
                    }
                    PacketType::Register => {}
                }
            }
            Err(error) if recoverable_udp_receive_error(&error) => {
                // Nonblocking mode is shared by all socket clones, including
                // audio/probe senders. Pace idle reads without blocking sends.
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => {
                tracing::warn!(error = %error, "adaptive UDP receiver stopped");
                let _ = events.try_send(TransportEvent::Health(TransportHealth::Recovering(
                    format!("UDP receive failed: {error}"),
                )));
                repair_requested.request_repair();
            }
        }
    }
}

fn encode_media_packet(
    kind: PacketType,
    session_id: u64,
    sequence: u32,
    timestamp: u32,
    payload: &[u8],
    security: Option<&MediaSecurity>,
    output: &mut Vec<u8>,
) -> io::Result<()> {
    let Some(security) = security else {
        return VoicePacket::encode_parts_into(
            kind, 0, session_id, sequence, timestamp, 0, payload, output,
        )
        .map_err(io::Error::other);
    };
    let mut packet = VoicePacket {
        kind,
        flags: 0,
        session_id,
        sequence,
        timestamp,
        key_epoch: 0,
        payload: Vec::new(),
    };
    let cipher = security.send_key().map_err(io::Error::other)?;
    packet.key_epoch = cipher.epoch();
    let header = packet
        .authenticated_header(payload.len() + 16)
        .map_err(io::Error::other)?;
    packet.payload = cipher
        .seal(&header, session_id, sequence, payload)
        .map_err(io::Error::other)?;
    packet.encode_into(output).map_err(io::Error::other)
}

fn ensure_pairwise(
    contexts: &Arc<Mutex<HashMap<u64, PairwiseSecurity>>>,
    security: &MediaSecurity,
    self_id: u64,
    peer: &PeerCandidates,
) -> io::Result<()> {
    // Outgoing contexts can be busy during peer cleanup. Fall back to SFU
    // instead of waiting on the 20 ms audio worker.
    let mut contexts = contexts
        .try_lock()
        .map_err(|_| io::Error::new(io::ErrorKind::WouldBlock, "P2P security busy"))?;
    if contexts.contains_key(&peer.peer_id) {
        return Ok(());
    }
    let public: [u8; 32] = STANDARD_NO_PAD
        .decode(&peer.security_key)
        .map_err(|_| io::Error::other("invalid peer security key"))?
        .try_into()
        .map_err(|_| io::Error::other("invalid peer security key"))?;
    contexts.insert(
        peer.peer_id,
        security
            .pairwise(public, self_id, peer.peer_id)
            .map_err(io::Error::other)?,
    );
    Ok(())
}

fn encode_pairwise_packet(
    contexts: &Arc<Mutex<HashMap<u64, PairwiseSecurity>>>,
    peer_id: u64,
    session_id: u64,
    sequence: u32,
    timestamp: u32,
    payload: &[u8],
    output: &mut Vec<u8>,
) -> io::Result<()> {
    let mut contexts = contexts
        .try_lock()
        .map_err(|_| io::Error::new(io::ErrorKind::WouldBlock, "P2P security busy"))?;
    let context = contexts
        .get_mut(&peer_id)
        .ok_or_else(|| io::Error::other("P2P key is not ready"))?;
    let mut packet = VoicePacket {
        kind: PacketType::Audio,
        flags: 0,
        session_id,
        sequence,
        timestamp,
        key_epoch: context.outbound_epoch(sequence).map_err(io::Error::other)?,
        payload: Vec::new(),
    };
    let header = packet
        .authenticated_header(payload.len() + 16)
        .map_err(io::Error::other)?;
    packet.payload = context
        .seal(&header, session_id, sequence, payload)
        .map_err(io::Error::other)?;
    packet.encode_into(output).map_err(io::Error::other)
}

fn receive_datagram(
    socket: &UdpSocket,
    buffer: &mut [u8],
    _server: SocketAddr,
) -> io::Result<(usize, SocketAddr)> {
    #[cfg(test)]
    if let Some(error) = tests::injected_receive_error(_server) {
        return Err(error);
    }
    socket.recv_from(buffer)
}

fn recoverable_udp_receive_error(error: &io::Error) -> bool {
    // On UDP, Windows WSAECONNRESET (10054) can be an ICMP Port
    // Unreachable response to a peer probe, not a broken socket. Rebuilding
    // here changes the SFU endpoint and can repeat on every failed probe.
    matches!(
        error.kind(),
        io::ErrorKind::TimedOut
            | io::ErrorKind::WouldBlock
            | io::ErrorKind::Interrupted
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::HostUnreachable
            | io::ErrorKind::NetworkUnreachable
    )
}

#[allow(clippy::too_many_arguments)]
fn send_server_probe(
    socket: &UdpSocket,
    server: SocketAddr,
    session_id: u64,
    next_challenge: &mut u64,
    state: &mut ServerProbeState,
    security: Option<&MediaSecurity>,
    sequence: &AtomicU32,
    rekey_state: &AtomicU8,
    events: &Sender<TransportEvent>,
) {
    state
        .pending
        .retain(|_, sent_at| sent_at.elapsed() < Duration::from_secs(5));
    *next_challenge = next_challenge.wrapping_add(1);
    let challenge = *next_challenge;
    let probe_sequence = if security.is_some() {
        let value = sequence.fetch_add(1, Ordering::Relaxed);
        if guard_shared_media_sequence(value, rekey_state, events).is_err() {
            state.last_sent = Instant::now();
            return;
        }
        value
    } else {
        challenge as u32
    };
    let mut bytes = Vec::with_capacity(HEADER_SIZE + 24);
    if encode_media_packet(
        PacketType::Probe,
        session_id,
        probe_sequence,
        0,
        &challenge.to_be_bytes(),
        security,
        &mut bytes,
    )
    .is_ok()
        && socket.send_to(&bytes, server).is_ok()
    {
        state.pending.insert(challenge, Instant::now());
        state.stats.sent = state.stats.sent.saturating_add(1);
    }
    state.last_sent = Instant::now();
}

fn observe_server_ack(packet: VoicePacket, state: &mut ServerProbeState) {
    let Ok(challenge_bytes) = packet.payload.as_slice().try_into() else {
        return;
    };
    let challenge = u64::from_be_bytes(challenge_bytes);
    let Some(sent_at) = state.pending.remove(&challenge) else {
        return;
    };
    let rtt = sent_at.elapsed().as_secs_f64() * 1_000.0;
    let first = state.stats.last_received.is_none();
    if first {
        state.stats.jitter = 0.0;
    } else {
        state.stats.jitter += ((rtt - state.stats.last_rtt).abs() - state.stats.jitter) * 0.125;
    }
    state.stats.last_rtt = rtt;
    state.stats.last_received = Some(Instant::now());
    state.stats.received = state.stats.received.saturating_add(1);
}

// Use the path that delivered the authenticated, winning audio copy. A
// qualified standby P2P route says nothing about media arriving through SFU.
fn record_peer_path_metrics(
    peer_id: u64,
    source: SocketAddr,
    server: SocketAddr,
    server_probe: &ProbeStats,
    manager: &Mutex<RouteManager>,
    output: &Mutex<HashMap<u64, TimedVoicePathMetrics>>,
) {
    let now = Instant::now();
    let sample = if source == server {
        server_probe
            .last_received
            .filter(|at| now.saturating_duration_since(*at) < Duration::from_secs(3))
            .map(|observed_at| TimedVoicePathMetrics {
                value: VoicePathMetrics {
                    rtt_ms: server_probe.last_rtt,
                    jitter_ms: server_probe.jitter,
                },
                observed_at,
            })
    } else {
        manager.try_lock().ok().and_then(|routes| {
            routes
                .fresh_metrics_for(peer_id, source, now)
                .map(|(metrics, observed_at)| TimedVoicePathMetrics {
                    value: VoicePathMetrics {
                        rtt_ms: metrics.rtt_ms,
                        jitter_ms: metrics.jitter_ms,
                    },
                    observed_at,
                })
        })
    };
    if let Ok(mut output) = output.lock() {
        if let Some(sample) = sample {
            output.insert(peer_id, sample);
        } else {
            // A path switch without fresh probes must invalidate the old path.
            output.remove(&peer_id);
        }
    }
}

fn publish_voice_path_metrics(
    route_status: Option<&RouteStatus>,
    manager: &Arc<Mutex<RouteManager>>,
    server: &ProbeStats,
    output: &Mutex<Option<TimedVoicePathMetrics>>,
) {
    let server_metrics = server
        .last_received
        .filter(|received| received.elapsed() < Duration::from_secs(3))
        .map(|_| VoicePathMetrics {
            rtt_ms: server.last_rtt,
            jitter_ms: server.jitter,
        });
    let direct_metrics = manager.try_lock().ok().and_then(|manager| {
        let peers = manager.peers();
        let routes = peers
            .iter()
            .filter_map(|peer| manager.best(*peer, true))
            .collect::<Vec<_>>();
        (!routes.is_empty()).then(|| VoicePathMetrics {
            rtt_ms: routes
                .iter()
                .map(|route| route.metrics.rtt_ms)
                .fold(0.0, f64::max),
            jitter_ms: routes
                .iter()
                .map(|route| route.metrics.jitter_ms)
                .fold(0.0, f64::max),
        })
    });
    let metrics = match route_status.map(|status| status.actual_transport.as_str()) {
        Some("P2P") => direct_metrics,
        Some("Hybrid") => {
            server_metrics
                .zip(direct_metrics)
                .map(|(server, direct)| VoicePathMetrics {
                    rtt_ms: server.rtt_ms.max(direct.rtt_ms),
                    jitter_ms: server.jitter_ms.max(direct.jitter_ms),
                })
        }
        Some("SFU" | "Relay") => server_metrics,
        _ => None,
    };
    if let Ok(mut target) = output.try_lock() {
        *target = metrics.map(|value| TimedVoicePathMetrics {
            value,
            observed_at: Instant::now(),
        });
    }
}

fn guard_shared_media_sequence(
    sequence: u32,
    rekey_state: &AtomicU8,
    events: &Sender<TransportEvent>,
) -> io::Result<()> {
    if sequence >= u32::MAX - 4096 {
        if rekey_state
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let _ = events.try_send(TransportEvent::RekeyRequired);
        }
    } else if sequence < 4096 {
        match rekey_state.load(Ordering::Acquire) {
            1 => {
                return Err(io::Error::other(
                    "media rekey did not complete before sequence wrap",
                ));
            }
            2 => rekey_state.store(0, Ordering::Release),
            _ => {}
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn rebuild_udp_sockets(
    server: SocketAddr,
    session_id: u64,
    security: Option<&MediaSecurity>,
    sequence: &AtomicU32,
    rekey_state: &AtomicU8,
    events: &Sender<TransportEvent>,
) -> io::Result<(PreparedSockets, UdpSocket)> {
    let mut last_error = None;
    for _ in 0..3 {
        let result = (|| {
            let socket = UdpSocket::bind(if server.is_ipv6() {
                "[::]:0"
            } else {
                "0.0.0.0:0"
            })?;
            socket.set_nonblocking(true)?;
            // Finish interface discovery before switching the server's endpoint.
            // Only the receiver installs the completed replacement.
            let candidates = local_candidates(server, socket.local_addr()?.port());
            let receiver = socket.try_clone()?;
            let probe = socket.try_clone()?;
            let maintenance = socket.try_clone()?;
            let registration_sequence = if security.is_some() {
                let value = sequence.fetch_add(1, Ordering::Relaxed);
                guard_shared_media_sequence(value, rekey_state, events)?;
                value
            } else {
                0
            };
            send_registration(&socket, server, session_id, registration_sequence, security)?;
            Ok((
                PreparedSockets {
                    sender: socket,
                    receiver,
                    probe,
                    candidates,
                },
                maintenance,
            ))
        })();
        match result {
            Ok(value) => return Ok(value),
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    Err(last_error.unwrap_or_else(|| io::Error::other("UDP recovery exhausted")))
}

#[allow(clippy::too_many_arguments)]
fn send_probes(
    socket: &UdpSocket,
    server: SocketAddr,
    session_id: u64,
    manager: &Arc<Mutex<RouteManager>>,
    desired: &Arc<Mutex<Desired>>,
    stats: &mut HashMap<(u64, SocketAddr), ProbeStats>,
    challenges: &mut HashMap<(u64, u64), ProbeChallenge>,
    next_challenge: &mut u64,
    generations: &mut HashMap<u64, u64>,
    probe_cursors: &mut HashMap<u64, usize>,
    events: &Sender<TransportEvent>,
) -> Option<RouteStatus> {
    let (peers, route_config) = {
        let desired = desired.try_lock().ok()?;
        (
            desired.peers.values().cloned().collect::<Vec<_>>(),
            desired.route.clone(),
        )
    };
    let mut routes = manager.try_lock().ok()?;
    let active_peers = peers.iter().map(|peer| peer.peer_id).collect::<Vec<_>>();
    routes.retain_peers(&active_peers);
    stats.retain(|(peer_id, _), _| active_peers.contains(peer_id));
    challenges.retain(|(peer_id, _), challenge| {
        active_peers.contains(peer_id) && challenge.sent_at.elapsed() < Duration::from_secs(5)
    });
    generations.retain(|peer_id, _| active_peers.contains(peer_id));
    probe_cursors.retain(|peer_id, _| active_peers.contains(peer_id));
    if route_config.as_ref().is_some_and(|config| {
        config.room_network_policy == "force_sfu" || config.reason == "forced_sfu"
    }) {
        // Respect both room policy and a member forcing SFU. Ordinary SFU
        // fallback must still probe so automatic routing can qualify P2P.
        routes.retain_peers(&[]);
        stats.clear();
        challenges.clear();
        generations.clear();
        probe_cursors.clear();
        return Some(sfu_status(server, "SFU forced; peer probes disabled"));
    }
    for peer in &peers {
        if generations.get(&peer.peer_id) != Some(&peer.generation) {
            routes.remove_peer(peer.peer_id);
            stats.retain(|(peer_id, _), _| *peer_id != peer.peer_id);
            challenges.retain(|(peer_id, _), _| *peer_id != peer.peer_id);
            generations.insert(peer.peer_id, peer.generation);
            probe_cursors.remove(&peer.peer_id);
        }
    }
    let planned_transport = route_config
        .as_ref()
        .map_or("sfu", |config| config.transport.as_str())
        .to_owned();
    let degrade_duration = route_config
        .as_ref()
        .map_or(Duration::from_secs(2), |config| {
            Duration::from_millis(config.degrade_duration_ms)
        });
    if let Some(config) = route_config {
        routes.configure(
            Duration::from_millis(config.qualification_ms),
            RouteThresholds {
                rtt_ms: config.max_rtt_ms,
                loss_percent: config.max_loss_percent,
                jitter_ms: config.max_jitter_ms,
            },
            RouteThresholds {
                rtt_ms: config.degrade_rtt_ms,
                loss_percent: config.degrade_loss_percent,
                jitter_ms: config.degrade_jitter_ms,
            },
        );
    }
    routes.configure_degradation_duration(degrade_duration);
    let mut probe_packet = Vec::with_capacity(HEADER_SIZE + 8);
    for peer in peers {
        let mut endpoints = Vec::<SocketAddr>::new();
        for candidate in peer.candidates {
            let Ok(advertised_endpoint) = candidate.address.parse::<SocketAddr>() else {
                continue;
            };
            let kind = match candidate.kind.as_str() {
                "host" => RouteKind::LanDirect,
                "ipv6" => RouteKind::Ipv6Direct,
                _ => RouteKind::P2PDirect,
            };
            for endpoint in punch_targets(&candidate.kind, advertised_endpoint) {
                routes.add_candidate(peer.peer_id, kind, endpoint);
                if !endpoints.contains(&endpoint) {
                    endpoints.push(endpoint);
                }
                stats.entry((peer.peer_id, endpoint)).or_default();
            }
        }
        // Retire every expired qualification before choosing a fallback.
        // An alternate candidate is useful only with fresh ACK evidence.
        let expired: Vec<_> = stats
            .iter()
            .filter_map(|(&(id, endpoint), entry)| {
                (id == peer.peer_id
                    && entry.reported
                    && entry
                        .last_received
                        .is_none_or(|last| last.elapsed() >= degrade_duration))
                .then_some(endpoint)
            })
            .collect();
        for endpoint in &expired {
            stats.get_mut(&(peer.peer_id, *endpoint)).unwrap().reported = false;
            routes.degrade_route(peer.peer_id, *endpoint);
        }
        if !expired.is_empty() && routes.best(peer.peer_id, true).is_none() {
            let _ = events.try_send(TransportEvent::NetworkReport(NetworkReport {
                peer_id: peer.peer_id,
                qualified: false,
                rtt_ms: 0.0,
                packet_loss_percent: 100.0,
                jitter_ms: 0.0,
            }));
        }
        let probe_endpoints = routes.best(peer.peer_id, true).map_or_else(
            || {
                scheduled_probe_endpoints(
                    &endpoints,
                    probe_cursors.entry(peer.peer_id).or_default(),
                )
            },
            |route| vec![route.endpoint],
        );
        for endpoint in probe_endpoints {
            let entry = stats.entry((peer.peer_id, endpoint)).or_default();
            entry.sent += 1;
            *next_challenge = next_challenge.wrapping_add(1);
            let challenge = *next_challenge;
            challenges.insert(
                (peer.peer_id, challenge),
                ProbeChallenge {
                    sent_at: Instant::now(),
                    target: endpoint,
                },
            );
            let payload = challenge.to_be_bytes();
            if VoicePacket::encode_parts_into(
                PacketType::Probe,
                0,
                session_id,
                entry.sent as u32,
                challenge as u32,
                0,
                &payload,
                &mut probe_packet,
            )
            .is_ok()
            {
                let _ = socket.send_to(&probe_packet, endpoint);
            }
        }
    }
    let mut status = actual_route_status(&routes, &active_peers, &planned_transport, server);
    let mut probe_details = stats
        .iter()
        .map(|((peer_id, endpoint), probe)| {
            format!(
                "peer {peer_id} {endpoint}: {}/{} ACK",
                probe.received, probe.sent
            )
        })
        .collect::<Vec<_>>();
    probe_details.sort();
    if !probe_details.is_empty() {
        status.qualification = format!("{} · {}", status.qualification, probe_details.join(" · "));
    }
    Some(status)
}

fn scheduled_probe_endpoints(endpoints: &[SocketAddr], cursor: &mut usize) -> Vec<SocketAddr> {
    if endpoints.is_empty() {
        return Vec::new();
    }
    let count = endpoints.len().min(MAX_PROBES_PER_PEER_CYCLE);
    let start = *cursor % endpoints.len();
    let selected = (0..count)
        .map(|offset| endpoints[(start + offset) % endpoints.len()])
        .collect();
    *cursor = (start + count) % endpoints.len();
    selected
}

fn actual_route_status(
    routes: &RouteManager,
    expected_peers: &[u64],
    planned_transport: &str,
    server: SocketAddr,
) -> RouteStatus {
    let qualified = expected_peers
        .iter()
        .filter_map(|peer| routes.best(*peer, true))
        .collect::<Vec<_>>();
    let qualification = if expected_peers.is_empty() {
        "Waiting for peers".to_owned()
    } else if qualified.len() == expected_peers.len() {
        format!("Qualified ({}/{})", qualified.len(), expected_peers.len())
    } else {
        format!("Evaluating ({}/{})", qualified.len(), expected_peers.len())
    };
    let direct_endpoint = qualified.first().map(|route| {
        if qualified.len() == 1 {
            route.endpoint.to_string()
        } else {
            format!("{} (+{} peers)", route.endpoint, qualified.len() - 1)
        }
    });
    let direct_kind = qualified.first().map(|route| route_kind_label(route.kind));
    match planned_transport {
        "p2p"
            if qualified
                .iter()
                .any(|route| route.needs_sfu_protection(Instant::now())) =>
        {
            RouteStatus {
                actual_transport: "Hybrid".into(),
                selected_route: "P2P + SFU protection".into(),
                endpoint: format!("{} + {}", direct_endpoint.unwrap_or_default(), server),
                qualification,
            }
        }
        "p2p" if !expected_peers.is_empty() && qualified.len() == expected_peers.len() => {
            RouteStatus {
                actual_transport: "P2P".into(),
                selected_route: direct_kind.unwrap_or("P2P Direct").into(),
                endpoint: direct_endpoint.unwrap_or_default(),
                qualification,
            }
        }
        "hybrid" if !qualified.is_empty() => RouteStatus {
            actual_transport: "Hybrid".into(),
            selected_route: format!("{} + SFU", direct_kind.unwrap_or("P2P Direct")),
            endpoint: format!("{} + {}", direct_endpoint.unwrap_or_default(), server),
            qualification,
        },
        "relay" => RouteStatus {
            actual_transport: "Relay".into(),
            selected_route: "Server Relay".into(),
            endpoint: server.to_string(),
            qualification,
        },
        "p2p" => RouteStatus {
            actual_transport: "SFU".into(),
            selected_route: "SFU fallback (P2P not ready)".into(),
            endpoint: server.to_string(),
            qualification,
        },
        _ => RouteStatus {
            actual_transport: "SFU".into(),
            selected_route: "Packet SFU".into(),
            endpoint: server.to_string(),
            qualification,
        },
    }
}

fn sfu_status(server: SocketAddr, qualification: &str) -> RouteStatus {
    RouteStatus {
        actual_transport: "SFU".into(),
        selected_route: "Packet SFU".into(),
        endpoint: server.to_string(),
        qualification: qualification.into(),
    }
}

fn route_kind_label(kind: RouteKind) -> &'static str {
    match kind {
        RouteKind::LanDirect => "LAN Direct",
        RouteKind::Ipv6Direct => "IPv6 Direct",
        RouteKind::P2PDirect => "IPv4 P2P Direct",
        RouteKind::Sfu => "Packet SFU",
        RouteKind::Relay => "Server Relay",
    }
}

fn observe_ack(
    packet: VoicePacket,
    source: SocketAddr,
    manager: &Arc<Mutex<RouteManager>>,
    desired: &Arc<Mutex<Desired>>,
    events: &Sender<TransportEvent>,
    stats: &mut HashMap<(u64, SocketAddr), ProbeStats>,
    challenges: &mut HashMap<(u64, u64), ProbeChallenge>,
) {
    if packet.payload.len() != 8 || !authorize_peer_source(desired, packet.session_id, source) {
        return;
    }
    let Ok(challenge_bytes) = packet.payload[..8].try_into() else {
        return;
    };
    let challenge_id = u64::from_be_bytes(challenge_bytes);
    let Some(challenge) = challenges.remove(&(packet.session_id, challenge_id)) else {
        return;
    };
    let rtt = challenge.sent_at.elapsed().as_secs_f64() * 1_000.0;
    let entry = stats.entry((packet.session_id, source)).or_default();
    // Candidate exchange is asynchronous. A peer can legitimately ignore the
    // first probes until it has learned our candidate, so those warm-up sends
    // must not poison the qualification loss rate. Begin measurement with the
    // first acknowledged probe rather than the first attempted probe.
    let starts_measurement_epoch = entry.last_received.is_none();
    if starts_measurement_epoch {
        entry.sent = 0;
        entry.received = 0;
        entry.last_rtt = rtt;
        entry.jitter = 0.0;
    }
    if starts_measurement_epoch || challenge.target != source {
        entry.sent += 1;
    }
    entry.received += 1;
    entry.last_received = Some(Instant::now());
    entry.jitter += ((rtt - entry.last_rtt).abs() - entry.jitter) * 0.125;
    entry.last_rtt = rtt;
    let (qualified, loss) = manager
        .lock()
        .map(|mut manager| {
            if !manager.has_route(packet.session_id, source) {
                manager.add_candidate(packet.session_id, RouteKind::P2PDirect, source);
            }
            let qualified = manager.observe(
                packet.session_id,
                source,
                crate::transport::RouteMetrics {
                    rtt_ms: rtt,
                    jitter_ms: entry.jitter,
                    packets_received: entry.received,
                    packets_sent: entry.sent,
                    ..Default::default()
                },
                Instant::now(),
            );
            let loss = manager
                .metrics_for(packet.session_id, source)
                .map_or(100.0, |metrics| metrics.packet_loss_percent);
            (qualified, loss)
        })
        .unwrap_or((false, 100.0));
    let report_due = !entry.reported
        || entry
            .last_reported
            .is_none_or(|last_reported| last_reported.elapsed() >= Duration::from_secs(1));
    if !qualified && entry.reported {
        entry.reported = false;
        entry.last_reported = Some(Instant::now());
        let _ = events.try_send(TransportEvent::NetworkReport(NetworkReport {
            peer_id: packet.session_id,
            qualified: false,
            rtt_ms: rtt,
            packet_loss_percent: loss,
            jitter_ms: entry.jitter,
        }));
        return;
    }
    if qualified && report_due {
        entry.reported = true;
        entry.last_reported = Some(Instant::now());
        let _ = events.try_send(TransportEvent::NetworkReport(NetworkReport {
            peer_id: packet.session_id,
            qualified: true,
            rtt_ms: rtt,
            packet_loss_percent: loss,
            jitter_ms: entry.jitter,
        }));
    }
}

// A NAT can allocate a different source port for the peer-to-peer destination
// than it allocated for the server. Accept that peer-reflexive endpoint only
// when its IP was already vouched for by the server's candidate exchange.
fn authorize_peer_source(desired: &Arc<Mutex<Desired>>, peer: u64, source: SocketAddr) -> bool {
    desired
        .lock()
        .map(|desired| {
            desired.peers.get(&peer).is_some_and(|peer| {
                peer.candidates.iter().any(|candidate| {
                    candidate
                        .address
                        .parse::<SocketAddr>()
                        .is_ok_and(|candidate| candidate.ip() == source.ip())
                })
            })
        })
        .unwrap_or(false)
}

// frp XTCP predicts broader port ranges after classifying NAT behavior. Voice
// traffic uses a deliberately small adjacent-port window: it handles common
// sequential mappings without creating a noisy random-port scan.
fn punch_targets(kind: &str, endpoint: SocketAddr) -> Vec<SocketAddr> {
    if kind != "server_reflexive" {
        return vec![endpoint];
    }
    (-2_i32..=2)
        .filter_map(|offset| {
            let port = i32::from(endpoint.port()) + offset;
            (1..=i32::from(u16::MAX))
                .contains(&port)
                .then(|| SocketAddr::new(endpoint.ip(), port as u16))
        })
        .collect()
}

fn unix_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

fn send_registration(
    socket: &UdpSocket,
    server: SocketAddr,
    session_id: u64,
    sequence: u32,
    security: Option<&MediaSecurity>,
) -> io::Result<()> {
    #[cfg(test)]
    tests::pause_maintenance_operation(server, 0);
    let mut registration = Vec::with_capacity(HEADER_SIZE + 16);
    encode_media_packet(
        PacketType::Register,
        session_id,
        sequence,
        0,
        &[],
        security,
        &mut registration,
    )?;
    socket.send_to(&registration, server).map(|_| ())
}

fn route_local_ip(server: SocketAddr) -> Option<IpAddr> {
    #[cfg(test)]
    tests::pause_maintenance_operation(server, 1);
    let socket = UdpSocket::bind(if server.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    })
    .ok()?;
    socket.connect(server).ok()?;
    Some(socket.local_addr().ok()?.ip())
}

fn local_candidates(server: SocketAddr, port: u16) -> Vec<Candidate> {
    let preferred = route_local_ip(server);
    let mut ips = if_addrs::get_if_addrs()
        .map(|interfaces| {
            interfaces
                .into_iter()
                .map(|interface| interface.ip())
                .collect()
        })
        .unwrap_or_else(|_| preferred.into_iter().collect::<Vec<_>>());
    if let Some(ip) = preferred
        && !ips.contains(&ip)
    {
        ips.push(ip);
    }
    ips.retain(|ip| {
        !ip.is_unspecified()
            && !ip.is_multicast()
            && ip.is_ipv6() == server.is_ipv6()
            && !matches!(ip, IpAddr::V4(ip) if ip.is_link_local())
            && !matches!(ip, IpAddr::V6(ip) if ip.is_unicast_link_local())
            && (!ip.is_loopback() || Some(*ip) == preferred)
    });
    ips.sort_unstable();
    ips.dedup();
    ips.sort_by_key(|ip| if Some(*ip) == preferred { 0 } else { 1 });
    ips.into_iter()
        .map(|ip| Candidate {
            kind: if ip.is_ipv6() { "ipv6" } else { "host" }.into(),
            address: SocketAddr::new(ip, port).to_string(),
            priority: if Some(ip) == preferred {
                500
            } else if ip.is_loopback() {
                200
            } else {
                400
            },
        })
        .collect()
}

fn same_candidates(left: &[Candidate], right: &[Candidate]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.kind == right.kind
                && left.address == right.address
                && left.priority == right.priority
        })
}

pub(super) fn resolve_server(
    control_address: &str,
    advertised_voice_address: &str,
) -> io::Result<SocketAddr> {
    let control = control_address.to_socket_addrs()?.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "control address did not resolve",
        )
    })?;
    let advertised: Vec<_> = advertised_voice_address.to_socket_addrs()?.collect();
    let first = advertised.first().copied().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "voice address did not resolve")
    })?;
    if first.ip().is_unspecified() {
        return Ok(SocketAddr::new(control.ip(), first.port()));
    }
    Ok(advertised
        .iter()
        .copied()
        .find(|endpoint| endpoint.ip() == control.ip())
        .or_else(|| {
            advertised
                .iter()
                .copied()
                .find(|endpoint| endpoint.is_ipv4() == control.is_ipv4())
        })
        .unwrap_or(first))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::RouteMetrics;

    static RECEIVE_ERRORS: std::sync::LazyLock<
        Mutex<HashMap<SocketAddr, Arc<InjectedReceiveState>>>,
    > = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

    struct InjectedReceiveState {
        kind: io::ErrorKind,
        pending: AtomicBool,
        observed: AtomicBool,
    }
    struct InjectedReceiveError {
        server: SocketAddr,
        state: Arc<InjectedReceiveState>,
    }
    impl std::ops::Deref for InjectedReceiveError {
        type Target = InjectedReceiveState;
        fn deref(&self) -> &Self::Target {
            &self.state
        }
    }
    impl InjectedReceiveError {
        fn new(server: SocketAddr, kind: io::ErrorKind) -> Self {
            let state = Arc::new(InjectedReceiveState {
                kind,
                pending: AtomicBool::new(false),
                observed: AtomicBool::new(false),
            });
            RECEIVE_ERRORS.lock().unwrap().insert(server, state.clone());
            Self { server, state }
        }
    }
    impl Drop for InjectedReceiveError {
        fn drop(&mut self) {
            RECEIVE_ERRORS.lock().unwrap().remove(&self.server);
        }
    }
    pub(super) fn injected_receive_error(server: SocketAddr) -> Option<io::Error> {
        let state = RECEIVE_ERRORS.lock().unwrap().get(&server).cloned()?;
        if state.pending.swap(false, Ordering::AcqRel) {
            state.observed.store(true, Ordering::Release);
            Some(io::Error::from(state.kind))
        } else {
            None
        }
    }

    type MaintenanceGate = (Sender<()>, Receiver<()>);
    static MAINTENANCE_GATES: std::sync::LazyLock<
        Mutex<HashMap<(SocketAddr, u8), MaintenanceGate>>,
    > = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

    pub(super) fn pause_maintenance_operation(server: SocketAddr, operation: u8) {
        let gate = MAINTENANCE_GATES
            .lock()
            .unwrap()
            .get(&(server, operation))
            .cloned();
        if let Some((entered, release)) = gate {
            let _ = entered.try_send(());
            let _ = release.recv_timeout(Duration::from_secs(5));
        }
    }

    struct MaintenancePause {
        key: (SocketAddr, u8),
        entered: Receiver<()>,
        _release: Sender<()>,
    }

    impl MaintenancePause {
        fn new(server: SocketAddr, operation: u8) -> Self {
            let (entered_tx, entered) = bounded(8);
            let (release, release_rx) = bounded(1);
            MAINTENANCE_GATES
                .lock()
                .unwrap()
                .insert((server, operation), (entered_tx, release_rx));
            Self {
                key: (server, operation),
                entered,
                _release: release,
            }
        }
    }

    impl Drop for MaintenancePause {
        fn drop(&mut self) {
            MAINTENANCE_GATES.lock().unwrap().remove(&self.key);
            // Dropping the only release sender wakes any paused worker, also
            // on an assertion failure. Tests never leave a background gate held.
        }
    }

    #[test]
    fn audio_send_does_not_wait_for_socket_handoff_and_rebuilt_sockets_are_nonblocking() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let endpoint = server.local_addr().unwrap();
        let (control, events) = AdaptiveControl::new();
        let transport = AdaptiveTransport::connect(
            &endpoint.to_string(),
            &endpoint.to_string(),
            1,
            control,
            events.clone(),
            64,
            None,
        )
        .unwrap();
        let socket = transport.socket.clone();
        let (held_tx, held_rx) = bounded(1);
        let holder = thread::spawn(move || {
            let _guard = socket.lock().unwrap();
            held_tx.send(()).unwrap();
            thread::sleep(Duration::from_millis(200));
        });
        held_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let started = Instant::now();
        assert_eq!(
            transport.send_audio(b"voice").unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(started.elapsed() < Duration::from_millis(100));
        holder.join().unwrap();
        let (rebuilt, _) = rebuild_udp_sockets(
            endpoint,
            1,
            None,
            &transport.sequence,
            &transport.rekey_state,
            &events,
        )
        .unwrap();
        let mut bytes = [0; 32];
        for socket in [
            &*transport.socket.lock().unwrap(),
            &rebuilt.sender,
            &rebuilt.receiver,
            &rebuilt.probe,
        ] {
            let started = Instant::now();
            assert_eq!(
                socket.recv_from(&mut bytes).unwrap_err().kind(),
                io::ErrorKind::WouldBlock
            );
            assert!(started.elapsed() < Duration::from_millis(50));
        }
    }

    #[test]
    fn stalled_maintenance_does_not_block_sfu_media_including_encryption() {
        use crate::security::{EphemeralKey, SessionSecrets};
        for operation in [0, 1] {
            for encrypted in [false, true] {
                let secrets = SessionSecrets {
                    client_to_server_key: [1; 32],
                    server_to_client_key: [2; 32],
                    client_nonce_prefix: [3; 6],
                    server_nonce_prefix: [4; 6],
                };
                let reverse = SessionSecrets {
                    client_to_server_key: secrets.server_to_client_key,
                    server_to_client_key: secrets.client_to_server_key,
                    client_nonce_prefix: secrets.server_nonce_prefix,
                    server_nonce_prefix: secrets.client_nonce_prefix,
                };
                let security =
                    encrypted.then(|| MediaSecurity::new(secrets, &EphemeralKey::generate()));
                let remote_security =
                    encrypted.then(|| MediaSecurity::new(reverse, &EphemeralKey::generate()));
                let server = UdpSocket::bind("127.0.0.1:0").unwrap();
                let server_addr = server.local_addr().unwrap();
                let address = server_addr.to_string();
                let (control, events) = AdaptiveControl::new();
                let transport = AdaptiveTransport::connect(
                    &address, &address, 1, control, events, 64, security,
                )
                .unwrap();
                let client_addr = transport.socket.lock().unwrap().local_addr().unwrap();
                let client_addr = SocketAddr::new(server_addr.ip(), client_addr.port());
                let pause = MaintenancePause::new(server_addr, operation);
                pause
                    .entered
                    .recv_timeout(Duration::from_secs(2))
                    .expect("maintenance operation was not reached");
                let mut bytes = Vec::new();
                encode_media_packet(
                    PacketType::Audio,
                    2,
                    1,
                    0,
                    b"unblocked voice",
                    remote_security.as_ref(),
                    &mut bytes,
                )
                .unwrap();
                let started = Instant::now();
                server.send_to(&bytes, client_addr).unwrap();
                let packet = loop {
                    if let Some(packet) = transport.try_receive() {
                        break packet;
                    }
                    assert!(
                        started.elapsed() < Duration::from_millis(200),
                        "maintenance blocked media: operation={operation}, encrypted={encrypted}"
                    );
                    thread::sleep(Duration::from_millis(1));
                };
                assert_eq!(packet.payload, b"unblocked voice");
                // Keep the gate closed during an outgoing frame too: the
                // maintenance worker must not hold the shared send/cipher lock.
                transport.send_audio(b"outgoing voice").unwrap();
                assert!(started.elapsed() < Duration::from_millis(200));
                drop(transport);
                drop(pause);
            }
        }
    }

    #[test]
    fn busy_pairwise_security_falls_back_to_sfu_without_waiting() {
        use crate::security::{EphemeralKey, SessionSecrets};
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let endpoint = server.local_addr().unwrap();
        let (control, events) = AdaptiveControl::new();
        let security = MediaSecurity::new(
            SessionSecrets {
                client_to_server_key: [1; 32],
                server_to_client_key: [2; 32],
                client_nonce_prefix: [3; 6],
                server_nonce_prefix: [4; 6],
            },
            &EphemeralKey::generate(),
        );
        let transport = AdaptiveTransport::connect(
            &endpoint.to_string(),
            &endpoint.to_string(),
            1,
            control.clone(),
            events,
            64,
            Some(security),
        )
        .unwrap();
        control.set_peer_candidates(PeerCandidates {
            peer_id: 2,
            generation: 1,
            security_key: String::new(),
            candidates: vec![],
        });
        control.set_route(test_route());
        {
            let mut routes = transport.manager.lock().unwrap();
            *routes = RouteManager::new(Duration::ZERO);
            routes.add_candidate(2, RouteKind::LanDirect, endpoint);
            assert!(routes.observe(
                2,
                endpoint,
                RouteMetrics {
                    rtt_ms: 1.0,
                    packets_sent: 10,
                    packets_received: 10,
                    ..Default::default()
                },
                Instant::now()
            ));
        }
        let contexts = transport.pairwise.clone();
        let held = contexts.lock().unwrap();
        let sender_contexts = contexts.clone();
        let (done_tx, done_rx) = bounded(1);
        let sender = thread::spawn(move || {
            // Cover both lock acquisitions even if route maintenance changes
            // the qualified-route snapshot while this test is running.
            let peer = PeerCandidates {
                peer_id: 2,
                generation: 1,
                security_key: String::new(),
                candidates: vec![],
            };
            assert_eq!(
                ensure_pairwise(
                    &sender_contexts,
                    transport.security.as_ref().unwrap(),
                    1,
                    &peer
                )
                .unwrap_err()
                .kind(),
                io::ErrorKind::WouldBlock
            );
            assert_eq!(
                encode_pairwise_packet(&sender_contexts, 2, 1, 1, 0, b"voice", &mut Vec::new())
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::WouldBlock
            );
            let deadline = Instant::now() + Duration::from_millis(100);
            let result = loop {
                match transport.send_audio(b"voice") {
                    Err(error)
                        if error.kind() == io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(1));
                    }
                    result => break result,
                }
            };
            let _ = done_tx.send(result);
        });
        let result = done_rx.recv_timeout(Duration::from_millis(200));
        drop(held);
        sender.join().unwrap();
        result
            .expect("P2P security lock blocked the audio sender")
            .unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut buffer = [0; MAX_PACKET_SIZE + 1];
        loop {
            let (size, _) = server.recv_from(&mut buffer).unwrap();
            let packet = VoicePacket::decode(&buffer[..size]).unwrap();
            if packet.kind == PacketType::Audio {
                assert_eq!(packet.timestamp % 960, 0);
                assert!(packet.payload.len() > b"voice".len());
                break;
            }
        }
    }
    fn wait_for_local_candidate(control: &AdaptiveControl) -> Candidate {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(TransportEvent::LocalCandidates(candidates)) = control.try_event() {
                return candidates.into_iter().next().unwrap();
            }
            assert!(Instant::now() < deadline, "local candidate was not emitted");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn test_route() -> RouteUpdate {
        RouteUpdate {
            room_id: 1,
            topology: "p2p_evaluating".into(),
            transport: "p2p".into(),
            reason: "test".into(),
            room_network_policy: "auto".into(),
            qualification_ms: 100,
            minimum_dwell_ms: 0,
            migration_cooldown_ms: 0,
            degrade_duration_ms: 1_500,
            max_rtt_ms: 80.0,
            max_loss_percent: 2.0,
            max_jitter_ms: 15.0,
            degrade_rtt_ms: 250.0,
            degrade_loss_percent: 5.0,
            degrade_jitter_ms: 40.0,
        }
    }

    #[test]
    fn suspect_p2p_sends_sfu_copy_before_dequalification_and_recovers() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let endpoint = peer.local_addr().unwrap();
        let (control, events) = AdaptiveControl::new();
        control.set_route(test_route());
        control.set_peer_candidates(PeerCandidates {
            peer_id: 2,
            generation: 1,
            security_key: String::new(),
            candidates: vec![],
        });
        let mut routes = RouteManager::new(Duration::ZERO);
        routes.add_candidate(2, RouteKind::P2PDirect, endpoint);
        let sample = RouteMetrics {
            rtt_ms: 10.0,
            packets_sent: 10,
            packets_received: 10,
            ..Default::default()
        };
        routes.observe(
            2,
            endpoint,
            sample,
            Instant::now() - Duration::from_millis(600),
        );
        let transport = AdaptiveTransport {
            socket: Arc::new(Mutex::new(UdpSocket::bind("127.0.0.1:0").unwrap())),
            server: server.local_addr().unwrap(),
            session_id: 1,
            sequence: control.media.sequence.clone(),
            timestamp: control.media.timestamp.clone(),
            incoming: bounded(1).1,
            alive: Arc::new(AtomicBool::new(false)),
            manager: Arc::new(Mutex::new(routes)),
            desired: control.desired.clone(),
            packet_buffer: Mutex::new(Vec::new()),
            security: None,
            pairwise: control.media.pairwise.clone(),
            events,
            rekey_state: control.rekey_state.clone(),
            consecutive_send_failures: Arc::new(AtomicU8::new(0)),
            repair_requested: Arc::new(MaintenanceSignal::new()),
            voice_path_metrics: Arc::new(Mutex::new(None)),
            peer_path_metrics: Arc::new(Mutex::new(HashMap::new())),
        };
        transport.send_audio(b"protected").unwrap();
        let mut direct = [0; MAX_PACKET_SIZE];
        let direct_length = peer.recv(&mut direct).unwrap();
        let mut backup = [0; MAX_PACKET_SIZE];
        let backup_length = server.recv(&mut backup).unwrap();
        assert_eq!(&direct[..direct_length], &backup[..backup_length]);
        {
            let mut routes = transport.manager.lock().unwrap();
            assert!(
                routes.best(2, true).is_some(),
                "protection must preserve qualification"
            );
            assert_eq!(
                actual_route_status(&routes, &[2], "p2p", transport.server).actual_transport,
                "Hybrid"
            );
            routes.observe(2, endpoint, sample, Instant::now());
            assert_eq!(
                actual_route_status(&routes, &[2], "p2p", transport.server).actual_transport,
                "P2P"
            );
        }
        transport.send_audio(b"recovered").unwrap();
        let length = peer.recv(&mut direct).unwrap();
        assert_eq!(
            VoicePacket::decode(&direct[..length]).unwrap().payload,
            b"recovered"
        );
        server.set_nonblocking(true).unwrap();
        assert_eq!(
            server.recv(&mut backup).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn reports_sfu_when_p2p_is_planned_but_not_qualified() {
        let routes = RouteManager::new(Duration::from_secs(20));
        let server = "127.0.0.1:9001".parse().unwrap();
        let status = actual_route_status(&routes, &[2], "p2p", server);
        assert_eq!(status.actual_transport, "SFU");
        assert!(status.selected_route.contains("fallback"));
    }

    #[test]
    fn candidate_refresh_detects_vpn_interface_changes() {
        let base = vec![Candidate {
            kind: "host".into(),
            address: "192.0.2.10:4000".into(),
            priority: 500,
        }];
        assert!(same_candidates(&base, &base));
        let vpn = vec![Candidate {
            kind: "host".into(),
            address: "198.51.100.20:4000".into(),
            priority: 500,
        }];
        assert!(!same_candidates(&base, &vpn));
    }

    #[test]
    fn reports_the_actual_qualified_direct_endpoint() {
        let endpoint = "127.0.0.1:41000".parse().unwrap();
        let server = "127.0.0.1:9001".parse().unwrap();
        let mut routes = RouteManager::new(Duration::ZERO);
        routes.add_candidate(2, RouteKind::LanDirect, endpoint);
        assert!(routes.observe(
            2,
            endpoint,
            RouteMetrics {
                rtt_ms: 2.0,
                jitter_ms: 0.2,
                packets_sent: 10,
                packets_received: 10,
                ..Default::default()
            },
            Instant::now(),
        ));
        let status = actual_route_status(&routes, &[2], "p2p", server);
        assert_eq!(status.actual_transport, "P2P");
        assert_eq!(status.selected_route, "LAN Direct");
        assert_eq!(status.endpoint, endpoint.to_string());
    }

    #[test]
    fn missing_room_peer_keeps_p2p_on_sfu_fallback() {
        let endpoint = "127.0.0.1:41000".parse().unwrap();
        let server = "127.0.0.1:9001".parse().unwrap();
        let mut routes = RouteManager::new(Duration::ZERO);
        routes.add_candidate(2, RouteKind::LanDirect, endpoint);
        assert!(routes.observe(
            2,
            endpoint,
            RouteMetrics {
                rtt_ms: 2.0,
                jitter_ms: 0.2,
                packets_sent: 10,
                packets_received: 10,
                ..Default::default()
            },
            Instant::now(),
        ));

        let manager = Arc::new(Mutex::new(routes));
        let expected_peers = [2, 3, 4];
        let QualifiedTargets {
            peer_count,
            targets,
            ..
        } = try_qualified_targets(&manager, &expected_peers).expect("route lock");
        assert_eq!(peer_count, 3);
        assert_eq!(targets, vec![(2, endpoint)]);

        let status = actual_route_status(&manager.lock().unwrap(), &expected_peers, "p2p", server);
        assert_eq!(status.actual_transport, "SFU");
        assert_eq!(status.qualification, "Evaluating (1/3)");
    }

    #[test]
    fn server_probe_ack_produces_fresh_sfu_voice_metrics() {
        let challenge = 42_u64;
        let mut server_probe = ServerProbeState::default();
        server_probe
            .pending
            .insert(challenge, Instant::now() - Duration::from_millis(12));
        observe_server_ack(
            VoicePacket {
                kind: PacketType::ProbeAck,
                flags: 0,
                session_id: 7,
                sequence: 1,
                timestamp: 0,
                key_epoch: 0,
                payload: challenge.to_be_bytes().to_vec(),
            },
            &mut server_probe,
        );

        let manager = Arc::new(Mutex::new(RouteManager::new(Duration::ZERO)));
        let output = Mutex::new(None);
        let status = RouteStatus {
            actual_transport: "SFU".into(),
            selected_route: "Server SFU".into(),
            endpoint: "127.0.0.1:9001".into(),
            qualification: String::new(),
        };
        publish_voice_path_metrics(Some(&status), &manager, &server_probe.stats, &output);

        let metrics = fresh_voice_path_metrics(&output).unwrap();
        assert!(metrics.rtt_ms >= 10.0);
        assert_eq!(metrics.jitter_ms, 0.0);
    }

    #[test]
    fn peer_feedback_tracks_the_winning_media_path_and_expires_probe_evidence() {
        let server = "127.0.0.1:9001".parse().unwrap();
        let direct = "127.0.0.1:41000".parse().unwrap();
        let now = Instant::now();
        let mut routes = RouteManager::new(Duration::ZERO);
        routes.add_candidate(2, RouteKind::LanDirect, direct);
        routes.observe(
            2,
            direct,
            RouteMetrics {
                rtt_ms: 2.0,
                jitter_ms: 0.2,
                packets_sent: 10,
                packets_received: 10,
                ..Default::default()
            },
            now,
        );
        let manager = Mutex::new(routes);
        let output = Mutex::new(HashMap::new());
        let mut probe = ProbeStats {
            last_received: Some(now),
            last_rtt: 140.0,
            jitter: 18.0,
            ..Default::default()
        };
        // Forced/fallback SFU ignores the healthy standby direct route.
        record_peer_path_metrics(2, server, server, &probe, &manager, &output);
        assert_eq!(output.lock().unwrap()[&2].value.rtt_ms, 140.0);
        // Hybrid uses the path of the unique copy that actually arrived.
        record_peer_path_metrics(2, direct, server, &probe, &manager, &output);
        assert_eq!(output.lock().unwrap()[&2].value.rtt_ms, 2.0);
        probe.last_received = Some(now - Duration::from_secs(4));
        record_peer_path_metrics(2, server, server, &probe, &manager, &output);
        assert!(!output.lock().unwrap().contains_key(&2));
        // Media arrival cannot refresh stale probe measurements.
        let mut routes = manager.lock().unwrap();
        routes.observe(
            2,
            direct,
            RouteMetrics::default(),
            now - Duration::from_secs(4),
        );
        drop(routes);
        record_peer_path_metrics(2, direct, server, &probe, &manager, &output);
        assert!(!output.lock().unwrap().contains_key(&2));
    }

    #[test]
    fn hybrid_voice_metrics_cover_both_server_and_direct_legs() {
        let endpoint = "127.0.0.1:41000".parse().unwrap();
        let mut routes = RouteManager::new(Duration::ZERO);
        routes.add_candidate(2, RouteKind::LanDirect, endpoint);
        assert!(routes.observe(
            2,
            endpoint,
            RouteMetrics {
                rtt_ms: 35.0,
                jitter_ms: 4.0,
                packets_sent: 1,
                packets_received: 1,
                ..Default::default()
            },
            Instant::now(),
        ));
        let manager = Arc::new(Mutex::new(routes));
        let server = ProbeStats {
            last_received: Some(Instant::now()),
            last_rtt: 20.0,
            jitter: 6.0,
            ..Default::default()
        };
        let output = Mutex::new(None);
        let status = RouteStatus {
            actual_transport: "Hybrid".into(),
            selected_route: String::new(),
            endpoint: String::new(),
            qualification: String::new(),
        };
        publish_voice_path_metrics(Some(&status), &manager, &server, &output);

        let metrics = fresh_voice_path_metrics(&output).unwrap();
        assert_eq!(metrics.rtt_ms, 35.0);
        assert_eq!(metrics.jitter_ms, 6.0);
    }

    #[test]
    fn realtime_route_snapshot_never_waits_for_probe_bookkeeping() {
        let manager = Arc::new(Mutex::new(RouteManager::new(Duration::ZERO)));
        let guard = manager.lock().unwrap();
        assert!(try_qualified_targets(&manager, &[2]).is_none());
        drop(guard);
        assert_eq!(
            try_qualified_targets(&manager, &[2]),
            Some(QualifiedTargets {
                peer_count: 1,
                ..Default::default()
            })
        );
    }

    #[test]
    fn hybrid_duplicates_cannot_evict_another_speakers_unique_audio() {
        let mut deduplicator = AudioDeduplicator::default();
        let (incoming, received) = bounded(2);
        let stale = received.clone();
        let packet = |session_id, timestamp| VoicePacket {
            kind: PacketType::Audio,
            flags: 0,
            session_id,
            sequence: timestamp / 960,
            timestamp,
            key_epoch: 0,
            payload: vec![session_id as u8],
        };
        let mut deliver = |packet: VoicePacket| {
            if deduplicator.accept(packet.session_id, packet.timestamp) {
                send_latest_packet(&incoming, &stale, packet, Instant::now());
            }
        };

        deliver(packet(2, 960));
        deliver(packet(3, 960));
        // Direct and SFU copies from speaker 3 arrive while the queue is full.
        deliver(packet(3, 960));
        deliver(packet(3, 960));

        assert_eq!(received.try_recv().unwrap().packet.session_id, 2);
        assert_eq!(received.try_recv().unwrap().packet.session_id, 3);
        assert!(received.try_recv().is_err());
    }

    #[test]
    fn audio_deduplication_preserves_reordering_and_timestamp_wrap() {
        let mut deduplicator = AudioDeduplicator::default();
        assert!(deduplicator.accept(2, u32::MAX - 959));
        assert!(deduplicator.accept(2, 1));
        assert!(deduplicator.accept(2, u32::MAX - 1919));
        assert!(!deduplicator.accept(2, 1));

        for frame in 2..=RECENT_AUDIO_TIMESTAMPS_PER_PEER as u32 + 1 {
            assert!(deduplicator.accept(2, frame * 960));
        }
        assert!(deduplicator.accept(2, u32::MAX - 959));
    }

    #[test]
    fn audio_deduplication_expires_for_sender_restart() {
        let mut deduplicator = AudioDeduplicator::default();
        let started = Instant::now();
        assert!(deduplicator.accept_at(2, 0, started));
        assert!(!deduplicator.accept_at(2, 0, started + Duration::from_millis(20)));
        assert!(deduplicator.accept_at(2, 0, started + AUDIO_DUPLICATE_WINDOW));
    }

    #[test]
    fn saturated_udp_handoff_keeps_the_newest_media() {
        let (tx, rx) = bounded(2);
        let packet = |sequence| VoicePacket {
            kind: PacketType::Audio,
            flags: 0,
            session_id: 7,
            sequence,
            timestamp: sequence * 960,
            key_epoch: 0,
            payload: vec![sequence as u8],
        };
        let received_at = Instant::now();
        send_latest_packet(&tx, &rx, packet(1), received_at);
        send_latest_packet(&tx, &rx, packet(2), received_at);
        send_latest_packet(&tx, &rx, packet(3), received_at);
        let oldest = rx.try_recv().unwrap();
        assert_eq!(oldest.packet.sequence, 2);
        assert_eq!(oldest.received_at, received_at);
        let newest = rx.try_recv().unwrap();
        assert_eq!(newest.packet.sequence, 3);
        assert_eq!(newest.received_at, received_at);
    }

    #[test]
    fn adaptive_audio_preserves_ingress_time_and_expires_after_a_worker_stall() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let server_addr = server.local_addr().unwrap();
        let address = server_addr.to_string();
        let (control, events) = AdaptiveControl::new();
        let transport =
            AdaptiveTransport::connect(&address, &address, 1, control, events, 16, None).unwrap();
        let port = transport
            .socket
            .lock()
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let packet = VoicePacket {
            kind: PacketType::Audio,
            flags: 0,
            session_id: 7,
            sequence: 1,
            timestamp: 960,
            key_epoch: 0,
            payload: vec![1],
        };
        server
            .send_to(
                &packet.encode().unwrap(),
                SocketAddr::new(server_addr.ip(), port),
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while transport.incoming.is_empty() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(2));
        }
        let dequeued_after = Instant::now();
        let Some(ReceivedAudio::Packet {
            packet,
            received_at,
        }) = transport.try_receive_audio()
        else {
            panic!("expected fresh adaptive audio");
        };
        assert!(received_at <= dequeued_after);

        // Replace only the handoff queue to age the same authenticated packet
        // deterministically, without slowing the test with a scheduler sleep.
        let (incoming, received) = bounded(16);
        let mut transport = transport;
        transport.incoming = received;
        incoming
            .try_send(QueuedAudio {
                packet,
                received_at: Instant::now() - Duration::from_secs(1),
            })
            .unwrap();
        assert!(matches!(
            transport.try_receive_audio(),
            Some(ReceivedAudio::Expired { session_id: 7 })
        ));
        assert!(transport.try_receive_audio().is_none());
    }

    #[test]
    fn first_ack_starts_a_clean_probe_measurement_epoch() {
        let peer_id = 2;
        let endpoint: SocketAddr = "127.0.0.1:41000".parse().unwrap();
        let desired = Arc::new(Mutex::new(Desired::default()));
        desired.lock().unwrap().peers.insert(
            peer_id,
            PeerCandidates {
                peer_id,
                generation: 1,
                security_key: String::new(),
                candidates: vec![Candidate {
                    kind: "host".into(),
                    address: endpoint.to_string(),
                    priority: 400,
                }],
            },
        );
        let manager = Arc::new(Mutex::new(RouteManager::new(Duration::ZERO)));
        manager
            .lock()
            .unwrap()
            .add_candidate(peer_id, RouteKind::LanDirect, endpoint);
        let (events_tx, events_rx) = bounded(4);
        let mut stats = HashMap::from([(
            (peer_id, endpoint),
            ProbeStats {
                sent: 20,
                ..Default::default()
            },
        )]);
        let challenge_id = 42_u64;
        let mut challenges = HashMap::from([(
            (peer_id, challenge_id),
            ProbeChallenge {
                sent_at: Instant::now(),
                target: endpoint,
            },
        )]);

        observe_ack(
            VoicePacket {
                kind: PacketType::ProbeAck,
                flags: 0,
                session_id: peer_id,
                sequence: 20,
                timestamp: 0,
                key_epoch: 0,
                payload: challenge_id.to_be_bytes().to_vec(),
            },
            endpoint,
            &manager,
            &desired,
            &events_tx,
            &mut stats,
            &mut challenges,
        );

        let report = match events_rx.try_recv().unwrap() {
            TransportEvent::NetworkReport(report) => report,
            event => panic!("unexpected event: {event:?}"),
        };
        assert!(report.qualified);
        assert_eq!(report.packet_loss_percent, 0.0);
        let entry = stats.get(&(peer_id, endpoint)).unwrap();
        assert_eq!((entry.sent, entry.received), (1, 1));
    }

    #[test]
    fn degraded_qualified_route_reports_failure_without_waiting_for_timeout() {
        let peer_id = 2;
        let endpoint: SocketAddr = "127.0.0.1:41000".parse().unwrap();
        let desired = Arc::new(Mutex::new(Desired::default()));
        desired.lock().unwrap().peers.insert(
            peer_id,
            PeerCandidates {
                peer_id,
                generation: 1,
                security_key: String::new(),
                candidates: vec![Candidate {
                    kind: "host".into(),
                    address: endpoint.to_string(),
                    priority: 400,
                }],
            },
        );
        let manager = Arc::new(Mutex::new(RouteManager::new(Duration::ZERO)));
        manager
            .lock()
            .unwrap()
            .add_candidate(peer_id, RouteKind::LanDirect, endpoint);
        manager
            .lock()
            .unwrap()
            .configure_degradation_duration(Duration::ZERO);
        let (events, reports) = bounded(4);
        let mut stats = HashMap::new();
        let mut challenges = HashMap::from([(
            (peer_id, 1),
            ProbeChallenge {
                sent_at: Instant::now(),
                target: endpoint,
            },
        )]);
        let ack = |challenge: u64| VoicePacket {
            kind: PacketType::ProbeAck,
            flags: 0,
            session_id: peer_id,
            sequence: challenge as u32,
            timestamp: 0,
            key_epoch: 0,
            payload: challenge.to_be_bytes().to_vec(),
        };
        observe_ack(
            ack(1),
            endpoint,
            &manager,
            &desired,
            &events,
            &mut stats,
            &mut challenges,
        );
        assert!(matches!(
            reports.try_recv(),
            Ok(TransportEvent::NetworkReport(NetworkReport {
                qualified: true,
                ..
            }))
        ));
        challenges.insert(
            (peer_id, 2),
            ProbeChallenge {
                sent_at: Instant::now() - Duration::from_millis(300),
                target: endpoint,
            },
        );
        observe_ack(
            ack(2),
            endpoint,
            &manager,
            &desired,
            &events,
            &mut stats,
            &mut challenges,
        );
        assert!(matches!(
            reports.try_recv(),
            Ok(TransportEvent::NetworkReport(NetworkReport {
                qualified: false,
                ..
            }))
        ));
    }

    #[test]
    fn unsolicited_probe_ack_cannot_authorize_a_peer_reflexive_route() {
        let peer_id = 2;
        let advertised: SocketAddr = "203.0.113.10:41000".parse().unwrap();
        let observed: SocketAddr = "203.0.113.10:41002".parse().unwrap();
        let desired = Arc::new(Mutex::new(Desired::default()));
        desired.lock().unwrap().peers.insert(
            peer_id,
            PeerCandidates {
                peer_id,
                generation: 1,
                security_key: String::new(),
                candidates: vec![Candidate {
                    kind: "server_reflexive".into(),
                    address: advertised.to_string(),
                    priority: 100,
                }],
            },
        );
        let manager = Arc::new(Mutex::new(RouteManager::new(Duration::ZERO)));
        let (events, _) = bounded(1);
        let mut stats = HashMap::new();
        let mut challenges = HashMap::new();
        observe_ack(
            VoicePacket {
                kind: PacketType::ProbeAck,
                flags: 0,
                session_id: peer_id,
                sequence: 1,
                timestamp: 0,
                key_epoch: 0,
                payload: 999_u64.to_be_bytes().to_vec(),
            },
            observed,
            &manager,
            &desired,
            &events,
            &mut stats,
            &mut challenges,
        );
        assert!(!manager.lock().unwrap().has_route(peer_id, observed));
    }

    #[test]
    fn udp_reset_notification_keeps_port_and_sfu_audio_alive() {
        for (kind, should_rebuild) in [
            (io::ErrorKind::ConnectionReset, false),
            (io::ErrorKind::InvalidData, true),
        ] {
            let server = UdpSocket::bind("127.0.0.1:0").unwrap();
            server.set_nonblocking(true).unwrap();
            let address = server.local_addr().unwrap();
            let (control, events) = AdaptiveControl::new();
            let transport = AdaptiveTransport::connect(
                &address.to_string(),
                &address.to_string(),
                1,
                control.clone(),
                events,
                64,
                None,
            )
            .unwrap();
            let initial = transport.socket.lock().unwrap().local_addr().unwrap();
            let injected = InjectedReceiveError::new(address, kind);
            let started = Instant::now();
            let mut frame = 0u32;
            let mut sent_error = false;
            let mut ports = std::collections::HashSet::new();
            let mut incoming = 0;
            let mut outgoing = 0;
            let mut recovering = false;
            let mut bytes = [0u8; MAX_PACKET_SIZE];
            while started.elapsed() < Duration::from_millis(900) {
                if started.elapsed() >= Duration::from_millis(150) && !sent_error {
                    injected.pending.store(true, Ordering::Release);
                    sent_error = true;
                }
                frame += 1;
                transport.send_audio(b"uplink survives").unwrap();
                while let Ok((n, source)) = server.recv_from(&mut bytes) {
                    let packet = VoicePacket::decode(&bytes[..n]).unwrap();
                    ports.insert(source.port());
                    if packet.kind == PacketType::Audio {
                        assert_eq!(packet.payload, b"uplink survives");
                        outgoing += 1;
                        let reply = VoicePacket {
                            kind: PacketType::Audio,
                            flags: 0,
                            session_id: 2,
                            sequence: frame,
                            timestamp: frame * 960,
                            key_epoch: 0,
                            payload: b"downlink survives".to_vec(),
                        }
                        .encode()
                        .unwrap();
                        server.send_to(&reply, source).unwrap();
                    }
                }
                while let Some(packet) = transport.try_receive() {
                    assert_eq!(packet.payload, b"downlink survives");
                    incoming += 1;
                }
                while let Some(event) = control.try_event() {
                    recovering |= matches!(
                        event,
                        TransportEvent::Health(TransportHealth::Recovering(_))
                    );
                }
                thread::sleep(Duration::from_millis(10));
            }
            assert!(
                injected.observed.load(Ordering::Acquire),
                "receiver must consume injected error"
            );
            assert!(
                incoming > 30 && outgoing > 30,
                "both directions must continue receiving media"
            );
            assert_eq!(
                recovering, should_rebuild,
                "recovery classification for {kind:?}"
            );
            if should_rebuild {
                assert!(ports.len() > 1, "fatal socket errors must still rebuild");
            } else {
                assert_eq!(
                    ports.len(),
                    1,
                    "an unreachable peer must not churn the SFU port"
                );
                assert_eq!(
                    transport.socket.lock().unwrap().local_addr().unwrap(),
                    initial
                );
            }
        }
    }

    #[test]
    fn forced_sfu_stops_peer_probes_and_auto_resumes_them() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
        peer.set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let (control, events) = AdaptiveControl::new();
        let transport = AdaptiveTransport::connect(
            &server.local_addr().unwrap().to_string(),
            &server.local_addr().unwrap().to_string(),
            1,
            control.clone(),
            events,
            64,
            None,
        )
        .unwrap();
        let mut route = test_route();
        route.transport = "sfu".into();
        route.room_network_policy = "force_sfu".into();
        route.reason = "forced_sfu".into();
        control.set_route(route.clone());
        control.set_peer_candidates(PeerCandidates {
            peer_id: 2,
            generation: 1,
            security_key: String::new(),
            candidates: vec![Candidate {
                kind: "host".into(),
                address: peer.local_addr().unwrap().to_string(),
                priority: 100,
            }],
        });
        // Drain any probe already in flight before the policy update was observed.
        thread::sleep(Duration::from_millis(250));
        let mut bytes = [0u8; MAX_PACKET_SIZE];
        while peer.recv_from(&mut bytes).is_ok() {}
        let until = Instant::now() + Duration::from_millis(350);
        while Instant::now() < until {
            assert!(
                peer.recv_from(&mut bytes).is_err(),
                "forced SFU still sent a peer probe"
            );
        }
        route.room_network_policy = "auto".into();
        route.reason = "awaiting_full_room_qualification".into();
        control.set_route(route);
        let until = Instant::now() + Duration::from_secs(2);
        let mut resumed = false;
        while Instant::now() < until && !resumed {
            if let Ok((n, _)) = peer.recv_from(&mut bytes) {
                resumed =
                    VoicePacket::decode(&bytes[..n]).is_ok_and(|p| p.kind == PacketType::Probe);
            }
        }
        assert!(
            resumed,
            "automatic mode must resume qualification even while media uses SFU"
        );
        drop(transport);
    }

    #[test]
    fn unreachable_candidate_does_not_stop_the_udp_receiver() {
        for kind in [
            io::ErrorKind::TimedOut,
            io::ErrorKind::WouldBlock,
            io::ErrorKind::Interrupted,
            io::ErrorKind::ConnectionRefused,
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::HostUnreachable,
            io::ErrorKind::NetworkUnreachable,
        ] {
            assert!(recoverable_udp_receive_error(&io::Error::from(kind)));
        }
        assert!(!recoverable_udp_receive_error(&io::Error::from(
            io::ErrorKind::InvalidData
        )));
    }

    #[test]
    fn server_reflexive_candidates_get_a_bounded_port_prediction_window() {
        let endpoint = "203.0.113.10:41000".parse().unwrap();
        let targets = punch_targets("server_reflexive", endpoint);
        assert_eq!(targets.len(), 5);
        assert_eq!(targets.first().unwrap().port(), 40998);
        assert_eq!(targets.last().unwrap().port(), 41002);
        assert_eq!(punch_targets("host", endpoint), vec![endpoint]);
    }

    #[test]
    fn unqualified_candidate_probes_are_bounded_and_rotate() {
        let endpoints = (10_000..10_012)
            .map(|port| SocketAddr::from(([127, 0, 0, 1], port)))
            .collect::<Vec<_>>();
        let mut cursor = 0;
        let first = scheduled_probe_endpoints(&endpoints, &mut cursor);
        let second = scheduled_probe_endpoints(&endpoints, &mut cursor);
        assert_eq!(first.len(), MAX_PROBES_PER_PEER_CYCLE);
        assert_eq!(second.len(), MAX_PROBES_PER_PEER_CYCLE);
        assert_eq!(first[0], endpoints[0]);
        assert_eq!(second[0], endpoints[MAX_PROBES_PER_PEER_CYCLE]);
        assert!(first.iter().chain(&second).all(|x| endpoints.contains(x)));
    }

    #[test]
    fn non_loopback_servers_do_not_publish_loopback_candidates() {
        let candidates = local_candidates("192.0.2.1:9001".parse().unwrap(), 41000);
        assert!(candidates.iter().all(|candidate| {
            !candidate
                .address
                .parse::<SocketAddr>()
                .unwrap()
                .ip()
                .is_loopback()
        }));
    }

    #[test]
    fn receive_only_sfu_recovers_after_nat_switch_without_interface_changes() {
        use crate::security::{EphemeralKey, SessionSecrets};

        for encrypted in [false, true] {
            let secrets = SessionSecrets {
                client_to_server_key: [1; 32],
                server_to_client_key: [2; 32],
                client_nonce_prefix: [3; 6],
                server_nonce_prefix: [4; 6],
            };
            let reverse = SessionSecrets {
                client_to_server_key: secrets.server_to_client_key,
                server_to_client_key: secrets.client_to_server_key,
                client_nonce_prefix: secrets.server_nonce_prefix,
                server_nonce_prefix: secrets.client_nonce_prefix,
            };
            let security =
                encrypted.then(|| MediaSecurity::new(secrets, &EphemeralKey::generate()));
            let server_security =
                encrypted.then(|| MediaSecurity::new(reverse, &EphemeralKey::generate()));
            let relay = UdpSocket::bind("127.0.0.1:0").unwrap();
            let front = UdpSocket::bind("127.0.0.1:0").unwrap();
            let exits = [
                UdpSocket::bind("127.0.0.1:0").unwrap(),
                UdpSocket::bind("127.0.0.1:0").unwrap(),
            ];
            for socket in [&relay, &front, &exits[0], &exits[1]] {
                socket.set_nonblocking(true).unwrap();
            }
            let address = front.local_addr().unwrap().to_string();
            let (control, events) = AdaptiveControl::new();
            let transport =
                AdaptiveTransport::connect(&address, &address, 1, control, events, 64, security)
                    .unwrap();
            let mut client_address = None;
            let mut registered = None;
            let mut buffer = [0u8; MAX_PACKET_SIZE + 1];
            let mut media_sequence = 0;
            // Change only the proxy's egress. The client's socket, local IP,
            // interface list and successful UDP sends stay unchanged throughout.
            // Exercise both enabling and disabling a VPN, including one lost
            // registration on each migration. The microphone never sends audio.
            for (round, active_exit) in [0, 1, 0, 0].into_iter().enumerate() {
                let previous_client_address = client_address;
                if round == 3 {
                    // Exercise socket replacement as well as NAT-only changes.
                    transport.repair_requested.request_repair();
                }
                let started = Instant::now();
                let mut drop_registration = round != 0;
                let mut delivered = false;
                while started.elapsed() < Duration::from_millis(950) {
                    while let Ok((length, source)) = front.recv_from(&mut buffer) {
                        client_address = Some(source);
                        exits[active_exit]
                            .send_to(&buffer[..length], relay.local_addr().unwrap())
                            .unwrap();
                    }
                    while let Ok((length, source)) = relay.recv_from(&mut buffer) {
                        let packet = VoicePacket::decode(&buffer[..length]).unwrap();
                        if packet.kind != PacketType::Register {
                            continue;
                        }
                        if drop_registration {
                            drop_registration = false;
                            continue;
                        }
                        if let Some(security) = &server_security {
                            security
                                .inbound
                                .lock()
                                .unwrap()
                                .open(
                                    &buffer[..HEADER_SIZE],
                                    packet.session_id,
                                    packet.key_epoch,
                                    packet.sequence,
                                    &packet.payload,
                                )
                                .unwrap();
                        }
                        registered = Some(source);
                    }
                    if let Some(endpoint) = registered {
                        let mut bytes = Vec::new();
                        media_sequence += 1;
                        encode_media_packet(
                            PacketType::Audio,
                            2,
                            media_sequence,
                            0,
                            &[round as u8],
                            server_security.as_ref(),
                            &mut bytes,
                        )
                        .unwrap();
                        relay.send_to(&bytes, endpoint).unwrap();
                    }
                    for (index, exit) in exits.iter().enumerate() {
                        while let Ok((length, _)) = exit.recv_from(&mut buffer) {
                            if index == active_exit {
                                front
                                    .send_to(&buffer[..length], client_address.unwrap())
                                    .unwrap();
                            }
                        }
                    }
                    while let Some(packet) = transport.try_receive() {
                        if packet.payload == [round as u8]
                            && (round != 3 || client_address != previous_client_address)
                        {
                            delivered = true;
                            break;
                        }
                    }
                    if delivered {
                        break;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                assert!(
                    delivered,
                    "receive-only SFU failed to recover: encrypted={encrypted}, round={round}"
                );
                eprintln!(
                    "SFU NAT switch encrypted={encrypted} round={round}: {:?}",
                    started.elapsed()
                );
                if round == 3 {
                    let settled = Instant::now();
                    let mut renewals = 0;
                    while settled.elapsed() < Duration::from_millis(800) {
                        while let Ok((length, source)) = front.recv_from(&mut buffer) {
                            if VoicePacket::decode(&buffer[..length]).unwrap().kind
                                == PacketType::Register
                            {
                                assert_eq!(
                                    Some(source),
                                    client_address,
                                    "maintenance renewed the obsolete socket after handoff"
                                );
                                renewals += 1;
                            }
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                    assert!(
                        renewals >= 2,
                        "new socket did not keep renewing registration"
                    );
                }
            }
        }
    }

    #[test]
    fn two_local_adaptive_transports_qualify_over_loopback() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let server_address = server.local_addr().unwrap().to_string();
        let (control_a, events_a) = AdaptiveControl::new();
        let (control_b, events_b) = AdaptiveControl::new();
        let transport_a = AdaptiveTransport::connect(
            &server_address,
            &server_address,
            1,
            control_a.clone(),
            events_a,
            64,
            None,
        )
        .unwrap();
        let transport_b = AdaptiveTransport::connect(
            &server_address,
            &server_address,
            2,
            control_b.clone(),
            events_b,
            64,
            None,
        )
        .unwrap();
        let candidate_a = wait_for_local_candidate(&control_a);
        let candidate_b = wait_for_local_candidate(&control_b);
        control_a.set_peer_candidates(PeerCandidates {
            peer_id: 2,
            generation: 1,
            security_key: String::new(),
            candidates: vec![candidate_b],
        });
        control_b.set_peer_candidates(PeerCandidates {
            peer_id: 1,
            generation: 1,
            security_key: String::new(),
            candidates: vec![candidate_a],
        });
        control_a.set_route(test_route());
        control_b.set_route(test_route());

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut a_qualified = false;
        let mut b_qualified = false;
        while Instant::now() < deadline && !(a_qualified && b_qualified) {
            while let Some(event) = control_a.try_event() {
                if let TransportEvent::RouteStatus(status) = event {
                    a_qualified |= status.actual_transport == "P2P";
                }
            }
            while let Some(event) = control_b.try_event() {
                if let TransportEvent::RouteStatus(status) = event {
                    b_qualified |= status.actual_transport == "P2P";
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(a_qualified, "first loopback transport did not qualify");
        assert!(b_qualified, "second loopback transport did not qualify");

        for operation in [0, 1] {
            let pause = MaintenancePause::new(server.local_addr().unwrap(), operation);
            for _ in 0..2 {
                pause
                    .entered
                    .recv_timeout(Duration::from_secs(2))
                    .expect("both maintenance workers must reach the gate");
            }
            let started = Instant::now();
            transport_a.send_audio(b"a-to-b").unwrap();
            transport_b.send_audio(b"b-to-a").unwrap();
            let mut received_a = false;
            let mut received_b = false;
            while !(received_a && received_b) {
                if let Some(packet) = transport_a.try_receive() {
                    assert_eq!(packet.payload, b"b-to-a");
                    received_a = true;
                }
                if let Some(packet) = transport_b.try_receive() {
                    assert_eq!(packet.payload, b"a-to-b");
                    received_b = true;
                }
                assert!(
                    started.elapsed() < Duration::from_millis(200),
                    "maintenance operation {operation} blocked P2P media"
                );
                thread::sleep(Duration::from_millis(1));
            }
            drop(pause);
        }
        drop((transport_a, transport_b, server));
    }

    #[test]
    fn hybrid_direct_and_sfu_copies_are_delivered_once() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        server.set_nonblocking(true).unwrap();
        let server_address = server.local_addr().unwrap().to_string();
        let (control_a, events_a) = AdaptiveControl::new();
        let (control_b, events_b) = AdaptiveControl::new();
        let transport_a = AdaptiveTransport::connect(
            &server_address,
            &server_address,
            1,
            control_a.clone(),
            events_a,
            64,
            None,
        )
        .unwrap();
        let transport_b = AdaptiveTransport::connect(
            &server_address,
            &server_address,
            2,
            control_b.clone(),
            events_b,
            64,
            None,
        )
        .unwrap();
        let candidate_a = wait_for_local_candidate(&control_a);
        let candidate_b = wait_for_local_candidate(&control_b);
        let transport_b_address: SocketAddr = candidate_b.address.parse().unwrap();
        control_a.set_peer_candidates(PeerCandidates {
            peer_id: 2,
            generation: 1,
            security_key: String::new(),
            candidates: vec![candidate_b],
        });
        control_b.set_peer_candidates(PeerCandidates {
            peer_id: 1,
            generation: 1,
            security_key: String::new(),
            candidates: vec![candidate_a],
        });
        let mut hybrid_route = test_route();
        hybrid_route.transport = "hybrid".into();
        control_a.set_route(hybrid_route);
        control_b.set_route(test_route());

        let qualification_deadline = Instant::now() + Duration::from_secs(3);
        let mut a_qualified = false;
        while Instant::now() < qualification_deadline && !a_qualified {
            while let Some(event) = control_a.try_event() {
                if let TransportEvent::RouteStatus(status) = event {
                    a_qualified |= status.actual_transport == "Hybrid";
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(a_qualified, "hybrid route did not qualify");

        let mut buffer = [0u8; MAX_PACKET_SIZE];
        while server.recv_from(&mut buffer).is_ok() {}
        transport_a.send_audio(b"hybrid-once").unwrap();
        let started = Instant::now();
        let mut relayed = false;
        let mut received = 0;
        while started.elapsed() < Duration::from_millis(300) {
            while let Ok((length, _)) = server.recv_from(&mut buffer) {
                if VoicePacket::decode(&buffer[..length])
                    .is_ok_and(|packet| packet.kind == PacketType::Audio && packet.session_id == 1)
                {
                    server
                        .send_to(&buffer[..length], transport_b_address)
                        .unwrap();
                    relayed = true;
                }
            }
            while let Some(packet) = transport_b.try_receive() {
                if packet.payload == b"hybrid-once" {
                    received += 1;
                }
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(relayed, "hybrid audio did not traverse the SFU path");
        assert_eq!(received, 1, "direct and SFU copies were both delivered");
    }

    #[test]
    fn maintenance_exits_when_dropped_during_pending_socket_handoff() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let alive = Arc::new(AtomicBool::new(true));
        let worker_alive = alive.clone();
        let (events, _event_rx) = bounded(16);
        let (replacements, replacement_rx) = bounded(1);
        let (done, done_rx) = bounded(1);
        let address = server.local_addr().unwrap();
        let repair = Arc::new(MaintenanceSignal::new());
        repair.request_repair();
        let worker_repair = repair.clone();
        let worker = thread::spawn(move || {
            maintenance_loop(
                socket,
                address,
                1,
                events,
                worker_alive,
                vec![],
                None,
                Arc::new(AtomicU32::new(1)),
                Arc::new(AtomicU8::new(0)),
                worker_repair,
                replacements,
            );
            let _ = done.send(());
        });
        let replacement = replacement_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        // Keep the ACK sender alive without acknowledging installation. Drop
        // must still terminate the maintenance worker promptly.
        alive.store(false, Ordering::Release);
        repair.wake();
        done_rx
            .recv_timeout(Duration::from_millis(200))
            .expect("maintenance leaked while waiting for installation");
        worker.join().unwrap();
        drop(replacement);
    }

    #[test]
    fn maintenance_notifications_coalesce_without_losing_pending_repair() {
        let signal = MaintenanceSignal::new();
        for _ in 0..1000 {
            signal.request_repair();
        }
        assert_eq!(signal.wake_rx.len(), 1);
        signal.wake_rx.try_recv().unwrap();
        // A backoff can consume the wake, but must retain the repair request.
        assert!(signal.requested.load(Ordering::Acquire));
        signal.request_repair();
        assert!(signal.wake_rx.is_empty());
        assert!(signal.requested.swap(false, Ordering::AcqRel));
        signal.request_repair();
        signal.wake_rx.try_recv().unwrap();
        // Shutdown wake is independent of a pending repair/backoff.
        signal.wake();
        signal.wake_rx.try_recv().unwrap();
    }

    #[test]
    fn maintenance_sleep_is_interruptible_for_repair_and_shutdown() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let alive = Arc::new(AtomicBool::new(true));
        let worker_alive = alive.clone();
        let signal = Arc::new(MaintenanceSignal::new());
        let worker_signal = signal.clone();
        let (events, _event_rx) = bounded(16);
        let (replacements, replacement_rx) = bounded(1);
        let (done, done_rx) = bounded(1);
        let address = server.local_addr().unwrap();
        let worker = thread::spawn(move || {
            maintenance_loop(
                socket,
                address,
                1,
                events,
                worker_alive,
                vec![],
                None,
                Arc::new(AtomicU32::new(1)),
                Arc::new(AtomicU8::new(0)),
                worker_signal,
                replacements,
            );
            done.send(()).unwrap();
        });
        let mut bytes = [0; MAX_PACKET_SIZE];
        // A regular registration proves the worker is running. Shortly after
        // it, the next periodic deadline is still almost 250 ms away.
        server.recv_from(&mut bytes).unwrap();
        thread::sleep(Duration::from_millis(20));
        signal.request_repair();
        let replacement = replacement_rx
            .recv_timeout(Duration::from_millis(150))
            .expect("repair waited for the periodic deadline");
        replacement.installed.send(()).unwrap();
        thread::sleep(Duration::from_millis(20));
        alive.store(false, Ordering::Release);
        signal.wake();
        done_rx
            .recv_timeout(Duration::from_millis(150))
            .expect("shutdown waited for the periodic deadline");
        worker.join().unwrap();
    }
    #[test]
    fn encrypted_direct_receive_survives_busy_outbound_contexts() {
        use crate::security::{EphemeralKey, SessionSecrets};
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = server.local_addr().unwrap().to_string();
        let keys = [EphemeralKey::generate(), EphemeralKey::generate()];
        let mut controls = Vec::new();
        let mut transports = Vec::new();
        let mut securities = Vec::new();
        for (index, key) in keys.iter().enumerate() {
            let security = MediaSecurity::new(
                SessionSecrets {
                    client_to_server_key: [index as u8 + 1; 32],
                    server_to_client_key: [9; 32],
                    client_nonce_prefix: [1; 6],
                    server_nonce_prefix: [2; 6],
                },
                key,
            );
            let (control, events) = AdaptiveControl::new();
            transports.push(
                AdaptiveTransport::connect(
                    &address,
                    &address,
                    index as u64 + 1,
                    control.clone(),
                    events,
                    16,
                    Some(security.clone()),
                )
                .unwrap(),
            );
            controls.push(control);
            securities.push(security);
        }
        for (index, control) in controls.iter().enumerate() {
            let peer = 1 - index;
            let mut endpoint = transports[peer]
                .socket
                .lock()
                .unwrap()
                .local_addr()
                .unwrap();
            endpoint.set_ip(std::net::Ipv4Addr::LOCALHOST.into());
            control.set_peer_candidates(PeerCandidates {
                peer_id: peer as u64 + 1,
                generation: 1,
                security_key: STANDARD_NO_PAD.encode(keys[peer].public),
                candidates: vec![Candidate {
                    kind: "host".into(),
                    address: endpoint.to_string(),
                    priority: 1,
                }],
            });
            control.set_route(test_route());
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while !transports.iter().enumerate().all(|(index, transport)| {
            transport
                .manager
                .lock()
                .unwrap()
                .best((2 - index) as u64, true)
                .is_some()
        }) {
            assert!(
                Instant::now() < deadline,
                "direct pair failed qualification"
            );
            thread::sleep(Duration::from_millis(5));
        }
        let receive_side_outbound = controls[1].media.pairwise.lock().unwrap();
        let sender_sfu_cipher = securities[0].outbound.lock().unwrap();
        let deadline = Instant::now() + Duration::from_millis(300);
        let mut delivered = false;
        while Instant::now() < deadline {
            // A momentary route lock may still request SFU; retry on the next tick.
            let _ = transports[0].send_audio(b"full-duplex");
            if let Some(packet) = transports[1].try_receive() {
                assert_eq!(packet.payload, b"full-duplex");
                delivered = true;
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        drop(sender_sfu_cipher);
        drop(receive_side_outbound);
        assert!(delivered, "P2P receive depended on an outbound cipher lock");
    }
}
#[cfg(test)]
mod transport_recovery_regression_tests {
    use super::*;
    use crate::security::{EphemeralKey, SessionSecrets};
    #[test]
    fn sfu_receive_survives_busy_route_and_control_locks() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let endpoint = server.local_addr().unwrap();
        let (control, events) = AdaptiveControl::new();
        let security = MediaSecurity::new(SessionSecrets::default(), &EphemeralKey::generate());
        let transport = AdaptiveTransport::connect(
            &endpoint.to_string(),
            &endpoint.to_string(),
            77,
            control,
            events,
            16,
            Some(security.clone()),
        )
        .unwrap();
        let client = SocketAddr::new(
            endpoint.ip(),
            transport
                .socket
                .lock()
                .unwrap()
                .local_addr()
                .unwrap()
                .port(),
        );
        let _routes = transport.manager.lock().unwrap();
        let _desired = transport.desired.lock().unwrap();
        let _outbound = security.outbound.lock().unwrap();
        let mut remote = crate::security::MediaCipher::new([0; 32], [0; 6], 1);
        // Span multiple probe cycles while all control locks remain held.
        for n in 1..=20 {
            let mut packet = VoicePacket {
                kind: PacketType::Audio,
                flags: 0,
                session_id: 88,
                sequence: n,
                timestamp: n * 960,
                key_epoch: 1,
                payload: Vec::new(),
            };
            let aad = packet.authenticated_header(17).unwrap();
            packet.payload = remote.seal(&aad, 88, n, &[42]).unwrap();
            server.send_to(&packet.encode().unwrap(), client).unwrap();
            let received = transport
                .incoming
                .recv_timeout(Duration::from_millis(200))
                .expect("SFU receiver stalled behind route/control locks");
            assert_eq!(received.packet.payload, [42]);
            transport.send_audio(&[24]).unwrap();
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn busy_outbound_cipher_does_not_block_media_sender() {
        let security = MediaSecurity::new(
            SessionSecrets {
                client_to_server_key: [1; 32],
                server_to_client_key: [2; 32],
                client_nonce_prefix: [3; 6],
                server_nonce_prefix: [4; 6],
            },
            &EphemeralKey::generate(),
        );
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = server.local_addr().unwrap().to_string();
        let (control, events) = AdaptiveControl::new();
        let transport = AdaptiveTransport::connect(
            &address,
            &address,
            77,
            control,
            events,
            16,
            Some(security.clone()),
        )
        .unwrap();
        let held = security.outbound.lock().unwrap();
        let (began, began_rx) = bounded(1);
        let (done, done_rx) = bounded(1);
        let worker = thread::spawn(move || {
            began.send(()).unwrap();
            let start = Instant::now();
            let result = transport.send_audio(&[1]);
            done.send((result, start.elapsed())).unwrap();
        });
        began_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (result, _) = done_rx
            .recv_timeout(Duration::from_millis(80))
            .expect("media sender blocked on cipher lock");
        result.expect("voice must still be sent while the control cipher is busy");
        drop(held);
        worker.join().unwrap();
    }
    #[test]
    fn expired_alternate_routes_fall_back_to_sfu_immediately() {
        use crate::transport::RouteMetrics;
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_nonblocking(true).unwrap();
        let server = "127.0.0.1:9001".parse().unwrap();
        let mut routes = RouteManager::new(Duration::ZERO);
        let mut candidates = Vec::new();
        let mut stats = HashMap::new();
        for port in 40000..40006 {
            let endpoint = SocketAddr::from(([127, 0, 0, 1], port));
            routes.add_candidate(2, RouteKind::LanDirect, endpoint);
            assert!(routes.observe(
                2,
                endpoint,
                RouteMetrics {
                    rtt_ms: 10.0,
                    packets_sent: 100,
                    packets_received: 100,
                    ..Default::default()
                },
                Instant::now()
            ));
            candidates.push(Candidate {
                kind: "host".into(),
                address: endpoint.to_string(),
                priority: 1,
            });
            stats.insert(
                (2, endpoint),
                ProbeStats {
                    sent: 100,
                    received: 100,
                    last_rtt: 10.0,
                    reported: true,
                    last_received: Some(Instant::now() - Duration::from_secs(10)),
                    ..Default::default()
                },
            );
        }
        let (control, events) = AdaptiveControl::new();
        control.set_peer_candidates(PeerCandidates {
            peer_id: 2,
            generation: 1,
            candidates,
            security_key: String::new(),
        });
        control.set_route(serde_json::from_value(serde_json::json!({"room_id":1,"topology":"p2p_stable","transport":"p2p","reason":"full_room_qualified","qualification_ms":0,"minimum_dwell_ms":0,"migration_cooldown_ms":0,"degrade_duration_ms":1500,"max_rtt_ms":80.0,"max_loss_percent":2.0,"max_jitter_ms":15.0})).unwrap());
        let manager = Arc::new(Mutex::new(routes));
        let mut challenges = HashMap::new();
        let mut counter = 0;
        let mut generations = HashMap::from([(2, 1)]);
        let mut cursors = HashMap::new();
        for _ in 1..=6 {
            let status = send_probes(
                &socket,
                server,
                1,
                &manager,
                &control.desired,
                &mut stats,
                &mut challenges,
                &mut counter,
                &mut generations,
                &mut cursors,
                &events,
            );
            assert_eq!(status.unwrap().actual_transport, "SFU");
        }
    }
}

#[cfg(test)]
mod session_regression_tests {
    use super::*;
    use crate::security::{EphemeralKey, MediaCipher, SessionSecrets};

    #[test]
    fn restarted_audio_keeps_session_sequence_timestamp_and_registration_valid() {
        let security = MediaSecurity::new(
            SessionSecrets {
                client_to_server_key: [1; 32],
                server_to_client_key: [2; 32],
                client_nonce_prefix: [3; 6],
                server_nonce_prefix: [4; 6],
            },
            &EphemeralKey::generate(),
        );
        let epoch = security.outbound.lock().unwrap().epoch();
        let mut receiver = MediaCipher::new([1; 32], [3; 6], epoch);
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let address = server.local_addr().unwrap().to_string();
        let (control, events) = AdaptiveControl::new();
        control.media.sequence.store(100, Ordering::Relaxed);
        control.media.timestamp.store(96_000, Ordering::Relaxed);
        let read = |receiver: &mut MediaCipher, kind| {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                assert!(Instant::now() < deadline, "expected media packet missing");
                let mut bytes = [0; MAX_PACKET_SIZE];
                let (length, _) = server.recv_from(&mut bytes).unwrap();
                let packet = VoicePacket::decode(&bytes[..length]).unwrap();
                if packet.kind == kind {
                    receiver
                        .open(
                            &bytes[..HEADER_SIZE],
                            77,
                            packet.key_epoch,
                            packet.sequence,
                            &packet.payload,
                        )
                        .unwrap();
                    return packet;
                }
            }
        };
        let first = AdaptiveTransport::connect(
            &address,
            &address,
            77,
            control.clone(),
            events.clone(),
            16,
            Some(security.clone()),
        )
        .unwrap();
        assert_eq!(read(&mut receiver, PacketType::Register).sequence, 100);
        let send = |transport: &AdaptiveTransport, payload: &[u8]| {
            let deadline = Instant::now() + Duration::from_secs(1);
            loop {
                let timestamp = transport.timestamp.load(Ordering::Relaxed);
                match transport.send_audio(payload) {
                    Ok(()) => return timestamp,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "media cipher stayed busy");
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("audio send failed: {error}"),
                }
            }
        };
        let before_timestamp = send(&first, &[1]);
        let before = read(&mut receiver, PacketType::Audio);
        assert_eq!(before.timestamp, before_timestamp);
        assert!(before.timestamp >= 96_000);
        drop(first);
        let second =
            AdaptiveTransport::connect(&address, &address, 77, control, events, 16, Some(security))
                .unwrap();
        let registration = read(&mut receiver, PacketType::Register);
        assert!(registration.sequence > before.sequence);
        let after_timestamp = send(&second, &[2]);
        let after = read(&mut receiver, PacketType::Audio);
        assert!(after.sequence > registration.sequence);
        assert_eq!(after.timestamp, after_timestamp);
        // A skipped realtime frame still consumes its timestamp and nonce.
        assert!(after.timestamp >= before.timestamp + 960);
    }

    #[test]
    fn wildcard_media_address_uses_connected_control_peer() {
        use std::net::{TcpListener, TcpStream};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("localhost:{}", listener.local_addr().unwrap().port());
        let stream = address
            .to_socket_addrs()
            .unwrap()
            .find_map(|endpoint| {
                TcpStream::connect_timeout(&endpoint, Duration::from_millis(100)).ok()
            })
            .unwrap();
        let peer = stream.peer_addr().unwrap();
        for advertised in ["0.0.0.0:9001", "[::]:9001", "localhost:9001"] {
            assert_eq!(
                resolve_server(&peer.to_string(), advertised).unwrap(),
                SocketAddr::new(peer.ip(), 9001)
            );
        }
        // An explicitly advertised independent address remains authoritative.
        assert_eq!(
            resolve_server(&peer.to_string(), "192.0.2.7:9001").unwrap(),
            "192.0.2.7:9001".parse().unwrap()
        );
    }
}
