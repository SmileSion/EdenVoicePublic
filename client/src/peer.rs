use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use crate::{
    codec::{FRAME_SAMPLES, OpusDecoder},
    jitter::{DEFAULT_PREFILL_FRAMES, JitterBuffer},
    network::NetworkQuality,
};

pub struct RemotePeer {
    pub session_id: u64,
    decoder: OpusDecoder,
    jitter: JitterBuffer<Vec<u8>>,
    last_packet: Option<Instant>,
    pub packets_received: u64,
    pub decode_errors: u64,
    lost_before_reset: u64,
    reordered_before_reset: u64,
    late_before_reset: u64,
    local_quality: NetworkQuality,
    consecutive_missing: u32,
    stable_frames: u32,
    playout_resyncs: u64,
    last_output_sample: f32,
    tail_faded: bool,
    fec_recovered_packets: u64,
    plc_frames: u64,
    jitter_buffer_underruns: u64,
    window_max_loss_run: u32,
    window_bursts: u32,
    catchup_ticks: u32,
    expansion_ticks: u32,
    empty_playout_ticks: u32,
    onset_frames: u32,
    playout_pcm: VecDeque<f32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeOutcome {
    Normal,
    Fec,
    Plc,
    Failed,
    Silence,
}

pub struct DecodedFrame {
    pub samples: [f32; FRAME_SAMPLES],
    pub outcome: DecodeOutcome,
}

impl RemotePeer {
    pub fn new(session_id: u64) -> audiopus::Result<Self> {
        Ok(Self {
            session_id,
            decoder: OpusDecoder::new()?,
            // Packet sequence numbers also cover encrypted registration
            // keepalives. RTP-style timestamps advance only for audio frames,
            // so they are the correct continuity key for playout.
            jitter: JitterBuffer::new_with_step(DEFAULT_PREFILL_FRAMES, FRAME_SAMPLES as u32),
            last_packet: None,
            packets_received: 0,
            decode_errors: 0,
            lost_before_reset: 0,
            reordered_before_reset: 0,
            late_before_reset: 0,
            local_quality: NetworkQuality::Good,
            consecutive_missing: 0,
            stable_frames: 0,
            playout_resyncs: 0,
            last_output_sample: 0.0,
            tail_faded: false,
            fec_recovered_packets: 0,
            plc_frames: 0,
            jitter_buffer_underruns: 0,
            window_max_loss_run: 0,
            window_bursts: 0,
            catchup_ticks: 0,
            expansion_ticks: 0,
            empty_playout_ticks: 0,
            onset_frames: 5,
            playout_pcm: VecDeque::with_capacity(FRAME_SAMPLES * 3),
        })
    }

    pub fn push(&mut self, timestamp: u32, payload: Vec<u8>) {
        if self
            .last_packet
            .is_some_and(|last| last.elapsed() >= Duration::from_millis(300))
        {
            self.lost_before_reset = self.lost_before_reset.saturating_add(self.jitter.lost);
            self.late_before_reset = self.late_before_reset.saturating_add(self.jitter.late);
            self.reordered_before_reset = self
                .reordered_before_reset
                .saturating_add(self.jitter.reordered);
            self.jitter = JitterBuffer::new_with_step(DEFAULT_PREFILL_FRAMES, FRAME_SAMPLES as u32);
            self.jitter.set_network_quality(self.local_quality);
            self.consecutive_missing = 0;
            self.stable_frames = 0;
            self.catchup_ticks = 0;
            self.expansion_ticks = 0;
            self.playout_pcm.clear();
            self.last_output_sample = 0.0;
            self.tail_faded = true;
            self.empty_playout_ticks = 0;
            self.onset_frames = 5;
        }
        self.jitter.push(timestamp, payload);
        self.last_packet = Some(Instant::now());
        self.packets_received = self.packets_received.saturating_add(1);
    }

    pub fn next_frame(&mut self) -> Option<DecodedFrame> {
        self.catchup_ticks = if self.onset_frames == 0 && self.jitter.can_accelerate() {
            self.catchup_ticks.saturating_add(1)
        } else {
            0
        };
        let accelerate = self.catchup_ticks >= 25;
        self.expansion_ticks = if self.onset_frames == 0 && self.jitter.can_expand() {
            self.expansion_ticks.saturating_add(1)
        } else {
            0
        };
        let mut outcome = DecodeOutcome::Normal;
        if self.playout_pcm.len() < FRAME_SAMPLES {
            match self.decode_frame() {
                Some(frame) => {
                    outcome = frame.outcome;
                    self.playout_pcm.extend(frame.samples);
                }
                None if self.playout_pcm.is_empty() => return None,
                None => {
                    // Preserve the sub-frame tail left by waveform alignment.
                    let last = self.playout_pcm.back().copied().unwrap_or_default();
                    let remaining = FRAME_SAMPLES - self.playout_pcm.len();
                    self.playout_pcm.extend(
                        (0..remaining).map(|i| last * (1.0 - (i + 1) as f32 / remaining as f32)),
                    );
                    outcome = DecodeOutcome::Silence;
                }
            }
        }
        if accelerate && outcome == DecodeOutcome::Normal {
            self.catchup_ticks = 0;
            self.expansion_ticks = 0;
            if let Some(next) = self.decode_frame() {
                self.playout_pcm.extend(next.samples);
                outcome = next.outcome;
                if outcome == DecodeOutcome::Normal {
                    let samples: [f32; FRAME_SAMPLES * 2] =
                        std::array::from_fn(|i| self.playout_pcm[i]);
                    let (output, consumed) = aligned_compression(&samples);
                    self.playout_pcm.drain(..consumed);
                    self.last_output_sample = output[FRAME_SAMPLES - 1];
                    return Some(DecodedFrame {
                        samples: output,
                        outcome,
                    });
                }
            }
        }
        if self.expansion_ticks >= 5 && outcome == DecodeOutcome::Normal {
            self.expansion_ticks = 0;
            let input = std::array::from_fn(|i| self.playout_pcm[i]);
            let (samples, consumed) = aligned_expansion(&input);
            self.playout_pcm.drain(..consumed);
            self.last_output_sample = samples[FRAME_SAMPLES - 1];
            return Some(DecodedFrame { samples, outcome });
        }
        let samples = std::array::from_fn(|_| self.playout_pcm.pop_front().unwrap_or_default());
        self.last_output_sample = samples[FRAME_SAMPLES - 1];
        Some(DecodedFrame { samples, outcome })
    }

    fn decode_frame(&mut self) -> Option<DecodedFrame> {
        // A scheduler pause must not discard audio that is already buffered.
        // The wall-clock talking timeout only ends an actually empty spurt.
        if !self.is_talking() && self.jitter.depth() == 0 {
            return None;
        }
        if self.jitter.has_started() && self.jitter.depth() == 0 {
            self.empty_playout_ticks = self.empty_playout_ticks.saturating_add(1);
            self.onset_frames = 5;
            self.catchup_ticks = 0;
            self.expansion_ticks = 0;
        }
        let packet = match self.jitter.pop() {
            Some(packet) => packet,
            None if !self.tail_faded
                && (self.jitter.has_started() || self.last_output_sample != 0.0) =>
            {
                self.tail_faded = true;
                return Some(DecodedFrame {
                    samples: fade_to_silence(self.last_output_sample),
                    outcome: DecodeOutcome::Silence,
                });
            }
            None => return None,
        };
        if self.empty_playout_ticks > 0 {
            // Empty then contiguous is ambiguous: a VAD/PTT pause and delayed
            // delivery have identical timestamps in the current protocol.
            // Count only when a following packet proves a missing frame;
            // neither quiet speech nor a normal pause is network-loss evidence.
            if packet.is_none() {
                self.jitter_buffer_underruns = self.jitter_buffer_underruns.saturating_add(1);
            }
            self.empty_playout_ticks = 0;
        }
        if packet.is_none() {
            self.consecutive_missing = self.consecutive_missing.saturating_add(1);
            self.window_max_loss_run = self.window_max_loss_run.max(self.consecutive_missing);
            if self.consecutive_missing == 2 {
                self.window_bursts = self.window_bursts.saturating_add(1);
            }
            self.stable_frames = 0;
            self.local_quality = if self.consecutive_missing >= 3 {
                NetworkQuality::Critical
            } else if self.consecutive_missing >= 2 {
                NetworkQuality::Poor
            } else {
                // Isolated loss is what Opus FEC/PLC is for. Do not turn a
                // single missing 20 ms frame into a 100 ms buffering pause.
                self.local_quality
            };
            self.jitter.set_network_quality(self.local_quality);
        } else {
            self.consecutive_missing = 0;
            self.stable_frames = self.stable_frames.saturating_add(1);
            let recovered = match self.local_quality {
                NetworkQuality::Critical | NetworkQuality::Poor if self.stable_frames >= 20 => {
                    NetworkQuality::Fair
                }
                NetworkQuality::Fair if self.stable_frames >= 50 => NetworkQuality::Good,
                current => current,
            };
            if recovered != self.local_quality {
                self.local_quality = recovered;
                self.jitter.set_network_quality(recovered);
            }
        }
        let mut output = [0.0; FRAME_SAMPLES];
        let (result, mut outcome) = match packet.as_deref() {
            Some(payload) => (
                self.decoder.decode(Some(payload), &mut output),
                DecodeOutcome::Normal,
            ),
            None => match self.jitter.expected_packet() {
                Some(next_payload) => {
                    let outcome = if self.decoder.can_recover_voice_fec(next_payload) {
                        DecodeOutcome::Fec
                    } else {
                        // decode_fec succeeds with PLC when no redundancy is
                        // present. Successful decoding alone is not recovery.
                        DecodeOutcome::Plc
                    };
                    (self.decoder.decode_fec(next_payload, &mut output), outcome)
                }
                None => (self.decoder.decode(None, &mut output), DecodeOutcome::Plc),
            },
        };
        if result.is_err() {
            self.decode_errors = self.decode_errors.saturating_add(1);
            output.fill(0.0);
            outcome = DecodeOutcome::Failed;
        } else {
            match outcome {
                DecodeOutcome::Fec => {
                    self.fec_recovered_packets = self.fec_recovered_packets.saturating_add(1);
                }
                DecodeOutcome::Plc => self.plc_frames = self.plc_frames.saturating_add(1),
                _ => {}
            }
        }
        if outcome == DecodeOutcome::Normal {
            // A tiny ramp at a real silence boundary removes a hard edge; it
            // does not reshape the rest of the opening consonant or vowel.
            if self.tail_faded {
                for (index, sample) in output[..120].iter_mut().enumerate() {
                    *sample *= (index + 1) as f32 / 120.0;
                }
            }
            self.onset_frames = self.onset_frames.saturating_sub(1);
        }
        self.last_output_sample = output.last().copied().unwrap_or_default();
        self.tail_faded = false;
        Some(DecodedFrame {
            samples: output,
            outcome,
        })
    }

    /// Drops queued stale media after the receiver has had to shed backlog.
    /// A fresh decoder prevents predictive state from smearing across the jump.
    pub fn resync_playout(&mut self) {
        // resync clears queued media but retains counters. Archive them only
        // when replacing the whole jitter buffer at a new talk spurt.
        self.jitter.resync();
        if let Ok(decoder) = OpusDecoder::new() {
            self.decoder = decoder;
        }
        self.consecutive_missing = 0;
        self.stable_frames = 0;
        self.catchup_ticks = 0;
        self.expansion_ticks = 0;
        self.playout_pcm.clear();
        self.playout_resyncs = self.playout_resyncs.saturating_add(1);
        self.last_output_sample = 0.0;
        self.tail_faded = true;
        self.empty_playout_ticks = 0;
        self.onset_frames = 5;
    }

    pub fn is_talking(&self) -> bool {
        self.last_packet
            .is_some_and(|last| last.elapsed() < Duration::from_millis(300))
    }
    pub fn packets_lost(&self) -> u64 {
        self.lost_before_reset.saturating_add(self.jitter.lost)
    }
    pub fn packets_reordered(&self) -> u64 {
        self.reordered_before_reset
            .saturating_add(self.jitter.reordered)
    }
    pub fn packets_late(&self) -> u64 {
        self.late_before_reset.saturating_add(self.jitter.late)
    }

    pub fn take_loss_window(&mut self) -> (u32, u32) {
        (
            std::mem::take(&mut self.window_bursts),
            std::mem::take(&mut self.window_max_loss_run),
        )
    }

    #[cfg(test)]
    pub(crate) fn buffered_frames(&self) -> usize {
        self.jitter.depth()
    }
    pub fn max_consecutive_loss(&self) -> u32 {
        self.jitter.max_consecutive_loss()
    }
    pub fn jitter_target_delay_ms(&self) -> f64 {
        (self.jitter.target_delay_frames() * 20) as f64
    }
    pub fn local_network_quality(&self) -> NetworkQuality {
        self.local_quality
    }
    pub fn actual_playout_delay_ms(&self) -> f64 {
        self.jitter.depth() as f64 * 20.0 + self.playout_pcm.len() as f64 / 48.0
    }

    pub fn jitter_depth(&self) -> usize {
        self.jitter.depth()
    }
    pub fn playout_resyncs(&self) -> u64 {
        self.playout_resyncs
    }
    pub fn fec_recovered_packets(&self) -> u64 {
        self.fec_recovered_packets
    }
    pub fn plc_frames(&self) -> u64 {
        self.plc_frames
    }
    pub fn jitter_buffer_underruns(&self) -> u64 {
        self.jitter_buffer_underruns
    }
    pub fn is_stale(&self) -> bool {
        self.last_packet
            .is_some_and(|last| last.elapsed() >= Duration::from_secs(10))
    }
}

// Search a pitch-aligned overlap instead of assuming adjacent 20 ms frames
// have the same phase. Keep unconsumed samples for the next output frame.
fn aligned_compression(input: &[f32; FRAME_SAMPLES * 2]) -> ([f32; FRAME_SAMPLES], usize) {
    let correlation = |shift: usize| {
        let mut cross = 0.0_f64;
        let mut a_energy = 0.0_f64;
        let mut b_energy = 0.0_f64;
        for i in (0..FRAME_SAMPLES).step_by(4) {
            let a = f64::from(input[i]);
            let b = f64::from(input[i + shift]);
            cross += a * b;
            a_energy += a * a;
            b_energy += b * b;
        }
        cross / (a_energy * b_energy).sqrt().max(1e-12)
    };
    let mut best = FRAME_SAMPLES;
    let mut score = correlation(best);
    for shift in (FRAME_SAMPLES / 2..FRAME_SAMPLES).step_by(4) {
        let candidate = correlation(shift);
        if candidate > score {
            score = candidate;
            best = shift;
        }
    }
    let coarse = best;
    for shift in coarse.saturating_sub(3).max(FRAME_SAMPLES / 2)..=(coarse + 3).min(FRAME_SAMPLES) {
        let candidate = correlation(shift);
        if candidate > score {
            score = candidate;
            best = shift;
        }
    }
    // Unvoiced consonants/transients often have no matching overlap. Keeping
    // the original frame is preferable to blending unrelated syllables.
    if score < 0.85 && input.iter().map(|sample| sample * sample).sum::<f32>() > 0.01 {
        return (std::array::from_fn(|i| input[i]), FRAME_SAMPLES);
    }
    let output = std::array::from_fn(|i| {
        let weight = i as f32 / (FRAME_SAMPLES - 1) as f32;
        input[i] * (1.0 - weight) + input[i + best] * weight
    });
    (output, FRAME_SAMPLES + best)
}

// Insert a pitch-aligned segment with a 2.5 ms crossfade. At most once per
// five playout frames, consuming 10-18 ms for 20 ms output; pitch is unchanged.
fn aligned_expansion(input: &[f32; FRAME_SAMPLES]) -> ([f32; FRAME_SAMPLES], usize) {
    const SPLICE: usize = FRAME_SAMPLES / 2;
    const OVERLAP: usize = 120;
    let correlation = |lag: usize| {
        let mut cross = 0.0_f64;
        let mut a_energy = 0.0_f64;
        let mut b_energy = 0.0_f64;
        for i in (0..OVERLAP).step_by(2) {
            let a = f64::from(input[SPLICE + i]);
            let b = f64::from(input[SPLICE + i - lag]);
            cross += a * b;
            a_energy += a * a;
            b_energy += b * b;
        }
        cross / (a_energy * b_energy).sqrt().max(1e-12)
    };
    let coarse = (96..=SPLICE)
        .step_by(4)
        .max_by(|&a, &b| correlation(a).total_cmp(&correlation(b)))
        .unwrap();
    let lag = (coarse.saturating_sub(3).max(96)..=(coarse + 3).min(SPLICE))
        .max_by(|&a, &b| correlation(a).total_cmp(&correlation(b)))
        .unwrap();
    if correlation(lag) < 0.85 && input.iter().map(|sample| sample * sample).sum::<f32>() > 0.01 {
        return (*input, FRAME_SAMPLES);
    }
    let output = std::array::from_fn(|i| {
        if i < SPLICE {
            input[i]
        } else if i < SPLICE + OVERLAP {
            let weight = (i - SPLICE) as f32 / (OVERLAP - 1) as f32;
            input[i] * (1.0 - weight) + input[i - lag] * weight
        } else {
            input[i - lag]
        }
    });
    (output, FRAME_SAMPLES - lag)
}

const TAIL_FADE_SAMPLES: usize = FRAME_SAMPLES / 4;

fn fade_to_silence(last_sample: f32) -> [f32; FRAME_SAMPLES] {
    let last_sample = if last_sample.is_finite() {
        last_sample.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    std::array::from_fn(|index| {
        if index >= TAIL_FADE_SAMPLES {
            0.0
        } else {
            last_sample * (1.0 - (index + 1) as f32 / TAIL_FADE_SAMPLES as f32)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{DEFAULT_BITRATE, OpusEncoder, packet_has_voice_fec};

    fn timestamp(frame: u32) -> u32 {
        frame.wrapping_mul(FRAME_SAMPLES as u32)
    }

    #[test]
    fn expansion_preserves_waveform_and_consumes_less_than_a_frame() {
        for hz in [100.0, 200.0, 440.0] {
            let input = std::array::from_fn(|i| {
                (std::f32::consts::TAU * hz * i as f32 / 48000.0).sin() * 0.4
            });
            let (output, consumed) = aligned_expansion(&input);
            assert!((480..=864).contains(&consumed));
            assert!(output.iter().all(|sample| sample.is_finite()));
            let rms = (output.iter().map(|s| s * s).sum::<f32>() / FRAME_SAMPLES as f32).sqrt();
            assert!((0.24..0.32).contains(&rms));
            assert!(
                output
                    .windows(2)
                    .all(|pair| (pair[1] - pair[0]).abs() < 0.06)
            );
            assert!((output[959] - input[consumed]).abs() < 0.06);
        }
    }

    #[test]
    fn increased_margin_grows_without_stopping_continuous_speech() {
        let mut encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
        // Encode a continuous waveform, rather than replaying the first Opus
        // packet of a DC input (which is not a stable voiced signal).
        let mut packet = |n: u32| {
            let samples = std::array::from_fn::<_, FRAME_SAMPLES, _>(|i| {
                (std::f32::consts::TAU * 180.0 * (n as usize * FRAME_SAMPLES + i) as f32 / 48000.0)
                    .sin()
                    * 0.2
            });
            encoder.encode(&samples).unwrap().to_vec()
        };
        let mut peer = RemotePeer::new(99).unwrap();
        for n in 0..3 {
            peer.push(timestamp(n), packet(n));
        }
        peer.next_frame().unwrap();
        peer.local_quality = NetworkQuality::Poor;
        peer.jitter.set_network_quality(NetworkQuality::Poor);
        let initial = peer.actual_playout_delay_ms();
        let mut maximum = initial;
        for n in 3..103 {
            peer.push(timestamp(n), packet(n));
            let frame = peer.next_frame().expect("buffer growth stopped speech");
            assert_eq!(frame.outcome, DecodeOutcome::Normal);
            maximum = maximum.max(peer.actual_playout_delay_ms());
        }
        assert!(
            maximum > initial,
            "target changed without growing the actual buffer"
        );
        assert_eq!(peer.plc_frames(), 0);
    }

    #[test]
    fn aligned_catchup_preserves_voiced_energy_and_tail_continuity() {
        for frequency in [150.0, 175.0, 200.0, 225.0, 440.0] {
            let signal: [f32; FRAME_SAMPLES * 2] = std::array::from_fn(|i| {
                (std::f32::consts::TAU * frequency * i as f32 / 48000.0).sin() * 0.3
            });
            let (out, consumed) = aligned_compression(&signal);
            let energy = |samples: &[f32]| samples.iter().map(|v| v * v).sum::<f32>();
            assert!(
                energy(&out) / energy(&signal[..FRAME_SAMPLES]) > 0.9,
                "energy dip at {frequency} Hz"
            );
            assert_eq!(out[0], signal[0]);
            assert!((out[FRAME_SAMPLES - 1] - signal[consumed - 1]).abs() < 1e-6);
            assert!((FRAME_SAMPLES * 3 / 2..=FRAME_SAMPLES * 2).contains(&consumed));
        }
    }

    #[test]
    fn healthy_playout_converges_after_stall_and_quality_recovery() {
        for degraded in [false, true] {
            let mut encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
            let payload = encoder.encode(&[0.05; FRAME_SAMPLES]).unwrap().to_vec();
            let mut peer = RemotePeer::new(99).unwrap();
            if degraded {
                peer.local_quality = NetworkQuality::Critical;
                peer.jitter.set_network_quality(NetworkQuality::Critical);
            }
            let initial = if degraded { 8 } else { 3 };
            // Exercise wrapping timestamps as well as regular catch-up.
            let base = u32::MAX - 10_000;
            let ts = |n: u32| base.wrapping_add(timestamp(n));
            for n in 0..initial {
                peer.push(ts(n), payload.clone());
            }
            peer.next_frame().unwrap();
            let mut next = initial;
            if !degraded {
                for _ in 0..8 {
                    peer.push(ts(next), payload.clone());
                    next += 1;
                }
            }
            for _ in 0..300 {
                peer.push(ts(next), payload.clone());
                next += 1;
                let frame = peer.next_frame().expect("continuous playout");
                assert_eq!(frame.outcome, DecodeOutcome::Normal);
                assert!(
                    frame
                        .samples
                        .iter()
                        .all(|v| v.is_finite() && v.abs() <= 1.0)
                );
            }
            assert_eq!(peer.local_quality, NetworkQuality::Good);
            assert!(peer.jitter.depth() <= DEFAULT_PREFILL_FRAMES);
            assert_eq!(peer.packets_lost(), 0);
            assert_eq!(peer.plc_frames(), 0);
            assert_eq!(peer.decode_errors, 0);
        }
    }

    #[test]
    fn peer_owns_decoder_jitter_and_plc() {
        let mut encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
        let packets: Vec<Vec<u8>> = (0..3)
            .map(|index| {
                encoder
                    .encode(&[index as f32 * 0.01; FRAME_SAMPLES])
                    .unwrap()
                    .to_vec()
            })
            .collect();
        let mut peer = RemotePeer::new(7).unwrap();
        peer.push(timestamp(10), packets[0].clone());
        peer.push(timestamp(12), packets[2].clone());
        peer.push(timestamp(11), packets[1].clone());
        for _ in 0..3 {
            let frame = peer.next_frame().unwrap();
            assert!(frame.samples.iter().all(|sample| sample.is_finite()));
        }
        // With no later packet yet, there is no proven loss. Waiting avoids an
        // artificial Opus PLC tail during a normal voice-activity pause.
        let tail = peer.next_frame().unwrap();
        assert!(tail.samples.iter().all(|sample| sample.is_finite()));
        assert!(peer.next_frame().is_none());
        assert_eq!(peer.packets_lost(), 0);
    }

    #[test]
    fn new_talk_spurt_resets_old_sequence_window() {
        let mut encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
        let payload = encoder.encode(&[0.0; FRAME_SAMPLES]).unwrap().to_vec();
        let mut peer = RemotePeer::new(8).unwrap();
        for sequence in 1..=3 {
            peer.push(timestamp(sequence), payload.clone());
        }
        assert!(peer.next_frame().is_some());
        peer.last_packet = Some(Instant::now() - Duration::from_secs(1));
        for sequence in 10_000..10_003 {
            peer.push(timestamp(sequence), payload.clone());
        }
        assert!(peer.next_frame().is_some());
    }

    #[test]
    fn missing_frame_uses_the_following_packet_for_fec_or_plc() {
        let mut encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
        encoder
            .apply_adaptation(crate::network::AudioAdaptationDecision {
                target_bitrate_bps: 48_000,
                fec_enabled: true,
                expected_packet_loss_percent: 10,
                dtx_enabled: false,
            })
            .unwrap();
        let packets: Vec<Vec<u8>> = (0..4)
            .map(|index| {
                let frame: [f32; FRAME_SAMPLES] = std::array::from_fn(|sample| {
                    let phase =
                        (index * FRAME_SAMPLES + sample) as f32 * 440.0 * std::f32::consts::TAU
                            / crate::codec::SAMPLE_RATE as f32;
                    phase.sin() * 0.2
                });
                encoder.encode(&frame).unwrap().to_vec()
            })
            .collect();
        let mut peer = RemotePeer::new(9).unwrap();
        peer.push(timestamp(1), packets[0].clone());
        peer.push(timestamp(2), packets[1].clone());
        peer.push(timestamp(4), packets[3].clone());
        assert!(peer.next_frame().is_some());
        assert!(peer.next_frame().is_some());
        let recovered = peer.next_frame().unwrap();
        assert!(recovered.samples.iter().all(|sample| sample.is_finite()));
        assert!(recovered.samples.iter().any(|sample| sample.abs() > 0.0001));
        let has_fec = packet_has_voice_fec(&packets[3]);
        assert_eq!(
            recovered.outcome,
            if has_fec {
                DecodeOutcome::Fec
            } else {
                DecodeOutcome::Plc
            }
        );
        assert_eq!(peer.fec_recovered_packets(), u64::from(has_fec));
        assert_eq!(peer.plc_frames(), u64::from(!has_fec));
    }

    #[test]
    fn recovered_playout_keeps_speech_frames_until_a_natural_gap() {
        let mut encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
        let payload = encoder.encode(&[0.1; FRAME_SAMPLES]).unwrap().to_vec();
        let mut peer = RemotePeer::new(13).unwrap();
        peer.local_quality = NetworkQuality::Critical;
        peer.jitter.set_network_quality(NetworkQuality::Critical);
        for frame in 0..8 {
            peer.push(timestamp(frame), payload.clone());
        }
        let first = peer.next_frame().unwrap();
        assert_eq!(first.outcome, DecodeOutcome::Normal);
        let mut normally_decoded = 1;
        assert_eq!(peer.buffered_frames(), 7);

        for frame in 8..40 {
            peer.push(timestamp(frame), payload.clone());
            let decoded = peer.next_frame().unwrap();
            assert!(decoded.samples.iter().all(|sample| sample.is_finite()));
            normally_decoded += usize::from(decoded.outcome == DecodeOutcome::Normal);
        }

        assert_eq!(peer.local_network_quality(), NetworkQuality::Fair);
        assert_eq!(peer.jitter_target_delay_ms(), 80.0);
        assert_eq!(peer.buffered_frames(), 7);
        while let Some(decoded) = peer.next_frame() {
            normally_decoded += usize::from(decoded.outcome == DecodeOutcome::Normal);
        }
        assert_eq!(normally_decoded, 40, "recovery discarded audible speech");
        assert_eq!(peer.buffered_frames(), 0);
    }

    #[test]
    fn weak_network_keeps_buffered_speech_before_fading() {
        let mut encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
        let payload = encoder.encode(&[0.2; FRAME_SAMPLES]).unwrap().to_vec();
        let mut peer = RemotePeer::new(14).unwrap();
        for frame in [0, 1, 2, 5] {
            peer.push(timestamp(frame), payload.clone());
        }
        for _ in 0..3 {
            assert_eq!(peer.next_frame().unwrap().outcome, DecodeOutcome::Normal);
        }
        assert!(matches!(
            peer.next_frame().unwrap().outcome,
            DecodeOutcome::Fec | DecodeOutcome::Plc
        ));
        assert!(matches!(
            peer.next_frame().unwrap().outcome,
            DecodeOutcome::Fec | DecodeOutcome::Plc
        ));

        let transition = peer.next_frame().expect("rebuffering hard-cut to silence");
        assert_eq!(transition.outcome, DecodeOutcome::Normal);
        assert!(
            transition
                .samples
                .iter()
                .any(|sample| sample.abs() > 0.0001)
        );
        assert_eq!(peer.next_frame().unwrap().outcome, DecodeOutcome::Silence);
        assert!(peer.next_frame().is_none());
    }

    #[test]
    fn stable_good_network_never_degrades_on_successful_packets() {
        let mut encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
        let payload = encoder.encode(&[0.1; FRAME_SAMPLES]).unwrap().to_vec();
        let mut peer = RemotePeer::new(10).unwrap();
        for sequence in 0..80 {
            peer.push(timestamp(sequence), payload.clone());
            let _ = peer.next_frame();
            assert_eq!(peer.local_network_quality(), NetworkQuality::Good);
        }
    }

    #[test]
    fn non_audio_packet_sequence_gaps_do_not_create_fake_loss() {
        let mut encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
        let payload = encoder.encode(&[0.1; FRAME_SAMPLES]).unwrap().to_vec();
        let mut peer = RemotePeer::new(11).unwrap();
        // Transport packet sequence numbers may contain registration and probe
        // packets. Continuous audio timestamps must still decode contiguously.
        for frame in 0..4 {
            peer.push(timestamp(frame), payload.clone());
        }
        for _ in 0..4 {
            assert!(peer.next_frame().is_some());
        }
        assert_eq!(peer.packets_lost(), 0);
    }

    #[test]
    fn backlog_resync_discards_old_playout_and_restarts_cleanly() {
        let mut encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
        let payload = encoder.encode(&[0.1; FRAME_SAMPLES]).unwrap().to_vec();
        let mut peer = RemotePeer::new(12).unwrap();
        for frame in 0..3 {
            peer.push(timestamp(frame), payload.clone());
        }
        assert!(peer.next_frame().is_some());
        peer.resync_playout();
        assert_eq!(peer.buffered_frames(), 0);
        assert_eq!(peer.playout_resyncs(), 1);
        for frame in 100..103 {
            peer.push(timestamp(frame), payload.clone());
        }
        assert!(peer.next_frame().is_some());
    }

    #[test]
    fn repeated_resync_does_not_count_old_losses_or_late_packets_twice() {
        let mut encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
        let payload = encoder.encode(&[0.1; FRAME_SAMPLES]).unwrap().to_vec();
        let mut peer = RemotePeer::new(15).unwrap();
        for frame in [0, 2, 3] {
            peer.push(timestamp(frame), payload.clone());
        }
        peer.next_frame();
        peer.next_frame();
        peer.push(timestamp(0), payload.clone());
        let before = (
            peer.packets_lost(),
            peer.packets_late(),
            peer.packets_reordered(),
        );
        assert_eq!(before.0, 1);
        assert_eq!(before.1, 1);
        for _ in 0..3 {
            peer.resync_playout();
            assert_eq!(
                (
                    peer.packets_lost(),
                    peer.packets_late(),
                    peer.packets_reordered()
                ),
                before
            );
        }
        peer.last_packet = Some(Instant::now() - Duration::from_secs(1));
        peer.push(timestamp(10), payload);
        assert_eq!(
            (
                peer.packets_lost(),
                peer.packets_late(),
                peer.packets_reordered()
            ),
            before
        );
    }

    #[test]
    fn speech_tail_fades_once_then_stays_silent() {
        let frame = fade_to_silence(0.8);
        assert!(frame[0] < 0.8 && frame[0] > 0.0);
        assert_eq!(frame[TAIL_FADE_SAMPLES - 1], 0.0);
        assert!(
            frame[TAIL_FADE_SAMPLES..]
                .iter()
                .all(|sample| *sample == 0.0)
        );
        assert!(frame.windows(2).all(|pair| pair[1] <= pair[0]));
    }

    #[test]
    fn isolated_loss_without_fec_does_not_pause_or_claim_recovery() {
        let mut encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
        let mut peer = RemotePeer::new(20).unwrap();
        for frame in 0..8 {
            let pcm = std::array::from_fn::<_, FRAME_SAMPLES, _>(|sample| {
                ((frame * FRAME_SAMPLES + sample) as f32 * 440.0 * std::f32::consts::TAU
                    / crate::codec::SAMPLE_RATE as f32)
                    .sin()
                    * 0.2
            });
            let payload = encoder.encode(&pcm).unwrap().to_vec();
            assert!(!packet_has_voice_fec(&payload));
            if frame != 3 {
                peer.push(timestamp(frame as u32), payload);
            }
        }
        for frame in 0..8 {
            let decoded = peer
                .next_frame()
                .expect("isolated loss inserted a buffering gap");
            assert_eq!(
                decoded.outcome,
                if frame == 3 {
                    DecodeOutcome::Plc
                } else {
                    DecodeOutcome::Normal
                }
            );
        }
        assert_eq!(peer.fec_recovered_packets(), 0);
        assert_eq!(peer.plc_frames(), 1);
        assert_eq!(peer.jitter_buffer_underruns(), 0);
        assert_eq!(peer.take_loss_window(), (0, 1));
        assert_eq!(peer.take_loss_window(), (0, 0));
    }
}

#[cfg(test)]
mod continuity_regressions {
    use super::*;
    use crate::codec::{DEFAULT_BITRATE, OpusEncoder};

    fn packets(count: usize) -> Vec<Vec<u8>> {
        let mut encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
        (0..count)
            .map(|n| {
                let signal = std::array::from_fn::<_, FRAME_SAMPLES, _>(|i| {
                    let t = (n * FRAME_SAMPLES + i) as f32 / 48000.0;
                    0.2 * (std::f32::consts::TAU * 180.0 * t).sin()
                        + 0.08 * (std::f32::consts::TAU * 427.0 * t).sin()
                });
                encoder.encode(&signal).unwrap().to_vec()
            })
            .collect()
    }

    #[test]
    fn delayed_packets_resume_on_arrival_without_an_added_refill_pause() {
        let packets = packets(150);
        for delay in [0usize, 2, 3, 4, 5] {
            let mut peer = RemotePeer::new(123).unwrap();
            let mut affected = 0;
            for tick in 0..150 {
                for (n, packet) in packets.iter().enumerate() {
                    if n + if n >= 50 { delay } else { 0 } == tick {
                        peer.push((n * FRAME_SAMPLES) as u32, packet.clone());
                    }
                }
                let output = peer.next_frame();
                if tick > 10
                    && output
                        .as_ref()
                        .is_none_or(|f| f.outcome == DecodeOutcome::Silence)
                {
                    affected += 1;
                }
                if tick == 50 + delay {
                    assert_eq!(
                        output.unwrap().outcome,
                        DecodeOutcome::Normal,
                        "first resumed packet unnecessarily waited for prefill"
                    );
                }
            }
            // Initial 40 ms margin absorbs two frames. There is no additional
            // two-frame rebuffer pause once the delayed packet arrives.
            assert!(
                affected <= delay.saturating_sub(2),
                "delay={delay}, affected={affected}"
            );
            assert_eq!(peer.packets_lost(), 0);
            assert_eq!(peer.packets_late(), 0);
            assert_eq!(
                peer.plc_frames(),
                0,
                "must not invent speech during a VAD pause"
            );
            // Delayed but contiguous cannot be distinguished from a sender
            // pause. Do not label it a confirmed network buffer underrun.
            assert_eq!(peer.jitter_buffer_underruns(), 0);
            println!(
                "additional delivery delay={}ms, affected={}ms",
                delay * 20,
                affected * 20
            );
        }
    }

    #[test]
    fn next_phrase_preserves_decoder_history_and_opening_samples() {
        let packets = packets(12);
        let mut reference = OpusDecoder::new().unwrap();
        let expected: Vec<_> = packets
            .iter()
            .map(|packet| {
                let mut samples = [0.0; FRAME_SAMPLES];
                reference.decode(Some(packet), &mut samples).unwrap();
                samples
            })
            .collect();
        let mut peer = RemotePeer::new(124).unwrap();
        for n in 0..3 {
            peer.push((n * FRAME_SAMPLES) as u32, packets[n].clone());
        }
        for _ in 0..3 {
            peer.next_frame().unwrap();
        }
        let tail = peer.next_frame().unwrap();
        assert_eq!(tail.outcome, DecodeOutcome::Silence);
        for _ in 0..20 {
            assert!(peer.next_frame().is_none());
        }
        peer.last_packet = Some(Instant::now() - Duration::from_secs(1));
        // A new phrase can arrive in a burst (pre-roll or network batching).
        for n in 3..12 {
            peer.push((n * FRAME_SAMPLES) as u32, packets[n].clone());
        }
        for n in 3..8 {
            let frame = peer.next_frame().unwrap();
            assert_eq!(frame.outcome, DecodeOutcome::Normal);
            // Only the first 2.5 ms is ramped; all following opening samples
            // retain the true decoder waveform, without compression/expansion.
            let start = if n == 3 { 120 } else { 0 };
            assert_eq!(frame.samples[start..], expected[n][start..]);
        }
        assert_eq!(peer.packets_late(), 0);
        assert_eq!(peer.jitter_buffer_underruns(), 0);
    }

    #[test]
    fn uncorrelated_transients_are_not_stretched() {
        let mut seed = 0x12345678u32;
        let input = std::array::from_fn::<_, { FRAME_SAMPLES * 2 }, _>(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32 - 0.5) * 0.4
        });
        let first = std::array::from_fn(|i| input[i]);
        let (compressed, consumed) = aligned_compression(&input);
        assert_eq!(consumed, FRAME_SAMPLES);
        assert_eq!(compressed, first);
        let (expanded, consumed) = aligned_expansion(&first);
        assert_eq!(consumed, FRAME_SAMPLES);
        assert_eq!(expanded, first);
    }
}

#[cfg(test)]
mod pause_statistics_regressions {
    use super::*;
    use crate::codec::{DEFAULT_BITRATE, OpusEncoder};

    #[test]
    fn normal_short_and_long_pauses_are_not_confirmed_underruns() {
        for empty_ticks in [1, 2, 3, 4, 5, 10, 20, 40] {
            let mut encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
            let mut peer = RemotePeer::new(201).unwrap();
            let mut sequence = 0;
            for _ in 0..12 {
                for _ in 0..20 {
                    let samples = std::array::from_fn::<_, FRAME_SAMPLES, _>(|i| {
                        ((sequence as usize * FRAME_SAMPLES + i) as f32
                            * std::f32::consts::TAU
                            * 180.0
                            / 48000.0)
                            .sin()
                            * 0.1
                    });
                    let payload = encoder.encode(&samples).unwrap().to_vec();
                    peer.push(sequence * FRAME_SAMPLES as u32, payload);
                    sequence += 1;
                    peer.next_frame();
                }
                while peer.jitter.depth() > 0 {
                    peer.next_frame();
                }
                for _ in 0..empty_ticks {
                    peer.next_frame();
                }
            }
            assert_eq!(
                peer.jitter_buffer_underruns(),
                0,
                "pause ticks={empty_ticks}"
            );
            assert_eq!(peer.packets_lost(), 0);
            assert_eq!(peer.packets_late(), 0);
            assert_eq!(peer.decode_errors, 0);
        }
    }

    #[test]
    fn empty_buffer_followed_by_a_proven_missing_packet_is_counted_once() {
        let mut encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
        let mut peer = RemotePeer::new(202).unwrap();
        let packets: Vec<_> = (0..6)
            .map(|_| encoder.encode(&[0.1; FRAME_SAMPLES]).unwrap().to_vec())
            .collect();
        for n in 0..3 {
            peer.push(n as u32 * 960, packets[n].clone());
        }
        for _ in 0..3 {
            peer.next_frame();
        }
        peer.next_frame();
        peer.push(4 * 960, packets[4].clone());
        assert!(matches!(
            peer.next_frame().unwrap().outcome,
            DecodeOutcome::Plc | DecodeOutcome::Fec
        ));
        assert_eq!(peer.jitter_buffer_underruns(), 1);
        assert_eq!(peer.packets_lost(), 1);
        assert_eq!(peer.next_frame().unwrap().outcome, DecodeOutcome::Normal);
        assert_eq!(peer.jitter_buffer_underruns(), 1);
    }
}
