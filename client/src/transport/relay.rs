use super::adaptive::resolve_server;
use std::{
    collections::HashMap,
    io,
    net::{SocketAddr, UdpSocket},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crossbeam_channel::{Receiver, bounded};

use crate::{
    protocol::voice::{MAX_PACKET_SIZE, PacketType, VoicePacket},
    transport::{
        QueuedAudio, ReceivedAudio, TimedVoicePathMetrics, VoicePathMetrics, VoiceTransport,
        fresh_voice_path_metrics, receive_audio, receive_fresh_packet, send_latest_packet,
    },
};

pub struct RelayTransport {
    socket: UdpSocket,
    server: SocketAddr,
    session_id: u64,
    sequence: AtomicU32,
    timestamp: AtomicU32,
    incoming: Receiver<QueuedAudio>,
    alive: Arc<AtomicBool>,
    packet_buffer: Mutex<Vec<u8>>,
    voice_path_metrics: Arc<Mutex<Option<TimedVoicePathMetrics>>>,
}

impl RelayTransport {
    pub fn connect(
        control_address: &str,
        advertised_voice_address: &str,
        session_id: u64,
    ) -> io::Result<Self> {
        let server = resolve_server(control_address, advertised_voice_address)?;
        let bind_address = if server.is_ipv6() {
            "[::]:0"
        } else {
            "0.0.0.0:0"
        };
        let socket = UdpSocket::bind(bind_address)?;
        socket.set_nonblocking(true)?;
        let registration = VoicePacket {
            kind: PacketType::Register,
            flags: 0,
            session_id,
            sequence: 0,
            timestamp: 0,
            key_epoch: 0,
            payload: Vec::new(),
        }
        .encode()
        .map_err(io::Error::other)?;
        socket.send_to(&registration, server)?;
        tracing::info!(session_id, server = %server, local = %socket.local_addr()?, "udp registered");

        let receive_socket = socket.try_clone()?;
        let (incoming_tx, incoming_rx) = bounded(64);
        let incoming_stale = incoming_rx.clone();
        let alive = Arc::new(AtomicBool::new(true));
        let receiver_alive = Arc::clone(&alive);
        let voice_path_metrics = Arc::new(Mutex::new(None));
        let receiver_voice_path_metrics = Arc::clone(&voice_path_metrics);
        thread::Builder::new()
            .name("voice-receiver".into())
            .spawn(move || {
                let mut buffer = [0_u8; MAX_PACKET_SIZE + 1];
                let mut next_challenge = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_micros() as u64;
                let mut pending = HashMap::<u64, Instant>::new();
                let mut last_probe = Instant::now() - Duration::from_secs(1);
                let mut last_rtt = 0.0;
                let mut jitter = 0.0;
                while receiver_alive.load(Ordering::Relaxed) {
                    if last_probe.elapsed() >= Duration::from_secs(1) {
                        pending.retain(|_, sent_at| sent_at.elapsed() < Duration::from_secs(5));
                        next_challenge = next_challenge.wrapping_add(1);
                        let probe = VoicePacket {
                            kind: PacketType::Probe,
                            flags: 0,
                            session_id,
                            sequence: next_challenge as u32,
                            timestamp: 0,
                            key_epoch: 0,
                            payload: next_challenge.to_be_bytes().to_vec(),
                        };
                        if let Ok(bytes) = probe.encode()
                            && receive_socket.send_to(&bytes, server).is_ok()
                        {
                            pending.insert(next_challenge, Instant::now());
                        }
                        last_probe = Instant::now();
                    }
                    match receive_socket.recv_from(&mut buffer) {
                        Ok((length, source)) if source == server => {
                            let received_at = Instant::now();
                            if let Ok(packet) = VoicePacket::decode(&buffer[..length]) {
                                if packet.kind == PacketType::ProbeAck
                                    && packet.session_id == session_id
                                {
                                    let Ok(challenge_bytes) = packet.payload.as_slice().try_into()
                                    else {
                                        continue;
                                    };
                                    let challenge = u64::from_be_bytes(challenge_bytes);
                                    let Some(sent_at) = pending.remove(&challenge) else {
                                        continue;
                                    };
                                    let rtt = sent_at.elapsed().as_secs_f64() * 1_000.0;
                                    if last_rtt != 0.0 {
                                        jitter += ((rtt - last_rtt).abs() - jitter) * 0.125;
                                    }
                                    last_rtt = rtt;
                                    if let Ok(mut metrics) = receiver_voice_path_metrics.lock() {
                                        *metrics = Some(TimedVoicePathMetrics {
                                            value: VoicePathMetrics {
                                                rtt_ms: rtt,
                                                jitter_ms: jitter,
                                            },
                                            observed_at: Instant::now(),
                                        });
                                    }
                                } else if packet.kind == PacketType::Audio {
                                    send_latest_packet(
                                        &incoming_tx,
                                        &incoming_stale,
                                        packet,
                                        received_at,
                                    );
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(error)
                            if matches!(
                                error.kind(),
                                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                            ) =>
                        {
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(_) => break,
                    }
                }
            })?;
        Ok(Self {
            socket,
            server,
            session_id,
            sequence: AtomicU32::new(0),
            timestamp: AtomicU32::new(0),
            incoming: incoming_rx,
            alive,
            packet_buffer: Mutex::new(Vec::with_capacity(MAX_PACKET_SIZE)),
            voice_path_metrics,
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }
}

impl VoiceTransport for RelayTransport {
    fn send_audio(&self, opus_payload: &[u8]) -> io::Result<()> {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let timestamp = self.timestamp.fetch_add(960, Ordering::Relaxed);
        let mut packet = self
            .packet_buffer
            .lock()
            .map_err(|_| io::Error::other("voice packet buffer unavailable"))?;
        VoicePacket::encode_parts_into(
            PacketType::Audio,
            0,
            self.session_id,
            sequence,
            timestamp,
            0,
            opus_payload,
            &mut packet,
        )
        .map_err(io::Error::other)?;
        self.socket.send_to(&packet, self.server).map(|_| ())
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

    fn voice_path_metrics(&self) -> Option<VoicePathMetrics> {
        fresh_voice_path_metrics(&self.voice_path_metrics)
    }

    fn network_metrics(&self, _peer_id: u64) -> Option<(f64, f64)> {
        self.voice_path_metrics()
            .map(|metrics| (metrics.rtt_ms, metrics.jitter_ms))
    }
}

impl Drop for RelayTransport {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn registers_sends_and_receives() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let server_address = server.local_addr().unwrap();
        let transport =
            RelayTransport::connect("127.0.0.1:9000", &server_address.to_string(), 77).unwrap();
        let mut buffer = [0_u8; MAX_PACKET_SIZE];
        let (length, client_address) = server.recv_from(&mut buffer).unwrap();
        let registration = VoicePacket::decode(&buffer[..length]).unwrap();
        assert_eq!(registration.kind, PacketType::Register);
        assert_eq!(registration.session_id, 77);

        let (length, _) = server.recv_from(&mut buffer).unwrap();
        let probe = VoicePacket::decode(&buffer[..length]).unwrap();
        assert_eq!(probe.kind, PacketType::Probe);
        let probe_ack = VoicePacket {
            kind: PacketType::ProbeAck,
            ..probe
        }
        .encode()
        .unwrap();
        server.send_to(&probe_ack, client_address).unwrap();

        transport.send_audio(&[1, 2, 3]).unwrap();
        let (length, _) = server.recv_from(&mut buffer).unwrap();
        let audio = VoicePacket::decode(&buffer[..length]).unwrap();
        assert_eq!(audio.kind, PacketType::Audio);
        assert_eq!(audio.sequence, 0);
        assert_eq!(audio.timestamp, 0);

        let started = Instant::now();
        loop {
            if let Some(metrics) = transport.voice_path_metrics() {
                assert!(metrics.rtt_ms.is_finite());
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(2));
            thread::sleep(Duration::from_millis(10));
        }

        let remote = VoicePacket {
            kind: PacketType::Audio,
            flags: 0,
            session_id: 88,
            sequence: 9,
            timestamp: 960,
            key_epoch: 0,
            payload: vec![4, 5],
        }
        .encode()
        .unwrap();
        server.send_to(&remote, client_address).unwrap();
        let started = Instant::now();
        while transport.incoming.is_empty() {
            assert!(started.elapsed() < Duration::from_secs(2));
            thread::sleep(Duration::from_millis(10));
        }
        let dequeued_after = Instant::now();
        let Some(ReceivedAudio::Packet {
            packet,
            received_at,
        }) = transport.try_receive_audio()
        else {
            panic!("expected fresh relay audio");
        };
        assert_eq!(packet.session_id, 88);
        assert!(received_at <= dequeued_after);
    }

    #[test]
    fn relay_reports_old_audio_instead_of_restarting_its_arrival_time() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let (incoming, received) = bounded(64);
        let transport = RelayTransport {
            server: socket.local_addr().unwrap(),
            socket,
            session_id: 1,
            sequence: AtomicU32::new(0),
            timestamp: AtomicU32::new(0),
            incoming: received,
            alive: Arc::new(AtomicBool::new(false)),
            packet_buffer: Mutex::new(Vec::new()),
            voice_path_metrics: Arc::new(Mutex::new(None)),
        };
        incoming
            .try_send(QueuedAudio {
                packet: VoicePacket {
                    kind: PacketType::Audio,
                    flags: 0,
                    session_id: 88,
                    sequence: 1,
                    timestamp: 960,
                    key_epoch: 0,
                    payload: vec![4, 5],
                },
                received_at: Instant::now() - Duration::from_secs(1),
            })
            .unwrap();
        assert!(matches!(
            transport.try_receive_audio(),
            Some(ReceivedAudio::Expired { session_id: 88 })
        ));
        assert!(transport.try_receive_audio().is_none());
    }

    #[test]
    fn replaces_unspecified_advertised_host() {
        let resolved = resolve_server("127.0.0.1:9000", "0.0.0.0:9001").unwrap();
        assert_eq!(resolved, "127.0.0.1:9001".parse().unwrap());
    }
}
