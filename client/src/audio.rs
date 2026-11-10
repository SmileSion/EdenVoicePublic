mod capture;
mod muted_speech;
pub use capture::AudioTiming;
use capture::*;
pub use muted_speech::MutedSpeechNotice;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use cpal::{
    FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig, SupportedStreamConfig,
    traits::{DeviceTrait, StreamTrait},
};
use crossbeam_channel::{Receiver, Sender, TrySendError, bounded, unbounded};
use sonora_common_audio::push_sinc_resampler::PushSincResampler;

use crate::{
    codec::{DEFAULT_BITRATE, FRAME_SAMPLES, OpusEncoder, SAMPLE_RATE},
    devices::{AudioDeviceCatalog, CpalDeviceCatalog},
    dsp::{
        AudioFrame, CaptureAudioProcessor, DspMetrics, InputMode, NoiseSuppressionMode,
        NoiseSuppressionStrength,
    },
    mixer,
    model::MicTestStatus,
    network::AudioAdaptationDecision,
    peer::RemotePeer,
    protocol::voice::{PacketType, VoicePacket},
    security::MediaSecurity,
    transport::{
        AdaptiveControl, AdaptiveTransport, LocalTransport, ReceivedAudio, RelayTransport,
        TransportEvent, VoicePathMetrics, VoiceTransport,
    },
};

#[derive(Clone, Debug)]
pub enum AudioEvent {
    Ready,
    Stopped,
    MutedSpeech(Instant),
    InputLevel(f32),
    InputPeak(f32),
    OutputLevel(f32),
    Talking(u64),
    Stats {
        sent: u64,
        received: u64,
        lost: u64,
        reordered: u64,
        jitter_depth: usize,
        opus_bitrate_bps: i32,
    },
    QoeFeedback(crate::protocol::control::AudioQoeFeedback),
    VoicePathMetrics(Option<VoicePathMetrics>),
    DspStatus {
        mode: NoiseSuppressionMode,
        echo_cancellation: bool,
        automatic_gain: bool,
        average_ms: f32,
        peak_ms: f32,
        deadline_misses: u64,
    },
    DspError(String),
    DspLoading(Option<NoiseSuppressionMode>),
    DspPerformanceWarning(Option<String>),
    MicTestStatus(MicTestStatus),
    Timing(AudioTiming),
    QueueStats {
        capture_overruns: u64,
        playback_underruns: u64,
    },
    DeviceStatus {
        input: String,
        output: String,
        input_sample_rate: u32,
        output_sample_rate: u32,
        fallback: bool,
    },
    DeviceError {
        direction: Option<AudioDeviceDirection>,
        message: String,
    },
    Error(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioDeviceDirection {
    Input,
    Output,
}

pub(crate) struct OpenedAudioStream {
    name: String,
    sample_rate: u32,
    stream: Stream,
}

#[derive(Clone)]
pub(crate) struct CaptureControls {
    ptt: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
}

pub(crate) trait AudioDeviceBackend {
    fn list_input_devices(&self) -> Vec<String>;
    fn list_output_devices(&self) -> Vec<String>;
    fn open_input(
        &self,
        name: Option<&str>,
        frames: Sender<CapturedFrame>,
        stale_frames: Receiver<CapturedFrame>,
        overruns: Arc<AtomicU64>,
        controls: CaptureControls,
        events: Sender<AudioEvent>,
    ) -> Result<OpenedAudioStream>;
    fn open_output(
        &self,
        name: Option<&str>,
        frames: OutputQueue,
        underruns: Arc<AtomicU64>,
        events: Sender<AudioEvent>,
    ) -> Result<OpenedAudioStream>;
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CpalAudioBackend;

impl AudioDeviceBackend for CpalAudioBackend {
    fn list_input_devices(&self) -> Vec<String> {
        CpalDeviceCatalog.enumerate().inputs
    }

    fn list_output_devices(&self) -> Vec<String> {
        CpalDeviceCatalog.enumerate().outputs
    }

    fn open_input(
        &self,
        name: Option<&str>,
        frames: Sender<CapturedFrame>,
        stale_frames: Receiver<CapturedFrame>,
        overruns: Arc<AtomicU64>,
        controls: CaptureControls,
        events: Sender<AudioEvent>,
    ) -> Result<OpenedAudioStream> {
        let device = CpalDeviceCatalog
            .input_named(name)
            .ok_or_else(|| anyhow!("no input audio device"))?;
        let config = choose_input_config(&device)?;
        let sample_rate = config.sample_rate();
        let stream = build_input(
            &device,
            config,
            frames,
            stale_frames,
            overruns,
            controls,
            events,
        )?;
        Ok(OpenedAudioStream {
            name: device.to_string(),
            sample_rate,
            stream,
        })
    }

    fn open_output(
        &self,
        name: Option<&str>,
        frames: OutputQueue,
        underruns: Arc<AtomicU64>,
        events: Sender<AudioEvent>,
    ) -> Result<OpenedAudioStream> {
        let device = CpalDeviceCatalog
            .output_named(name)
            .ok_or_else(|| anyhow!("no output audio device"))?;
        let config = choose_output_config(&device)?;
        let sample_rate = config.sample_rate();
        let stream = build_output(&device, config, frames, underruns, events)?;
        Ok(OpenedAudioStream {
            name: device.to_string(),
            sample_rate,
            stream,
        })
    }
}

pub struct AudioController {
    muted: Arc<AtomicBool>,
    commands: Sender<AudioCommand>,
    pub events: Receiver<AudioEvent>,
}

#[derive(Clone, Debug)]
pub struct AudioSettings {
    pub transport: AudioTransportSettings,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub input_gain: f32,
    pub output_volume: f32,
    pub input_mode: InputMode,
    pub echo_cancellation: bool,
    pub automatic_gain: bool,
    pub noise_suppression_mode: NoiseSuppressionMode,
    pub noise_suppression_strength: NoiseSuppressionStrength,
}

#[derive(Clone, Debug)]
pub enum AudioTransportSettings {
    Local,
    Relay {
        control_address: String,
        voice_address: String,
        session_id: u64,
    },
    Adaptive {
        control_address: String,
        voice_address: String,
        session_id: u64,
        control: AdaptiveControl,
        events: Sender<TransportEvent>,
        media_queue_size: usize,
        security: Option<MediaSecurity>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioProcessingSettings {
    pub input_gain: f32,
    pub input_mode: InputMode,
    pub echo_cancellation: bool,
    pub automatic_gain: bool,
    pub noise_suppression_mode: NoiseSuppressionMode,
    pub noise_suppression_strength: NoiseSuppressionStrength,
}

enum AudioCommand {
    Start(AudioSettings),
    SetMutedSpeechAnalysis(bool),
    SetDeafened(bool),
    SetUserVolume(u64, f32),
    SetUserLocalMute(u64, bool),
    SetMasterOutputVolume(f32),
    SetPushToTalkPressed(bool),
    ApplyAdaptation(AudioAdaptationDecision),
    PlayOutputTest,
    StartMicTest,
    PlayMicTestOriginal,
    PlayMicTestProcessed,
    StopMicTest,
    UpdateProcessing(AudioProcessingSettings),
    SwitchDevices {
        input: Option<String>,
        output: Option<String>,
    },
    Stop,
    Shutdown,
}

struct DspLoadRequest {
    generation: u64,
    settings: AudioProcessingSettings,
}

struct DspLoadResult {
    generation: u64,
    input_mode: InputMode,
    processor: Result<CaptureAudioProcessor, String>,
}

impl AudioController {
    pub fn spawn() -> Self {
        let (commands_tx, commands_rx) = unbounded();
        let (events_tx, events_rx) = bounded(128);
        let thread_events = events_tx.clone();
        let muted = Arc::new(AtomicBool::new(false));
        let worker_muted = muted.clone();
        if let Err(error) = thread::Builder::new()
            .name("audio-controller".into())
            .spawn(move || controller_worker(commands_rx, thread_events, worker_muted))
        {
            let _ = events_tx.try_send(AudioEvent::Error(format!(
                "audio controller creation failed: {error}"
            )));
        }
        Self {
            muted,
            commands: commands_tx,
            events: events_rx,
        }
    }
    pub fn start(&self, settings: AudioSettings) {
        let _ = self.commands.send(AudioCommand::Start(settings));
    }
    pub fn set_muted(&self, value: bool) {
        self.muted.store(value, Ordering::Release);
    }
    pub fn set_muted_speech_analysis(&self, enabled: bool) {
        let _ = self
            .commands
            .send(AudioCommand::SetMutedSpeechAnalysis(enabled));
    }
    pub fn set_deafened(&self, value: bool) {
        let _ = self.commands.send(AudioCommand::SetDeafened(value));
    }
    pub fn stop(&self) {
        let _ = self.commands.send(AudioCommand::Stop);
    }
    pub fn set_user_volume(&self, session_id: u64, volume: f32) {
        let _ = self
            .commands
            .send(AudioCommand::SetUserVolume(session_id, volume));
    }
    pub fn set_user_local_mute(&self, session_id: u64, muted: bool) {
        let _ = self
            .commands
            .send(AudioCommand::SetUserLocalMute(session_id, muted));
    }
    pub fn set_master_output_volume(&self, volume: f32) {
        let _ = self
            .commands
            .send(AudioCommand::SetMasterOutputVolume(volume));
    }
    pub fn set_push_to_talk_pressed(&self, pressed: bool) {
        let _ = self
            .commands
            .send(AudioCommand::SetPushToTalkPressed(pressed));
    }
    pub fn apply_adaptation(&self, decision: AudioAdaptationDecision) {
        let _ = self.commands.send(AudioCommand::ApplyAdaptation(decision));
    }
    pub fn play_output_test(&self) {
        let _ = self.commands.send(AudioCommand::PlayOutputTest);
    }
    pub fn start_mic_test(&self) {
        let _ = self.commands.send(AudioCommand::StartMicTest);
    }
    pub fn play_mic_test_original(&self) {
        let _ = self.commands.send(AudioCommand::PlayMicTestOriginal);
    }
    pub fn play_mic_test_processed(&self) {
        let _ = self.commands.send(AudioCommand::PlayMicTestProcessed);
    }
    pub fn stop_mic_test(&self) {
        let _ = self.commands.send(AudioCommand::StopMicTest);
    }
    pub fn update_processing(&self, settings: AudioProcessingSettings) {
        let _ = self.commands.send(AudioCommand::UpdateProcessing(settings));
    }
    pub fn switch_devices(&self, input: Option<String>, output: Option<String>) {
        let _ = self
            .commands
            .send(AudioCommand::SwitchDevices { input, output });
    }
}

impl Drop for AudioController {
    fn drop(&mut self) {
        let _ = self.commands.send(AudioCommand::Shutdown);
    }
}

fn controller_worker(
    commands: Receiver<AudioCommand>,
    events: Sender<AudioEvent>,
    muted: Arc<AtomicBool>,
) {
    let (dsp_load_tx, dsp_load_rx) = unbounded::<DspLoadRequest>();
    let (dsp_loaded_tx, dsp_loaded_rx) = unbounded::<DspLoadResult>();
    if let Err(error) = thread::Builder::new()
        .name("dsp-loader".into())
        .spawn(move || dsp_loader_worker(dsp_load_rx, dsp_loaded_tx))
    {
        let _ = events.try_send(AudioEvent::Error(format!(
            "DSP loader creation failed: {error}"
        )));
    }
    let mut engine: Option<AudioEngine> = None;
    let mut muted_speech_analysis = false;
    let mut deafened = false;
    let mut push_to_talk_pressed = false;
    let mut dsp_generation = 0u64;
    let mut current_processing: Option<AudioProcessingSettings> = None;
    loop {
        match commands.recv_timeout(Duration::from_millis(20)) {
            Ok(AudioCommand::Start(settings)) => {
                dsp_generation = dsp_generation.wrapping_add(1);
                engine = None;
                let desired_processing = AudioProcessingSettings {
                    input_gain: settings.input_gain,
                    input_mode: settings.input_mode,
                    echo_cancellation: settings.echo_cancellation,
                    automatic_gain: settings.automatic_gain,
                    noise_suppression_mode: settings.noise_suppression_mode,
                    noise_suppression_strength: settings.noise_suppression_strength,
                };
                current_processing = Some(desired_processing);
                let initial_processing = startup_processing_settings(desired_processing);
                let transport: anyhow::Result<Box<dyn VoiceTransport>> = match &settings.transport {
                    AudioTransportSettings::Local => Ok(Box::new(LocalTransport)),
                    AudioTransportSettings::Relay {
                        control_address,
                        voice_address,
                        session_id,
                    } => RelayTransport::connect(control_address, voice_address, *session_id)
                        .map(|transport| Box::new(transport) as Box<dyn VoiceTransport>)
                        .map_err(anyhow::Error::from),
                    AudioTransportSettings::Adaptive {
                        control_address,
                        voice_address,
                        session_id,
                        control,
                        events,
                        media_queue_size,
                        security,
                    } => AdaptiveTransport::connect(
                        control_address,
                        voice_address,
                        *session_id,
                        control.clone(),
                        events.clone(),
                        *media_queue_size,
                        security.clone(),
                    )
                    .map(|transport| Box::new(transport) as Box<dyn VoiceTransport>)
                    .map_err(anyhow::Error::from),
                };
                let result = transport.and_then(|transport| {
                    AudioEngine::start_with_mute(
                        transport,
                        settings.input_device.as_deref(),
                        settings.output_device.as_deref(),
                        settings.output_volume,
                        initial_processing,
                        muted.clone(),
                    )
                });
                match result {
                    Ok(value) => {
                        value.set_muted_speech_analysis(muted_speech_analysis);
                        value.set_deafened(deafened);
                        value.set_push_to_talk_pressed(push_to_talk_pressed);
                        let device_status = value.device_status(
                            settings.input_device.as_deref(),
                            settings.output_device.as_deref(),
                        );
                        engine = Some(value);
                        let _ = events.try_send(AudioEvent::Ready);
                        let _ = events.try_send(device_status);
                        if initial_processing != desired_processing {
                            let _ = events.try_send(AudioEvent::DspLoading(Some(
                                desired_processing.noise_suppression_mode,
                            )));
                            if dsp_load_tx
                                .send(DspLoadRequest {
                                    generation: dsp_generation,
                                    settings: desired_processing,
                                })
                                .is_err()
                            {
                                let _ = events.try_send(AudioEvent::DspLoading(None));
                                let _ = events.try_send(AudioEvent::DspError(
                                    "DSP loader is unavailable; Standard remains active".into(),
                                ));
                            }
                        }
                    }
                    Err(error) => {
                        let _ = events.try_send(AudioEvent::Error(format!(
                            "audio initialization failed: {error}"
                        )));
                    }
                }
            }
            Ok(AudioCommand::SetMutedSpeechAnalysis(value)) => {
                muted_speech_analysis = value;
                if let Some(engine) = &engine {
                    engine.set_muted_speech_analysis(value);
                }
            }
            Ok(AudioCommand::SetDeafened(value)) => {
                deafened = value;
                if let Some(engine) = &engine {
                    engine.set_deafened(value);
                }
            }
            Ok(AudioCommand::SetUserVolume(session_id, volume)) => {
                if let Some(engine) = &engine {
                    engine.set_user_volume(session_id, volume);
                }
            }
            Ok(AudioCommand::SetUserLocalMute(session_id, muted)) => {
                if let Some(engine) = &engine {
                    engine.set_user_local_mute(session_id, muted);
                }
            }
            Ok(AudioCommand::SetMasterOutputVolume(volume)) => {
                if let Some(engine) = &engine {
                    engine.set_master_output_volume(volume);
                }
            }
            Ok(AudioCommand::SetPushToTalkPressed(pressed)) => {
                push_to_talk_pressed = pressed;
                if let Some(engine) = &engine {
                    engine.set_push_to_talk_pressed(pressed);
                }
            }
            Ok(AudioCommand::ApplyAdaptation(decision)) => {
                if let Some(engine) = &engine {
                    engine.apply_adaptation(decision);
                }
            }
            Ok(AudioCommand::PlayOutputTest) => {
                if let Some(engine) = &engine {
                    engine.play_output_test();
                }
            }
            Ok(AudioCommand::StartMicTest) => {
                if let Some(engine) = &engine {
                    engine.start_mic_test();
                }
            }
            Ok(AudioCommand::PlayMicTestOriginal) => {
                if let Some(engine) = &engine {
                    engine.play_mic_test_original();
                }
            }
            Ok(AudioCommand::PlayMicTestProcessed) => {
                if let Some(engine) = &engine {
                    engine.play_mic_test_processed();
                }
            }
            Ok(AudioCommand::StopMicTest) => {
                if let Some(engine) = &engine {
                    engine.stop_mic_test();
                }
            }
            Ok(AudioCommand::UpdateProcessing(settings)) => {
                current_processing = Some(settings);
                if let Some(engine) = &engine {
                    engine.stop_mic_test();
                    let _ = events.try_send(AudioEvent::MicTestStatus(MicTestStatus::Idle));
                    dsp_generation = dsp_generation.wrapping_add(1);
                    let request = DspLoadRequest {
                        generation: dsp_generation,
                        settings,
                    };
                    let _ = events.try_send(AudioEvent::DspLoading(Some(
                        settings.noise_suppression_mode,
                    )));
                    if dsp_load_tx.send(request).is_err() {
                        let _ = events.try_send(AudioEvent::DspLoading(None));
                        let _ = events.try_send(AudioEvent::DspError(
                            "DSP loader is unavailable; previous processor remains active".into(),
                        ));
                    }
                }
            }
            Ok(AudioCommand::SwitchDevices { input, output }) => {
                if let Some(engine) = engine.as_mut() {
                    match engine.switch_devices(input.as_deref(), output.as_deref()) {
                        Ok(status) => {
                            let _ = events.try_send(status);
                            if let Some(settings) = current_processing {
                                dsp_generation = dsp_generation.wrapping_add(1);
                                let _ = events.try_send(AudioEvent::DspLoading(Some(
                                    settings.noise_suppression_mode,
                                )));
                                if dsp_load_tx
                                    .send(DspLoadRequest {
                                        generation: dsp_generation,
                                        settings,
                                    })
                                    .is_err()
                                {
                                    let _ = events.try_send(AudioEvent::DspLoading(None));
                                    let _ = events.try_send(AudioEvent::DspError(
                                        "DSP loader is unavailable; previous processor remains active"
                                            .into(),
                                    ));
                                }
                            }
                        }
                        Err(error) => {
                            let _ = events.try_send(AudioEvent::DeviceError {
                                direction: None,
                                message: format!(
                                    "device switch failed; previous devices restored: {error}"
                                ),
                            });
                        }
                    }
                }
            }
            Ok(AudioCommand::Stop) => {
                dsp_generation = dsp_generation.wrapping_add(1);
                current_processing = None;
                engine = None;
                let _ = events.try_send(AudioEvent::Stopped);
            }
            Ok(AudioCommand::Shutdown) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return;
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
        }
        while let Ok(loaded) = dsp_loaded_rx.try_recv() {
            if loaded.generation != dsp_generation {
                continue;
            }
            if let Some(engine) = &engine {
                match loaded.processor {
                    Ok(processor) => {
                        engine.replace_capture_processor(processor, loaded.input_mode);
                    }
                    Err(error) => {
                        let _ = events.try_send(AudioEvent::DspError(format!(
                            "{error}; previous processor remains active"
                        )));
                    }
                }
                let _ = events.try_send(AudioEvent::DspLoading(None));
            }
        }
        if let Some(engine) = engine.as_mut() {
            while let Ok(event) = engine.events.try_recv() {
                let recovery = match &event {
                    AudioEvent::DeviceError {
                        direction: Some(direction),
                        ..
                    } => Some(*direction),
                    _ => None,
                };
                let _ = events.try_send(event);
                if let Some(direction) = recovery {
                    match engine.recover_default(direction) {
                        Ok(status) => {
                            let _ = events.try_send(status);
                            if let Some(settings) = current_processing {
                                dsp_generation = dsp_generation.wrapping_add(1);
                                let _ = events.try_send(AudioEvent::DspLoading(Some(
                                    settings.noise_suppression_mode,
                                )));
                                if dsp_load_tx
                                    .send(DspLoadRequest {
                                        generation: dsp_generation,
                                        settings,
                                    })
                                    .is_err()
                                {
                                    let _ = events.try_send(AudioEvent::DspLoading(None));
                                    let _ = events.try_send(AudioEvent::DspError(
                                        "DSP loader is unavailable; previous processor remains active"
                                            .into(),
                                    ));
                                }
                            }
                        }
                        Err(error) => {
                            let _ = events.try_send(AudioEvent::Error(format!(
                                "audio device recovery failed: {error}"
                            )));
                        }
                    }
                }
            }
        }
    }
}

fn startup_processing_settings(desired: AudioProcessingSettings) -> AudioProcessingSettings {
    if matches!(
        desired.noise_suppression_mode,
        NoiseSuppressionMode::DeepFilterNet | NoiseSuppressionMode::DeepFilterNetStrong
    ) {
        AudioProcessingSettings {
            noise_suppression_mode: NoiseSuppressionMode::Standard,
            ..desired
        }
    } else {
        desired
    }
}

fn dsp_loader_worker(requests: Receiver<DspLoadRequest>, loaded: Sender<DspLoadResult>) {
    while let Ok(mut request) = requests.recv() {
        while let Ok(newer) = requests.try_recv() {
            request = newer;
        }
        let processor = create_capture_processor(request.settings);
        if loaded
            .send(DspLoadResult {
                generation: request.generation,
                input_mode: request.settings.input_mode,
                processor,
            })
            .is_err()
        {
            return;
        }
    }
}

// Own this before spawning: an early return while starting either device
// cancels the worker just like dropping a fully constructed engine does.
struct AudioWorkerLifetime(Arc<AtomicBool>);

impl Drop for AudioWorkerLifetime {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub struct AudioEngine {
    device_backend: Box<dyn AudioDeviceBackend>,
    muted: Arc<AtomicBool>,
    deafened: Arc<AtomicBool>,
    push_to_talk_pressed: Arc<AtomicBool>,
    _lifetime: AudioWorkerLifetime,
    worker_commands: Sender<WorkerCommand>,
    pub events: Receiver<AudioEvent>,
    capture_tx: Sender<CapturedFrame>,
    capture_rx: Receiver<CapturedFrame>,
    output_queue: OutputQueue,
    capture_overruns: Arc<AtomicU64>,
    playback_underruns: Arc<AtomicU64>,
    event_tx: Sender<AudioEvent>,
    input_name: String,
    output_name: String,
    input_sample_rate: u32,
    output_sample_rate: u32,
    input: Stream,
    output: Stream,
}

impl AudioEngine {
    pub fn start(
        transport: Box<dyn VoiceTransport>,
        input_name: Option<&str>,
        output_name: Option<&str>,
        output_volume: f32,
        processing: AudioProcessingSettings,
    ) -> Result<Self> {
        Self::start_with_mute(
            transport,
            input_name,
            output_name,
            output_volume,
            processing,
            Arc::new(AtomicBool::new(false)),
        )
    }

    fn start_with_mute(
        transport: Box<dyn VoiceTransport>,
        input_name: Option<&str>,
        output_name: Option<&str>,
        output_volume: f32,
        processing: AudioProcessingSettings,
        muted: Arc<AtomicBool>,
    ) -> Result<Self> {
        let (capture_tx, capture_rx) = bounded(8);
        let (playback_tx, playback_rx) = bounded(8);
        let (references, stale_references) = bounded(16);
        let output_queue = OutputQueue {
            frames: playback_rx.clone(),
            references,
            stale_references,
            reference_drops: Arc::new(AtomicU64::new(0)),
        };
        let (event_tx, event_rx) = bounded(128);
        let (worker_commands_tx, worker_commands_rx) = bounded(64);
        let capture_overruns = Arc::new(AtomicU64::new(0));
        let playback_underruns = Arc::new(AtomicU64::new(0));
        let push_to_talk_pressed = Arc::new(AtomicBool::new(false));
        let device_backend: Box<dyn AudioDeviceBackend> = Box::new(CpalAudioBackend);
        let opened_input = device_backend.open_input(
            input_name,
            capture_tx.clone(),
            capture_rx.clone(),
            Arc::clone(&capture_overruns),
            CaptureControls {
                ptt: push_to_talk_pressed.clone(),
                muted: muted.clone(),
            },
            event_tx.clone(),
        )?;
        let opened_output = device_backend.open_output(
            output_name,
            output_queue.clone(),
            Arc::clone(&playback_underruns),
            event_tx.clone(),
        )?;
        let active_input_name = opened_input.name;
        let active_output_name = opened_output.name;
        let active_input_sample_rate = opened_input.sample_rate;
        let active_output_sample_rate = opened_output.sample_rate;
        let input_stream = opened_input.stream;
        let output_stream = opened_output.stream;
        let deafened = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let lifetime = AudioWorkerLifetime(Arc::clone(&alive));
        let worker_muted = Arc::clone(&muted);
        let worker_deafened = Arc::clone(&deafened);
        let worker_alive = Arc::clone(&alive);
        let worker_capture_overruns = Arc::clone(&capture_overruns);
        let worker_playback_underruns = Arc::clone(&playback_underruns);
        let worker_event_tx = event_tx.clone();
        let worker_capture_rx = capture_rx.clone();
        let worker_playback_rx = playback_rx.clone();
        let worker_output_queue = output_queue.clone();
        // Keep a small application-side reserve between the best-effort worker
        // scheduler and the real-time device callback. Without it, even a
        // momentary OS scheduling delay turns into a full audible 20 ms gap.
        prime_playback_queue(&playback_tx);
        thread::Builder::new()
            .name("voice-audio-worker".into())
            .spawn(move || {
                run_worker(
                    transport,
                    worker_capture_rx,
                    playback_tx,
                    worker_playback_rx,
                    worker_event_tx,
                    WorkerSettings {
                        output_queue: worker_output_queue,
                        control: WorkerControl {
                            muted: worker_muted,
                            deafened: worker_deafened,
                            alive: worker_alive,
                            capture_overruns: worker_capture_overruns,
                            playback_underruns: worker_playback_underruns,
                        },
                        input_gain: processing.input_gain,
                        output_volume,
                        input_mode: processing.input_mode,
                        echo_cancellation: processing.echo_cancellation,
                        automatic_gain: processing.automatic_gain,
                        noise_suppression_mode: processing.noise_suppression_mode,
                        noise_suppression_strength: processing.noise_suppression_strength,
                        commands: worker_commands_rx,
                    },
                )
            })?;
        input_stream.play().context("start input stream")?;
        output_stream.play().context("start output stream")?;
        Ok(Self {
            device_backend,
            muted,
            deafened,
            push_to_talk_pressed,
            _lifetime: lifetime,
            worker_commands: worker_commands_tx,
            events: event_rx,
            capture_tx,
            capture_rx,
            output_queue,
            capture_overruns,
            playback_underruns,
            event_tx,
            input_name: active_input_name,
            output_name: active_output_name,
            input_sample_rate: active_input_sample_rate,
            output_sample_rate: active_output_sample_rate,
            input: input_stream,
            output: output_stream,
        })
    }

    pub fn set_muted_speech_analysis(&self, enabled: bool) {
        let _ = self
            .worker_commands
            .try_send(WorkerCommand::MutedSpeechAnalysis(enabled));
    }

    pub fn set_muted(&self, value: bool) {
        self.muted.store(value, Ordering::Release);
    }
    pub fn set_deafened(&self, value: bool) {
        self.deafened.store(value, Ordering::Relaxed);
    }
    pub fn set_user_volume(&self, session_id: u64, volume: f32) {
        let _ = self
            .worker_commands
            .try_send(WorkerCommand::UserVolume(session_id, volume));
    }
    pub fn set_user_local_mute(&self, session_id: u64, muted: bool) {
        let _ = self
            .worker_commands
            .try_send(WorkerCommand::UserLocalMute(session_id, muted));
    }
    pub fn set_master_output_volume(&self, volume: f32) {
        let _ = self
            .worker_commands
            .try_send(WorkerCommand::MasterOutputVolume(volume));
    }
    pub fn set_push_to_talk_pressed(&self, pressed: bool) {
        self.push_to_talk_pressed.store(pressed, Ordering::Release);
    }
    pub fn apply_adaptation(&self, decision: AudioAdaptationDecision) {
        let _ = self
            .worker_commands
            .try_send(WorkerCommand::ApplyAdaptation(decision));
    }
    fn play_output_test(&self) {
        let _ = self.worker_commands.try_send(WorkerCommand::PlayOutputTest);
    }
    fn start_mic_test(&self) {
        let _ = self.worker_commands.try_send(WorkerCommand::StartMicTest);
    }
    fn play_mic_test_original(&self) {
        let _ = self
            .worker_commands
            .try_send(WorkerCommand::PlayMicTestOriginal);
    }
    fn play_mic_test_processed(&self) {
        let _ = self
            .worker_commands
            .try_send(WorkerCommand::PlayMicTestProcessed);
    }
    fn stop_mic_test(&self) {
        let _ = self.worker_commands.try_send(WorkerCommand::StopMicTest);
    }
    fn replace_capture_processor(&self, processor: CaptureAudioProcessor, input_mode: InputMode) {
        let _ = self
            .worker_commands
            .send(WorkerCommand::ReplaceCaptureProcessor {
                processor,
                input_mode,
            });
    }

    fn device_status(
        &self,
        requested_input: Option<&str>,
        requested_output: Option<&str>,
    ) -> AudioEvent {
        AudioEvent::DeviceStatus {
            input: self.input_name.clone(),
            output: self.output_name.clone(),
            input_sample_rate: self.input_sample_rate,
            output_sample_rate: self.output_sample_rate,
            fallback: requested_device_fell_back(requested_input, &self.input_name)
                || requested_device_fell_back(requested_output, &self.output_name),
        }
    }

    fn switch_devices(
        &mut self,
        requested_input: Option<&str>,
        requested_output: Option<&str>,
    ) -> Result<AudioEvent> {
        let opened_input = self.device_backend.open_input(
            requested_input,
            self.capture_tx.clone(),
            self.capture_rx.clone(),
            Arc::clone(&self.capture_overruns),
            CaptureControls {
                ptt: self.push_to_talk_pressed.clone(),
                muted: self.muted.clone(),
            },
            self.event_tx.clone(),
        )?;
        let opened_output = self.device_backend.open_output(
            requested_output,
            self.output_queue.clone(),
            Arc::clone(&self.playback_underruns),
            self.event_tx.clone(),
        )?;
        let input_name = opened_input.name;
        let output_name = opened_output.name;
        let input_sample_rate = opened_input.sample_rate;
        let output_sample_rate = opened_output.sample_rate;
        let new_input = opened_input.stream;
        let new_output = opened_output.stream;

        self.input.pause().context("pause previous input stream")?;
        if let Err(error) = self.output.pause().context("pause previous output stream") {
            let _ = self.input.play();
            return Err(error);
        }
        if let Err(error) = new_input
            .play()
            .context("start replacement input stream")
            .and_then(|_| new_output.play().context("start replacement output stream"))
        {
            let _ = new_input.pause();
            let _ = self.input.play();
            let _ = self.output.play();
            return Err(error);
        }

        self.input = new_input;
        self.output = new_output;
        self.input_name = input_name.clone();
        self.output_name = output_name.clone();
        self.input_sample_rate = input_sample_rate;
        self.output_sample_rate = output_sample_rate;
        let fallback = requested_device_fell_back(requested_input, &input_name)
            || requested_device_fell_back(requested_output, &output_name);
        Ok(AudioEvent::DeviceStatus {
            input: input_name,
            output: output_name,
            input_sample_rate,
            output_sample_rate,
            fallback,
        })
    }

    fn recover_default(&mut self, direction: AudioDeviceDirection) -> Result<AudioEvent> {
        let input = (direction != AudioDeviceDirection::Input).then(|| self.input_name.clone());
        let output = (direction != AudioDeviceDirection::Output).then(|| self.output_name.clone());
        match self.switch_devices(input.as_deref(), output.as_deref())? {
            AudioEvent::DeviceStatus {
                input,
                output,
                input_sample_rate,
                output_sample_rate,
                ..
            } => Ok(AudioEvent::DeviceStatus {
                input,
                output,
                input_sample_rate,
                output_sample_rate,
                fallback: true,
            }),
            _ => unreachable!(),
        }
    }
}

const PLAYBACK_PREROLL_FRAMES: usize = 1;
const PLAYBACK_TARGET_QUEUED_FRAMES: usize = 2;
const PLAYBACK_FRAME_DURATION_MS: u64 = 20;
const PLAYBACK_CLOCK_CORRECTION_MS: u64 = 1;

fn prime_playback_queue(playback: &Sender<[f32; FRAME_SAMPLES]>) {
    for _ in 0..PLAYBACK_PREROLL_FRAMES {
        let _ = playback.try_send([0.0; FRAME_SAMPLES]);
    }
}

/// Sends the newest real-time frame without ever waiting. Returns true when a
/// stale queued frame had to be discarded (or the queue disconnected).
fn send_latest<T>(sender: &Sender<T>, stale: &Receiver<T>, value: T) -> bool {
    match sender.try_send(value) {
        Ok(()) => false,
        Err(TrySendError::Full(value)) => {
            let _ = stale.try_recv();
            let _ = sender.try_send(value);
            true
        }
        Err(TrySendError::Disconnected(_)) => true,
    }
}

fn playback_cadence(queued_frames: usize) -> Duration {
    let millis = match queued_frames.cmp(&PLAYBACK_TARGET_QUEUED_FRAMES) {
        std::cmp::Ordering::Less => PLAYBACK_FRAME_DURATION_MS - PLAYBACK_CLOCK_CORRECTION_MS,
        std::cmp::Ordering::Equal => PLAYBACK_FRAME_DURATION_MS,
        std::cmp::Ordering::Greater => PLAYBACK_FRAME_DURATION_MS + PLAYBACK_CLOCK_CORRECTION_MS,
    };
    Duration::from_millis(millis)
}

fn requested_device_fell_back(requested: Option<&str>, actual: &str) -> bool {
    requested.is_some_and(|name| name != actual)
}

fn choose_input_config(device: &cpal::Device) -> Result<SupportedStreamConfig> {
    choose_config(
        device
            .supported_input_configs()
            .context("query input formats")?,
    )
}
fn choose_output_config(device: &cpal::Device) -> Result<SupportedStreamConfig> {
    choose_config(
        device
            .supported_output_configs()
            .context("query output formats")?,
    )
}
fn choose_config(
    configs: impl Iterator<Item = cpal::SupportedStreamConfigRange>,
) -> Result<SupportedStreamConfig> {
    let target = SAMPLE_RATE;
    configs
        .filter(|range| supported_format(range.sample_format()))
        .filter_map(|range| {
            let selected = target.clamp(range.min_sample_rate(), range.max_sample_rate());
            range.try_with_sample_rate(selected)
        })
        .min_by_key(|config| {
            (
                config.sample_rate().abs_diff(target),
                format_rank(config.sample_format()),
                config.channels(),
            )
        })
        .ok_or_else(|| anyhow!("device does not expose a supported PCM format"))
}

fn supported_format(format: SampleFormat) -> bool {
    matches!(
        format,
        SampleFormat::F32
            | SampleFormat::F64
            | SampleFormat::I16
            | SampleFormat::I32
            | SampleFormat::U16
            | SampleFormat::U32
    )
}
fn format_rank(format: SampleFormat) -> u8 {
    match format {
        SampleFormat::F32 => 0,
        SampleFormat::I16 => 1,
        SampleFormat::U16 => 2,
        _ => 3,
    }
}

fn build_input(
    device: &cpal::Device,
    supported: SupportedStreamConfig,
    tx: Sender<CapturedFrame>,
    stale_frames: Receiver<CapturedFrame>,
    capture_overruns: Arc<AtomicU64>,
    controls: CaptureControls,
    errors: Sender<AudioEvent>,
) -> Result<Stream> {
    let channels = supported.channels() as usize;
    let sample_rate = supported.sample_rate();
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    macro_rules! build {
        ($sample:ty) => {
            build_input_t::<$sample>(
                device,
                config,
                channels,
                sample_rate,
                tx,
                stale_frames,
                capture_overruns,
                controls,
                errors,
            )
        };
    }
    match format {
        SampleFormat::F32 => build!(f32),
        SampleFormat::I16 => build!(i16),
        SampleFormat::U16 => build!(u16),
        SampleFormat::I32 => build!(i32),
        SampleFormat::U32 => build!(u32),
        SampleFormat::F64 => build!(f64),
        other => Err(anyhow!("unsupported input sample format {other}")),
    }
}

#[allow(clippy::too_many_arguments)]
fn build_input_t<T>(
    device: &cpal::Device,
    config: StreamConfig,
    channels: usize,
    sample_rate: u32,
    tx: Sender<CapturedFrame>,
    stale_frames: Receiver<CapturedFrame>,
    capture_overruns: Arc<AtomicU64>,
    controls: CaptureControls,
    errors: Sender<AudioEvent>,
) -> Result<Stream>
where
    T: SizedSample + Copy,
    f32: FromSample<T>,
{
    let mut capture_buffer =
        CaptureFrameBuffer::new(sample_rate, controls.muted.load(Ordering::Acquire));
    let stream_overruns = capture_overruns.clone();
    device
        .build_input_stream(
            config,
            move |data: &[T], info| {
                let stamp = info.timestamp();
                let lag = stamp.callback.duration_since(stamp.capture);
                let callback_start = Instant::now().checked_sub(lag).unwrap_or_else(Instant::now);
                let mut offset = 0usize;
                for source in data.chunks(channels) {
                    let muted = controls.muted.load(Ordering::Acquire);
                    offset += 1;
                    let mono =
                        source.iter().copied().map(f32::from_sample).sum::<f32>() / channels as f32;
                    capture_buffer.push(mono.clamp(-1.0, 1.0), muted);
                    while let Some(frame) = capture_buffer.next_frame() {
                        if send_latest(
                            &tx,
                            &stale_frames,
                            CapturedFrame {
                                samples: frame,
                                ptt_pressed: controls.ptt.load(Ordering::Acquire),
                                transmission_allowed: !muted
                                    && !controls.muted.load(Ordering::Acquire),
                                queued_at: Instant::now(),
                                captured_at: (callback_start
                                    + Duration::from_secs_f64(offset as f64 / sample_rate as f64))
                                .checked_sub(Duration::from_millis(20))
                                .unwrap_or(callback_start),
                            },
                        ) {
                            capture_overruns.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            },
            move |error| {
                if is_non_fatal_stream_error(&error) {
                    if error.kind() == cpal::ErrorKind::Xrun {
                        stream_overruns.fetch_add(1, Ordering::Relaxed);
                    }
                    tracing::debug!(error = %error, "non-fatal input stream event");
                    return;
                }
                let _ = errors.try_send(AudioEvent::DeviceError {
                    direction: Some(AudioDeviceDirection::Input),
                    message: format!("input device: {error}"),
                });
            },
            None,
        )
        .context("open input stream")
}

fn build_output(
    device: &cpal::Device,
    supported: SupportedStreamConfig,
    rx: OutputQueue,
    playback_underruns: Arc<AtomicU64>,
    errors: Sender<AudioEvent>,
) -> Result<Stream> {
    let channels = supported.channels() as usize;
    let sample_rate = supported.sample_rate();
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    macro_rules! build {
        ($sample:ty) => {
            build_output_t::<$sample>(
                device,
                config,
                channels,
                sample_rate,
                rx,
                playback_underruns,
                errors,
            )
        };
    }
    match format {
        SampleFormat::F32 => build!(f32),
        SampleFormat::I16 => build!(i16),
        SampleFormat::U16 => build!(u16),
        SampleFormat::I32 => build!(i32),
        SampleFormat::U32 => build!(u32),
        SampleFormat::F64 => build!(f64),
        other => Err(anyhow!("unsupported output sample format {other}")),
    }
}

fn build_output_t<T>(
    device: &cpal::Device,
    config: StreamConfig,
    channels: usize,
    sample_rate: u32,
    rx: OutputQueue,
    playback_underruns: Arc<AtomicU64>,
    errors: Sender<AudioEvent>,
) -> Result<Stream>
where
    T: SizedSample + FromSample<f32> + Copy,
{
    let mut resampler = BandLimitedResampler::new(SAMPLE_RATE, sample_rate);
    let mut last_output_sample = 0.0_f32;
    let stream_underruns = playback_underruns.clone();
    device
        .build_output_stream(
            config,
            move |data: &mut [T], info| {
                let stamp = info.timestamp();
                let playback_start = Instant::now() + stamp.playback.duration_since(stamp.callback);
                for (offset, target) in data.chunks_mut(channels).enumerate() {
                    let sample = loop {
                        if let Some(sample) = resampler.next() {
                            break sample;
                        }
                        let frame = rx.take_frame(
                            playback_start
                                + Duration::from_secs_f64(offset as f64 / sample_rate as f64),
                            last_output_sample,
                            &playback_underruns,
                        );
                        resampler.extend(&frame);
                    };
                    last_output_sample = sample;
                    let sample = T::from_sample(sample.clamp(-1.0, 1.0));
                    for channel in target {
                        *channel = sample;
                    }
                }
            },
            move |error| {
                if is_non_fatal_stream_error(&error) {
                    if error.kind() == cpal::ErrorKind::Xrun {
                        stream_underruns.fetch_add(1, Ordering::Relaxed);
                    }
                    tracing::debug!(error = %error, "non-fatal output stream event");
                    return;
                }
                let _ = errors.try_send(AudioEvent::DeviceError {
                    direction: Some(AudioDeviceDirection::Output),
                    message: format!("output device: {error}"),
                });
            },
            None,
        )
        .context("open output stream")
}

const UNDERRUN_FADE_SAMPLES: usize = FRAME_SAMPLES / 4;

fn smooth_underrun_frame(last_sample: f32) -> [f32; FRAME_SAMPLES] {
    let last_sample = if last_sample.is_finite() {
        last_sample.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    std::array::from_fn(|index| {
        if index >= UNDERRUN_FADE_SAMPLES {
            0.0
        } else {
            last_sample * (1.0 - (index + 1) as f32 / UNDERRUN_FADE_SAMPLES as f32)
        }
    })
}

fn is_non_fatal_stream_error(error: &cpal::Error) -> bool {
    matches!(
        error.kind(),
        cpal::ErrorKind::Xrun | cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::RealtimeDenied
    )
}

// Capture permissions apply to every source sample. On a mute boundary we
// discard only the partial frame and the resampler's buffered/history samples;
// new samples immediately begin a clean frame without a recovery timer.
struct CaptureFrameBuffer {
    frame: AudioFrame,
    position: usize,
    resampler: BandLimitedResampler,
    sample_rate: u32,
    muted: bool,
}
impl CaptureFrameBuffer {
    fn new(sample_rate: u32, muted: bool) -> Self {
        Self {
            frame: [0.0; FRAME_SAMPLES],
            position: 0,
            resampler: BandLimitedResampler::new(sample_rate, SAMPLE_RATE),
            sample_rate,
            muted,
        }
    }
    fn push(&mut self, sample: f32, muted: bool) {
        if muted != self.muted {
            *self = Self::new(self.sample_rate, muted);
        }
        self.resampler.push(sample);
    }
    fn next_frame(&mut self) -> Option<AudioFrame> {
        while let Some(sample) = self.resampler.next() {
            self.frame[self.position] = sample;
            self.position += 1;
            if self.position == FRAME_SAMPLES {
                self.position = 0;
                return Some(std::mem::replace(&mut self.frame, [0.0; FRAME_SAMPLES]));
            }
        }
        None
    }
}

struct BandLimitedResampler {
    input: VecDeque<f32>,
    output: VecDeque<f32>,
    source_block: Vec<f32>,
    destination_block: Vec<f32>,
    converter: Option<PushSincResampler>,
}

impl BandLimitedResampler {
    fn new(input_sample_rate: u32, output_sample_rate: u32) -> Self {
        assert!(input_sample_rate > 0);
        assert!(output_sample_rate > 0);
        if input_sample_rate == output_sample_rate {
            return Self {
                input: VecDeque::new(),
                output: VecDeque::with_capacity(FRAME_SAMPLES * 2),
                source_block: Vec::new(),
                destination_block: Vec::new(),
                converter: None,
            };
        }

        let divisor = greatest_common_divisor(input_sample_rate, output_sample_rate);
        let source_ratio = (input_sample_rate / divisor) as usize;
        let destination_ratio = (output_sample_rate / divisor) as usize;
        let target_source_frames = (input_sample_rate / 100).max(1) as usize;
        let scale = (target_source_frames / source_ratio).max(1);
        let source_frames = source_ratio * scale;
        let destination_frames = destination_ratio * scale;
        Self {
            input: VecDeque::with_capacity(source_frames * 2),
            output: VecDeque::with_capacity(destination_frames * 2),
            source_block: vec![0.0; source_frames],
            destination_block: vec![0.0; destination_frames],
            converter: Some(PushSincResampler::new(source_frames, destination_frames)),
        }
    }

    fn push(&mut self, sample: f32) {
        if self.converter.is_none() {
            self.output.push_back(sample);
            return;
        }
        self.input.push_back(sample);
        self.process_available_blocks();
    }

    fn extend(&mut self, samples: &[f32]) {
        if self.converter.is_none() {
            self.output.extend(samples.iter().copied());
            return;
        }
        self.input.extend(samples.iter().copied());
        self.process_available_blocks();
    }

    fn next(&mut self) -> Option<f32> {
        self.output.pop_front()
    }

    fn process_available_blocks(&mut self) {
        while self.input.len() >= self.source_block.len() {
            for sample in &mut self.source_block {
                *sample = self.input.pop_front().unwrap_or_default();
            }
            if let Some(converter) = &mut self.converter {
                converter.resample(&self.source_block, &mut self.destination_block);
            }
            self.output.extend(self.destination_block.iter().copied());
        }
    }
}

fn greatest_common_divisor(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left.max(1)
}

struct WorkerControl {
    muted: Arc<AtomicBool>,
    deafened: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    capture_overruns: Arc<AtomicU64>,
    playback_underruns: Arc<AtomicU64>,
}

enum WorkerCommand {
    MutedSpeechAnalysis(bool),
    UserVolume(u64, f32),
    UserLocalMute(u64, bool),
    MasterOutputVolume(f32),
    ApplyAdaptation(AudioAdaptationDecision),
    PlayOutputTest,
    StartMicTest,
    PlayMicTestOriginal,
    PlayMicTestProcessed,
    StopMicTest,
    ReplaceCaptureProcessor {
        processor: CaptureAudioProcessor,
        input_mode: InputMode,
    },
}

struct WorkerSettings {
    output_queue: OutputQueue,
    control: WorkerControl,
    input_gain: f32,
    output_volume: f32,
    input_mode: InputMode,
    echo_cancellation: bool,
    automatic_gain: bool,
    noise_suppression_mode: NoiseSuppressionMode,
    noise_suppression_strength: NoiseSuppressionStrength,
    commands: Receiver<WorkerCommand>,
}

#[derive(Clone, Copy)]
struct UserAudioSettings {
    volume: f32,
    muted: bool,
}

// The default master volume is 0.8, so 1.25 keeps ordinary remote speech at unity.
// Extra user-requested volume remains limiter-protected, but normal speech no longer enters
// the nonlinear limiter early enough to add audible harmonic distortion.
const VOICE_PLAYBACK_GAIN: f32 = 1.25;
const MAX_NETWORK_CATCHUP_FRAMES_PER_PEER: usize = 10;

fn voice_playback_gain(master_output_volume: f32) -> f32 {
    master_output_volume.clamp(0.0, 3.0) * VOICE_PLAYBACK_GAIN
}

impl Default for UserAudioSettings {
    fn default() -> Self {
        Self {
            volume: 1.0,
            muted: false,
        }
    }
}

fn run_worker(
    transport: Box<dyn VoiceTransport>,
    capture: Receiver<CapturedFrame>,
    playback: Sender<[f32; FRAME_SAMPLES]>,
    stale_playback: Receiver<[f32; FRAME_SAMPLES]>,
    events: Sender<AudioEvent>,
    settings: WorkerSettings,
) {
    let self_session_id = transport.session_id();
    let network_enabled = self_session_id != 0;
    let mut encoder = match OpusEncoder::new(DEFAULT_BITRATE) {
        Ok(value) => value,
        Err(error) => {
            let _ = events.try_send(AudioEvent::Error(format!("Opus encoder: {error}")));
            return;
        }
    };
    let mut current_bitrate = DEFAULT_BITRATE;
    let mut peers: HashMap<u64, RemotePeer> = HashMap::new();
    let mut pending_network = HashMap::<u64, VecDeque<VoicePacket>>::new();
    let mut pending_resync = HashSet::<u64>::new();
    let mut expired_network = HashSet::<u64>::new();
    let mut next_mix = Instant::now();
    let mut next_stats = Instant::now() + Duration::from_secs(1);
    let mut next_qoe_feedback = Instant::now() + Duration::from_millis(500);
    let mut qoe_baseline = HashMap::<u64, (u64, u64, u64, u64, u64, u64, u64, u64, u64)>::new();
    let mut next_level = Instant::now();
    let mut sent = 0u64;
    let mut received = 0u64;
    let mut archived_lost = 0u64;
    let mut archived_reordered = 0u64;
    let initial_settings = AudioProcessingSettings {
        input_gain: settings.input_gain,
        input_mode: settings.input_mode,
        echo_cancellation: settings.echo_cancellation,
        automatic_gain: settings.automatic_gain,
        noise_suppression_mode: settings.noise_suppression_mode,
        noise_suppression_strength: settings.noise_suppression_strength,
    };
    let pipeline = match CapturePipeline::spawn(
        initial_settings,
        capture,
        settings.output_queue.clone(),
        settings.control.alive.clone(),
        settings.control.muted.clone(),
        events.clone(),
    ) {
        Ok(pipeline) => pipeline,
        Err(error) => {
            let _ = events.try_send(AudioEvent::Error(format!("capture worker: {error}")));
            return;
        }
    };
    let mut capture_status: Option<CaptureStatus> = None;
    let mut timing = AudioTiming::default();
    let mut worker_times = TimingHistogram::default();
    let mut playout_lateness = TimingHistogram::default();
    let mut interval_sent = 0_u64;
    let mut interval_fec = 0_u64;
    let mut capture_times = TimingHistogram::default();
    let mut previous_dsp_metrics = DspMetrics::default();
    let mut performance_guard = DspPerformanceGuard::default();
    let mut user_audio = HashMap::<u64, UserAudioSettings>::new();
    let mut master_output_volume = settings.output_volume.clamp(0.01, 3.0);
    let mut test_tone = TestTone::default();
    let mut mic_test = MicTest::default();
    let mut onset = VoiceOnsetCache::default();
    let mut muted_speech_analysis = false;
    let mut output_rms = 0.0_f32;
    // Keep capacity across 20 ms mix cycles; grow only when more peers speak.
    let mut mix_frames = Vec::with_capacity(6);
    while settings.control.alive.load(Ordering::Acquire) {
        let worker_started = Instant::now();
        while let Ok(command) = settings.commands.try_recv() {
            match command {
                WorkerCommand::MutedSpeechAnalysis(enabled) => {
                    muted_speech_analysis = enabled;
                }
                WorkerCommand::UserVolume(session_id, volume) => {
                    user_audio.entry(session_id).or_default().volume = volume.clamp(0.0, 3.0);
                }
                WorkerCommand::UserLocalMute(session_id, muted) => {
                    user_audio.entry(session_id).or_default().muted = muted;
                }
                WorkerCommand::MasterOutputVolume(volume) => {
                    master_output_volume = volume.clamp(0.01, 3.0);
                }
                WorkerCommand::ApplyAdaptation(decision) => {
                    match encoder.apply_adaptation(decision) {
                        Ok(()) => current_bitrate = decision.target_bitrate_bps,
                        Err(error) => {
                            let _ = events.try_send(AudioEvent::Error(format!(
                                "Opus adaptation failed: {error}"
                            )));
                        }
                    }
                }
                WorkerCommand::PlayOutputTest => test_tone.start(),
                WorkerCommand::StartMicTest => {
                    onset.clear();
                    mic_test.start();
                    let _ = events.try_send(AudioEvent::MicTestStatus(MicTestStatus::Recording));
                }
                WorkerCommand::PlayMicTestOriginal => {
                    if mic_test.play_original() {
                        let _ = events
                            .try_send(AudioEvent::MicTestStatus(MicTestStatus::PlayingOriginal));
                    }
                }
                WorkerCommand::PlayMicTestProcessed => {
                    if mic_test.play_processed() {
                        let _ = events
                            .try_send(AudioEvent::MicTestStatus(MicTestStatus::PlayingProcessed));
                    }
                }
                WorkerCommand::StopMicTest => {
                    mic_test.stop();
                    let _ = events.try_send(AudioEvent::MicTestStatus(mic_test.status()));
                }
                WorkerCommand::ReplaceCaptureProcessor {
                    processor,
                    input_mode: new_input_mode,
                } => {
                    onset.clear();
                    pipeline.replace(processor, new_input_mode);
                }
            }
        }
        if !network_enabled
            || mic_test.blocks_transmission()
            || settings.control.muted.load(Ordering::Relaxed)
        {
            onset.clear();
        }
        let analyze_muted = muted_speech_analysis
            && network_enabled
            && mic_test.status() == MicTestStatus::Idle
            && settings.control.muted.load(Ordering::Acquire);
        pipeline
            .muted_speech_analysis
            .store(analyze_muted, Ordering::Release);
        pipeline.unused.store(
            capture_output_unused(
                settings.control.muted.load(Ordering::Relaxed) && !analyze_muted,
                mic_test.status(),
                false,
            ),
            Ordering::Relaxed,
        );
        // DSP work is isolated; only completed, fresh frames enter this thread.
        while Instant::now() < next_mix {
            let Ok(result) = pipeline.results.try_recv() else {
                break;
            };
            if capture_status.is_none_or(|old| old.generation != result.status.generation) {
                onset.clear();
                previous_dsp_metrics = DspMetrics::default();
                performance_guard = DspPerformanceGuard::default();
            }
            capture_status = Some(result.status);
            timing.capture_age_ms = result.captured_at.elapsed().as_secs_f64() * 1000.0;
            capture_times.record(result.processing_time);
            if result.queued_at.elapsed() > Duration::from_millis(120) {
                onset.clear();
                pipeline.stale_drops.fetch_add(1, Ordering::Relaxed);
                continue;
            }

            let original_frame = result.original;
            let frame = result.transmit_frame;
            let analysis = result.analysis;
            let input_mode = result.input_mode;
            if mic_test.record(original_frame, result.frame) {
                let _ = events.try_send(AudioEvent::MicTestStatus(MicTestStatus::PreviewReady));
            }
            if Instant::now() >= next_level {
                let _ = events.try_send(AudioEvent::InputLevel(analysis.level.rms));
                let _ = events.try_send(AudioEvent::InputPeak(analysis.level.peak));
                let _ = events.try_send(AudioEvent::OutputLevel(output_rms));
                next_level = Instant::now() + Duration::from_millis(100);
            }
            let allowed = network_enabled
                // `ptt_pressed` carries the DSP timing permission, including
                // capture-time mute, in both VAD and PTT modes.
                && result.ptt_pressed
                && !mic_test.blocks_transmission()
                && !settings.control.muted.load(Ordering::Relaxed);
            let send_current = allowed
                && should_send_capture(
                    false,
                    input_mode,
                    analysis.voice_active,
                    result.ptt_pressed,
                );
            let pre_roll = onset.process(
                &frame,
                result.captured_at,
                input_mode,
                allowed,
                analysis.voice_active,
            );
            if send_current {
                // Preserve soft consonants before VAD opens. Flush the bounded
                // pre-roll now; never hold an already active capture frame.
                for samples in pre_roll.iter().chain(std::iter::once(&frame)) {
                    if let Ok(payload) = encode_final_capture(&mut encoder, samples)
                        && !settings.control.muted.load(Ordering::Acquire)
                        && transport.send_audio(payload).is_ok()
                    {
                        sent = sent.saturating_add(1);
                        interval_sent += 1;
                        interval_fec += u64::from(crate::codec::packet_has_voice_fec(payload));
                        let _ = events.try_send(AudioEvent::Talking(self_session_id));
                    }
                }
            }
        }
        while Instant::now() < next_mix {
            let Some(received_audio) = transport.try_receive_audio() else {
                break;
            };
            let packet = match received_audio {
                ReceivedAudio::Packet { packet, .. } => {
                    expired_network.remove(&packet.session_id);
                    packet
                }
                ReceivedAudio::Expired { session_id } => {
                    discard_expired_network_audio(
                        session_id,
                        &mut pending_network,
                        &mut pending_resync,
                        &mut expired_network,
                        &mut peers,
                    );
                    timing.receive_stale_drops = timing.receive_stale_drops.saturating_add(1);
                    continue;
                }
            };
            if packet.kind != PacketType::Audio {
                continue;
            }
            let session_id = packet.session_id;
            queue_network_packet(&mut pending_network, &mut pending_resync, packet);
            received = received.saturating_add(1);
            let _ = events.try_send(AudioEvent::Talking(session_id));
        }
        for (&session_id, packets) in &mut pending_network {
            if packets.is_empty() {
                continue;
            }
            if let std::collections::hash_map::Entry::Vacant(entry) = peers.entry(session_id) {
                match RemotePeer::new(session_id) {
                    Ok(peer) => {
                        entry.insert(peer);
                    }
                    Err(error) => {
                        let _ =
                            events.try_send(AudioEvent::Error(format!("Opus decoder: {error}")));
                        continue;
                    }
                }
            }
            if let Some(peer) = peers.get_mut(&session_id) {
                if pending_resync.remove(&session_id) {
                    peer.resync_playout();
                }
                while let Some(packet) = packets.pop_front() {
                    peer.push(packet.timestamp, packet.payload);
                }
            }
        }
        if Instant::now() >= next_mix {
            let mix_started = Instant::now();
            playout_lateness.record(mix_started.saturating_duration_since(next_mix));
            if mix_started.saturating_duration_since(next_mix)
                >= crate::transport::MAX_RECEIVED_AUDIO_AGE
            {
                // Receive draining is deadline-bounded, so after a long pause
                // this branch runs before the expired datagrams can be read.
                // The producer may also have evicted them already. Clear old
                // decoded media before emitting the first post-stall frame.
                for (&session_id, peer) in &mut peers {
                    if expired_network.insert(session_id) {
                        peer.resync_playout();
                    }
                }
                pending_network.clear();
                pending_resync.clear();
                for _ in 0..stale_playback.len() {
                    let _ = stale_playback.try_recv();
                }
            }
            let missed_deadline =
                mix_started.saturating_duration_since(next_mix) >= Duration::from_millis(20);
            mix_frames.clear();
            mix_frames.extend(peers.iter_mut().filter_map(|(session_id, peer)| {
                let mut frame = peer.next_frame()?.samples;
                let settings = user_audio.get(session_id).copied().unwrap_or_default();
                apply_user_audio_settings(&mut frame, settings);
                Some(frame)
            }));
            let mut output = [0.0; FRAME_SAMPLES];
            if let Some(test_frame) = mic_test.next_playback_frame() {
                output = test_frame;
                crate::dsp::apply_gain(&mut output, master_output_volume);
            } else if !settings.control.deafened.load(Ordering::Relaxed) {
                mixer::mix_iter(
                    mix_frames.iter().map(|frame| frame.as_slice()),
                    &mut output,
                    voice_playback_gain(master_output_volume),
                );
            }
            if mic_test.take_finished() {
                let _ = events.try_send(AudioEvent::MicTestStatus(MicTestStatus::PreviewReady));
            }
            test_tone.mix_into(&mut output, master_output_volume);
            output_rms = frame_rms(&output);
            let _ = send_latest(&playback, &stale_playback, output);
            let cadence = playback_cadence(playback.len());
            next_mix = if missed_deadline {
                // After a long scheduler pause, skip obsolete deadlines rather
                // than bursting stale frames into the hardware queue.
                mix_started + cadence
            } else {
                next_mix + cadence
            };
        }
        if Instant::now() >= next_stats {
            peers.retain(|_, peer| {
                if peer.is_stale() {
                    archived_lost = archived_lost.saturating_add(peer.packets_lost());
                    archived_reordered =
                        archived_reordered.saturating_add(peer.packets_reordered());
                    false
                } else {
                    true
                }
            });
            pending_network.retain(|session_id, _| peers.contains_key(session_id));
            pending_resync.retain(|session_id| peers.contains_key(session_id));
            expired_network.retain(|session_id| peers.contains_key(session_id));
            let lost =
                archived_lost.saturating_add(peers.values().map(RemotePeer::packets_lost).sum());
            let reordered = archived_reordered
                .saturating_add(peers.values().map(RemotePeer::packets_reordered).sum());
            let jitter_depth = peers.values().map(RemotePeer::jitter_depth).sum();
            let _ = events.try_send(AudioEvent::Stats {
                sent,
                received,
                lost,
                reordered,
                jitter_depth,
                opus_bitrate_bps: current_bitrate,
            });
            let _ = events.try_send(AudioEvent::VoicePathMetrics(transport.voice_path_metrics()));
            let _ = events.try_send(AudioEvent::QueueStats {
                capture_overruns: settings.control.capture_overruns.load(Ordering::Relaxed),
                playback_underruns: settings.control.playback_underruns.load(Ordering::Relaxed),
            });
            if let Some(status) = capture_status {
                let metrics = status.metrics;
                let interval = DspMetricInterval::between(previous_dsp_metrics, metrics);
                previous_dsp_metrics = metrics;
                let _ = events.try_send(AudioEvent::DspStatus {
                    mode: status.mode,
                    echo_cancellation: status.echo_cancellation,
                    automatic_gain: status.automatic_gain,
                    average_ms: metrics.average_processing_time().as_secs_f32() * 1000.0,
                    peak_ms: metrics.peak_processing_time.as_secs_f32() * 1000.0,
                    deadline_misses: metrics.deadline_misses,
                });
                let _ = events.try_send(AudioEvent::DspPerformanceWarning(
                    performance_guard.observe(status.mode, interval),
                ));
                timing.dsp_silence_frames = status.silence_frames;
                timing.aec_delay_ms = status.aec_delay_ms;
            }
            timing.playback_queue_ms = playback.len() as f64 * 20.0;
            timing.actual_jitter_ms = peers
                .values()
                .map(|peer| peer.actual_playout_delay_ms())
                .fold(0.0, f64::max);
            timing.playout_lateness_p99_ms = playout_lateness.percentile(99);
            timing.sent_fec_percent = 100.0 * interval_fec as f64 / interval_sent.max(1) as f64;
            playout_lateness = TimingHistogram::default();
            interval_sent = 0;
            interval_fec = 0;
            timing.worker_p95_ms = worker_times.percentile(95);
            timing.worker_p99_ms = worker_times.percentile(99);
            timing.capture_processing_p95_ms = capture_times.percentile(95);
            timing.capture_stale_drops = pipeline.stale_drops.load(Ordering::Relaxed);
            timing.render_reference_drops = pipeline.reference_drops.load(Ordering::Relaxed);
            let _ = events.try_send(AudioEvent::Timing(timing.clone()));
            worker_times = TimingHistogram::default();
            capture_times = TimingHistogram::default();
            next_stats += Duration::from_secs(1);
        }
        if Instant::now() >= next_qoe_feedback {
            for (&peer_id, peer) in &mut peers {
                let (window_bursts, window_max_consecutive_loss) = peer.take_loss_window();
                let current = (
                    peer.packets_received,
                    peer.packets_lost(),
                    peer.packets_late(),
                    peer.packets_reordered(),
                    peer.playout_resyncs(),
                    peer.jitter_buffer_underruns(),
                    peer.plc_frames(),
                    peer.fec_recovered_packets(),
                    peer.decode_errors,
                );
                // Counters are lifetime values. Establishing the baseline is
                // not a QoE sample: treating all prior packets as a 500 ms
                // window makes a past glitch look like a current outage.
                let Some(previous) = qoe_baseline.insert(peer_id, current) else {
                    continue;
                };
                let received_delta = current.0.saturating_sub(previous.0);
                let lost_delta = current.1.saturating_sub(previous.1);
                let late_delta = current.2.saturating_sub(previous.2);
                let reordered_delta = current.3.saturating_sub(previous.3);
                let resync_delta = current.4.saturating_sub(previous.4);
                let underrun_delta = current.5.saturating_sub(previous.5);
                let plc_delta = current.6.saturating_sub(previous.6);
                let fec_delta = current.7.saturating_sub(previous.7);
                let decode_error_delta = current.8.saturating_sub(previous.8);
                let observed_packets = received_delta.saturating_add(lost_delta);
                if !qoe_window_has_evidence(received_delta, lost_delta) {
                    continue;
                }
                let expected = observed_packets.max(1) as f64;
                let transport_loss = 100.0 * lost_delta as f64 / expected;
                let late_loss = 100.0 * late_delta as f64 / expected;
                let effective_loss =
                    100.0 * plc_delta.saturating_add(decode_error_delta) as f64 / expected;
                let path_metrics = transport.network_metrics(peer_id);
                let (rtt_ms, jitter_ms) = path_metrics.unwrap_or_default();
                let _ = events.try_send(AudioEvent::QoeFeedback(
                    crate::protocol::control::AudioQoeFeedback {
                        peer_id,
                        observed_packets: Some(observed_packets),
                        path_metrics_available: Some(path_metrics.is_some()),
                        window_ms: 500,
                        transport_loss_percent: transport_loss,
                        late_loss_percent: late_loss,
                        effective_audio_loss_percent: effective_loss.min(100.0),
                        burst_loss_percent: if window_max_consecutive_loss > 1 {
                            effective_loss.min(100.0)
                        } else {
                            0.0
                        },
                        burst_count: window_bursts,
                        max_consecutive_loss: window_max_consecutive_loss,
                        packet_reorder_percent: 100.0 * reordered_delta as f64 / expected,
                        jitter_ms,
                        rtt_ms,
                        jitter_buffer_delay_ms: peer.jitter_target_delay_ms(),
                        jitter_buffer_underruns: underrun_delta,
                        plc_frames: plc_delta,
                        fec_recovered_packets: fec_delta,
                        playout_resync_count: resync_delta,
                    },
                ));
            }
            qoe_baseline.retain(|peer_id, _| peers.contains_key(peer_id));
            next_qoe_feedback += Duration::from_millis(500);
        }
        worker_times.record(worker_started.elapsed());
        thread::sleep(Duration::from_millis(1));
    }
}

fn qoe_window_has_evidence(received: u64, lost: u64) -> bool {
    // Short silent/talk-tail windows cannot prove recovery; any measured loss
    // remains actionable even when the window contains very few packets.
    received >= 5 || lost > 0
}

fn discard_expired_network_audio(
    session_id: u64,
    pending: &mut HashMap<u64, VecDeque<VoicePacket>>,
    resync: &mut HashSet<u64>,
    expired: &mut HashSet<u64>,
    peers: &mut HashMap<u64, RemotePeer>,
) {
    pending.remove(&session_id);
    resync.remove(&session_id);
    // Reset once for a run of expired packets; a following fresh packet starts
    // from a clean decoder without replaying pre-stall jitter/PCM contents.
    if expired.insert(session_id)
        && let Some(peer) = peers.get_mut(&session_id)
    {
        peer.resync_playout();
    }
}

fn queue_network_packet(
    pending: &mut HashMap<u64, VecDeque<VoicePacket>>,
    resync: &mut HashSet<u64>,
    packet: VoicePacket,
) {
    let session_id = packet.session_id;
    let queued = pending.entry(session_id).or_default();
    if queued.len() >= MAX_NETWORK_CATCHUP_FRAMES_PER_PEER {
        queued.pop_front();
        resync.insert(session_id);
    }
    queued.push_back(packet);
}

fn frame_rms(frame: &AudioFrame) -> f32 {
    (frame.iter().map(|sample| sample * sample).sum::<f32>() / frame.len() as f32).sqrt()
}

fn capture_output_unused(muted: bool, test: MicTestStatus, transitioning: bool) -> bool {
    muted && test == MicTestStatus::Idle && !transitioning
}

fn encode_final_capture<'a>(
    encoder: &'a mut OpusEncoder,
    processed: &AudioFrame,
) -> audiopus::Result<&'a [u8]> {
    encoder.encode(processed)
}

const PROCESSOR_TRANSITION_FRAMES: usize = 5;

fn crossfade_processor_transition(
    previous: &AudioFrame,
    next: &mut AudioFrame,
    transition_frame: usize,
) {
    let start = transition_frame.min(PROCESSOR_TRANSITION_FRAMES - 1) * FRAME_SAMPLES;
    let total = PROCESSOR_TRANSITION_FRAMES * FRAME_SAMPLES;
    for (index, (previous, next)) in previous.iter().copied().zip(next.iter_mut()).enumerate() {
        let mix = (start + index + 1) as f32 / total as f32;
        *next = (previous * (1.0 - mix) + *next * mix).clamp(-1.0, 1.0);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct DspMetricInterval {
    average_ms: f32,
    deadline_misses: u64,
}

impl DspMetricInterval {
    fn between(previous: DspMetrics, current: DspMetrics) -> Self {
        let frames = current
            .processed_frames
            .saturating_sub(previous.processed_frames);
        let duration = current
            .total_processing_time
            .saturating_sub(previous.total_processing_time);
        Self {
            average_ms: if frames == 0 {
                0.0
            } else {
                duration.as_secs_f32() * 1_000.0 / frames as f32
            },
            deadline_misses: current
                .deadline_misses
                .saturating_sub(previous.deadline_misses),
        }
    }
}

#[derive(Default)]
struct DspPerformanceGuard {
    consecutive_pressure_intervals: u8,
}

impl DspPerformanceGuard {
    fn observe(
        &mut self,
        mode: NoiseSuppressionMode,
        interval: DspMetricInterval,
    ) -> Option<String> {
        let pressure = interval.average_ms >= 10.0 || interval.deadline_misses >= 3;
        if pressure {
            self.consecutive_pressure_intervals =
                self.consecutive_pressure_intervals.saturating_add(1);
        } else {
            self.consecutive_pressure_intervals = 0;
        }
        if self.consecutive_pressure_intervals < 3 {
            return None;
        }
        Some(format!(
            "{} is using significant processing time ({:.1} ms average, {} deadline misses in the last interval). Consider a lighter mode if audio becomes unstable.",
            match mode {
                NoiseSuppressionMode::Off => "Audio processing",
                NoiseSuppressionMode::Standard => "Standard noise suppression",
                NoiseSuppressionMode::RnNoise => "RNNoise",
                NoiseSuppressionMode::DeepFilterNet => "DeepFilterNet natural",
                NoiseSuppressionMode::DeepFilterNetStrong => "DeepFilterNet strong",
            },
            interval.average_ms,
            interval.deadline_misses,
        ))
    }
}

#[derive(Default)]
struct TestTone {
    sample_index: usize,
    samples_remaining: usize,
}

const MIC_TEST_MAX_FRAMES: usize = SAMPLE_RATE as usize * 10 / FRAME_SAMPLES;

struct MicTest {
    status: MicTestStatus,
    original: Vec<AudioFrame>,
    processed: Vec<AudioFrame>,
    playback_index: usize,
    finished: bool,
}

impl Default for MicTest {
    fn default() -> Self {
        Self {
            status: MicTestStatus::Idle,
            original: Vec::new(),
            processed: Vec::new(),
            playback_index: 0,
            finished: false,
        }
    }
}

impl MicTest {
    fn start(&mut self) {
        self.status = MicTestStatus::Recording;
        self.original = Vec::with_capacity(MIC_TEST_MAX_FRAMES);
        self.processed = Vec::with_capacity(MIC_TEST_MAX_FRAMES);
        self.playback_index = 0;
        self.finished = false;
    }

    fn stop(&mut self) {
        self.status = match self.status {
            MicTestStatus::PlayingOriginal | MicTestStatus::PlayingProcessed => {
                MicTestStatus::PreviewReady
            }
            MicTestStatus::Recording if !self.original.is_empty() => MicTestStatus::PreviewReady,
            _ => {
                self.original.clear();
                self.processed.clear();
                MicTestStatus::Idle
            }
        };
        self.playback_index = 0;
        self.finished = false;
    }

    fn status(&self) -> MicTestStatus {
        self.status
    }

    fn blocks_transmission(&self) -> bool {
        matches!(
            self.status,
            MicTestStatus::Recording
                | MicTestStatus::PlayingOriginal
                | MicTestStatus::PlayingProcessed
        )
    }

    fn record(&mut self, original: AudioFrame, processed: AudioFrame) -> bool {
        if self.status != MicTestStatus::Recording {
            return false;
        }
        self.original.push(original);
        self.processed.push(processed);
        if self.processed.len() < MIC_TEST_MAX_FRAMES {
            return false;
        }
        self.status = MicTestStatus::PreviewReady;
        true
    }

    fn play_original(&mut self) -> bool {
        self.start_playback(MicTestStatus::PlayingOriginal)
    }

    fn play_processed(&mut self) -> bool {
        self.start_playback(MicTestStatus::PlayingProcessed)
    }

    fn start_playback(&mut self, status: MicTestStatus) -> bool {
        if self.original.is_empty() || self.original.len() != self.processed.len() {
            return false;
        }
        self.status = status;
        self.playback_index = 0;
        self.finished = false;
        true
    }

    fn next_playback_frame(&mut self) -> Option<AudioFrame> {
        let frames = match self.status {
            MicTestStatus::PlayingOriginal => &self.original,
            MicTestStatus::PlayingProcessed => &self.processed,
            _ => return None,
        };
        let frame = frames.get(self.playback_index).copied();
        self.playback_index += usize::from(frame.is_some());
        if self.playback_index == frames.len() {
            self.status = MicTestStatus::PreviewReady;
            self.playback_index = 0;
            self.finished = true;
        }
        frame
    }

    fn take_finished(&mut self) -> bool {
        std::mem::take(&mut self.finished)
    }
}

impl TestTone {
    const FREQUENCY_HZ: f32 = 440.0;
    const DURATION_SAMPLES: usize = SAMPLE_RATE as usize / 2;

    fn start(&mut self) {
        self.sample_index = 0;
        self.samples_remaining = Self::DURATION_SAMPLES;
    }

    fn mix_into(&mut self, frame: &mut [f32; FRAME_SAMPLES], volume: f32) {
        use std::f32::consts::TAU;

        let gain = volume.clamp(0.0, 3.0) * 0.15;
        for sample in frame.iter_mut().take(self.samples_remaining) {
            let phase = self.sample_index as f32 * Self::FREQUENCY_HZ * TAU / SAMPLE_RATE as f32;
            *sample = (*sample + phase.sin() * gain).clamp(-1.0, 1.0);
            self.sample_index += 1;
            self.samples_remaining -= 1;
        }
    }
}

fn create_capture_processor(
    settings: AudioProcessingSettings,
) -> Result<CaptureAudioProcessor, String> {
    match settings.noise_suppression_mode {
        NoiseSuppressionMode::Standard => CaptureAudioProcessor::standard(
            settings.input_gain,
            true,
            true,
            settings.noise_suppression_strength,
        ),
        NoiseSuppressionMode::Off => Ok(CaptureAudioProcessor::off(
            settings.input_gain,
            settings.echo_cancellation,
            settings.automatic_gain,
        )),
        NoiseSuppressionMode::RnNoise => Ok(CaptureAudioProcessor::rnnoise(
            settings.input_gain,
            settings.echo_cancellation,
            settings.automatic_gain,
        )),
        NoiseSuppressionMode::DeepFilterNet => CaptureAudioProcessor::deep_filter_net(
            settings.input_gain,
            settings.echo_cancellation,
            settings.automatic_gain,
            false,
        ),
        NoiseSuppressionMode::DeepFilterNetStrong => CaptureAudioProcessor::deep_filter_net(
            settings.input_gain,
            settings.echo_cancellation,
            settings.automatic_gain,
            true,
        ),
    }
}

fn apply_user_audio_settings(frame: &mut [f32; FRAME_SAMPLES], settings: UserAudioSettings) {
    if settings.muted {
        frame.fill(0.0);
    } else {
        crate::dsp::apply_gain(frame, settings.volume);
    }
}

#[derive(Default)]
struct VoiceOnsetCache {
    previous: VecDeque<(AudioFrame, Instant)>,
    invalid_before: Option<Instant>,
}

impl VoiceOnsetCache {
    fn clear(&mut self) {
        self.previous.clear();
        self.invalid_before = Some(Instant::now());
    }

    fn process(
        &mut self,
        frame: &AudioFrame,
        captured_at: Instant,
        input_mode: InputMode,
        allowed: bool,
        voice_active: bool,
    ) -> Vec<AudioFrame> {
        if !allowed || input_mode != InputMode::VoiceActivity {
            self.clear();
            return Vec::new();
        }
        if self
            .invalid_before
            .is_some_and(|boundary| captured_at < boundary)
        {
            return Vec::new();
        }
        if self.previous.back().is_some_and(|(_, at)| {
            captured_at <= *at || captured_at.duration_since(*at) >= Duration::from_millis(40)
        }) {
            self.previous.clear();
        }
        // At most 60 ms of contiguous capture. Never replay stale frames across
        // mute, device/DSP changes, callback drops or a mode change.
        while self
            .previous
            .front()
            .is_some_and(|(_, at)| captured_at.duration_since(*at) > Duration::from_millis(80))
        {
            self.previous.pop_front();
        }
        if voice_active {
            self.previous
                .drain(..)
                .map(|(samples, _)| samples)
                .collect()
        } else {
            if self.previous.len() == 3 {
                self.previous.pop_front();
            }
            self.previous.push_back((*frame, captured_at));
            Vec::new()
        }
    }
}

fn should_send_capture(
    muted: bool,
    input_mode: InputMode,
    voice_active: bool,
    push_to_talk_pressed: bool,
) -> bool {
    if muted {
        return false;
    }
    match input_mode {
        InputMode::VoiceActivity => voice_active,
        InputMode::PushToTalk => push_to_talk_pressed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unmute_discards_partial_frame_and_resampler_tail_then_resumes_first_clean_frame() {
        for sample_rate in [16_000, 44_100, 48_000, 96_000] {
            let mut capture = CaptureFrameBuffer::new(sample_rate, true);
            for _ in 0..(sample_rate / 75) {
                capture.push(0.7, true);
                while capture.next_frame().is_some() {}
            }
            let mut clean = CaptureFrameBuffer::new(sample_rate, false);
            let mut first_frame_samples = None;
            let mut frames = 0;
            // A soft first syllable must be identical to a clean capture start.
            for i in 0..(sample_rate / 10) {
                let sample =
                    0.003 * (i as f32 * 440.0 * std::f32::consts::TAU / sample_rate as f32).sin();
                capture.push(sample, false);
                clean.push(sample, false);
                let actual = capture.next_frame();
                let expected = clean.next_frame();
                assert_eq!(
                    actual, expected,
                    "{sample_rate} Hz retained muted resampler history"
                );
                if let Some(frame) = actual {
                    first_frame_samples.get_or_insert(i + 1);
                    frames += 1;
                    assert!(
                        frame_rms(&frame) > 0.001,
                        "soft first syllable was suppressed"
                    );
                    assert!(frame.iter().all(|sample| sample.abs() < 0.01));
                }
            }
            assert_eq!(frames, 5);
            assert_eq!(
                first_frame_samples,
                Some(sample_rate / 50),
                "extra unmute delay"
            );
        }
    }

    #[test]
    fn mute_closes_shared_capture_gate_before_controller_dequeues_commands() {
        let (commands, queued) = unbounded();
        let (_, events) = bounded(1);
        let muted = Arc::new(AtomicBool::new(false));
        let controller = AudioController {
            muted: muted.clone(),
            commands,
            events,
        };
        controller.set_muted(true);
        assert!(muted.load(Ordering::Acquire));
        assert!(
            queued.is_empty(),
            "closing capture must not depend on controller scheduling"
        );
    }

    #[test]
    fn expired_receive_backlog_clears_only_that_speakers_pending_and_playout_audio() {
        let mut encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
        let payload = encoder.encode(&[0.1; FRAME_SAMPLES]).unwrap().to_vec();
        let mut peers = HashMap::new();
        for id in [2, 3] {
            let mut peer = RemotePeer::new(id).unwrap();
            for n in 0..3 {
                peer.push(n * FRAME_SAMPLES as u32, payload.clone());
            }
            peer.next_frame().unwrap();
            peers.insert(id, peer);
        }
        let mut pending = HashMap::from([(2, VecDeque::new()), (3, VecDeque::new())]);
        let mut resync = HashSet::from([2]);
        let mut expired = HashSet::new();
        for _ in 0..4 {
            discard_expired_network_audio(2, &mut pending, &mut resync, &mut expired, &mut peers);
        }
        assert!(!pending.contains_key(&2));
        assert!(pending.contains_key(&3));
        assert!(!resync.contains(&2));
        assert_eq!(peers[&2].playout_resyncs(), 1);
        assert_eq!(peers[&2].actual_playout_delay_ms(), 0.0);
        assert!(peers.get_mut(&2).unwrap().next_frame().is_none());
        assert!(peers.get_mut(&3).unwrap().next_frame().is_some());
        let peer = peers.get_mut(&2).unwrap();
        for n in 100..103 {
            peer.push(n * FRAME_SAMPLES as u32, payload.clone());
        }
        assert_eq!(
            peer.next_frame().unwrap().outcome,
            crate::peer::DecodeOutcome::Normal
        );
        assert_eq!(peer.packets_lost(), 0);
    }
    #[test]
    fn onset_keeps_last_sixty_ms_and_never_delays_active_frames() {
        let mut cache = VoiceOnsetCache::default();
        let at = Instant::now();
        for n in 0..5 {
            assert!(
                cache
                    .process(
                        &[n as f32; FRAME_SAMPLES],
                        at + Duration::from_millis(n * 20),
                        InputMode::VoiceActivity,
                        true,
                        false
                    )
                    .is_empty()
            );
        }
        assert_eq!(
            cache.process(
                &[9.0; FRAME_SAMPLES],
                at + Duration::from_millis(100),
                InputMode::VoiceActivity,
                true,
                true
            ),
            vec![
                [2.0; FRAME_SAMPLES],
                [3.0; FRAME_SAMPLES],
                [4.0; FRAME_SAMPLES]
            ]
        );
        assert!(
            cache
                .process(
                    &[10.0; FRAME_SAMPLES],
                    at + Duration::from_millis(120),
                    InputMode::VoiceActivity,
                    true,
                    true
                )
                .is_empty()
        );
    }

    #[test]
    fn onset_discards_stale_muted_and_push_to_talk_audio() {
        let at = Instant::now();
        for (mode, allowed) in [
            (InputMode::VoiceActivity, false),
            (InputMode::PushToTalk, true),
        ] {
            let mut cache = VoiceOnsetCache::default();
            cache.process(
                &[1.0; FRAME_SAMPLES],
                at,
                InputMode::VoiceActivity,
                true,
                false,
            );
            cache.process(
                &[2.0; FRAME_SAMPLES],
                at + Duration::from_millis(20),
                mode,
                allowed,
                false,
            );
            assert!(
                cache
                    .process(
                        &[3.0; FRAME_SAMPLES],
                        at + Duration::from_millis(40),
                        InputMode::VoiceActivity,
                        true,
                        true
                    )
                    .is_empty()
            );
        }
        let mut cache = VoiceOnsetCache::default();
        cache.process(
            &[1.0; FRAME_SAMPLES],
            at,
            InputMode::VoiceActivity,
            true,
            false,
        );
        assert!(
            cache
                .process(
                    &[2.0; FRAME_SAMPLES],
                    at + Duration::from_millis(40),
                    InputMode::VoiceActivity,
                    true,
                    true
                )
                .is_empty()
        );
        cache.clear();
        cache.process(
            &[1.0; FRAME_SAMPLES],
            at,
            InputMode::VoiceActivity,
            true,
            false,
        );
        assert!(
            cache
                .process(
                    &[2.0; FRAME_SAMPLES],
                    at + Duration::from_millis(20),
                    InputMode::VoiceActivity,
                    true,
                    true
                )
                .is_empty()
        );
    }

    #[test]
    fn qoe_requires_real_traffic_for_healthy_feedback() {
        assert!(!qoe_window_has_evidence(0, 0));
        assert!(!qoe_window_has_evidence(4, 0));
        assert!(qoe_window_has_evidence(5, 0));
        assert!(qoe_window_has_evidence(0, 1));
    }

    #[test]
    fn failed_device_start_cancels_worker_and_releases_transport() {
        // Same ownership order as AudioEngine::start, without opening hardware.
        for fail_output in [false, true] {
            let (released_tx, released_rx) = bounded(1);
            struct Resource(Sender<()>);
            impl Drop for Resource {
                fn drop(&mut self) {
                    let _ = self.0.send(());
                }
            }
            let mut worker = None;
            let result: Result<()> = (|| {
                let alive = Arc::new(AtomicBool::new(true));
                let _lifetime = AudioWorkerLifetime(Arc::clone(&alive));
                worker = Some(thread::spawn(move || {
                    let _transport = Resource(released_tx);
                    while alive.load(Ordering::Acquire) {
                        thread::sleep(Duration::from_millis(1));
                    }
                }));
                if !fail_output {
                    anyhow::bail!("input play failed");
                }
                anyhow::bail!("output play failed");
            })();
            assert!(result.is_err());
            released_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("worker resource leaked");
            worker.unwrap().join().unwrap();
        }
    }

    #[test]
    fn unused_capture_only_for_explicit_mute_without_test_or_transition() {
        assert!(capture_output_unused(true, MicTestStatus::Idle, false));
        assert!(!capture_output_unused(false, MicTestStatus::Idle, false));
        assert!(!capture_output_unused(
            true,
            MicTestStatus::Recording,
            false
        ));
        assert!(!capture_output_unused(
            true,
            MicTestStatus::PreviewReady,
            false
        ));
        assert!(!capture_output_unused(true, MicTestStatus::Idle, true));
    }

    use crate::{
        codec::{OpusDecoder, OpusEncoder},
        peer::RemotePeer,
    };

    #[test]
    fn push_to_talk_release_is_independent_of_the_saturated_command_queue() {
        let pressed = AtomicBool::new(true);
        let (commands, _worker) = bounded(1);
        commands.send(WorkerCommand::PlayOutputTest).unwrap();
        assert!(commands.try_send(WorkerCommand::StopMicTest).is_err());

        pressed.store(false, Ordering::Release);

        assert!(!pressed.load(Ordering::Acquire));
    }

    #[test]
    fn audio_config_prefers_internal_rate_and_falls_back_to_closest_device_rate() {
        let bluetooth_only = cpal::SupportedStreamConfigRange::new(
            1,
            24_000,
            24_000,
            cpal::SupportedBufferSize::Unknown,
            SampleFormat::F32,
        );
        let selected = choose_config([bluetooth_only].into_iter()).unwrap();
        assert_eq!(selected.sample_rate(), 24_000);

        let flexible = cpal::SupportedStreamConfigRange::new(
            2,
            16_000,
            96_000,
            cpal::SupportedBufferSize::Unknown,
            SampleFormat::F32,
        );
        let selected = choose_config([flexible].into_iter()).unwrap();
        assert_eq!(selected.sample_rate(), SAMPLE_RATE);
    }

    #[test]
    fn band_limited_resampler_bridges_bluetooth_and_internal_rates() {
        let mut upsample = BandLimitedResampler::new(24_000, 48_000);
        upsample.extend(&[0.25; 480]);
        let upsampled: Vec<_> = std::iter::from_fn(|| upsample.next()).collect();
        assert_eq!(upsampled.len(), 960);
        assert!(
            upsampled[64..]
                .iter()
                .all(|sample| (*sample - 0.25).abs() < 0.01)
        );

        let mut downsample = BandLimitedResampler::new(48_000, 16_000);
        downsample.extend(&[-0.5; 960]);
        let downsampled: Vec<_> = std::iter::from_fn(|| downsample.next()).collect();
        assert_eq!(downsampled.len(), 320);
        assert!(
            downsampled[32..]
                .iter()
                .all(|sample| (*sample + 0.5).abs() < 0.01)
        );
    }

    #[test]
    fn band_limited_resampler_rejects_aliases_above_the_output_nyquist() {
        let mut resampler = BandLimitedResampler::new(48_000, 16_000);
        let input: Vec<_> = (0..4_800)
            .map(|sample| (sample as f32 * 12_000.0 * std::f32::consts::TAU / 48_000.0).sin())
            .collect();
        resampler.extend(&input);
        let output: Vec<_> = std::iter::from_fn(|| resampler.next()).collect();
        let settled = &output[320..];
        let rms = (settled.iter().map(|sample| sample * sample).sum::<f32>()
            / settled.len() as f32)
            .sqrt();
        assert!(rms < 0.03, "out-of-band alias RMS was {rms}");
    }

    #[test]
    fn band_limited_resampler_preserves_voice_band_energy() {
        let mut resampler = BandLimitedResampler::new(44_100, 48_000);
        let input: Vec<_> = (0..4_410)
            .map(|sample| (sample as f32 * 3_000.0 * std::f32::consts::TAU / 44_100.0).sin() * 0.5)
            .collect();
        resampler.extend(&input);
        let output: Vec<_> = std::iter::from_fn(|| resampler.next()).collect();
        assert_eq!(output.len(), 4_800);
        let settled = &output[480..];
        let rms = (settled.iter().map(|sample| sample * sample).sum::<f32>()
            / settled.len() as f32)
            .sqrt();
        let expected_rms = 0.5 / 2.0_f32.sqrt();
        assert!(
            (rms - expected_rms).abs() < 0.02,
            "voice-band RMS was {rms}"
        );
    }

    #[test]
    fn audio_format_preferences_are_stable() {
        assert!(format_rank(SampleFormat::F32) < format_rank(SampleFormat::I16));
        assert!(supported_format(SampleFormat::F64));
    }

    #[test]
    fn deep_filter_net_startup_uses_standard_until_background_load_is_ready() {
        let desired = AudioProcessingSettings {
            input_gain: 1.25,
            input_mode: InputMode::PushToTalk,
            echo_cancellation: true,
            automatic_gain: false,
            noise_suppression_mode: NoiseSuppressionMode::DeepFilterNet,
            noise_suppression_strength: NoiseSuppressionStrength::High,
        };
        let initial = startup_processing_settings(desired);
        assert_eq!(
            initial.noise_suppression_mode,
            NoiseSuppressionMode::Standard
        );
        assert_eq!(initial.input_gain, desired.input_gain);
        assert_eq!(initial.input_mode, desired.input_mode);
        assert!(initial.echo_cancellation);
        assert!(!initial.automatic_gain);
        assert_eq!(startup_processing_settings(initial), initial);
    }

    #[test]
    fn per_user_volume_and_local_mute_apply_before_mixing() {
        let mut boosted = [0.5; FRAME_SAMPLES];
        apply_user_audio_settings(
            &mut boosted,
            UserAudioSettings {
                volume: 1.5,
                muted: false,
            },
        );
        assert!(boosted.iter().all(|sample| *sample == 0.75));

        apply_user_audio_settings(
            &mut boosted,
            UserAudioSettings {
                volume: 2.0,
                muted: true,
            },
        );
        assert!(boosted.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn default_remote_voice_preserves_headroom_before_limiting() {
        assert_eq!(voice_playback_gain(0.0), 0.0);
        assert!((voice_playback_gain(0.5) - 0.625).abs() < f32::EPSILON);
        assert!((voice_playback_gain(0.8) - 1.0).abs() < f32::EPSILON);
        assert!((voice_playback_gain(1.0) - 1.25).abs() < f32::EPSILON);
        assert!((voice_playback_gain(2.0) - 2.5).abs() < f32::EPSILON);
        assert!((voice_playback_gain(3.0) - 3.75).abs() < f32::EPSILON);
        assert!((voice_playback_gain(5.0) - 3.75).abs() < f32::EPSILON);

        let speech = [0.75; FRAME_SAMPLES];
        let mut output = [0.0; FRAME_SAMPLES];
        mixer::mix(&[&speech], &mut output, voice_playback_gain(0.8));
        assert!(output.iter().all(|sample| (*sample - 0.75).abs() < 0.0001));
    }

    #[test]
    fn output_underrun_fades_to_silence_without_a_click() {
        let frame = smooth_underrun_frame(0.8);
        assert!(frame[0] < 0.8 && frame[0] > 0.0);
        assert_eq!(frame[UNDERRUN_FADE_SAMPLES - 1], 0.0);
        assert!(
            frame[UNDERRUN_FADE_SAMPLES..]
                .iter()
                .all(|sample| *sample == 0.0)
        );
        assert!(frame.windows(2).all(|pair| pair[1] <= pair[0]));
    }

    #[test]
    fn realtime_queues_replace_stale_frames_instead_of_adding_latency() {
        let (tx, rx) = bounded(2);
        assert!(!send_latest(&tx, &rx, 1));
        assert!(!send_latest(&tx, &rx, 2));
        assert!(send_latest(&tx, &rx, 3));
        assert_eq!(rx.try_recv(), Ok(2));
        assert_eq!(rx.try_recv(), Ok(3));
    }

    #[test]
    fn playback_clock_correction_converges_on_the_target_depth() {
        assert_eq!(playback_cadence(0), Duration::from_millis(19));
        assert_eq!(
            playback_cadence(PLAYBACK_TARGET_QUEUED_FRAMES),
            Duration::from_millis(20)
        );
        assert_eq!(playback_cadence(4), Duration::from_millis(21));
    }

    #[test]
    fn scheduler_stall_keeps_only_recent_media_per_speaker() {
        let mut pending = HashMap::new();
        let mut resync = HashSet::new();
        for sequence in 0..25 {
            queue_network_packet(
                &mut pending,
                &mut resync,
                VoicePacket {
                    kind: PacketType::Audio,
                    flags: 0,
                    session_id: 9,
                    sequence,
                    timestamp: sequence * FRAME_SAMPLES as u32,
                    key_epoch: 0,
                    payload: vec![sequence as u8],
                },
            );
        }
        let queued = pending.get(&9).unwrap();
        assert_eq!(queued.len(), MAX_NETWORK_CATCHUP_FRAMES_PER_PEER);
        assert_eq!(queued.front().unwrap().sequence, 15);
        assert_eq!(queued.back().unwrap().sequence, 24);
        assert!(resync.contains(&9));
    }

    #[test]
    fn input_gate_obeys_mute_vad_and_push_to_talk_priority() {
        assert!(!should_send_capture(
            true,
            InputMode::VoiceActivity,
            true,
            true
        ));
        assert!(should_send_capture(
            false,
            InputMode::VoiceActivity,
            true,
            false
        ));
        assert!(!should_send_capture(
            false,
            InputMode::VoiceActivity,
            false,
            true
        ));
        assert!(should_send_capture(
            false,
            InputMode::PushToTalk,
            false,
            true
        ));
        assert!(!should_send_capture(
            false,
            InputMode::PushToTalk,
            true,
            false
        ));
    }

    #[test]
    fn processing_factory_constructs_each_real_backend_truthfully() {
        let base = AudioProcessingSettings {
            input_gain: 1.0,
            input_mode: InputMode::VoiceActivity,
            echo_cancellation: false,
            automatic_gain: false,
            noise_suppression_mode: NoiseSuppressionMode::Standard,
            noise_suppression_strength: NoiseSuppressionStrength::Medium,
        };
        let standard = create_capture_processor(base).unwrap();
        assert_eq!(
            standard.noise_suppression_mode(),
            NoiseSuppressionMode::Standard
        );
        assert!(standard.echo_cancellation_enabled());
        assert!(standard.automatic_gain_enabled());

        let rnnoise = create_capture_processor(AudioProcessingSettings {
            noise_suppression_mode: NoiseSuppressionMode::RnNoise,
            ..base
        })
        .unwrap();
        assert_eq!(
            rnnoise.noise_suppression_mode(),
            NoiseSuppressionMode::RnNoise
        );

        let deep_filter = create_capture_processor(AudioProcessingSettings {
            noise_suppression_mode: NoiseSuppressionMode::DeepFilterNet,
            ..base
        })
        .unwrap();
        assert_eq!(
            deep_filter.noise_suppression_mode(),
            NoiseSuppressionMode::DeepFilterNet
        );

        let deep_filter_strong = create_capture_processor(AudioProcessingSettings {
            noise_suppression_mode: NoiseSuppressionMode::DeepFilterNetStrong,
            ..base
        })
        .unwrap();
        assert_eq!(
            deep_filter_strong.noise_suppression_mode(),
            NoiseSuppressionMode::DeepFilterNetStrong
        );
    }

    #[test]
    fn replacing_a_processor_starts_with_fresh_algorithm_state() {
        let settings = AudioProcessingSettings {
            input_gain: 1.0,
            input_mode: InputMode::VoiceActivity,
            echo_cancellation: false,
            automatic_gain: false,
            noise_suppression_mode: NoiseSuppressionMode::RnNoise,
            noise_suppression_strength: NoiseSuppressionStrength::Medium,
        };
        let mut warmed = create_capture_processor(settings).unwrap();
        for frame_index in 0..8 {
            let mut frame = std::array::from_fn(|sample| {
                ((sample + frame_index * FRAME_SAMPLES) as f32 * 0.03).sin() * 0.08
            });
            warmed.process_capture(&mut frame);
        }

        let mut replacement = create_capture_processor(settings).unwrap();
        let mut independently_fresh = create_capture_processor(settings).unwrap();
        let input = std::array::from_fn(|sample| (sample as f32 * 0.071).sin() * 0.12);
        let mut replaced_output = input;
        let mut fresh_output = input;
        replacement.process_capture(&mut replaced_output);
        independently_fresh.process_capture(&mut fresh_output);
        assert_eq!(replaced_output, fresh_output);
    }

    #[test]
    fn opus_handoff_encodes_the_final_processed_buffer() {
        let final_processed = [0.0; FRAME_SAMPLES];
        let mut encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
        let payload = encode_final_capture(&mut encoder, &final_processed)
            .unwrap()
            .to_vec();
        let mut decoder = OpusDecoder::new().unwrap();
        let mut decoded = [1.0; FRAME_SAMPLES];
        decoder.decode(Some(&payload), &mut decoded).unwrap();
        assert!(decoded.iter().all(|sample| sample.abs() < 1.0e-6));
    }

    #[test]
    fn device_fallback_is_only_reported_for_a_missing_explicit_selection() {
        assert!(!requested_device_fell_back(None, "Default Device"));
        assert!(!requested_device_fell_back(
            Some("USB Headset"),
            "USB Headset"
        ));
        assert!(requested_device_fell_back(
            Some("Removed Headset"),
            "Built-in Audio"
        ));
    }

    #[test]
    fn output_test_tone_is_bounded_and_stops_after_half_a_second() {
        let mut tone = TestTone::default();
        tone.start();
        let mut produced_signal = false;
        for _ in 0..25 {
            let mut frame = [0.0; FRAME_SAMPLES];
            tone.mix_into(&mut frame, 1.0);
            produced_signal |= frame.iter().any(|sample| sample.abs() > 0.0);
            assert!(frame.iter().all(|sample| (-1.0..=1.0).contains(sample)));
        }
        assert!(produced_signal);
        let mut finished = [0.0; FRAME_SAMPLES];
        tone.mix_into(&mut finished, 1.0);
        assert!(finished.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn microphone_test_records_one_ab_pair_and_replays_each_side() {
        let mut test = MicTest::default();
        test.start();
        assert_eq!(test.status(), MicTestStatus::Recording);
        assert!(test.blocks_transmission());
        for index in 0..MIC_TEST_MAX_FRAMES {
            let completed = test.record(
                [index as f32; FRAME_SAMPLES],
                [-(index as f32); FRAME_SAMPLES],
            );
            assert_eq!(completed, index + 1 == MIC_TEST_MAX_FRAMES);
        }
        assert_eq!(test.status(), MicTestStatus::PreviewReady);
        assert!(!test.blocks_transmission());

        assert!(test.play_processed());
        assert!(test.blocks_transmission());
        for index in 0..MIC_TEST_MAX_FRAMES {
            let frame = test.next_playback_frame().unwrap();
            assert_eq!(frame[0], -(index as f32));
        }
        assert_eq!(test.status(), MicTestStatus::PreviewReady);
        assert!(test.take_finished());
        assert!(!test.take_finished());

        assert!(test.play_original());
        assert_eq!(test.next_playback_frame().unwrap()[0], 0.0);
        test.stop();
        assert_eq!(test.status(), MicTestStatus::PreviewReady);
        test.stop();
        assert_eq!(test.status(), MicTestStatus::Idle);
        assert!(test.next_playback_frame().is_none());
    }

    #[test]
    fn microphone_test_can_be_stopped_manually_before_the_ten_second_limit() {
        let mut test = MicTest::default();
        test.start();
        for index in 0..25 {
            assert!(!test.record(
                [index as f32; FRAME_SAMPLES],
                [-(index as f32); FRAME_SAMPLES],
            ));
        }
        test.stop();
        assert_eq!(test.status(), MicTestStatus::PreviewReady);
        assert!(test.play_processed());
        assert_eq!(test.next_playback_frame().unwrap()[0], 0.0);
    }

    #[test]
    fn performance_guard_requires_sustained_pressure_and_never_changes_mode() {
        let pressured = DspMetricInterval {
            average_ms: 12.0,
            deadline_misses: 3,
        };
        let mut guard = DspPerformanceGuard::default();
        assert!(
            guard
                .observe(NoiseSuppressionMode::DeepFilterNet, pressured)
                .is_none()
        );
        assert!(
            guard
                .observe(NoiseSuppressionMode::DeepFilterNet, pressured)
                .is_none()
        );
        let warning = guard
            .observe(NoiseSuppressionMode::DeepFilterNet, pressured)
            .unwrap();
        assert!(warning.contains("DeepFilterNet"));
        assert!(warning.contains("Consider a lighter mode"));
        assert!(
            guard
                .observe(
                    NoiseSuppressionMode::DeepFilterNet,
                    DspMetricInterval::default()
                )
                .is_none()
        );
    }

    #[test]
    fn metric_intervals_use_deltas_instead_of_lifetime_averages() {
        let previous = DspMetrics {
            processed_frames: 100,
            total_processing_time: Duration::from_millis(20),
            peak_processing_time: Duration::from_millis(1),
            deadline_misses: 1,
        };
        let current = DspMetrics {
            processed_frames: 200,
            total_processing_time: Duration::from_millis(50),
            peak_processing_time: Duration::from_millis(2),
            deadline_misses: 4,
        };
        let interval = DspMetricInterval::between(previous, current);
        assert!((interval.average_ms - 0.3).abs() < 0.001);
        assert_eq!(interval.deadline_misses, 3);
    }

    #[test]
    fn transient_stream_events_do_not_become_permanent_device_failures() {
        for kind in [
            cpal::ErrorKind::Xrun,
            cpal::ErrorKind::DeviceChanged,
            cpal::ErrorKind::RealtimeDenied,
        ] {
            assert!(is_non_fatal_stream_error(&cpal::Error::from(kind)));
        }
        for kind in [
            cpal::ErrorKind::DeviceNotAvailable,
            cpal::ErrorKind::PermissionDenied,
            cpal::ErrorKind::UnsupportedConfig,
        ] {
            assert!(!is_non_fatal_stream_error(&cpal::Error::from(kind)));
        }
    }

    #[test]
    fn processor_transition_moves_monotonically_from_old_to_new_signal() {
        let previous = [-1.0; FRAME_SAMPLES];
        let mut first = [1.0; FRAME_SAMPLES];
        crossfade_processor_transition(&previous, &mut first, 0);
        assert!(first[0] < -0.99);
        assert!(first[FRAME_SAMPLES - 1] < -0.5);

        let mut last = [1.0; FRAME_SAMPLES];
        crossfade_processor_transition(&previous, &mut last, PROCESSOR_TRANSITION_FRAMES - 1);
        assert!(last[0] > 0.5);
        assert!((last[FRAME_SAMPLES - 1] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn dsp_loader_coalesces_queued_requests_to_the_latest_generation() {
        let (request_tx, request_rx) = unbounded();
        let (loaded_tx, loaded_rx) = unbounded();
        let base = AudioProcessingSettings {
            input_gain: 1.0,
            input_mode: InputMode::VoiceActivity,
            echo_cancellation: true,
            automatic_gain: true,
            noise_suppression_mode: NoiseSuppressionMode::Standard,
            noise_suppression_strength: NoiseSuppressionStrength::Medium,
        };
        request_tx
            .send(DspLoadRequest {
                generation: 10,
                settings: base,
            })
            .unwrap();
        request_tx
            .send(DspLoadRequest {
                generation: 11,
                settings: AudioProcessingSettings {
                    noise_suppression_mode: NoiseSuppressionMode::RnNoise,
                    ..base
                },
            })
            .unwrap();
        drop(request_tx);

        dsp_loader_worker(request_rx, loaded_tx);
        let loaded = loaded_rx.recv().unwrap();
        assert_eq!(loaded.generation, 11);
        assert_eq!(
            loaded.processor.unwrap().noise_suppression_mode(),
            NoiseSuppressionMode::RnNoise
        );
        assert!(loaded_rx.try_recv().is_err());
    }

    #[derive(Debug)]
    struct PipelineStressResult {
        mixed_frames: usize,
        decode_errors: u64,
        max_buffered_frames: usize,
        deadline_misses: u64,
        max_consecutive_deadline_misses: u64,
        peak_frame_time: Duration,
    }

    fn run_five_peer_opus_pipeline(frames: u32) -> PipelineStressResult {
        const PEERS: usize = 5;
        const FRAME_DEADLINE: Duration = Duration::from_millis(20);
        let mut encoders: Vec<_> = (0..PEERS)
            .map(|_| OpusEncoder::new(DEFAULT_BITRATE).unwrap())
            .collect();
        let mut peers: Vec<_> = (0..PEERS)
            .map(|id| RemotePeer::new(id as u64 + 1).unwrap())
            .collect();
        let mut mixed_frames = 0;
        let mut max_buffered_frames = 0;
        let mut deadline_misses = 0;
        let mut consecutive_deadline_misses = 0;
        let mut max_consecutive_deadline_misses = 0;
        let mut peak_frame_time = Duration::ZERO;
        let mut decoded = Vec::with_capacity(PEERS);
        let decoded_allocation = decoded.as_ptr();
        for sequence in 0..frames {
            let frame_started = Instant::now();
            for index in 0..PEERS {
                let amplitude = 0.01 * (index + 1) as f32;
                let pcm = [amplitude; FRAME_SAMPLES];
                let payload = encoders[index].encode(&pcm).unwrap().to_vec();
                assert!(
                    payload.len()
                        < crate::protocol::voice::MAX_PACKET_SIZE
                            - crate::protocol::voice::HEADER_SIZE
                );
                peers[index].push(sequence.wrapping_mul(FRAME_SAMPLES as u32), payload);
            }
            max_buffered_frames = max_buffered_frames.max(
                peers
                    .iter()
                    .map(RemotePeer::buffered_frames)
                    .max()
                    .unwrap_or_default(),
            );
            decoded.clear();
            decoded.extend(
                peers
                    .iter_mut()
                    .filter_map(|peer| peer.next_frame().map(|frame| frame.samples)),
            );
            assert_eq!(decoded.as_ptr(), decoded_allocation);
            if decoded.len() == PEERS {
                let mut output = [0.0; FRAME_SAMPLES];
                mixer::mix_iter(
                    decoded.iter().map(|frame| frame.as_slice()),
                    &mut output,
                    1.0,
                );
                assert!(
                    output
                        .iter()
                        .all(|sample| sample.is_finite() && (-1.0..=1.0).contains(sample))
                );
                mixed_frames += 1;
            }
            let frame_time = frame_started.elapsed();
            peak_frame_time = peak_frame_time.max(frame_time);
            if frame_time >= FRAME_DEADLINE {
                deadline_misses += 1;
                consecutive_deadline_misses += 1;
                max_consecutive_deadline_misses =
                    max_consecutive_deadline_misses.max(consecutive_deadline_misses);
            } else {
                consecutive_deadline_misses = 0;
            }
        }
        PipelineStressResult {
            mixed_frames,
            decode_errors: peers.iter().map(|peer| peer.decode_errors).sum(),
            max_buffered_frames,
            deadline_misses,
            max_consecutive_deadline_misses,
            peak_frame_time,
        }
    }

    fn assert_stable_pipeline(result: &PipelineStressResult, frames: u32) {
        assert_eq!(
            result.mixed_frames,
            frames as usize - crate::jitter::DEFAULT_PREFILL_FRAMES + 1
        );
        assert_eq!(result.decode_errors, 0);
        assert!(
            result.max_buffered_frames <= crate::jitter::DEFAULT_PREFILL_FRAMES,
            "jitter state grew unexpectedly: {result:?}"
        );
        assert!(
            result.max_consecutive_deadline_misses < 3,
            "sustained 20 ms deadline misses: {result:?}"
        );
    }

    #[test]
    fn five_peer_opus_pipeline_stays_stable_for_ten_simulated_seconds() {
        const FRAMES: u32 = 500;
        assert_stable_pipeline(&run_five_peer_opus_pipeline(FRAMES), FRAMES);
    }

    #[test]
    fn five_speakers_recover_from_jitter_reordering_and_burst_loss() {
        use crate::peer::DecodeOutcome;
        const FRAMES: usize = 1500;
        let mut encoders: Vec<_> = (0..5)
            .map(|_| OpusEncoder::new(DEFAULT_BITRATE).unwrap())
            .collect();
        let mut peers: Vec<_> = (0..5).map(|id| RemotePeer::new(id).unwrap()).collect();
        let mut arrivals = vec![Vec::new(); FRAMES + 100];
        let mut silent = [0; 5];
        let mut longest_gap = [0; 5];
        let mut gaps = [0; 5];
        let mut max_depth = 0;
        for tick in 0..FRAMES + 100 {
            if tick < FRAMES {
                for (speaker, encoder) in encoders.iter_mut().enumerate() {
                    let pcm: AudioFrame = std::array::from_fn(|sample| {
                        let t = (tick * FRAME_SAMPLES + sample) as f32 / SAMPLE_RATE as f32;
                        (std::f32::consts::TAU * (180.0 + speaker as f32 * 70.0) * t).sin() * 0.08
                    });
                    let payload = encoder.encode(&pcm).unwrap().to_vec();
                    // Deterministic 0–40 ms arrival jitter, reordering, isolated
                    // losses and 60 ms bursts at different times per speaker.
                    let phase = tick + speaker * 13;
                    if phase % 97 == 50 || (100..103).contains(&(phase % 300)) {
                        continue;
                    }
                    let delay = (tick * 17 + speaker) % 3;
                    arrivals[tick + delay].push((speaker, tick as u32, payload));
                }
            }
            for (speaker, timestamp, payload) in std::mem::take(&mut arrivals[tick]) {
                peers[speaker].push(timestamp.wrapping_mul(FRAME_SAMPLES as u32), payload);
            }
            let mut decoded = Vec::new();
            for (speaker, peer) in peers.iter_mut().enumerate() {
                max_depth = max_depth.max(peer.buffered_frames());
                let frame = peer.next_frame();
                if tick > 10 && tick < FRAMES {
                    if frame
                        .as_ref()
                        .is_none_or(|f| f.outcome == DecodeOutcome::Silence)
                    {
                        silent[speaker] += 1;
                        gaps[speaker] += 1;
                        longest_gap[speaker] = longest_gap[speaker].max(gaps[speaker]);
                    } else {
                        gaps[speaker] = 0;
                    }
                }
                if let Some(frame) = frame {
                    decoded.push(frame.samples);
                }
            }
            let mut mixed = [0.0; FRAME_SAMPLES];
            mixer::mix_iter(
                decoded.iter().map(|frame| frame.as_slice()),
                &mut mixed,
                1.0,
            );
            assert!(mixed.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
        }
        assert!(
            peers
                .iter()
                .all(|peer| peer.decode_errors == 0 && peer.buffered_frames() == 0)
        );
        assert!(max_depth <= 16, "unbounded latency: {max_depth}");
        assert!(
            silent.iter().all(|count| *count < FRAMES / 10),
            "excess buffering silence: {silent:?}"
        );
        assert!(
            longest_gap.iter().all(|count| *count <= 10),
            "recovery stuck: {longest_gap:?}"
        );
        eprintln!(
            "weak network: max_depth={max_depth}, silent_frames={silent:?}, longest_gap_frames={longest_gap:?}"
        );
    }

    #[test]
    #[ignore = "five-minute simulated release stress gate"]
    fn five_peer_opus_pipeline_stays_bounded_for_five_simulated_minutes() {
        const FRAMES: u32 = 5 * 60 * 50;
        let result = run_five_peer_opus_pipeline(FRAMES);
        assert_stable_pipeline(&result, FRAMES);
        eprintln!(
            "five-minute pipeline: misses={}, peak={:?}, max_buffered={}",
            result.deadline_misses, result.peak_frame_time, result.max_buffered_frames
        );
    }
}
