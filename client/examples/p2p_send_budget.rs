//! Local encrypted full-mesh transport budget, not microphone-to-speaker latency.
//! Run with: cargo run --release --example p2p_send_budget -- 2 6
use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
use edenvoice_client::{
    protocol::{
        control::{Candidate, PeerCandidates, RouteUpdate},
        voice::{HEADER_SIZE, PacketType, VoicePacket},
    },
    security::{EphemeralKey, MediaSecurity, SessionSecrets},
    transport::{AdaptiveControl, AdaptiveTransport, TransportEvent, VoiceTransport},
};
use std::{
    collections::{HashMap, HashSet},
    net::UdpSocket,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

fn summarize(label: &str, mut us: Vec<f64>) {
    us.sort_by(f64::total_cmp);
    assert!(!us.is_empty());
    let mean = us.iter().sum::<f64>() / us.len() as f64;
    println!(
        "{label}: n={} mean_us={mean:.3} p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3}",
        us.len(),
        us[us.len() / 2],
        us[us.len() * 95 / 100],
        us[us.len() * 99 / 100],
        us[us.len() - 1]
    );
}

fn run(size: usize, trial: usize) {
    assert!(
        (2..=6).contains(&size),
        "default server P2P room limit is six"
    );
    let server = UdpSocket::bind("127.0.0.1:0").unwrap();
    let address = server.local_addr().unwrap().to_string();
    server
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let running = Arc::new(AtomicBool::new(true));
    let server_running = running.clone();
    let fallback = Arc::new(AtomicU64::new(0));
    let server_fallback = fallback.clone();
    let keys: Vec<_> = (0..size).map(|_| EphemeralKey::generate()).collect();
    let server_security: Vec<_> = keys
        .iter()
        .enumerate()
        .map(|(i, key)| {
            MediaSecurity::new(
                SessionSecrets {
                    client_to_server_key: [i as u8 + 101; 32],
                    server_to_client_key: [i as u8 + 1; 32],
                    client_nonce_prefix: [2; 6],
                    server_nonce_prefix: [1; 6],
                },
                key,
            )
        })
        .collect();
    let sink = thread::spawn(move || {
        let mut bytes = [0; 1500];
        let mut endpoints = HashMap::new();
        while server_running.load(Ordering::Relaxed) {
            if let Ok((n, source)) = server.recv_from(&mut bytes) {
                let packet = VoicePacket::decode(&bytes[..n]).unwrap();
                let sender = packet.session_id as usize - 1;
                let plaintext = server_security[sender]
                    .inbound
                    .lock()
                    .unwrap()
                    .open(
                        &bytes[..HEADER_SIZE],
                        packet.session_id,
                        packet.key_epoch,
                        packet.sequence,
                        &packet.payload,
                    )
                    .unwrap();
                if packet.kind == PacketType::Register {
                    endpoints.insert(sender, source);
                }
                if packet.kind == PacketType::Audio {
                    server_fallback.fetch_add(1, Ordering::Relaxed);
                    // Model the fallback as well: a momentarily unavailable route
                    // lock deliberately uses SFU, not a lost audio frame. This is
                    // a minimal encrypted relay, not production SFU route policy.
                    for (&recipient, &endpoint) in &endpoints {
                        if recipient == sender {
                            continue;
                        }
                        let mut forwarded = packet.clone();
                        let mut cipher = server_security[recipient].outbound.lock().unwrap();
                        forwarded.key_epoch = cipher.epoch();
                        let header = forwarded
                            .authenticated_header(plaintext.len() + 16)
                            .unwrap();
                        forwarded.payload = cipher
                            .seal(
                                &header,
                                forwarded.session_id,
                                forwarded.sequence,
                                &plaintext,
                            )
                            .unwrap();
                        server
                            .send_to(&forwarded.encode().unwrap(), endpoint)
                            .unwrap();
                    }
                }
            }
        }
    });
    let mut controls = Vec::new();
    let mut transports = Vec::new();
    let mut candidates = Vec::new();
    for (i, key) in keys.iter().enumerate() {
        let (control, events) = AdaptiveControl::new();
        let security = MediaSecurity::new(
            SessionSecrets {
                client_to_server_key: [i as u8 + 1; 32],
                server_to_client_key: [i as u8 + 101; 32],
                client_nonce_prefix: [1; 6],
                server_nonce_prefix: [2; 6],
            },
            key,
        );
        let transport = AdaptiveTransport::connect(
            &address,
            &address,
            i as u64 + 1,
            control.clone(),
            events,
            64,
            Some(security),
        )
        .unwrap();
        let limit = Instant::now() + Duration::from_secs(2);
        let candidate = loop {
            if let Some(TransportEvent::LocalCandidates(values)) = control.try_event()
                && let Some(c) = values
                    .into_iter()
                    .find(|c| c.address.starts_with("127.0.0.1:"))
            {
                break c;
            }
            assert!(Instant::now() < limit, "no loopback candidate");
            thread::sleep(Duration::from_millis(1));
        };
        candidates.push(candidate);
        controls.push(control);
        transports.push(transport);
    }
    for (i, control) in controls.iter().enumerate() {
        for (j, key) in keys.iter().enumerate() {
            if i != j {
                control.set_peer_candidates(PeerCandidates {
                    peer_id: j as u64 + 1,
                    generation: 1,
                    security_key: STANDARD_NO_PAD.encode(key.public),
                    candidates: vec![Candidate {
                        kind: "host".into(),
                        ..candidates[j].clone()
                    }],
                });
            }
        }
        control.set_route(RouteUpdate {
            room_id: 1,
            topology: "p2p_evaluating".into(),
            transport: "p2p".into(),
            reason: "local budget diagnostic".into(),
            room_network_policy: "auto".into(),
            qualification_ms: 1500,
            minimum_dwell_ms: 0,
            migration_cooldown_ms: 0,
            degrade_duration_ms: 1500,
            max_rtt_ms: 80.0,
            max_loss_percent: 2.0,
            max_jitter_ms: 15.0,
            degrade_rtt_ms: 250.0,
            degrade_loss_percent: 5.0,
            degrade_jitter_ms: 40.0,
        });
    }
    let mut qualified = vec![false; size];
    let limit = Instant::now() + Duration::from_secs(8);
    while !qualified.iter().all(|v| *v) {
        for (i, control) in controls.iter().enumerate() {
            while let Some(event) = control.try_event() {
                if let TransportEvent::RouteStatus(status) = event {
                    qualified[i] = status.actual_transport == "P2P";
                }
            }
        }
        assert!(
            Instant::now() < limit,
            "full mesh failed qualification: {qualified:?}"
        );
        thread::sleep(Duration::from_millis(5));
    }
    let mut sends = Vec::new();
    let mut cold = Vec::new();
    let mut deliveries = Vec::new();
    let mut seen = HashSet::new();
    let mut duplicates = 0;
    let base = Instant::now();
    let mut round = 0u64;
    while base.elapsed() < Duration::from_millis(4100) {
        if round < 200 && base.elapsed() >= Duration::from_millis(round * 20) {
            for (i, transport) in transports.iter().enumerate() {
                let mut payload = [0x5a; 160];
                payload[..8].copy_from_slice(&round.to_be_bytes());
                payload[8..16].copy_from_slice(&(base.elapsed().as_micros() as u64).to_be_bytes());
                payload[16] = i as u8;
                let started = Instant::now();
                transport.send_audio(&payload).unwrap();
                let elapsed = started.elapsed().as_secs_f64() * 1e6;
                if round == 0 {
                    cold.push(elapsed);
                }
                if round >= 50 {
                    sends.push(elapsed);
                }
            }
            round += 1;
        }
        for (i, transport) in transports.iter().enumerate() {
            while let Some(packet) = transport.try_receive() {
                let payload = &packet.payload;
                assert_eq!(payload.len(), 160);
                assert_eq!(packet.session_id, payload[16] as u64 + 1);
                assert_ne!(packet.session_id, i as u64 + 1);
                assert!(payload[17..].iter().all(|v| *v == 0x5a));
                let sequence = u64::from_be_bytes(payload[..8].try_into().unwrap());
                if !seen.insert((i, packet.session_id, sequence)) {
                    duplicates += 1;
                    continue;
                }
                if sequence >= 50 {
                    let started = u64::from_be_bytes(payload[8..16].try_into().unwrap());
                    deliveries.push(base.elapsed().as_micros() as f64 - started as f64);
                }
            }
            while let Some(event) = controls[i].try_event() {
                if let TransportEvent::RouteStatus(status) = event {
                    assert_eq!(
                        status.actual_transport, "P2P",
                        "route degraded during benchmark"
                    );
                }
            }
        }
        thread::sleep(Duration::from_millis(1));
    }
    println!(
        "room={size} trial={trial} payload_bytes=160 frame_ms=20 debug={} received={} expected={} sfu_fallback_packets={} duplicate_deliveries={duplicates}",
        cfg!(debug_assertions),
        seen.len(),
        200 * size * (size - 1),
        fallback.load(Ordering::Relaxed)
    );
    summarize("cold_send", cold);
    summarize("steady_send", sends);
    summarize("delivery_including_harness_poll", deliveries);
    drop(transports);
    running.store(false, Ordering::Relaxed);
    sink.join().unwrap();
    assert_eq!(
        seen.len(),
        200 * size * (size - 1),
        "missing unique media deliveries"
    );
}

fn main() {
    let sizes: Vec<usize> = std::env::args()
        .skip(1)
        .map(|s| s.parse().unwrap())
        .collect();
    let sizes = if sizes.is_empty() { vec![2, 6] } else { sizes };
    for trial in 1..=3 {
        for &size in &sizes {
            run(size, trial);
        }
    }
}
