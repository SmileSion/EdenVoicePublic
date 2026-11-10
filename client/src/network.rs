use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomNetworkPolicy {
    #[default]
    Auto,
    ForceSfu,
    ForceP2p,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkQuality {
    #[default]
    Good,
    Fair,
    Poor,
    Critical,
}

impl NetworkQuality {
    fn severity(self) -> u8 {
        match self {
            Self::Good => 0,
            Self::Fair => 1,
            Self::Poor => 2,
            Self::Critical => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkTrend {
    Improving,
    #[default]
    Stable,
    Degrading,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkObservation {
    pub observed_packets: Option<u64>,
    pub path_metrics_available: bool,
    pub rtt_ms: f64,
    pub jitter_ms: f64,
    pub transport_loss_percent: f64,
    pub late_loss_percent: f64,
    pub effective_audio_loss_percent: f64,
    pub burst_loss_percent: f64,
    pub max_consecutive_loss: u32,
    pub packet_reorder_percent: f64,
    pub jitter_buffer_delay_ms: f64,
    pub jitter_buffer_underruns: u64,
    pub plc_frames: u64,
    pub fec_recovered_packets: u64,
    pub playout_resync_count: u64,
    pub route_healthy: bool,
    pub reconnect_events: u32,
}

impl Default for NetworkObservation {
    fn default() -> Self {
        Self {
            observed_packets: None,
            path_metrics_available: true,
            rtt_ms: 0.0,
            jitter_ms: 0.0,
            transport_loss_percent: 0.0,
            late_loss_percent: 0.0,
            effective_audio_loss_percent: 0.0,
            burst_loss_percent: 0.0,
            max_consecutive_loss: 0,
            packet_reorder_percent: 0.0,
            jitter_buffer_delay_ms: 0.0,
            jitter_buffer_underruns: 0,
            plc_frames: 0,
            fec_recovered_packets: 0,
            playout_resync_count: 0,
            route_healthy: true,
            reconnect_events: 0,
        }
    }
}

impl NetworkObservation {
    pub fn normalized(mut self) -> Self {
        self.rtt_ms = finite_non_negative(self.rtt_ms);
        self.jitter_ms = finite_non_negative(self.jitter_ms);
        self.transport_loss_percent = percent(self.transport_loss_percent);
        self.late_loss_percent = percent(self.late_loss_percent);
        self.burst_loss_percent = percent(self.burst_loss_percent);
        self.packet_reorder_percent = percent(self.packet_reorder_percent);
        self.jitter_buffer_delay_ms = finite_non_negative(self.jitter_buffer_delay_ms);
        self.effective_audio_loss_percent = if self.effective_audio_loss_percent.is_finite() {
            percent(self.effective_audio_loss_percent)
        } else {
            percent(self.transport_loss_percent + self.late_loss_percent)
        };
        self
    }
}

fn finite_non_negative(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn percent(value: f64) -> f64 {
    finite_non_negative(value).min(100.0)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QualityAssessment {
    pub quality: NetworkQuality,
    pub trend: NetworkTrend,
    pub score: f64,
}

/// All listeners share one encoded stream. A healthy listener's report must
/// not overwrite another listener's still-current loss report.
#[derive(Default)]
pub struct ReceiverFeedbackWindow {
    samples: HashMap<u64, (NetworkObservation, Instant)>,
}

impl ReceiverFeedbackWindow {
    pub fn observe(
        &mut self,
        peer_id: u64,
        observation: NetworkObservation,
        now: Instant,
    ) -> NetworkObservation {
        self.samples
            .retain(|_, (_, at)| now.saturating_duration_since(*at) < Duration::from_secs(3));
        self.samples
            .insert(peer_id, (observation.normalized(), now));
        self.samples
            .values()
            .map(|(observation, _)| *observation)
            .max_by(|left, right| {
                let left_score = quality_score(left);
                let right_score = quality_score(right);
                classify(left_score, left)
                    .severity()
                    .cmp(&classify(right_score, right).severity())
                    .then_with(|| left_score.total_cmp(&right_score))
            })
            .unwrap_or_default()
    }
}

pub struct NetworkQualityEngine {
    current: NetworkQuality,
    pending: Option<(NetworkQuality, u8, Instant)>,
    last_change: Instant,
    previous_score: Option<f64>,
    minimum_hold: Duration,
    recovery_samples: u8,
}

impl NetworkQualityEngine {
    pub fn new(now: Instant) -> Self {
        Self {
            current: NetworkQuality::Good,
            pending: None,
            last_change: now,
            previous_score: None,
            minimum_hold: Duration::from_secs(2),
            recovery_samples: 4,
        }
    }

    pub fn evaluate(&mut self, observation: NetworkObservation, now: Instant) -> QualityAssessment {
        let observation = observation.normalized();
        let score = quality_score(&observation);
        let candidate = classify(score, &observation);
        let trend = match self.previous_score.replace(score) {
            Some(previous) if score > previous + 5.0 => NetworkTrend::Degrading,
            Some(previous) if score + 5.0 < previous => NetworkTrend::Improving,
            _ => NetworkTrend::Stable,
        };

        // Missing traffic is not evidence of recovery. Legacy feedback has no
        // count; new receivers also suppress empty windows before transmission.
        let insufficient = observation.observed_packets.is_some_and(|count| count < 5);
        if insufficient && candidate.severity() <= self.current.severity() {
            self.pending = None;
        } else if candidate.severity() > self.current.severity() {
            let required = if candidate == NetworkQuality::Critical {
                1
            } else {
                2
            };
            self.advance(candidate, required, now, true);
        } else if candidate.severity() < self.current.severity()
            && now.duration_since(self.last_change) >= self.minimum_hold
        {
            self.advance(candidate, self.recovery_samples, now, false);
        } else if candidate == self.current {
            self.pending = None;
        }

        QualityAssessment {
            quality: self.current,
            trend,
            score,
        }
    }

    fn advance(&mut self, candidate: NetworkQuality, required: u8, now: Instant, degrading: bool) {
        let count = match self.pending {
            Some((pending, count, at)) if pending == candidate => {
                let elapsed = now.saturating_duration_since(at);
                if elapsed < Duration::from_millis(500) {
                    return;
                }
                if elapsed <= Duration::from_millis(1500) {
                    count.saturating_add(1)
                } else {
                    1
                }
            }
            _ => 1,
        };
        self.pending = Some((candidate, count, now));
        if count >= required {
            self.current = candidate;
            self.pending = None;
            self.last_change = now;
            if degrading {
                self.recovery_samples = 4;
            }
        }
    }
}

fn quality_score(x: &NetworkObservation) -> f64 {
    let loss = x.effective_audio_loss_percent * 6.0;
    let burst = x.burst_loss_percent * 4.0 + x.max_consecutive_loss.saturating_sub(1) as f64 * 5.0;
    let (latency, jitter) = if x.path_metrics_available {
        (
            ((x.rtt_ms - 80.0).max(0.0) / 8.0).min(30.0),
            ((x.jitter_ms - 10.0).max(0.0) * 0.8).min(30.0),
        )
    } else {
        (0.0, 0.0)
    };
    let receiver =
        (x.jitter_buffer_underruns.min(5) * 8) as f64 + (x.playout_resync_count.min(3) * 8) as f64;
    let route = if x.route_healthy { 0.0 } else { 80.0 };
    (loss + burst + latency + jitter + receiver + route).min(100.0)
}

fn classify(score: f64, x: &NetworkObservation) -> NetworkQuality {
    if !x.route_healthy || x.max_consecutive_loss >= 10 || score >= 75.0 {
        NetworkQuality::Critical
    } else if score >= 42.0 || x.effective_audio_loss_percent >= 8.0 {
        NetworkQuality::Poor
    } else if score >= 18.0 || x.effective_audio_loss_percent >= 2.0 {
        NetworkQuality::Fair
    } else {
        NetworkQuality::Good
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum AudioQualityPolicy {
    #[default]
    Auto,
    Fixed {
        target_bitrate_bps: i32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioAdaptationDecision {
    pub target_bitrate_bps: i32,
    pub fec_enabled: bool,
    pub expected_packet_loss_percent: u8,
    pub dtx_enabled: bool,
}

pub struct AudioAdaptationController {
    policy: AudioQualityPolicy,
}

impl AudioAdaptationController {
    pub fn new(policy: AudioQualityPolicy) -> Self {
        Self { policy }
    }

    pub fn set_policy(&mut self, policy: AudioQualityPolicy) {
        self.policy = policy;
    }

    pub fn decide(&self, quality: NetworkQuality) -> AudioAdaptationDecision {
        let (auto_bitrate, fec_enabled, expected_loss) = match quality {
            NetworkQuality::Good => (72_000, false, 0),
            NetworkQuality::Fair => (56_000, true, 5),
            NetworkQuality::Poor => (48_000, true, 10),
            NetworkQuality::Critical => (32_000, true, 20),
        };
        let target_bitrate_bps = match self.policy {
            AudioQualityPolicy::Auto => auto_bitrate,
            AudioQualityPolicy::Fixed { target_bitrate_bps } => {
                target_bitrate_bps.clamp(6_000, 510_000)
            }
        };
        AudioAdaptationDecision {
            target_bitrate_bps,
            fec_enabled,
            expected_packet_loss_percent: expected_loss,
            dtx_enabled: quality == NetworkQuality::Critical,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioQoeFeedback {
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

#[derive(Default)]
pub struct QoeFeedbackAggregator {
    packets_expected: u64,
    packets_received: u64,
    late_packets: u64,
    reordered_packets: u64,
    burst_count: u32,
    current_loss_run: u32,
    max_loss_run: u32,
    plc_frames: u64,
    fec_recovered: u64,
    underruns: u64,
    resyncs: u64,
}

impl QoeFeedbackAggregator {
    pub fn packet(&mut self, received: bool, late: bool, reordered: bool) {
        self.packets_expected = self.packets_expected.saturating_add(1);
        if received {
            self.packets_received = self.packets_received.saturating_add(1);
        }
        if late {
            self.late_packets = self.late_packets.saturating_add(1);
        }
        if reordered {
            self.reordered_packets = self.reordered_packets.saturating_add(1);
        }
        if !received || late {
            if self.current_loss_run == 0 {
                self.burst_count = self.burst_count.saturating_add(1);
            }
            self.current_loss_run = self.current_loss_run.saturating_add(1);
            self.max_loss_run = self.max_loss_run.max(self.current_loss_run);
        } else {
            self.current_loss_run = 0;
        }
    }

    pub fn receiver_event(&mut self, plc: bool, fec_recovered: bool, underrun: bool, resync: bool) {
        self.plc_frames += u64::from(plc);
        self.fec_recovered += u64::from(fec_recovered);
        self.underruns += u64::from(underrun);
        self.resyncs += u64::from(resync);
    }

    pub fn finish(
        &mut self,
        window: Duration,
        rtt_ms: f64,
        jitter_ms: f64,
        buffer_delay_ms: f64,
    ) -> AudioQoeFeedback {
        let expected = self.packets_expected.max(1) as f64;
        let lost = self.packets_expected.saturating_sub(self.packets_received);
        let transport_loss = 100.0 * lost as f64 / expected;
        let late_loss = 100.0 * self.late_packets as f64 / expected;
        let recovered = 100.0 * self.fec_recovered.min(lost + self.late_packets) as f64 / expected;
        let feedback = AudioQoeFeedback {
            window_ms: window.as_millis().min(u32::MAX as u128) as u32,
            transport_loss_percent: transport_loss,
            late_loss_percent: late_loss,
            effective_audio_loss_percent: (transport_loss + late_loss - recovered).max(0.0),
            burst_loss_percent: 100.0 * (lost + self.late_packets) as f64 / expected,
            burst_count: self.burst_count,
            max_consecutive_loss: self.max_loss_run,
            packet_reorder_percent: 100.0 * self.reordered_packets as f64 / expected,
            jitter_ms,
            rtt_ms,
            jitter_buffer_delay_ms: buffer_delay_ms,
            jitter_buffer_underruns: self.underruns,
            plc_frames: self.plc_frames,
            fec_recovered_packets: self.fec_recovered,
            playout_resync_count: self.resyncs,
        };
        *self = Self::default();
        feedback
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionHealth {
    Connected,
    Suspected,
    Reconnecting,
    Recovered,
    Disconnected,
}

pub struct ConnectionRecovery {
    state: ConnectionHealth,
    failures: u8,
}

impl Default for ConnectionRecovery {
    fn default() -> Self {
        Self {
            state: ConnectionHealth::Connected,
            failures: 0,
        }
    }
}

impl ConnectionRecovery {
    pub fn state(&self) -> ConnectionHealth {
        self.state
    }
    pub fn missed_heartbeat(&mut self) -> ConnectionHealth {
        self.failures = self.failures.saturating_add(1);
        self.state = if self.failures < 3 {
            ConnectionHealth::Suspected
        } else {
            ConnectionHealth::Reconnecting
        };
        self.state
    }
    pub fn transport_restored(&mut self) -> ConnectionHealth {
        self.failures = 0;
        self.state = ConnectionHealth::Recovered;
        self.state
    }
    pub fn resume_complete(&mut self) {
        self.state = ConnectionHealth::Connected;
    }
    pub fn exhausted(&mut self) {
        self.state = ConnectionHealth::Disconnected;
    }
}

#[derive(Clone, Debug)]
pub struct NetworkTimelineEntry {
    pub at: Instant,
    pub quality: NetworkQuality,
    pub trend: NetworkTrend,
    pub route: String,
    pub detail: String,
}

pub struct NetworkTimeline {
    capacity: usize,
    entries: VecDeque<NetworkTimelineEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeakNetworkProfile {
    pub random_loss_percent: f64,
    pub burst_every_packets: Option<u32>,
    pub burst_length: u32,
    pub base_rtt_ms: u32,
    pub jitter_ms: u32,
    pub reorder_every_packets: Option<u32>,
    pub blackout: Option<(u32, u32)>,
}

impl Default for WeakNetworkProfile {
    fn default() -> Self {
        Self {
            random_loss_percent: 0.0,
            burst_every_packets: None,
            burst_length: 0,
            base_rtt_ms: 20,
            jitter_ms: 0,
            reorder_every_packets: None,
            blackout: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SimulatedPacket {
    Delivered { delay_ms: u32 },
    Reordered { delay_ms: u32 },
    Lost,
}

pub struct WeakNetworkSimulator {
    profile: WeakNetworkProfile,
    state: u64,
}

impl WeakNetworkSimulator {
    pub fn new(profile: WeakNetworkProfile, seed: u64) -> Self {
        Self {
            profile,
            state: seed.max(1),
        }
    }

    pub fn packet(&mut self, sequence: u32) -> SimulatedPacket {
        if self
            .profile
            .blackout
            .is_some_and(|(start, length)| sequence >= start && sequence < start + length)
        {
            return SimulatedPacket::Lost;
        }
        if self.profile.burst_every_packets.is_some_and(|period| {
            period > 0 && sequence % period < self.profile.burst_length.min(period)
        }) {
            return SimulatedPacket::Lost;
        }
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        if ((self.state % 10_000) as f64) < percent(self.profile.random_loss_percent) * 100.0 {
            return SimulatedPacket::Lost;
        }
        let jitter = if self.profile.jitter_ms == 0 {
            0
        } else {
            (self.state as u32 % (self.profile.jitter_ms * 2 + 1)) as i64
                - i64::from(self.profile.jitter_ms)
        };
        let delay_ms = (i64::from(self.profile.base_rtt_ms / 2) + jitter).max(0) as u32;
        if self
            .profile
            .reorder_every_packets
            .is_some_and(|period| period > 0 && sequence.is_multiple_of(period))
        {
            SimulatedPacket::Reordered {
                delay_ms: delay_ms + self.profile.jitter_ms,
            }
        } else {
            SimulatedPacket::Delivered { delay_ms }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteRecoveryAction {
    KeepCurrent,
    ProbeSfu,
    UseTemporarySfu,
    ProbeP2p,
    MigrateToP2p,
}

pub struct RouteRecoveryController {
    policy: RoomNetworkPolicy,
    last_transition: Instant,
    cooldown: Duration,
}

impl RouteRecoveryController {
    pub fn new(policy: RoomNetworkPolicy, now: Instant) -> Self {
        Self {
            policy,
            last_transition: now,
            cooldown: Duration::from_secs(10),
        }
    }

    pub fn evaluate(
        &mut self,
        current_route: &str,
        quality: NetworkQuality,
        candidate_qualified: bool,
        now: Instant,
    ) -> RouteRecoveryAction {
        if current_route == "p2p" && quality == NetworkQuality::Critical {
            self.last_transition = now;
            return if self.policy == RoomNetworkPolicy::ForceP2p {
                RouteRecoveryAction::UseTemporarySfu
            } else {
                RouteRecoveryAction::ProbeSfu
            };
        }
        if self.policy == RoomNetworkPolicy::ForceP2p && current_route == "sfu" {
            if !candidate_qualified || now.duration_since(self.last_transition) < self.cooldown {
                return RouteRecoveryAction::ProbeP2p;
            }
            self.last_transition = now;
            return RouteRecoveryAction::MigrateToP2p;
        }
        RouteRecoveryAction::KeepCurrent
    }
}

impl NetworkTimeline {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: VecDeque::new(),
        }
    }
    pub fn push(&mut self, entry: NetworkTimelineEntry) {
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }
    pub fn entries(&self) -> impl Iterator<Item = &NetworkTimelineEntry> {
        self.entries.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_bursts_cannot_accelerate_quality_changes() {
        let now = Instant::now();
        let mut engine = NetworkQualityEngine::new(now);
        let bad = NetworkObservation {
            effective_audio_loss_percent: 4.0,
            observed_packets: Some(25),
            ..Default::default()
        };
        for _ in 0..100 {
            assert_eq!(engine.evaluate(bad, now).quality, NetworkQuality::Good);
        }
        assert_eq!(
            engine
                .evaluate(bad, now + Duration::from_millis(499))
                .quality,
            NetworkQuality::Good
        );
        assert_eq!(
            engine
                .evaluate(bad, now + Duration::from_millis(500))
                .quality,
            NetworkQuality::Fair
        );
        let healthy = NetworkObservation {
            observed_packets: Some(25),
            ..Default::default()
        };
        for _ in 0..100 {
            assert_eq!(
                engine
                    .evaluate(healthy, now + Duration::from_secs(3))
                    .quality,
                NetworkQuality::Fair
            );
        }
        for tick in 1..=3 {
            engine.evaluate(healthy, now + Duration::from_millis(3000 + tick * 500));
        }
        assert_eq!(
            engine
                .evaluate(healthy, now + Duration::from_secs(5))
                .quality,
            NetworkQuality::Good
        );
    }

    #[test]
    fn empty_and_sparse_windows_cannot_prove_recovery() {
        let now = Instant::now();
        let mut engine = NetworkQualityEngine::new(now);
        engine.evaluate(
            NetworkObservation {
                route_healthy: false,
                ..Default::default()
            },
            now,
        );
        for n in 1..=20 {
            assert_eq!(
                engine
                    .evaluate(
                        NetworkObservation {
                            observed_packets: Some(0),
                            ..Default::default()
                        },
                        now + Duration::from_secs(n)
                    )
                    .quality,
                NetworkQuality::Critical
            );
        }
        for n in 1..=4 {
            assert_eq!(
                engine
                    .evaluate(
                        NetworkObservation {
                            observed_packets: Some(25),
                            ..Default::default()
                        },
                        now + Duration::from_secs(20 + n * 3)
                    )
                    .quality,
                NetworkQuality::Critical
            );
        }
    }

    #[test]
    fn healthy_listeners_do_not_mask_a_current_weak_receiver() {
        let now = Instant::now();
        let mut reports = ReceiverFeedbackWindow::default();
        let weak = NetworkObservation {
            effective_audio_loss_percent: 12.0,
            ..Default::default()
        };
        reports.observe(1, weak, now);
        for peer in 2..=5 {
            assert_eq!(
                reports.observe(peer, NetworkObservation::default(), now),
                weak
            );
        }
        assert_eq!(
            reports.observe(
                1,
                NetworkObservation::default(),
                now + Duration::from_millis(500)
            ),
            NetworkObservation::default()
        );
    }

    #[test]
    fn departed_receivers_do_not_hold_the_encoder_in_weak_network_mode() {
        let now = Instant::now();
        let mut reports = ReceiverFeedbackWindow::default();
        reports.observe(
            1,
            NetworkObservation {
                route_healthy: false,
                ..Default::default()
            },
            now,
        );
        assert_eq!(
            reports.observe(
                2,
                NetworkObservation::default(),
                now + Duration::from_secs(3)
            ),
            NetworkObservation::default()
        );
    }

    #[test]
    fn quality_uses_burst_and_effective_loss_not_one_metric() {
        let now = Instant::now();
        let mut engine = NetworkQualityEngine::new(now);
        let bad = NetworkObservation {
            effective_audio_loss_percent: 4.0,
            burst_loss_percent: 3.0,
            max_consecutive_loss: 3,
            route_healthy: true,
            ..Default::default()
        };
        assert_eq!(engine.evaluate(bad, now).quality, NetworkQuality::Good);
        assert_eq!(
            engine
                .evaluate(bad, now + Duration::from_millis(500))
                .quality,
            NetworkQuality::Poor
        );
    }

    #[test]
    fn recovery_is_slower_than_critical_degradation() {
        let now = Instant::now();
        let mut engine = NetworkQualityEngine::new(now);
        let critical = NetworkObservation {
            route_healthy: false,
            ..Default::default()
        };
        assert_eq!(
            engine.evaluate(critical, now).quality,
            NetworkQuality::Critical
        );
        let good = NetworkObservation {
            route_healthy: true,
            ..Default::default()
        };
        for index in 1..4 {
            assert_eq!(
                engine
                    .evaluate(good, now + Duration::from_secs(2 + index))
                    .quality,
                NetworkQuality::Critical
            );
        }
        assert_eq!(
            engine.evaluate(good, now + Duration::from_secs(6)).quality,
            NetworkQuality::Good
        );
    }

    #[test]
    fn fixed_quality_preserves_bitrate_but_keeps_resilience() {
        let controller = AudioAdaptationController::new(AudioQualityPolicy::Fixed {
            target_bitrate_bps: 64_000,
        });
        let decision = controller.decide(NetworkQuality::Critical);
        assert_eq!(decision.target_bitrate_bps, 64_000);
        assert!(decision.fec_enabled);
        assert_eq!(decision.expected_packet_loss_percent, 20);
    }

    #[test]
    fn automatic_quality_keeps_clear_voice_bitrates() {
        let controller = AudioAdaptationController::new(AudioQualityPolicy::Auto);
        assert_eq!(
            controller.decide(NetworkQuality::Good).target_bitrate_bps,
            72_000
        );
        assert_eq!(
            controller.decide(NetworkQuality::Fair).target_bitrate_bps,
            56_000
        );
        assert_eq!(
            controller.decide(NetworkQuality::Poor).target_bitrate_bps,
            48_000
        );
        assert_eq!(
            controller
                .decide(NetworkQuality::Critical)
                .target_bitrate_bps,
            32_000
        );
    }

    #[test]
    fn feedback_distinguishes_transport_late_and_effective_loss() {
        let mut aggregator = QoeFeedbackAggregator::default();
        for _ in 0..8 {
            aggregator.packet(true, false, false);
        }
        aggregator.packet(false, false, false);
        aggregator.packet(true, true, true);
        aggregator.receiver_event(true, true, false, false);
        let feedback = aggregator.finish(Duration::from_millis(500), 40.0, 12.0, 60.0);
        assert_eq!(feedback.transport_loss_percent, 10.0);
        assert_eq!(feedback.late_loss_percent, 10.0);
        assert_eq!(feedback.effective_audio_loss_percent, 10.0);
        assert_eq!(feedback.max_consecutive_loss, 2);
    }

    #[test]
    fn weak_network_simulator_repeats_bursts_blackouts_and_reordering() {
        let profile = WeakNetworkProfile {
            burst_every_packets: Some(10),
            burst_length: 2,
            reorder_every_packets: Some(5),
            blackout: Some((20, 3)),
            ..Default::default()
        };
        let mut simulator = WeakNetworkSimulator::new(profile, 7);
        assert_eq!(simulator.packet(10), SimulatedPacket::Lost);
        assert_eq!(simulator.packet(11), SimulatedPacket::Lost);
        assert!(matches!(
            simulator.packet(15),
            SimulatedPacket::Reordered { .. }
        ));
        assert_eq!(simulator.packet(21), SimulatedPacket::Lost);
    }

    #[test]
    fn force_p2p_recovers_through_sfu_then_converges_back() {
        let now = Instant::now();
        let mut recovery = RouteRecoveryController::new(RoomNetworkPolicy::ForceP2p, now);
        assert_eq!(
            recovery.evaluate("p2p", NetworkQuality::Critical, false, now),
            RouteRecoveryAction::UseTemporarySfu
        );
        assert_eq!(
            recovery.evaluate(
                "sfu",
                NetworkQuality::Fair,
                true,
                now + Duration::from_secs(5)
            ),
            RouteRecoveryAction::ProbeP2p
        );
        assert_eq!(
            recovery.evaluate(
                "sfu",
                NetworkQuality::Good,
                true,
                now + Duration::from_secs(11)
            ),
            RouteRecoveryAction::MigrateToP2p
        );
    }
}
