use std::{
    collections::VecDeque,
    ptr::null_mut,
    thread,
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};
use df::tract::{DfParams, DfTract, RuntimeParams};
use ndarray::{ArrayView2, ArrayViewMut2};
use rnnoise2::Denoiser;
use serde::{Deserialize, Serialize};
use sonora::config::{
    AdaptiveDigital, EchoCanceller as SonoraEchoCanceller, GainController2, HighPassFilter,
    MaxProcessingRate, NoiseSuppression, NoiseSuppressionLevel,
};
use sonora::{AudioProcessing, Config as SonoraConfig, StreamConfig};
use webrtc_audio_processing_sys as webrtc_apm;

use crate::codec::FRAME_SAMPLES;

pub type AudioFrame = [f32; FRAME_SAMPLES];
const RNNOISE_FRAME_SIZE: usize = 480;

/// Provenance for each 10 ms chunk, which is also the neural model's hop size.
/// A delayed 20 ms output may span two different capture/PTT intervals.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CaptureChunkTiming {
    pub captured_at: Instant,
    pub queued_at: Instant,
    pub ptt_pressed: bool,
    pub input_mode: InputMode,
    pub transmission_allowed: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CaptureTiming {
    pub chunks: [Option<CaptureChunkTiming>; 2],
}

impl CaptureTiming {
    pub(crate) fn new(
        captured_at: Instant,
        queued_at: Instant,
        ptt_pressed: bool,
        input_mode: InputMode,
        transmission_allowed: bool,
    ) -> Self {
        Self {
            chunks: std::array::from_fn(|index| {
                Some(CaptureChunkTiming {
                    captured_at: captured_at + Duration::from_millis(index as u64 * 10),
                    queued_at,
                    ptt_pressed,
                    input_mode,
                    transmission_allowed,
                })
            }),
        }
    }

    /// Crop unauthorized intervals before mixing processors with different delays.
    /// Keep the processed preview intact; this mask only applies to transmission.
    pub(crate) fn restrict_transmission(&self, frame: &mut AudioFrame, mode: InputMode) -> bool {
        let mut permitted = false;
        for (samples, timing) in frame.chunks_exact_mut(480).zip(self.chunks) {
            let allowed = timing.is_some_and(|timing| {
                timing.transmission_allowed
                    && (timing.input_mode == InputMode::VoiceActivity || timing.ptt_pressed)
                    && (mode == InputMode::VoiceActivity || timing.ptt_pressed)
            });
            if allowed {
                permitted = true;
            } else {
                samples.fill(0.0);
            }
        }
        permitted
    }

    pub(crate) fn captured_at(&self) -> Option<Instant> {
        self.chunks
            .iter()
            .flatten()
            .map(|chunk| chunk.captured_at)
            .min()
    }

    pub(crate) fn queued_at(&self) -> Option<Instant> {
        self.chunks
            .iter()
            .flatten()
            .map(|chunk| chunk.queued_at)
            .min()
    }
}

struct CaptureTimingDelay {
    chunks: VecDeque<Option<CaptureChunkTiming>>,
}

impl CaptureTimingDelay {
    fn new(delay_samples: usize) -> Self {
        assert_eq!(delay_samples % 480, 0);
        Self {
            chunks: std::iter::repeat_n(None, delay_samples / 480).collect(),
        }
    }

    fn process(&mut self, timing: CaptureTiming) -> CaptureTiming {
        self.chunks.extend(timing.chunks);
        CaptureTiming {
            chunks: std::array::from_fn(|_| self.chunks.pop_front().unwrap()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    #[default]
    VoiceActivity,
    PushToTalk,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoiseSuppressionMode {
    Off,
    #[default]
    Standard,
    RnNoise,
    DeepFilterNet,
    DeepFilterNetStrong,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoiseSuppressionStrength {
    #[default]
    Low,
    Medium,
    High,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoiseSuppressorCapabilities {
    pub supports_strength: bool,
    pub strength_levels: Vec<NoiseSuppressionStrength>,
}

impl NoiseSuppressorCapabilities {
    pub fn automatic() -> Self {
        Self {
            supports_strength: false,
            strength_levels: Vec::new(),
        }
    }
}

pub trait EchoCanceller: Send {
    fn process_capture(&mut self, frame: &mut AudioFrame);
    fn process_render_reference(&mut self, frame: &AudioFrame);
    fn set_enabled(&mut self, enabled: bool);
}

pub trait NoiseSuppressor: Send {
    fn process(&mut self, frame: &mut AudioFrame);
    fn reset(&mut self);
    fn latency_samples(&self) -> usize;
    fn mode(&self) -> NoiseSuppressionMode;
    fn capabilities(&self) -> NoiseSuppressorCapabilities;
}

pub trait AutomaticGainController: Send {
    fn process(&mut self, frame: &mut AudioFrame);
    fn set_enabled(&mut self, enabled: bool);
}

pub trait VoiceActivityDetector: Send {
    fn process(&mut self, frame: &AudioFrame) -> bool;
}

trait CaptureProcessingEngine: Send {
    fn speech_probability(&self) -> Option<f32> {
        None
    }
    fn set_echo_delay(&mut self, _delay_ms: i32) {}
    fn set_output_unused(&mut self, _unused: bool) {}
    fn process_capture(&mut self, frame: &mut AudioFrame) -> Result<(), String>;
    fn process_capture_timed(
        &mut self,
        frame: &mut AudioFrame,
        timing: CaptureTiming,
    ) -> Result<CaptureTiming, String> {
        self.process_capture(frame)?;
        Ok(timing)
    }
    fn process_render_reference(&mut self, frame: &AudioFrame) -> Result<(), String>;
    fn mode(&self) -> NoiseSuppressionMode;
    fn capabilities(&self) -> NoiseSuppressorCapabilities;
    fn processing_time(&mut self, synchronous_elapsed: Duration) -> Option<Duration> {
        Some(synchronous_elapsed)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LevelSnapshot {
    pub rms: f32,
    pub peak: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameAnalysis {
    pub level: LevelSnapshot,
    pub voice_active: bool,
    pub speech_probability: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DspMetrics {
    pub processed_frames: u64,
    pub total_processing_time: Duration,
    pub peak_processing_time: Duration,
    pub deadline_misses: u64,
}

impl DspMetrics {
    pub fn average_processing_time(self) -> Duration {
        if self.processed_frames == 0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(
                self.total_processing_time.as_secs_f64() / self.processed_frames as f64,
            )
        }
    }

    fn record(&mut self, elapsed: Duration) {
        self.processed_frames = self.processed_frames.saturating_add(1);
        self.total_processing_time = self.total_processing_time.saturating_add(elapsed);
        self.peak_processing_time = self.peak_processing_time.max(elapsed);
        if elapsed > Duration::from_millis(20) {
            self.deadline_misses = self.deadline_misses.saturating_add(1);
        }
    }
}

pub struct CaptureAudioProcessor {
    input_gain: f32,
    echo_cancellation: bool,
    automatic_gain: bool,
    engine: Box<dyn CaptureProcessingEngine>,
    voice_activity: Box<dyn VoiceActivityDetector>,
    metrics: DspMetrics,
    last_error: Option<String>,
}

impl CaptureAudioProcessor {
    pub fn passthrough(input_gain: f32) -> Self {
        Self {
            input_gain: sanitize_gain(input_gain),
            echo_cancellation: false,
            automatic_gain: false,
            engine: Box::new(PassthroughProcessingEngine),
            voice_activity: Box::new(EnergyVoiceActivityDetector::default()),
            metrics: DspMetrics::default(),
            last_error: None,
        }
    }

    pub fn standard(
        input_gain: f32,
        echo_cancellation: bool,
        automatic_gain: bool,
        strength: NoiseSuppressionStrength,
    ) -> Result<Self, String> {
        Ok(Self {
            input_gain: sanitize_gain(input_gain),
            echo_cancellation,
            automatic_gain,
            engine: Box::new(WebRtcProcessingEngine::new(
                echo_cancellation,
                automatic_gain,
                strength,
            )?),
            voice_activity: Box::new(EnergyVoiceActivityDetector::default()),
            metrics: DspMetrics::default(),
            last_error: None,
        })
    }

    pub fn off(input_gain: f32, echo_cancellation: bool, automatic_gain: bool) -> Self {
        if !echo_cancellation && !automatic_gain {
            return Self::passthrough(input_gain);
        }
        Self {
            input_gain: sanitize_gain(input_gain),
            echo_cancellation,
            automatic_gain,
            engine: Box::new(SonoraProcessingEngine::new(
                echo_cancellation,
                automatic_gain,
                None,
                NoiseSuppressionMode::Off,
                true,
            )),
            voice_activity: Box::new(EnergyVoiceActivityDetector::default()),
            metrics: DspMetrics::default(),
            last_error: None,
        }
    }

    pub fn rnnoise(input_gain: f32, echo_cancellation: bool, automatic_gain: bool) -> Self {
        Self {
            input_gain: sanitize_gain(input_gain),
            echo_cancellation,
            automatic_gain,
            engine: Box::new(RnNoiseProcessingEngine::new(
                echo_cancellation,
                automatic_gain,
            )),
            voice_activity: Box::new(EnergyVoiceActivityDetector::default()),
            metrics: DspMetrics::default(),
            last_error: None,
        }
    }

    pub fn deep_filter_net(
        input_gain: f32,
        echo_cancellation: bool,
        automatic_gain: bool,
        strong: bool,
    ) -> Result<Self, String> {
        let mode = if strong {
            NoiseSuppressionMode::DeepFilterNetStrong
        } else {
            NoiseSuppressionMode::DeepFilterNet
        };
        Ok(Self {
            input_gain: sanitize_gain(input_gain),
            echo_cancellation,
            automatic_gain,
            engine: Box::new(DeepFilterNetProcessingEngine::new(
                echo_cancellation,
                automatic_gain,
                mode,
            )?),
            voice_activity: Box::new(EnergyVoiceActivityDetector::default()),
            metrics: DspMetrics::default(),
            last_error: None,
        })
    }

    /// Advisory hint: engines without a safe low-power path keep processing.
    pub fn set_output_unused(&mut self, unused: bool) {
        self.engine.set_output_unused(unused);
    }

    #[cfg(test)]
    pub(crate) fn delayed_for_test(delay_samples: usize) -> Self {
        struct Delayed {
            samples: VecDeque<f32>,
            timing: CaptureTimingDelay,
        }
        impl CaptureProcessingEngine for Delayed {
            fn process_capture(&mut self, frame: &mut AudioFrame) -> Result<(), String> {
                self.samples.extend(frame.iter().copied());
                for sample in frame {
                    *sample = self.samples.pop_front().unwrap();
                }
                Ok(())
            }
            fn process_capture_timed(
                &mut self,
                frame: &mut AudioFrame,
                timing: CaptureTiming,
            ) -> Result<CaptureTiming, String> {
                self.process_capture(frame)?;
                Ok(self.timing.process(timing))
            }
            fn process_render_reference(&mut self, _: &AudioFrame) -> Result<(), String> {
                Ok(())
            }
            fn mode(&self) -> NoiseSuppressionMode {
                NoiseSuppressionMode::DeepFilterNet
            }
            fn capabilities(&self) -> NoiseSuppressorCapabilities {
                NoiseSuppressorCapabilities::automatic()
            }
        }
        let mut processor = Self::passthrough(1.0);
        processor.engine = Box::new(Delayed {
            samples: std::iter::repeat_n(0.0, delay_samples).collect(),
            timing: CaptureTimingDelay::new(delay_samples),
        });
        processor
    }

    #[cfg(test)]
    pub(crate) fn blocked_for_test(
        entered: crossbeam_channel::Sender<()>,
        release: crossbeam_channel::Receiver<()>,
    ) -> Self {
        struct Blocked {
            entered: crossbeam_channel::Sender<()>,
            release: crossbeam_channel::Receiver<()>,
        }
        impl CaptureProcessingEngine for Blocked {
            fn process_capture(&mut self, _frame: &mut AudioFrame) -> Result<(), String> {
                let _ = self.entered.try_send(());
                let _ = self.release.recv_timeout(Duration::from_secs(2));
                Ok(())
            }
            fn process_render_reference(&mut self, _: &AudioFrame) -> Result<(), String> {
                Ok(())
            }
            fn mode(&self) -> NoiseSuppressionMode {
                NoiseSuppressionMode::Off
            }
            fn capabilities(&self) -> NoiseSuppressorCapabilities {
                PassthroughProcessingEngine.capabilities()
            }
        }
        let mut processor = Self::passthrough(1.0);
        processor.engine = Box::new(Blocked { entered, release });
        processor
    }

    pub fn set_echo_delay(&mut self, delay: Duration) {
        self.engine
            .set_echo_delay(delay.as_millis().min(500) as i32);
    }

    pub fn process_capture(&mut self, frame: &mut AudioFrame) -> FrameAnalysis {
        self.process_capture_with_timing(frame, CaptureTiming::default())
            .0
    }

    pub(crate) fn process_capture_with_timing(
        &mut self,
        frame: &mut AudioFrame,
        timing: CaptureTiming,
    ) -> (FrameAnalysis, CaptureTiming) {
        let started = Instant::now();
        apply_gain(frame, self.input_gain);
        let output_timing = match self.engine.process_capture_timed(frame, timing) {
            Ok(timing) => timing,
            Err(error) => {
                self.last_error = Some(error);
                CaptureTiming::default()
            }
        };
        let level = analyze_level(frame);
        let voice_active = self.voice_activity.process(frame);
        if let Some(elapsed) = self.engine.processing_time(started.elapsed()) {
            self.metrics.record(elapsed);
        }
        (
            FrameAnalysis {
                level,
                voice_active,
                speech_probability: self.engine.speech_probability(),
            },
            output_timing,
        )
    }

    pub fn process_render_reference(&mut self, frame: &AudioFrame) {
        if let Err(error) = self.engine.process_render_reference(frame) {
            self.last_error = Some(error);
        }
    }

    pub fn metrics(&self) -> DspMetrics {
        self.metrics
    }

    pub fn noise_suppression_mode(&self) -> NoiseSuppressionMode {
        self.engine.mode()
    }

    pub fn echo_cancellation_enabled(&self) -> bool {
        self.echo_cancellation
    }

    pub fn automatic_gain_enabled(&self) -> bool {
        self.automatic_gain
    }

    pub fn noise_suppression_capabilities(&self) -> NoiseSuppressorCapabilities {
        self.engine.capabilities()
    }

    pub fn take_error(&mut self) -> Option<String> {
        self.last_error.take()
    }
}

pub fn sanitize_gain(gain: f32) -> f32 {
    if gain.is_finite() {
        gain.clamp(0.0, 3.0)
    } else {
        1.0
    }
}

pub fn apply_gain(frame: &mut AudioFrame, gain: f32) {
    let gain = sanitize_gain(gain);
    for sample in frame {
        let finite = if sample.is_finite() { *sample } else { 0.0 };
        *sample = (finite * gain).clamp(-1.0, 1.0);
    }
}

pub fn analyze_level(frame: &AudioFrame) -> LevelSnapshot {
    let mut sum_squares = 0.0_f32;
    let mut peak = 0.0_f32;
    for sample in frame {
        let sample = if sample.is_finite() { *sample } else { 0.0 };
        sum_squares += sample * sample;
        peak = peak.max(sample.abs());
    }
    LevelSnapshot {
        rms: (sum_squares / FRAME_SAMPLES as f32).sqrt().clamp(0.0, 1.0),
        peak: peak.clamp(0.0, 1.0),
    }
}

struct PassthroughProcessingEngine;

impl CaptureProcessingEngine for PassthroughProcessingEngine {
    fn process_capture(&mut self, _frame: &mut AudioFrame) -> Result<(), String> {
        Ok(())
    }

    fn process_render_reference(&mut self, _frame: &AudioFrame) -> Result<(), String> {
        Ok(())
    }

    fn mode(&self) -> NoiseSuppressionMode {
        NoiseSuppressionMode::Off
    }

    fn capabilities(&self) -> NoiseSuppressorCapabilities {
        NoiseSuppressorCapabilities::automatic()
    }
}

struct WebRtcProcessingEngine {
    output_unused: bool,
    echo_delay_ms: i32,
    processor: *mut webrtc_apm::AudioProcessing,
    stream: webrtc_apm::StreamConfig,
}

// The WebRTC object is owned by this engine and is only touched by the audio
// worker thread. The raw pointer is never shared across threads.
unsafe impl Send for WebRtcProcessingEngine {}

impl WebRtcProcessingEngine {
    fn new(
        echo_cancellation: bool,
        automatic_gain: bool,
        strength: NoiseSuppressionStrength,
    ) -> Result<Self, String> {
        let mut error = 0;
        let processor = unsafe { webrtc_apm::create_audio_processing(null_mut(), &mut error) };
        if processor.is_null() || error != 0 {
            return Err(format!(
                "WebRTC AudioProcessing initialization failed: {error}"
            ));
        }

        let config = web_rtc_config(echo_cancellation, automatic_gain, strength);

        unsafe {
            webrtc_apm::set_config(processor, &config);
            webrtc_apm::set_stream_delay_ms(processor, ECHO_PATH_DELAY_MS);
        }
        Ok(Self {
            processor,
            stream: webrtc_apm::StreamConfig::new(48_000, 1),
            output_unused: false,
            echo_delay_ms: ECHO_PATH_DELAY_MS,
        })
    }

    fn process_capture_chunk(&mut self, chunk: &mut [f32]) -> Result<(), String> {
        let channels = [chunk.as_mut_ptr()];
        let code = unsafe {
            webrtc_apm::set_stream_delay_ms(self.processor, self.echo_delay_ms);
            webrtc_apm::process_capture_frame(self.processor, &self.stream, channels.as_ptr())
        };
        (code == 0)
            .then_some(())
            .ok_or_else(|| format!("WebRTC capture processing failed: {code}"))
    }

    fn process_render_chunk(&mut self, chunk: &[f32]) -> Result<(), String> {
        let channels = [chunk.as_ptr()];
        let code = unsafe {
            webrtc_apm::analyze_render_frame(self.processor, &self.stream, channels.as_ptr())
        };
        (code == 0)
            .then_some(())
            .ok_or_else(|| format!("WebRTC render processing failed: {code}"))
    }
}

fn web_rtc_config(
    echo_cancellation: bool,
    automatic_gain: bool,
    strength: NoiseSuppressionStrength,
) -> webrtc_apm::AudioProcessing_Config {
    let mut config = webrtc_apm::AudioProcessing_Config::default();
    config.pipeline.maximum_internal_processing_rate = 48_000;
    config.high_pass_filter.enabled = true;
    config.high_pass_filter.apply_in_full_band = true;
    config.echo_canceller.enabled = echo_cancellation;
    config.echo_canceller.mobile_mode = false;
    config.echo_canceller.enforce_high_pass_filtering = true;
    config.noise_suppression.enabled = true;
    config.noise_suppression.level = match strength {
        NoiseSuppressionStrength::Low => {
            webrtc_apm::AudioProcessing_Config_NoiseSuppression_Level_kLow
        }
        NoiseSuppressionStrength::Medium => {
            webrtc_apm::AudioProcessing_Config_NoiseSuppression_Level_kModerate
        }
        NoiseSuppressionStrength::High => {
            webrtc_apm::AudioProcessing_Config_NoiseSuppression_Level_kHigh
        }
    };
    config.transient_suppression.enabled = true;
    config.gain_controller2.enabled = automatic_gain;
    config.gain_controller2.input_volume_controller.enabled = false;
    config.gain_controller2.adaptive_digital.enabled = automatic_gain;
    config.gain_controller2.adaptive_digital.headroom_db = 10.0;
    config.gain_controller2.adaptive_digital.max_gain_db = 12.0;
    config.gain_controller2.adaptive_digital.initial_gain_db = 0.0;
    config
        .gain_controller2
        .adaptive_digital
        .max_gain_change_db_per_second = 0.5;
    config
        .gain_controller2
        .adaptive_digital
        .max_output_noise_level_dbfs = -55.0;
    config
}

impl Drop for WebRtcProcessingEngine {
    fn drop(&mut self) {
        unsafe { webrtc_apm::delete_audio_processing(self.processor) };
    }
}

impl CaptureProcessingEngine for WebRtcProcessingEngine {
    fn set_echo_delay(&mut self, delay_ms: i32) {
        self.echo_delay_ms = delay_ms;
    }
    fn set_output_unused(&mut self, unused: bool) {
        if self.output_unused != unused {
            unsafe {
                webrtc_apm::set_output_will_be_muted(self.processor, unused);
            }
            self.output_unused = unused;
        }
    }

    fn process_capture(&mut self, frame: &mut AudioFrame) -> Result<(), String> {
        for chunk in frame.chunks_exact_mut(480) {
            self.process_capture_chunk(chunk)?;
        }
        Ok(())
    }

    fn process_render_reference(&mut self, frame: &AudioFrame) -> Result<(), String> {
        for chunk in frame.chunks_exact(480) {
            self.process_render_chunk(chunk)?;
        }
        Ok(())
    }

    fn mode(&self) -> NoiseSuppressionMode {
        NoiseSuppressionMode::Standard
    }

    fn capabilities(&self) -> NoiseSuppressorCapabilities {
        NoiseSuppressorCapabilities {
            supports_strength: true,
            strength_levels: vec![
                NoiseSuppressionStrength::Low,
                NoiseSuppressionStrength::Medium,
                NoiseSuppressionStrength::High,
            ],
        }
    }
}

struct SonoraProcessingEngine {
    processor: AudioProcessing,
    input: [f32; 480],
    output: [f32; 480],
    mode: NoiseSuppressionMode,
}

// Fallback for standalone DSP users. Live audio supplies CPAL-derived timing
// for each capture instead of keeping this fixed estimate.
const ECHO_PATH_DELAY_MS: i32 = 60;

impl SonoraProcessingEngine {
    fn new(
        echo_cancellation: bool,
        automatic_gain: bool,
        strength: Option<NoiseSuppressionStrength>,
        mode: NoiseSuppressionMode,
        high_pass_filter: bool,
    ) -> Self {
        let pipeline = sonora::config::Pipeline {
            maximum_internal_processing_rate: MaxProcessingRate::Rate48kHz,
            ..Default::default()
        };
        let config = SonoraConfig {
            pipeline,
            high_pass_filter: high_pass_filter.then(HighPassFilter::default),
            echo_canceller: echo_cancellation.then(SonoraEchoCanceller::default),
            noise_suppression: strength.map(|strength| NoiseSuppression {
                level: match strength {
                    NoiseSuppressionStrength::Low => NoiseSuppressionLevel::Low,
                    NoiseSuppressionStrength::Medium => NoiseSuppressionLevel::Moderate,
                    NoiseSuppressionStrength::High => NoiseSuppressionLevel::High,
                },
                ..Default::default()
            }),
            gain_controller2: automatic_gain.then(|| GainController2 {
                adaptive_digital: Some(AdaptiveDigital {
                    // WebRTC's generic defaults start at +15 dB and can chase
                    // speech up to +50 dB at 6 dB/s. For headset voice chat that
                    // is audibly pumpy, especially around denoiser/VAD decisions.
                    headroom_db: 10.0,
                    max_gain_db: 12.0,
                    initial_gain_db: 0.0,
                    max_gain_change_db_per_second: 0.5,
                    max_output_noise_level_dbfs: -55.0,
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let stream = StreamConfig::new(48_000, 1);
        let mut processor = AudioProcessing::builder()
            .config(config)
            .capture_config(stream)
            .render_config(stream)
            .build();
        let _ = processor.set_stream_delay_ms(ECHO_PATH_DELAY_MS);
        Self {
            processor,
            input: [0.0; 480],
            output: [0.0; 480],
            mode,
        }
    }

    fn process_chunks(&mut self, frame: &mut AudioFrame, capture: bool) -> Result<(), String> {
        for chunk in frame.chunks_exact_mut(480) {
            self.input.copy_from_slice(chunk);
            let result = if capture {
                self.processor
                    .process_capture_f32(&[&self.input], &mut [&mut self.output])
            } else {
                self.processor
                    .process_render_f32(&[&self.input], &mut [&mut self.output])
            };
            result.map_err(|error| format!("Sonora audio processing failed: {error}"))?;
            if capture {
                chunk.copy_from_slice(&self.output);
            }
        }
        Ok(())
    }
}

impl CaptureProcessingEngine for SonoraProcessingEngine {
    fn set_echo_delay(&mut self, delay_ms: i32) {
        let _ = self.processor.set_stream_delay_ms(delay_ms);
    }
    fn process_capture(&mut self, frame: &mut AudioFrame) -> Result<(), String> {
        self.process_chunks(frame, true)
    }

    fn process_render_reference(&mut self, frame: &AudioFrame) -> Result<(), String> {
        let mut copy = *frame;
        self.process_chunks(&mut copy, false)
    }

    fn mode(&self) -> NoiseSuppressionMode {
        self.mode
    }

    fn capabilities(&self) -> NoiseSuppressorCapabilities {
        if self.mode == NoiseSuppressionMode::Standard {
            NoiseSuppressorCapabilities {
                supports_strength: true,
                strength_levels: vec![
                    NoiseSuppressionStrength::Low,
                    NoiseSuppressionStrength::Medium,
                    NoiseSuppressionStrength::High,
                ],
            }
        } else {
            NoiseSuppressorCapabilities::automatic()
        }
    }
}

struct RnNoiseProcessingEngine {
    speech_probability: f32,
    echo_frontend: Option<SonoraProcessingEngine>,
    gain_backend: Option<SonoraProcessingEngine>,
    denoiser: Denoiser,
    input: [f32; RNNOISE_FRAME_SIZE],
    output: [f32; RNNOISE_FRAME_SIZE],
    #[cfg(test)]
    processed_chunks: u64,
}

impl RnNoiseProcessingEngine {
    fn new(echo_cancellation: bool, automatic_gain: bool) -> Self {
        let echo_frontend = echo_cancellation.then(|| {
            SonoraProcessingEngine::new(true, false, None, NoiseSuppressionMode::Off, true)
        });
        let gain_backend = automatic_gain.then(|| {
            SonoraProcessingEngine::new(false, true, None, NoiseSuppressionMode::Off, false)
        });
        Self {
            echo_frontend,
            gain_backend,
            speech_probability: 0.0,
            denoiser: Denoiser::new(None).expect("bundled RNNoise model should initialize"),
            input: [0.0; RNNOISE_FRAME_SIZE],
            output: [0.0; RNNOISE_FRAME_SIZE],
            #[cfg(test)]
            processed_chunks: 0,
        }
    }

    fn denoise_chunks(&mut self, frame: &mut AudioFrame) {
        debug_assert_eq!(Denoiser::frame_size(), RNNOISE_FRAME_SIZE);
        self.speech_probability = 0.0;
        for chunk in frame.chunks_exact_mut(RNNOISE_FRAME_SIZE) {
            for (input, sample) in self.input.iter_mut().zip(chunk.iter().copied()) {
                *input = (sample * 32_768.0).clamp(-32_768.0, 32_767.0);
            }
            self.speech_probability = self
                .speech_probability
                .max(self.denoiser.process(&self.input, &mut self.output));
            #[cfg(test)]
            {
                self.processed_chunks = self.processed_chunks.saturating_add(1);
            }
            for (sample, denoised) in chunk.iter_mut().zip(self.output.iter().copied()) {
                *sample = (denoised / 32_768.0).clamp(-1.0, 1.0);
            }
        }
    }
}

impl CaptureProcessingEngine for RnNoiseProcessingEngine {
    fn speech_probability(&self) -> Option<f32> {
        Some(self.speech_probability)
    }
    fn set_echo_delay(&mut self, delay_ms: i32) {
        if let Some(frontend) = &mut self.echo_frontend {
            frontend.set_echo_delay(delay_ms);
        }
    }
    fn process_capture(&mut self, frame: &mut AudioFrame) -> Result<(), String> {
        if let Some(frontend) = &mut self.echo_frontend {
            frontend.process_capture(frame)?;
        }
        self.denoise_chunks(frame);
        if let Some(backend) = &mut self.gain_backend {
            backend.process_capture(frame)?;
        }
        Ok(())
    }

    fn process_render_reference(&mut self, frame: &AudioFrame) -> Result<(), String> {
        if let Some(frontend) = &mut self.echo_frontend {
            frontend.process_render_reference(frame)?;
        }
        Ok(())
    }

    fn mode(&self) -> NoiseSuppressionMode {
        NoiseSuppressionMode::RnNoise
    }

    fn capabilities(&self) -> NoiseSuppressorCapabilities {
        NoiseSuppressorCapabilities::automatic()
    }
}

const DEEP_FILTER_DELAY_FRAMES: u64 = 2;
/// DeepFilterNet3 default model: (fft_size - hop_size) + lookahead * hop_size.
pub const DEEP_FILTER_MODEL_DELAY_SAMPLES: usize = 1_440;
pub const DEEP_FILTER_TOTAL_DELAY_SAMPLES: usize =
    DEEP_FILTER_DELAY_FRAMES as usize * FRAME_SAMPLES + DEEP_FILTER_MODEL_DELAY_SAMPLES;

#[derive(Clone, Copy)]
struct DeepFilterRequest {
    sequence: u64,
    frame: AudioFrame,
    timing: CaptureTiming,
}

struct DeepFilterResponse {
    sequence: u64,
    result: Result<(AudioFrame, Duration), String>,
    timing: CaptureTiming,
}

struct DeepFilterNetProcessingEngine {
    echo_frontend: Option<SonoraProcessingEngine>,
    gain_backend: Option<SonoraProcessingEngine>,
    requests: Option<Sender<DeepFilterRequest>>,
    responses: Option<Receiver<DeepFilterResponse>>,
    worker: Option<thread::JoinHandle<()>>,
    next_sequence: u64,
    pending_response: Option<DeepFilterResponse>,
    last_processing_time: Option<Duration>,
    mode: NoiseSuppressionMode,
}

impl DeepFilterNetProcessingEngine {
    fn new(
        echo_cancellation: bool,
        automatic_gain: bool,
        mode: NoiseSuppressionMode,
    ) -> Result<Self, String> {
        let (requests_tx, requests_rx) = bounded(4);
        let (responses_tx, responses_rx) = bounded(4);
        let (ready_tx, ready_rx) = bounded(1);
        let worker_mode = mode;
        let worker = thread::Builder::new()
            .name("deep-filter-net".into())
            .spawn(move || deep_filter_worker(requests_rx, responses_tx, ready_tx, worker_mode))
            .map_err(|error| format!("DeepFilterNet worker creation failed: {error}"))?;
        let initialized = match ready_rx.recv() {
            Ok(result) => result,
            Err(_) => {
                let _ = worker.join();
                return Err("DeepFilterNet worker stopped during initialization".to_owned());
            }
        };
        if let Err(error) = initialized {
            let _ = worker.join();
            return Err(error);
        }
        let echo_frontend = echo_cancellation.then(|| {
            SonoraProcessingEngine::new(true, false, None, NoiseSuppressionMode::Off, true)
        });
        let gain_backend = automatic_gain.then(|| {
            SonoraProcessingEngine::new(false, true, None, NoiseSuppressionMode::Off, false)
        });
        Ok(Self {
            echo_frontend,
            gain_backend,
            requests: Some(requests_tx),
            responses: Some(responses_rx),
            worker: Some(worker),
            next_sequence: 0,
            pending_response: None,
            last_processing_time: None,
            mode,
        })
    }

    #[cfg(test)]
    fn enhance_aligned(&mut self, frame: &mut AudioFrame) -> Result<(), String> {
        self.enhance_aligned_timed(frame, CaptureTiming::default())
            .map(|_| ())
    }

    fn enhance_aligned_timed(
        &mut self,
        frame: &mut AudioFrame,
        timing: CaptureTiming,
    ) -> Result<CaptureTiming, String> {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        let requests = self.requests.as_ref().ok_or_else(|| {
            "DeepFilterNet worker is shutting down; silence substituted".to_owned()
        })?;
        match requests.try_send(DeepFilterRequest {
            sequence,
            frame: *frame,
            timing,
        }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                frame.fill(0.0);
                return Err(
                    "DeepFilterNet worker backlog exceeded four frames; silence substituted".into(),
                );
            }
            Err(TrySendError::Disconnected(_)) => {
                frame.fill(0.0);
                return Err("DeepFilterNet worker is unavailable; silence substituted".into());
            }
        }

        // Prime a fixed two-frame (40 ms) pipeline. Never leak unprocessed dry
        // audio while the neural worker is starting or recovering.
        if sequence < DEEP_FILTER_DELAY_FRAMES {
            frame.fill(0.0);
            return Ok(CaptureTiming::default());
        }

        let expected = sequence.wrapping_sub(DEEP_FILTER_DELAY_FRAMES);
        let responses = self.responses.as_ref().ok_or_else(|| {
            "DeepFilterNet worker is shutting down; silence substituted".to_owned()
        })?;
        // The fixed 40 ms pipeline is the worker's processing budget. Waiting
        // here extends it and blocks remote playout on the same audio thread.
        // A late result must follow the existing silence/subsequent-frame
        // recovery path, never borrow another full 20 ms mix period.
        loop {
            let response = match self.pending_response.take() {
                Some(response) => response,
                None => match responses.try_recv() {
                    Ok(response) => response,
                    Err(_) => {
                        frame.fill(0.0);
                        return Err(format!(
                            "DeepFilterNet frame {expected} missed its fixed-delay deadline; silence substituted"
                        ));
                    }
                },
            };
            if response.sequence < expected {
                continue;
            }
            if response.sequence != expected {
                let actual = response.sequence;
                self.pending_response = Some(response);
                frame.fill(0.0);
                return Err(format!(
                    "DeepFilterNet frame {expected} is missing before frame {actual}; silence substituted"
                ));
            }
            let (processed, processing_time) = match response.result {
                Ok(processed) => processed,
                Err(error) => {
                    frame.fill(0.0);
                    return Err(error);
                }
            };
            self.last_processing_time = Some(processing_time);
            *frame = processed;
            return Ok(response.timing);
        }
    }
}

impl Drop for DeepFilterNetProcessingEngine {
    fn drop(&mut self) {
        self.requests.take();
        self.responses.take();
        // Switching processors happens on the audio worker. Detach instead of
        // joining an in-flight inference there: the disconnected response
        // channel makes the model-owning thread exit after its current frame
        // (or the next queued frame if it was idle). No model cache is retained.
        self.worker.take();
    }
}

impl CaptureProcessingEngine for DeepFilterNetProcessingEngine {
    fn set_echo_delay(&mut self, delay_ms: i32) {
        if let Some(frontend) = &mut self.echo_frontend {
            frontend.set_echo_delay(delay_ms);
        }
    }
    fn process_capture(&mut self, frame: &mut AudioFrame) -> Result<(), String> {
        self.process_capture_timed(frame, CaptureTiming::default())
            .map(|_| ())
    }

    fn process_capture_timed(
        &mut self,
        frame: &mut AudioFrame,
        timing: CaptureTiming,
    ) -> Result<CaptureTiming, String> {
        if let Some(frontend) = &mut self.echo_frontend {
            frontend.process_capture(frame)?;
        }
        let output_timing = self.enhance_aligned_timed(frame, timing)?;
        if let Some(backend) = &mut self.gain_backend {
            backend.process_capture(frame)?;
        }
        Ok(output_timing)
    }

    fn process_render_reference(&mut self, frame: &AudioFrame) -> Result<(), String> {
        if let Some(frontend) = &mut self.echo_frontend {
            frontend.process_render_reference(frame)?;
        }
        Ok(())
    }

    fn mode(&self) -> NoiseSuppressionMode {
        self.mode
    }

    fn capabilities(&self) -> NoiseSuppressorCapabilities {
        NoiseSuppressorCapabilities::automatic()
    }

    fn processing_time(&mut self, _synchronous_elapsed: Duration) -> Option<Duration> {
        self.last_processing_time.take()
    }
}

fn deep_filter_worker(
    requests: Receiver<DeepFilterRequest>,
    responses: Sender<DeepFilterResponse>,
    ready: Sender<Result<(), String>>,
    mode: NoiseSuppressionMode,
) {
    let initialized = (|| {
        let params = std::panic::catch_unwind(DfParams::default)
            .map_err(|_| "DeepFilterNet model package could not be decoded".to_owned())?;
        let runtime = deep_filter_runtime_params(mode);
        let model = DfTract::new(params, &runtime)
            .map_err(|error| format!("DeepFilterNet initialization failed: {error:#}"))?;
        if model.sr != 48_000 || model.hop_size != 480 || model.ch != 1 {
            return Err(format!(
                "DeepFilterNet model format is incompatible: {} Hz, {} samples, {} channels",
                model.sr, model.hop_size, model.ch
            ));
        }
        Ok(model)
    })();
    let mut model = match initialized {
        Ok(model) => {
            let _ = ready.send(Ok(()));
            model
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };

    // Advance provenance only for input actually accepted by the model. A full
    // request queue can skip sequence numbers; a FIFO outside this worker would
    // then bind surviving audio to the dropped frame's PTT state.
    let mut timing_delay = CaptureTimingDelay::new(DEEP_FILTER_MODEL_DELAY_SAMPLES);
    while let Ok(request) = requests.recv() {
        let mut output = [0.0; FRAME_SAMPLES];
        let started = Instant::now();
        let result = process_deep_filter_frame(&mut model, &request.frame, &mut output)
            .map(|()| (output, started.elapsed()));
        let failed = result.is_err();
        let timing = timing_delay.process(request.timing);
        if responses
            .send(DeepFilterResponse {
                sequence: request.sequence,
                result,
                timing,
            })
            .is_err()
            || failed
        {
            return;
        }
    }
}

fn deep_filter_runtime_params(mode: NoiseSuppressionMode) -> RuntimeParams {
    match mode {
        NoiseSuppressionMode::DeepFilterNetStrong => RuntimeParams::default()
            .with_atten_lim(100.0)
            .with_thresholds(-5.0, 40.0, 30.0)
            .with_post_filter(0.02),
        _ => RuntimeParams::default()
            .with_atten_lim(18.0)
            .with_thresholds(-15.0, 35.0, 25.0),
    }
}

fn process_deep_filter_frame(
    model: &mut DfTract,
    input_frame: &AudioFrame,
    output_frame: &mut AudioFrame,
) -> Result<(), String> {
    for (input, output) in input_frame
        .chunks_exact(480)
        .zip(output_frame.chunks_exact_mut(480))
    {
        let input = ArrayView2::from_shape((1, 480), input)
            .map_err(|error| format!("DeepFilterNet input shape failed: {error}"))?;
        let output = ArrayViewMut2::from_shape((1, 480), output)
            .map_err(|error| format!("DeepFilterNet output shape failed: {error}"))?;
        model
            .process(input, output)
            .map_err(|error| format!("DeepFilterNet processing failed: {error:#}"))?;
    }
    Ok(())
}

pub struct EnergyVoiceActivityDetector {
    threshold: f32,
    sustain_threshold: f32,
    attack_frames: u8,
    release_frames: u8,
    above_threshold: u8,
    release_remaining: u8,
    active: bool,
}

impl EnergyVoiceActivityDetector {
    pub fn new(threshold: f32, attack_frames: u8, release_frames: u8) -> Self {
        Self::with_thresholds(threshold, threshold, attack_frames, release_frames)
    }

    pub fn with_thresholds(
        threshold: f32,
        sustain_threshold: f32,
        attack_frames: u8,
        release_frames: u8,
    ) -> Self {
        let threshold = threshold.clamp(0.001, 0.5);
        Self {
            threshold,
            sustain_threshold: sustain_threshold.clamp(0.001, threshold),
            attack_frames: attack_frames.max(1),
            release_frames,
            above_threshold: 0,
            release_remaining: 0,
            active: false,
        }
    }
}

impl Default for EnergyVoiceActivityDetector {
    fn default() -> Self {
        // Keep low-energy consonants and sentence tails. The previous -38 dBFS
        // threshold interacted with AGC/denoising and clipped quiet syllables.
        // Open at about -48 dBFS; once speaking, sustain down to -54 dBFS.
        // Weak syllables refresh the existing 400 ms tail protection without
        // lowering the threshold that starts a new utterance.
        Self::with_thresholds(0.004, 0.002, 1, 20)
    }
}

impl VoiceActivityDetector for EnergyVoiceActivityDetector {
    fn process(&mut self, frame: &AudioFrame) -> bool {
        let threshold = if self.active {
            self.sustain_threshold
        } else {
            self.threshold
        };
        if analyze_level(frame).rms >= threshold {
            self.above_threshold = self.above_threshold.saturating_add(1);
            self.release_remaining = self.release_frames;
            if self.above_threshold >= self.attack_frames {
                self.active = true;
            }
        } else {
            self.above_threshold = 0;
            if self.active && self.release_remaining > 0 {
                self.release_remaining -= 1;
            } else {
                self.active = false;
            }
        }
        self.active
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn echo_delay_is_forwarded_and_clamped() {
        struct DelayProbe(std::sync::Arc<std::sync::atomic::AtomicI32>);
        impl super::CaptureProcessingEngine for DelayProbe {
            fn set_echo_delay(&mut self, delay_ms: i32) {
                self.0.store(delay_ms, std::sync::atomic::Ordering::Relaxed);
            }
            fn process_capture(&mut self, _: &mut super::AudioFrame) -> Result<(), String> {
                Ok(())
            }
            fn process_render_reference(&mut self, _: &super::AudioFrame) -> Result<(), String> {
                Ok(())
            }
            fn mode(&self) -> super::NoiseSuppressionMode {
                super::NoiseSuppressionMode::Off
            }
            fn capabilities(&self) -> super::NoiseSuppressorCapabilities {
                super::PassthroughProcessingEngine.capabilities()
            }
        }
        let delay = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(-1));
        let mut processor = super::CaptureAudioProcessor::passthrough(1.0);
        processor.engine = Box::new(DelayProbe(delay.clone()));
        for (requested, expected) in [(0, 0), (135, 135), (800, 500)] {
            processor.set_echo_delay(std::time::Duration::from_millis(requested));
            assert_eq!(delay.load(std::sync::atomic::Ordering::Relaxed), expected);
        }
    }

    use super::*;
    use std::f32::consts::TAU;

    #[test]
    fn gain_is_bounded_and_never_produces_non_finite_samples() {
        let mut frame = [0.75; FRAME_SAMPLES];
        frame[0] = f32::NAN;
        apply_gain(&mut frame, 2.0);
        assert!(frame.iter().skip(1).all(|sample| *sample == 1.0));
        assert_eq!(frame[0], 0.0);
        assert_eq!(sanitize_gain(f32::INFINITY), 1.0);
    }

    #[test]
    fn level_reports_rms_and_peak() {
        let mut frame = [0.25; FRAME_SAMPLES];
        frame[0] = 0.75;
        let level = analyze_level(&frame);
        assert!(level.rms >= 0.25);
        assert_eq!(level.peak, 0.75);
    }

    #[test]
    fn vad_uses_attack_and_release_hangover() {
        let loud = [0.1; FRAME_SAMPLES];
        let quiet = [0.0; FRAME_SAMPLES];
        let mut vad = EnergyVoiceActivityDetector::new(0.02, 2, 2);
        assert!(!vad.process(&loud));
        assert!(vad.process(&loud));
        assert!(vad.process(&quiet));
        assert!(vad.process(&quiet));
        assert!(!vad.process(&quiet));
    }

    #[test]
    fn default_vad_keeps_quiet_speech_and_sentence_tails() {
        let mut vad = EnergyVoiceActivityDetector::default();
        assert!(vad.process(&[0.005; FRAME_SAMPLES]));
        for _ in 0..20 {
            assert!(vad.process(&[0.0; FRAME_SAMPLES]));
        }
        assert!(!vad.process(&[0.0; FRAME_SAMPLES]));
    }

    #[test]
    fn voice_chat_agc_does_not_apply_a_large_startup_boost() {
        let mut processor = CaptureAudioProcessor::off(1.0, false, true);
        let mut frame =
            std::array::from_fn(|sample| (sample as f32 * 440.0 * TAU / 48_000.0).sin() * 0.02);
        let input_rms = analyze_level(&frame).rms;
        processor.process_capture(&mut frame);
        let output_rms = analyze_level(&frame).rms;
        assert!(
            output_rms <= input_rms * 1.25,
            "{input_rms} -> {output_rms}"
        );
        assert!(
            output_rms >= input_rms * 0.75,
            "{input_rms} -> {output_rms}"
        );
    }

    #[test]
    fn passthrough_pipeline_preserves_frame_at_unity_gain() {
        let mut processor = CaptureAudioProcessor::passthrough(1.0);
        let mut frame = [0.125; FRAME_SAMPLES];
        let analysis = processor.process_capture(&mut frame);
        assert_eq!(frame, [0.125; FRAME_SAMPLES]);
        assert_eq!(analysis.level.rms, 0.125);
        assert_eq!(
            processor.noise_suppression_mode(),
            NoiseSuppressionMode::Off
        );
        assert_eq!(processor.metrics().processed_frames, 1);
    }

    #[test]
    fn suppression_modes_serialize_stably() {
        let json = serde_json::to_string(&NoiseSuppressionMode::DeepFilterNet).unwrap();
        assert_eq!(json, "\"deep_filter_net\"");
        assert_eq!(
            serde_json::from_str::<NoiseSuppressionMode>(&json).unwrap(),
            NoiseSuppressionMode::DeepFilterNet
        );
        let strong = serde_json::to_string(&NoiseSuppressionMode::DeepFilterNetStrong).unwrap();
        assert_eq!(strong, "\"deep_filter_net_strong\"");
        assert_eq!(
            serde_json::from_str::<NoiseSuppressionMode>(&strong).unwrap(),
            NoiseSuppressionMode::DeepFilterNetStrong
        );
    }

    #[test]
    fn deep_filter_pipeline_outputs_only_the_matching_delayed_frame() {
        let (requests_tx, requests_rx) = bounded(4);
        let (responses_tx, responses_rx) = bounded(4);
        let mut engine = DeepFilterNetProcessingEngine {
            echo_frontend: None,
            gain_backend: None,
            requests: Some(requests_tx),
            responses: Some(responses_rx),
            worker: None,
            next_sequence: 0,
            pending_response: None,
            last_processing_time: None,
            mode: NoiseSuppressionMode::DeepFilterNet,
        };

        let mut first = [0.1; FRAME_SAMPLES];
        engine.enhance_aligned(&mut first).unwrap();
        assert_eq!(first, [0.0; FRAME_SAMPLES]);
        assert_eq!(requests_rx.recv().unwrap().sequence, 0);

        let mut second = [0.2; FRAME_SAMPLES];
        engine.enhance_aligned(&mut second).unwrap();
        assert_eq!(second, [0.0; FRAME_SAMPLES]);
        assert_eq!(requests_rx.recv().unwrap().sequence, 1);

        responses_tx
            .send(DeepFilterResponse {
                sequence: 0,
                result: Ok(([0.75; FRAME_SAMPLES], Duration::from_millis(3))),
                timing: CaptureTiming::default(),
            })
            .unwrap();
        let mut third = [0.3; FRAME_SAMPLES];
        engine.enhance_aligned(&mut third).unwrap();
        assert_eq!(third, [0.75; FRAME_SAMPLES]);
        assert_ne!(third, [0.3; FRAME_SAMPLES]);
        assert_eq!(requests_rx.recv().unwrap().sequence, 2);

        responses_tx
            .send(DeepFilterResponse {
                sequence: 1,
                result: Ok(([0.5; FRAME_SAMPLES], Duration::from_millis(4))),
                timing: CaptureTiming::default(),
            })
            .unwrap();
        let mut fourth = [0.4; FRAME_SAMPLES];
        engine.enhance_aligned(&mut fourth).unwrap();
        assert_eq!(fourth, [0.5; FRAME_SAMPLES]);
        assert_eq!(requests_rx.recv().unwrap().sequence, 3);
    }

    #[test]
    fn deep_filter_drop_does_not_wait_for_in_flight_inference() {
        let (requests_tx, requests_rx) = bounded::<DeepFilterRequest>(4);
        let (responses_tx, responses_rx) = bounded::<DeepFilterResponse>(4);
        let (release_tx, release_rx) = bounded::<()>(1);
        let (exited_tx, exited_rx) = bounded(1);
        let worker = thread::spawn(move || {
            let _ = release_rx.recv();
            assert!(requests_rx.recv().is_err());
            assert!(
                responses_tx
                    .send(DeepFilterResponse {
                        sequence: 0,
                        result: Ok(([0.0; FRAME_SAMPLES], Duration::ZERO)),
                        timing: CaptureTiming::default(),
                    })
                    .is_err()
            );
            let _ = exited_tx.send(());
        });
        let engine = DeepFilterNetProcessingEngine {
            echo_frontend: None,
            gain_backend: None,
            requests: Some(requests_tx),
            responses: Some(responses_rx),
            worker: Some(worker),
            next_sequence: 0,
            pending_response: None,
            last_processing_time: None,
            mode: NoiseSuppressionMode::DeepFilterNet,
        };
        let (dropped_tx, dropped_rx) = bounded(1);
        let audio = thread::spawn(move || {
            drop(engine);
            let _ = dropped_tx.send(());
        });
        let dropped = dropped_rx.recv_timeout(Duration::from_millis(200));
        drop(release_tx);
        audio.join().unwrap();
        exited_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        dropped.expect("switching denoisers blocked the audio worker on inference shutdown");
    }

    #[test]
    fn deep_filter_pipeline_substitutes_silence_instead_of_dry_audio_on_a_miss() {
        let (requests_tx, requests_rx) = bounded(4);
        let (responses_tx, responses_rx) = bounded(4);
        let mut engine = DeepFilterNetProcessingEngine {
            echo_frontend: None,
            gain_backend: None,
            requests: Some(requests_tx),
            responses: Some(responses_rx),
            worker: None,
            next_sequence: DEEP_FILTER_DELAY_FRAMES,
            pending_response: None,
            last_processing_time: None,
            mode: NoiseSuppressionMode::DeepFilterNetStrong,
        };
        let started = Instant::now();
        for _ in 0..8 {
            let mut frame = [0.9; FRAME_SAMPLES];
            let error = engine.enhance_aligned(&mut frame).unwrap_err();
            assert!(error.contains("deadline"));
            assert_eq!(frame, [0.0; FRAME_SAMPLES]);
            requests_rx.try_recv().unwrap();
        }
        // The old full-frame wait takes at least 160 ms. Allow ample scheduler
        // slack while proving a stalled model cannot consume each mix period.
        assert!(started.elapsed() < Duration::from_millis(80));

        // A late response must not be replayed; the next timely one recovers
        // without changing denoiser mode or exposing unprocessed input.
        for (sequence, value) in [(0, 0.2), (8, 0.6)] {
            responses_tx
                .send(DeepFilterResponse {
                    sequence,
                    result: Ok(([value; FRAME_SAMPLES], Duration::from_millis(1))),
                    timing: CaptureTiming::default(),
                })
                .unwrap();
        }
        let mut recovered = [0.9; FRAME_SAMPLES];
        engine.enhance_aligned(&mut recovered).unwrap();
        assert_eq!(recovered, [0.6; FRAME_SAMPLES]);
    }

    #[test]
    fn deep_filter_model_timing_follows_accepted_inputs_after_a_request_drop() {
        let start = Instant::now();
        let stamp = |index: u64, pressed| {
            CaptureTiming::new(
                start + Duration::from_millis(index * 20),
                start,
                pressed,
                InputMode::PushToTalk,
                true,
            )
        };
        let mut delay = CaptureTimingDelay::new(DEEP_FILTER_MODEL_DELAY_SAMPLES);
        let initial = delay.process(stamp(0, false));
        assert!(initial.chunks.iter().all(Option::is_none));
        // Request 1 was rejected by the full queue; neither the model nor its
        // provenance advances for it. Request 2 carries the next accepted audio.
        let next = delay.process(stamp(2, true));
        assert!(next.chunks[0].is_none());
        assert_eq!(next.chunks[1].unwrap().captured_at, start);
        let mixed = delay.process(stamp(3, false));
        assert_eq!(
            mixed.chunks[0].unwrap().captured_at,
            start + Duration::from_millis(10)
        );
        assert_eq!(
            mixed.chunks[1].unwrap().captured_at,
            start + Duration::from_millis(40)
        );
        let mut frame = [0.7; FRAME_SAMPLES];
        assert!(mixed.restrict_transmission(&mut frame, InputMode::PushToTalk));
        assert_eq!(&frame[..480], &[0.0; 480]);
        assert_eq!(&frame[480..], &[0.7; 480]);
    }

    #[test]
    fn deep_filter_pipeline_preserves_a_future_response_after_a_sequence_gap() {
        let (requests_tx, _requests_rx) = bounded(4);
        let (responses_tx, responses_rx) = bounded(4);
        let mut engine = DeepFilterNetProcessingEngine {
            echo_frontend: None,
            gain_backend: None,
            requests: Some(requests_tx),
            responses: Some(responses_rx),
            worker: None,
            next_sequence: DEEP_FILTER_DELAY_FRAMES,
            pending_response: None,
            last_processing_time: None,
            mode: NoiseSuppressionMode::DeepFilterNet,
        };
        let captured_at = Instant::now() - Duration::from_millis(70);
        let queued_at = Instant::now() - Duration::from_millis(60);
        responses_tx
            .send(DeepFilterResponse {
                sequence: 1,
                result: Ok(([0.6; FRAME_SAMPLES], Duration::from_millis(2))),
                timing: CaptureTiming::new(
                    captured_at,
                    queued_at,
                    true,
                    InputMode::PushToTalk,
                    true,
                ),
            })
            .unwrap();

        let mut missing = [0.8; FRAME_SAMPLES];
        let error = engine.enhance_aligned(&mut missing).unwrap_err();
        assert!(error.contains("frame 0 is missing"));
        assert_eq!(missing, [0.0; FRAME_SAMPLES]);

        let mut recovered = [0.9; FRAME_SAMPLES];
        let timing = engine
            .enhance_aligned_timed(&mut recovered, CaptureTiming::default())
            .unwrap();
        assert_eq!(recovered, [0.6; FRAME_SAMPLES]);
        assert_eq!(timing.captured_at(), Some(captured_at));
        assert_eq!(timing.queued_at(), Some(queued_at));
        assert!(timing.restrict_transmission(&mut recovered, InputMode::PushToTalk));
    }

    #[test]
    fn standard_engine_mute_resume_keeps_echo_processing_finite() {
        let mut processor =
            CaptureAudioProcessor::standard(1.0, true, true, NoiseSuppressionStrength::Medium)
                .unwrap();
        for i in 0..150 {
            processor.set_output_unused((20..120).contains(&i));
            let render = [0.01; FRAME_SAMPLES];
            processor.process_render_reference(&render);
            let mut capture = std::array::from_fn(|n| (n as f32 * 0.03).sin() * 0.1);
            let analysis = processor.process_capture(&mut capture);
            assert!(capture.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
            assert!(analysis.level.rms.is_finite());
            assert!(processor.take_error().is_none());
        }
        assert!(processor.echo_cancellation_enabled());
        assert!(processor.automatic_gain_enabled());
    }

    #[test]
    fn standard_engine_processes_two_ten_millisecond_chunks() {
        let mut processor =
            CaptureAudioProcessor::standard(1.0, true, true, NoiseSuppressionStrength::Medium)
                .unwrap();
        let render = [0.05; FRAME_SAMPLES];
        processor.process_render_reference(&render);
        let mut capture = [0.025; FRAME_SAMPLES];
        let analysis = processor.process_capture(&mut capture);
        assert!(processor.take_error().is_none());
        assert_eq!(
            processor.noise_suppression_mode(),
            NoiseSuppressionMode::Standard
        );
        assert!(processor.noise_suppression_capabilities().supports_strength);
        assert!(capture.iter().all(|sample| sample.is_finite()));
        assert!(analysis.level.rms.is_finite());
    }

    #[test]
    fn standard_mode_enables_webrtc_transient_suppression_at_each_strength() {
        for (strength, expected_level) in [
            (
                NoiseSuppressionStrength::Low,
                webrtc_apm::AudioProcessing_Config_NoiseSuppression_Level_kLow,
            ),
            (
                NoiseSuppressionStrength::Medium,
                webrtc_apm::AudioProcessing_Config_NoiseSuppression_Level_kModerate,
            ),
            (
                NoiseSuppressionStrength::High,
                webrtc_apm::AudioProcessing_Config_NoiseSuppression_Level_kHigh,
            ),
        ] {
            let config = web_rtc_config(false, false, strength);
            assert!(config.transient_suppression.enabled);
            assert!(config.noise_suppression.enabled);
            assert_eq!(config.noise_suppression.level, expected_level);
        }
    }

    #[test]
    fn deep_filter_profiles_have_distinct_voice_isolation_parameters() {
        let natural = deep_filter_runtime_params(NoiseSuppressionMode::DeepFilterNet);
        let strong = deep_filter_runtime_params(NoiseSuppressionMode::DeepFilterNetStrong);
        assert!(!natural.post_filter);
        assert_eq!(natural.atten_lim_db, 18.0);
        assert_eq!(
            (
                natural.min_db_thresh,
                natural.max_db_erb_thresh,
                natural.max_db_df_thresh
            ),
            (-15.0, 35.0, 25.0)
        );
        assert!(strong.post_filter);
        assert_eq!(strong.post_filter_beta, 0.02);
        assert_eq!(strong.atten_lim_db, 100.0);
        assert_eq!(
            (
                strong.min_db_thresh,
                strong.max_db_erb_thresh,
                strong.max_db_df_thresh
            ),
            (-5.0, 40.0, 30.0)
        );
    }

    #[test]
    fn deep_filter_fixed_delay_matches_the_bundled_model_configuration() {
        let model = DfTract::new(DfParams::default(), &RuntimeParams::default()).unwrap();
        let model_delay = model.fft_size - model.hop_size + model.lookahead * model.hop_size;
        assert_eq!(model_delay, DEEP_FILTER_MODEL_DELAY_SAMPLES);
        assert_eq!(DEEP_FILTER_TOTAL_DELAY_SAMPLES, 3_360);
    }

    #[test]
    fn rnnoise_engine_processes_two_native_frames_without_strength_claims() {
        let mut processor = CaptureAudioProcessor::rnnoise(1.0, true, true);
        let render = [0.01; FRAME_SAMPLES];
        processor.process_render_reference(&render);
        for frame_index in 0..2 {
            let mut capture = std::array::from_fn(|sample| {
                let phase = (sample + frame_index * FRAME_SAMPLES) as f32 * 0.07;
                phase.sin() * 0.08 + if sample % 17 == 0 { 0.03 } else { 0.0 }
            });
            let analysis = processor.process_capture(&mut capture);
            assert!(capture.iter().all(|sample| sample.is_finite()));
            assert!(capture.iter().all(|sample| (-1.0..=1.0).contains(sample)));
            assert!(analysis.level.rms.is_finite());
        }
        assert!(processor.take_error().is_none());
        assert_eq!(
            processor.noise_suppression_mode(),
            NoiseSuppressionMode::RnNoise
        );
        assert!(!processor.noise_suppression_capabilities().supports_strength);
    }

    #[test]
    fn off_mode_can_keep_aec_and_agc_without_claiming_noise_suppression() {
        let mut processor = CaptureAudioProcessor::off(1.0, true, true);
        let mut frame = [0.02; FRAME_SAMPLES];
        processor.process_render_reference(&frame);
        processor.process_capture(&mut frame);
        assert_eq!(
            processor.noise_suppression_mode(),
            NoiseSuppressionMode::Off
        );
        assert!(processor.echo_cancellation_enabled());
        assert!(processor.automatic_gain_enabled());
        assert!(processor.take_error().is_none());
    }

    #[test]
    fn deep_filter_net_worker_returns_processed_frames_and_real_timings() {
        let mut processor =
            CaptureAudioProcessor::deep_filter_net(1.0, false, false, false).unwrap();
        for frame_index in 0..20 {
            let mut frame = std::array::from_fn(|sample| {
                ((sample + frame_index * FRAME_SAMPLES) as f32 * 0.05).sin() * 0.05
            });
            processor.process_capture(&mut frame);
            assert!(frame.iter().all(|sample| sample.is_finite()));
            if processor.metrics().processed_frames > 0 {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            processor.noise_suppression_mode(),
            NoiseSuppressionMode::DeepFilterNet
        );
        assert!(processor.metrics().processed_frames > 0);
        assert!(processor.metrics().total_processing_time > Duration::ZERO);
        assert!(processor.take_error().is_none());
    }

    #[test]
    fn rnnoise_processes_two_10ms_chunks_with_one_persistent_state() {
        let input = std::array::from_fn(|index| {
            let phase = index as f32 * 733.0 * TAU / 48_000.0;
            phase.sin() * 0.08 + (phase * 2.31).sin() * 0.015
        });
        let mut engine = RnNoiseProcessingEngine::new(false, false);
        let state_address = &engine.denoiser as *const Denoiser;

        let mut first = input;
        engine.process_capture(&mut first).unwrap();
        assert_eq!(engine.processed_chunks, 2);
        assert!(first.iter().all(|sample| sample.is_finite()));

        let mut second = input;
        engine.process_capture(&mut second).unwrap();
        assert_eq!(engine.processed_chunks, 4);
        assert_eq!(state_address, &engine.denoiser as *const Denoiser);
        assert!(second.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn rnnoise_matches_native_pcm16_scale_contract() {
        let input = std::array::from_fn(|index| {
            let phase = index as f32 * 997.0 * TAU / 48_000.0;
            phase.sin() * 0.12 + (phase * 0.37).sin() * 0.02
        });
        let mut expected_state = Denoiser::new(None).unwrap();
        let mut expected_input = [0.0; RNNOISE_FRAME_SIZE];
        let mut expected_output = [0.0; RNNOISE_FRAME_SIZE];
        let mut expected = Vec::with_capacity(FRAME_SAMPLES);
        for chunk in input.chunks_exact(RNNOISE_FRAME_SIZE) {
            for (target, sample) in expected_input.iter_mut().zip(chunk.iter().copied()) {
                *target = (sample * 32_768.0).clamp(-32_768.0, 32_767.0);
            }
            expected_state.process(&expected_input, &mut expected_output);
            expected.extend(expected_output.iter().map(|sample| sample / 32_768.0));
        }

        let mut actual = input;
        let mut engine = RnNoiseProcessingEngine::new(false, false);
        engine.process_capture(&mut actual).unwrap();
        for (actual, expected) in actual.iter().zip(expected) {
            assert!((actual - expected).abs() <= 1.0e-7);
        }
    }

    #[test]
    fn neural_denoisers_place_aec_before_and_agc_after_the_model() {
        let rnnoise = RnNoiseProcessingEngine::new(true, true);
        assert!(rnnoise.echo_frontend.is_some());
        assert!(rnnoise.gain_backend.is_some());

        let deep =
            DeepFilterNetProcessingEngine::new(true, true, NoiseSuppressionMode::DeepFilterNet)
                .unwrap();
        assert!(deep.echo_frontend.is_some());
        assert!(deep.gain_backend.is_some());
    }
}

#[cfg(test)]
mod dual_threshold_regressions {
    use super::*;
    #[test]
    fn weak_syllables_sustain_speech_but_cannot_open_a_closed_gate() {
        let mut vad = EnergyVoiceActivityDetector::default();
        for _ in 0..100 {
            assert!(!vad.process(&[0.003; FRAME_SAMPLES]));
        }
        assert!(vad.process(&[0.005; FRAME_SAMPLES]));
        for _ in 0..100 {
            assert!(vad.process(&[0.003; FRAME_SAMPLES]));
        }
        for _ in 0..20 {
            assert!(vad.process(&[0.001; FRAME_SAMPLES]));
        }
        assert!(!vad.process(&[0.001; FRAME_SAMPLES]));
        assert!(!vad.process(&[0.003; FRAME_SAMPLES]));
    }
    #[test]
    fn weak_speech_during_hold_refreshes_the_full_tail() {
        let mut vad = EnergyVoiceActivityDetector::default();
        assert!(vad.process(&[0.005; FRAME_SAMPLES]));
        for _ in 0..19 {
            assert!(vad.process(&[0.0; FRAME_SAMPLES]));
        }
        assert!(vad.process(&[0.003; FRAME_SAMPLES]));
        for _ in 0..20 {
            assert!(vad.process(&[0.0; FRAME_SAMPLES]));
        }
        assert!(!vad.process(&[0.0; FRAME_SAMPLES]));
    }
    #[test]
    fn low_background_noise_does_not_reopen_or_hold_forever() {
        let mut vad = EnergyVoiceActivityDetector::default();
        assert!(vad.process(&[0.005; FRAME_SAMPLES]));
        for _ in 0..20 {
            assert!(vad.process(&[0.0015; FRAME_SAMPLES]));
        }
        for _ in 0..200 {
            assert!(!vad.process(&[0.0015; FRAME_SAMPLES]));
        }
    }
}

#[cfg(test)]
mod extended_gain_tests {
    use super::*;

    #[test]
    fn three_hundred_percent_boosts_quiet_input_without_overflow() {
        let mut frame = [0.1; FRAME_SAMPLES];
        apply_gain(&mut frame, 3.0);
        assert!(frame.iter().all(|s| (*s - 0.3).abs() < 0.0001));
        frame.fill(0.9);
        apply_gain(&mut frame, 3.0);
        assert!(frame.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        assert_eq!(sanitize_gain(100.0), 3.0);
    }
}
