//! Local-only analysis with fixed-size scratch buffers and no audio recording.
//! It never changes the capture frame or independent network authorization gate.
use crate::dsp::{AudioFrame, FrameAnalysis};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutedSpeechNotice {
    pub sequence: String,
    pub at_unix_ms: u64,
}

#[derive(Default)]
pub(super) struct SpeechReminder {
    voiced: Duration,
    last_frame: Option<Instant>,
    last_voice: Option<Instant>,
    last_notice: Option<Instant>,
}

impl SpeechReminder {
    pub(super) fn observe(&mut self, eligible: bool, speech: bool, at: Instant) -> bool {
        if !eligible {
            self.voiced = Duration::ZERO;
            self.last_frame = None;
            self.last_voice = None;
            return false;
        }
        let step = self
            .last_frame
            .map(|previous| at.saturating_duration_since(previous))
            .unwrap_or(Duration::from_millis(20));
        self.last_frame = Some(at);
        if step > Duration::from_millis(160) {
            self.voiced = Duration::ZERO;
        }
        if speech {
            self.voiced += step.min(Duration::from_millis(20));
            self.last_voice = Some(at);
        } else if self
            .last_voice
            .is_none_or(|last| at.saturating_duration_since(last) > Duration::from_millis(180))
        {
            self.voiced = Duration::ZERO;
        }
        if self.voiced < Duration::from_millis(1200)
            || self
                .last_notice
                .is_some_and(|last| at.saturating_duration_since(last) < Duration::from_secs(30))
        {
            return false;
        }
        self.voiced = Duration::ZERO;
        self.last_notice = Some(at);
        true
    }
}

pub(super) struct LocalSpeechProbe {
    denoiser: Option<rnnoise2::Denoiser>,
    input: [f32; 480],
    discarded: [f32; 480],
}

impl LocalSpeechProbe {
    pub(super) fn new() -> Self {
        Self {
            denoiser: None,
            input: [0.0; 480],
            discarded: [0.0; 480],
        }
    }

    pub(super) fn is_speech(&mut self, frame: &AudioFrame, analysis: FrameAnalysis) -> bool {
        // The existing post-DSP gate cheaply rejects silence, and the neural
        // probability distinguishes speech from sustained energy/noise. RNNoise
        // mode reuses its computed probability without a second inference.
        if !analysis.voice_active || analysis.level.rms < 0.004 {
            return false;
        }
        if let Some(probability) = analysis.speech_probability {
            return probability >= 0.65;
        }
        let denoiser = self
            .denoiser
            .get_or_insert_with(|| rnnoise2::Denoiser::new(None).expect("bundled RNNoise model"));
        let mut probability = 0.0_f32;
        for chunk in frame.chunks_exact(480) {
            for (input, sample) in self.input.iter_mut().zip(chunk) {
                *input = (*sample * 32768.0).clamp(-32768.0, 32767.0);
            }
            probability = probability.max(denoiser.process(&self.input, &mut self.discarded));
        }
        probability >= 0.65
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_noise_is_ignored_sustained_speech_notifies_and_cooldown_survives_gate_changes() {
        let start = Instant::now();
        let mut gate = SpeechReminder::default();
        for i in 0..10 {
            assert!(!gate.observe(true, true, start + Duration::from_millis(i * 20)));
        }
        for i in 10..30 {
            assert!(!gate.observe(true, false, start + Duration::from_millis(i * 20)));
        }
        let notices = (30..100)
            .filter(|i| gate.observe(true, true, start + Duration::from_millis(i * 20)))
            .count();
        assert_eq!(notices, 1);
        assert!(!gate.observe(false, true, start + Duration::from_secs(3)));
        for i in 0..100 {
            assert!(!gate.observe(
                true,
                true,
                start + Duration::from_secs(4) + Duration::from_millis(i * 20)
            ));
        }
        let notices = (0..100)
            .filter(|i| {
                gate.observe(
                    true,
                    true,
                    start + Duration::from_secs(34) + Duration::from_millis(i * 20),
                )
            })
            .count();
        assert_eq!(notices, 1);
    }
    #[test]
    fn ineligible_frames_never_accumulate_a_notice() {
        let mut gate = SpeechReminder::default();
        let at = Instant::now();
        for i in 0..200 {
            assert!(!gate.observe(false, true, at + Duration::from_millis(i * 20)));
        }
        for i in 200..250 {
            assert!(!gate.observe(true, true, at + Duration::from_millis(i * 20)));
        }
    }
    #[test]
    fn neural_probability_is_required_even_for_loud_post_dsp_frames() {
        let mut probe = LocalSpeechProbe::new();
        let mut analysis = FrameAnalysis {
            voice_active: true,
            level: crate::dsp::LevelSnapshot {
                rms: 0.2,
                peak: 0.3,
            },
            speech_probability: Some(0.1),
        };
        assert!(!probe.is_speech(&[0.2; crate::codec::FRAME_SAMPLES], analysis));
        analysis.speech_probability = Some(0.9);
        assert!(probe.is_speech(&[0.2; crate::codec::FRAME_SAMPLES], analysis));
        assert!(
            probe.denoiser.is_none(),
            "RNNoise mode must reuse its existing probability"
        );
        analysis.voice_active = false;
        assert!(!probe.is_speech(&[0.2; crate::codec::FRAME_SAMPLES], analysis));
    }
    #[test]
    fn isolated_neural_probe_rejects_silence_and_sustained_noise_without_touching_audio() {
        let mut probe = LocalSpeechProbe::new();
        let mut reminder = SpeechReminder::default();
        let start = Instant::now();
        let mut seed = 13_u32;
        assert!(!probe.is_speech(
            &[0.0; crate::codec::FRAME_SAMPLES],
            FrameAnalysis::default()
        ));
        assert!(
            probe.denoiser.is_none(),
            "quiet mute must not allocate or run a second model"
        );
        for i in 0..150 {
            let frame = std::array::from_fn(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32 * 0.08
            });
            let original = frame;
            let speech = probe.is_speech(
                &frame,
                FrameAnalysis {
                    voice_active: true,
                    level: crate::dsp::analyze_level(&frame),
                    speech_probability: None,
                },
            );
            assert_eq!(frame, original);
            assert!(
                !reminder.observe(true, speech, start + Duration::from_millis(i * 20)),
                "steady noise was reported as sustained speech"
            );
        }
        println!(
            "isolated muted probe: {:.3} ms per 20ms frame (local test build, noise pre-gate forced open)",
            start.elapsed().as_secs_f64() * 1000.0 / 150.0
        );
    }
}
