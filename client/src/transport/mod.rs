mod adaptive;
mod relay;
mod route;

pub use adaptive::{
    AdaptiveControl, AdaptiveTransport, RouteStatus, TransportEvent, TransportHealth,
};
pub use relay::RelayTransport;
pub use route::{RouteKind, RouteManager, RouteMetrics, RouteState, RouteThresholds};

use std::{
    io,
    sync::Mutex,
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, Sender, TrySendError};

use crate::protocol::voice::VoicePacket;

/// The UDP handoff is not a playout buffer. Audio that waited this long in
/// application queues must not become a new talkspurt after a worker stall.
pub const MAX_RECEIVED_AUDIO_AGE: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub enum ReceivedAudio {
    Packet {
        packet: VoicePacket,
        received_at: Instant,
    },
    /// One queued packet was discarded. Consumers can account for the loss and
    /// clear this speaker's old playout state without inventing a new arrival.
    Expired { session_id: u64 },
}

#[derive(Debug)]
pub(crate) struct QueuedAudio {
    pub packet: VoicePacket,
    pub received_at: Instant,
}

impl QueuedAudio {
    fn into_received(self, now: Instant) -> ReceivedAudio {
        if now.saturating_duration_since(self.received_at) >= MAX_RECEIVED_AUDIO_AGE {
            ReceivedAudio::Expired {
                session_id: self.packet.session_id,
            }
        } else {
            ReceivedAudio::Packet {
                packet: self.packet,
                received_at: self.received_at,
            }
        }
    }
}

pub(crate) fn receive_audio(incoming: &Receiver<QueuedAudio>) -> Option<ReceivedAudio> {
    incoming
        .try_recv()
        .ok()
        .map(|audio| audio.into_received(Instant::now()))
}

/// Compatibility for packet-only callers. Bound the scan even if the UDP
/// producer keeps refilling the queue while expired packets are discarded.
pub(crate) fn receive_fresh_packet(incoming: &Receiver<QueuedAudio>) -> Option<VoicePacket> {
    for _ in 0..incoming.capacity().unwrap_or(64) {
        match receive_audio(incoming)? {
            ReceivedAudio::Packet { packet, .. } => return Some(packet),
            ReceivedAudio::Expired { .. } => {}
        }
    }
    None
}

pub(crate) fn send_latest_packet(
    incoming: &Sender<QueuedAudio>,
    stale: &Receiver<QueuedAudio>,
    packet: VoicePacket,
    received_at: Instant,
) {
    let audio = QueuedAudio {
        packet,
        received_at,
    };
    if let Err(TrySendError::Full(audio)) = incoming.try_send(audio) {
        let _ = stale.try_recv();
        let _ = incoming.try_send(audio);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VoicePathMetrics {
    pub rtt_ms: f64,
    pub jitter_ms: f64,
}

pub(crate) struct TimedVoicePathMetrics {
    pub value: VoicePathMetrics,
    pub observed_at: Instant,
}

pub(crate) fn fresh_voice_path_metrics(
    metrics: &Mutex<Option<TimedVoicePathMetrics>>,
) -> Option<VoicePathMetrics> {
    metrics.try_lock().ok().and_then(|metrics| {
        metrics
            .as_ref()
            .filter(|sample| sample.observed_at.elapsed() < Duration::from_secs(3))
            .map(|sample| sample.value)
    })
}

pub struct LocalTransport;

impl VoiceTransport for LocalTransport {
    fn send_audio(&self, _opus_payload: &[u8]) -> io::Result<()> {
        Ok(())
    }

    fn try_receive(&self) -> Option<VoicePacket> {
        None
    }

    fn session_id(&self) -> u64 {
        0
    }
}

pub trait VoiceTransport: Send {
    fn send_audio(&self, opus_payload: &[u8]) -> io::Result<()>;
    fn try_receive(&self) -> Option<VoicePacket>;

    /// UDP transports override this to preserve the socket receive timestamp
    /// and surface one expired packet per call, keeping the worker's receive
    /// deadline in control. The default supports existing in-process adapters.
    fn try_receive_audio(&self) -> Option<ReceivedAudio> {
        self.try_receive().map(|packet| ReceivedAudio::Packet {
            packet,
            received_at: Instant::now(),
        })
    }

    fn session_id(&self) -> u64;

    /// Fresh measurements of the path delivering this peer's media. None means
    /// probe evidence is unavailable, rather than a zero-latency healthy path.
    fn network_metrics(&self, _peer_id: u64) -> Option<(f64, f64)> {
        None
    }

    fn voice_path_metrics(&self) -> Option<VoicePathMetrics> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::voice::PacketType;
    use crossbeam_channel::bounded;

    fn queued_audio(session_id: u64, received_at: Instant) -> QueuedAudio {
        QueuedAudio {
            packet: VoicePacket {
                kind: PacketType::Audio,
                flags: 0,
                session_id,
                sequence: 1,
                timestamp: 960,
                key_epoch: 0,
                payload: vec![1, 2],
            },
            received_at,
        }
    }

    #[test]
    fn local_transport_never_sends_or_receives_network_audio() {
        let transport = LocalTransport;
        assert_eq!(transport.session_id(), 0);
        assert!(transport.send_audio(b"local-only").is_ok());
        assert!(transport.try_receive().is_none());
        assert!(transport.try_receive_audio().is_none());
    }

    #[test]
    fn sparse_audio_expires_at_queue_age_limit() {
        let received_at = Instant::now();
        let fresh = queued_audio(7, received_at)
            .into_received(received_at + MAX_RECEIVED_AUDIO_AGE - Duration::from_nanos(1));
        assert!(matches!(
            fresh,
            ReceivedAudio::Packet { received_at: actual, .. } if actual == received_at
        ));
        let expired =
            queued_audio(7, received_at).into_received(received_at + MAX_RECEIVED_AUDIO_AGE);
        assert!(matches!(expired, ReceivedAudio::Expired { session_id: 7 }));
    }

    #[test]
    fn receive_reports_one_expired_speaker_per_call_before_fresh_audio() {
        let (incoming, received) = bounded(16);
        let stale_at = Instant::now() - Duration::from_secs(1);
        incoming.try_send(queued_audio(7, stale_at)).unwrap();
        incoming.try_send(queued_audio(8, stale_at)).unwrap();
        let fresh_at = Instant::now();
        incoming.try_send(queued_audio(7, fresh_at)).unwrap();

        assert!(matches!(
            receive_audio(&received),
            Some(ReceivedAudio::Expired { session_id: 7 })
        ));
        assert_eq!(received.len(), 2);
        assert!(matches!(
            receive_audio(&received),
            Some(ReceivedAudio::Expired { session_id: 8 })
        ));
        assert!(matches!(
            receive_audio(&received),
            Some(ReceivedAudio::Packet { packet, received_at })
                if packet.session_id == 7 && received_at == fresh_at
        ));
        assert!(receive_audio(&received).is_none());
    }

    #[test]
    fn packet_only_receivers_discard_old_audio_and_recover() {
        let (incoming, received) = bounded(2);
        let stale_at = Instant::now() - Duration::from_secs(1);
        incoming.try_send(queued_audio(7, stale_at)).unwrap();
        incoming.try_send(queued_audio(8, stale_at)).unwrap();
        assert!(receive_fresh_packet(&received).is_none());

        incoming.try_send(queued_audio(7, stale_at)).unwrap();
        incoming.try_send(queued_audio(8, Instant::now())).unwrap();
        assert_eq!(receive_fresh_packet(&received).unwrap().session_id, 8);
        assert!(receive_fresh_packet(&received).is_none());
    }
}
