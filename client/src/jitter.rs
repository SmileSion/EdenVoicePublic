use std::collections::BTreeMap;

use crate::network::NetworkQuality;

pub const DEFAULT_PREFILL_FRAMES: usize = 3;
const MAX_AHEAD_FRAMES: u32 = 128;

pub struct JitterBuffer<T> {
    packets: BTreeMap<u32, T>,
    expected: Option<u32>,
    sequence_step: u32,
    prefill_frames: usize,
    min_prefill_frames: usize,
    max_prefill_frames: usize,
    started: bool,
    has_played: bool,
    prefill_waits: usize,
    empty_ticks: usize,
    last_arrival: Option<u32>,
    pub duplicates: u64,
    pub late: u64,
    pub too_far: u64,
    pub lost: u64,
    pub reordered: u64,
    current_loss_run: u32,
    max_consecutive_loss: u32,
}

impl<T> JitterBuffer<T> {
    pub fn new(prefill_frames: usize) -> Self {
        Self::new_with_step(prefill_frames, 1)
    }

    pub fn new_with_step(prefill_frames: usize, sequence_step: u32) -> Self {
        Self {
            packets: BTreeMap::new(),
            expected: None,
            sequence_step: sequence_step.max(1),
            prefill_frames: prefill_frames.clamp(1, 10),
            min_prefill_frames: prefill_frames.clamp(1, 10),
            max_prefill_frames: 10,
            started: false,
            has_played: false,
            prefill_waits: 0,
            empty_ticks: 0,
            last_arrival: None,
            duplicates: 0,
            late: 0,
            too_far: 0,
            lost: 0,
            reordered: 0,
            current_loss_run: 0,
            max_consecutive_loss: 0,
        }
    }

    pub fn push(&mut self, sequence: u32, packet: T) {
        if self
            .last_arrival
            .is_some_and(|previous| is_before(sequence, previous))
        {
            self.reordered = self.reordered.saturating_add(1);
        }
        self.last_arrival = Some(sequence);
        if let Some(expected) = self.expected {
            if self.has_played && is_before(sequence, expected) {
                self.late += 1;
                return;
            }
            if !self.has_played && is_before(sequence, expected) {
                self.expected = Some(sequence);
            } else if sequence.wrapping_sub(expected) / self.sequence_step > MAX_AHEAD_FRAMES
                && !is_before(sequence, expected)
            {
                self.too_far += 1;
                return;
            }
        } else {
            self.expected = Some(sequence);
        }
        if self.packets.insert(sequence, packet).is_some() {
            self.duplicates += 1;
        }
    }

    pub fn pop(&mut self) -> Option<Option<T>> {
        if !self.started {
            if self.packets.is_empty() {
                return None;
            }
            // Bound waiting in playout ticks as well as packet count: a short
            // PTT/VAD spurt or a lost tail may never fill the target depth.
            self.prefill_waits = self.prefill_waits.saturating_add(1);
            if self.packets.len() < self.prefill_frames && self.prefill_waits < self.prefill_frames
            {
                return None;
            }
            self.started = true;
            self.prefill_waits = 0;
        }
        let expected = self.expected?;
        if self.packets.is_empty() {
            // Keep the active timeline through a short delivery gap. Do not
            // advance expected: old senders stop timestamps during VAD/PTT.
            // Refill only after 100 ms without queued audio; otherwise the next
            // real packet must play immediately, not wait another 40 ms.
            self.empty_ticks = self.empty_ticks.saturating_add(1);
            if self.empty_ticks >= 5 {
                self.started = false;
                self.prefill_waits = 0;
            }
            return None;
        }
        self.empty_ticks = 0;
        self.expected = Some(expected.wrapping_add(self.sequence_step));
        self.has_played = true;
        let result = self.packets.remove(&expected);
        if result.is_none() {
            self.lost += 1;
            self.current_loss_run = self.current_loss_run.saturating_add(1);
            self.max_consecutive_loss = self.max_consecutive_loss.max(self.current_loss_run);
        } else {
            self.current_loss_run = 0;
        }
        Some(result)
    }

    /// Returns the packet due on the next playout deadline. After a missing
    /// packet is popped, Opus can use this following packet for in-band FEC.
    pub fn expected_packet(&self) -> Option<&T> {
        self.expected
            .and_then(|sequence| self.packets.get(&sequence))
    }

    /// Acceleration must consume real consecutive packets, never a gap that
    /// needs FEC/PLC, and must leave the current safety margin intact.
    pub fn can_accelerate(&self) -> bool {
        self.started
            && self.packets.len() > self.prefill_frames
            && self.expected.is_some_and(|expected| {
                self.packets.contains_key(&expected)
                    && self
                        .packets
                        .contains_key(&expected.wrapping_add(self.sequence_step))
            })
    }

    pub fn depth(&self) -> usize {
        self.packets.len()
    }

    pub fn has_started(&self) -> bool {
        self.started
    }

    pub fn target_delay_frames(&self) -> usize {
        self.prefill_frames
    }

    pub fn max_consecutive_loss(&self) -> u32 {
        self.max_consecutive_loss
    }

    pub fn set_network_quality(&mut self, quality: NetworkQuality) -> bool {
        let target = match quality {
            NetworkQuality::Good => self.min_prefill_frames,
            NetworkQuality::Fair => self.min_prefill_frames.saturating_add(1),
            NetworkQuality::Poor => self.min_prefill_frames.saturating_add(3),
            NetworkQuality::Critical => self.min_prefill_frames.saturating_add(5),
        };
        let target = target.clamp(self.min_prefill_frames, self.max_prefill_frames);
        // Active speech grows its safety margin through pitch-aligned time
        // expansion in RemotePeer, rather than stopping to refill the queue.
        self.prefill_frames = target;
        false
    }

    pub fn can_expand(&self) -> bool {
        self.started && self.packets.len() < self.prefill_frames && self.expected_packet().is_some()
    }

    pub fn resync(&mut self) {
        self.packets.clear();
        self.expected = None;
        self.started = false;
        self.has_played = false;
        self.prefill_waits = 0;
        self.empty_ticks = 0;
        self.last_arrival = None;
        self.current_loss_run = 0;
    }
}

fn is_before(sequence: u32, reference: u32) -> bool {
    sequence != reference && sequence.wrapping_sub(reference) > (u32::MAX / 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catchup_requires_started_contiguous_audio_above_target() {
        let mut jitter = JitterBuffer::new_with_step(3, 960);
        for n in [0, 1, 3, 4, 5, 6] {
            jitter.push(n * 960, n);
        }
        assert!(!jitter.can_accelerate());
        assert_eq!(jitter.pop(), Some(Some(0)));
        assert!(
            !jitter.can_accelerate(),
            "next packet is missing; preserve FEC/PLC"
        );
        jitter.push(2 * 960, 2);
        assert!(jitter.can_accelerate());
        jitter.set_network_quality(NetworkQuality::Critical);
        assert!(!jitter.can_accelerate(), "respect the new safety margin");
    }

    #[test]
    fn reorders_and_reports_loss() {
        let mut jitter = JitterBuffer::new(3);
        jitter.push(10, "a");
        jitter.push(12, "c");
        jitter.push(11, "b");
        assert_eq!(jitter.pop(), Some(Some("a")));
        assert_eq!(jitter.pop(), Some(Some("b")));
        assert_eq!(jitter.pop(), Some(Some("c")));
        assert_eq!(jitter.reordered, 1);
        jitter.push(14, "e");
        assert_eq!(jitter.pop(), Some(None));
        assert_eq!(jitter.lost, 1);
    }
    #[test]
    fn handles_sequence_wrap() {
        let mut jitter = JitterBuffer::new(3);
        jitter.push(u32::MAX - 1, 1);
        jitter.push(u32::MAX, 2);
        jitter.push(0, 3);
        assert_eq!(jitter.pop(), Some(Some(1)));
        assert_eq!(jitter.pop(), Some(Some(2)));
        assert_eq!(jitter.pop(), Some(Some(3)));
    }
    #[test]
    fn rejects_late_and_counts_duplicates() {
        let mut jitter = JitterBuffer::new(1);
        jitter.push(5, 1);
        jitter.push(5, 2);
        assert_eq!(jitter.duplicates, 1);
        assert_eq!(jitter.pop(), Some(Some(2)));
        jitter.push(5, 3);
        assert_eq!(jitter.late, 1);
    }

    #[test]
    fn accepts_an_earlier_first_reordered_packet() {
        let mut jitter = JitterBuffer::new(3);
        jitter.push(11, "b");
        jitter.push(10, "a");
        jitter.push(12, "c");
        assert_eq!(jitter.pop(), Some(Some("a")));
        assert_eq!(jitter.pop(), Some(Some("b")));
    }

    #[test]
    fn bounds_far_future_packets() {
        let mut jitter = JitterBuffer::new(1);
        jitter.push(10, 1);
        jitter.push(1000, 2);
        assert_eq!(jitter.too_far, 1);
        assert_eq!(jitter.pop(), Some(Some(1)));
    }

    #[test]
    fn adapts_prefill_and_resyncs_without_replaying_stale_packets() {
        let mut jitter = JitterBuffer::new(3);
        jitter.set_network_quality(NetworkQuality::Critical);
        assert_eq!(jitter.target_delay_frames(), 8);
        for sequence in 1..=7 {
            jitter.push(sequence, sequence);
        }
        assert_eq!(jitter.pop(), None);
        jitter.resync();
        assert_eq!(jitter.depth(), 0);
        for sequence in 100..108 {
            jitter.push(sequence, sequence);
        }
        assert_eq!(jitter.pop(), Some(Some(100)));
    }

    #[test]
    fn increasing_target_keeps_buffered_speech_playing() {
        let mut jitter = JitterBuffer::new(1);
        jitter.push(1, 1);
        jitter.push(2, 2);
        assert_eq!(jitter.pop(), Some(Some(1)));
        assert!(!jitter.set_network_quality(NetworkQuality::Poor));
        assert_eq!(jitter.target_delay_frames(), 4);
        assert!(jitter.can_expand());
        assert_eq!(jitter.pop(), Some(Some(2)));
        assert!(!jitter.can_expand());
    }

    #[test]
    fn an_empty_voice_activity_gap_waits_without_synthesizing_plc() {
        let mut jitter = JitterBuffer::new(1);
        jitter.push(10, "before pause");
        assert_eq!(jitter.pop(), Some(Some("before pause")));
        for _ in 0..20 {
            assert_eq!(jitter.pop(), None);
        }
        assert_eq!(jitter.lost, 0);

        jitter.push(11, "after pause");
        assert_eq!(jitter.pop(), Some(Some("after pause")));
        assert_eq!(jitter.late, 0);
        assert_eq!(jitter.lost, 0);
    }

    #[test]
    fn supports_rtp_timestamp_steps_and_wrap() {
        let mut jitter = JitterBuffer::new_with_step(2, 960);
        let first = u32::MAX - 255;
        let second = first.wrapping_add(960);
        jitter.push(first, "a");
        jitter.push(second, "b");
        assert_eq!(jitter.pop(), Some(Some("a")));
        assert_eq!(jitter.pop(), Some(Some("b")));
    }

    #[test]
    fn rebuffering_never_rewinds_the_played_timestamp() {
        let mut jitter = JitterBuffer::new(1);
        jitter.push(10, "played");
        assert_eq!(jitter.pop(), Some(Some("played")));
        jitter.set_network_quality(NetworkQuality::Poor);
        jitter.push(10, "late duplicate");
        for sequence in 11..15 {
            jitter.push(sequence, "new");
        }
        assert_eq!(jitter.late, 1);
        assert_eq!(jitter.pop(), Some(Some("new")));
        assert_eq!(jitter.depth(), 3);
    }

    #[test]
    fn short_spurts_wait_but_active_tails_are_not_rebuffered() {
        let mut jitter = JitterBuffer::new(3);
        jitter.push(0, "short word");
        assert_eq!(jitter.pop(), None);
        assert_eq!(jitter.pop(), None);
        assert_eq!(jitter.pop(), Some(Some("short word")));
        jitter.set_network_quality(NetworkQuality::Critical);
        jitter.push(1, "tail");
        assert_eq!(jitter.pop(), Some(Some("tail")));
        assert_eq!(jitter.lost, 0);
    }

    #[test]
    fn a_short_empty_buffer_resumes_without_an_extra_prefill() {
        let mut jitter = JitterBuffer::new(3);
        for frame in 0..3 {
            jitter.push(frame, frame);
        }
        for frame in 0..3 {
            assert_eq!(jitter.pop(), Some(Some(frame)));
        }
        assert_eq!(jitter.pop(), None);
        for frame in 3..6 {
            jitter.push(frame, frame);
            assert_eq!(jitter.pop(), Some(Some(frame)));
        }
        assert_eq!(jitter.lost, 0);
    }
}

#[cfg(test)]
mod pause_regressions {
    use super::*;
    #[test]
    fn long_pause_prefills_without_advancing_or_losing_the_next_word() {
        let mut jitter = JitterBuffer::new(3);
        for n in 0..3 {
            jitter.push(n, n);
        }
        for n in 0..3 {
            assert_eq!(jitter.pop(), Some(Some(n)));
        }
        for _ in 0..30 {
            assert_eq!(jitter.pop(), None);
        }
        jitter.push(3, 3);
        assert_eq!(jitter.pop(), None);
        jitter.push(4, 4);
        assert_eq!(jitter.pop(), None);
        jitter.push(5, 5);
        assert_eq!(jitter.pop(), Some(Some(3)));
        assert_eq!(jitter.lost, 0);
        assert_eq!(jitter.late, 0);
    }
}
