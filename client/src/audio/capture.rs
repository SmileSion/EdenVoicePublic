use super::*;
use crate::dsp::{CaptureTiming, FrameAnalysis};

const MAX_CAPTURE_AGE: Duration = Duration::from_millis(80);

#[derive(Clone)]
pub(crate) struct CapturedFrame {
    pub samples: AudioFrame,
    pub ptt_pressed: bool,
    pub transmission_allowed: bool,
    pub captured_at: Instant,
    pub queued_at: Instant,
}

#[derive(Clone)]
pub(super) struct RenderReference {
    pub(super) samples: AudioFrame,
    pub(super) plays_at: Instant,
}

#[derive(Clone)]
pub(crate) struct OutputQueue {
    pub(super) frames: Receiver<AudioFrame>,
    pub(super) references: Sender<RenderReference>,
    pub(super) stale_references: Receiver<RenderReference>,
    pub(super) reference_drops: Arc<AtomicU64>,
}

impl OutputQueue {
    pub(super) fn take_frame(
        &self,
        plays_at: Instant,
        last_sample: f32,
        underruns: &AtomicU64,
    ) -> AudioFrame {
        let frame = self.frames.try_recv().unwrap_or_else(|_| {
            underruns.fetch_add(1, Ordering::Relaxed);
            smooth_underrun_frame(last_sample)
        });
        self.reference(frame, plays_at);
        frame
    }

    pub(super) fn reference(&self, samples: AudioFrame, plays_at: Instant) {
        if send_latest(
            &self.references,
            &self.stale_references,
            RenderReference { samples, plays_at },
        ) {
            self.reference_drops.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct CaptureStatus {
    pub(super) metrics: DspMetrics,
    pub(super) mode: NoiseSuppressionMode,
    pub(super) echo_cancellation: bool,
    pub(super) automatic_gain: bool,
    pub(super) generation: u64,
    pub(super) silence_frames: u64,
    pub(super) aec_delay_ms: f64,
}

impl CaptureStatus {
    pub(super) fn new(processor: &CaptureAudioProcessor, generation: u64) -> Self {
        Self {
            metrics: processor.metrics(),
            mode: processor.noise_suppression_mode(),
            echo_cancellation: processor.echo_cancellation_enabled(),
            automatic_gain: processor.automatic_gain_enabled(),
            generation,
            silence_frames: 0,
            aec_delay_ms: 0.0,
        }
    }
}

pub(super) struct ProcessedCapture {
    pub(super) original: AudioFrame,
    pub(super) frame: AudioFrame,
    pub(super) transmit_frame: AudioFrame,
    pub(super) analysis: FrameAnalysis,
    pub(super) input_mode: InputMode,
    pub(super) ptt_pressed: bool,
    pub(super) captured_at: Instant,
    pub(super) queued_at: Instant,
    pub(super) processing_time: Duration,
    pub(super) status: CaptureStatus,
}

pub(super) struct CapturePipeline {
    pub(super) results: Receiver<ProcessedCapture>,
    pub(super) replacements: Sender<(CaptureAudioProcessor, InputMode)>,
    pub(super) stale_replacements: Receiver<(CaptureAudioProcessor, InputMode)>,
    pub(super) unused: Arc<AtomicBool>,
    pub(super) muted_speech_analysis: Arc<AtomicBool>,
    pub(super) stale_drops: Arc<AtomicU64>,
    pub(super) reference_drops: Arc<AtomicU64>,
}

impl CapturePipeline {
    pub(super) fn spawn(
        settings: AudioProcessingSettings,
        capture: Receiver<CapturedFrame>,
        output: OutputQueue,
        alive: Arc<AtomicBool>,
        muted: Arc<AtomicBool>,
        events: Sender<AudioEvent>,
    ) -> Result<Self> {
        let (tx, results) = bounded(2);
        let stale_results = results.clone();
        let (replacements, incoming_replacements) = bounded(1);
        let stale_replacements = incoming_replacements.clone();
        let unused = Arc::new(AtomicBool::new(false));
        let worker_unused = unused.clone();
        let muted_speech_analysis = Arc::new(AtomicBool::new(false));
        let worker_speech_analysis = muted_speech_analysis.clone();
        let stale_drops = Arc::new(AtomicU64::new(0));
        let worker_drops = stale_drops.clone();
        let reference_drops = output.reference_drops.clone();
        thread::Builder::new()
            .name("voice-capture-dsp".into())
            .spawn(move || {
                let mut processor = create_capture_processor(settings).unwrap_or_else(|error| {
                    let _ = events.try_send(AudioEvent::DspError(error));
                    CaptureAudioProcessor::off(
                        settings.input_gain,
                        settings.echo_cancellation,
                        settings.automatic_gain,
                    )
                });
                let mut previous: Option<CaptureAudioProcessor> = None;
                let mut speech_probe = None;
                let mut speech_reminder = super::muted_speech::SpeechReminder::default();
                let mut transition = 0;
                let mut generation = 0;
                let mut input_mode = settings.input_mode;
                let mut silence_frames = 0;
                let mut latest_playback = None;
                let mut reported_error = false;
                while alive.load(Ordering::Acquire) {
                    if let Ok((replacement, mode)) = incoming_replacements.try_recv() {
                        previous = Some(std::mem::replace(&mut processor, replacement));
                        input_mode = mode;
                        transition = 0;
                        generation += 1;
                        reported_error = false;
                    }
                    // Feed only frames actually consumed by the output callback,
                    // including its underrun substitution, never queued/dropped mixes.
                    for reference in output.stale_references.try_iter().take(16) {
                        processor.process_render_reference(&reference.samples);
                        if let Some(old) = &mut previous {
                            old.process_render_reference(&reference.samples);
                        }
                        latest_playback = Some(reference.plays_at);
                    }
                    let input = match capture.recv_timeout(Duration::from_millis(2)) {
                        Ok(input) => input,
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                        Err(_) => break,
                    };
                    if input.queued_at.elapsed() > MAX_CAPTURE_AGE {
                        worker_drops.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    let delay = latest_playback
                        .map(|at: Instant| at.saturating_duration_since(input.captured_at))
                        .unwrap_or_default()
                        .min(Duration::from_millis(500));
                    processor.set_echo_delay(delay);
                    processor.set_output_unused(
                        worker_unused.load(Ordering::Relaxed) && previous.is_none(),
                    );
                    let started = Instant::now();
                    let mut frame = input.samples;
                    let timing = CaptureTiming::new(
                        input.captured_at,
                        input.queued_at,
                        input.ptt_pressed,
                        input_mode,
                        input.transmission_allowed && !muted.load(Ordering::Acquire),
                    );
                    let (mut analysis, output_timing) =
                        processor.process_capture_with_timing(&mut frame, timing);
                    let mut transmit_frame = frame;
                    let mut permitted =
                        output_timing.restrict_transmission(&mut transmit_frame, input_mode);
                    let mut captured_at = output_timing.captured_at();
                    let mut queued_at = output_timing.queued_at();
                    if let Some(old) = &mut previous {
                        old.set_echo_delay(delay);
                        old.set_output_unused(false);
                        let mut old_frame = input.samples;
                        let (old_analysis, old_timing) =
                            old.process_capture_with_timing(&mut old_frame, timing);
                        crossfade_processor_transition(&old_frame, &mut frame, transition);
                        let mut old_transmit_frame = old_frame;
                        permitted |=
                            old_timing.restrict_transmission(&mut old_transmit_frame, input_mode);
                        crossfade_processor_transition(
                            &old_transmit_frame,
                            &mut transmit_frame,
                            transition,
                        );
                        captured_at = captured_at
                            .into_iter()
                            .chain(old_timing.captured_at())
                            .min();
                        queued_at = queued_at.into_iter().chain(old_timing.queued_at()).min();
                        // The send gate must retain speech from either side of
                        // the crossfade, including a new model's silent warmup.
                        analysis.level = crate::dsp::analyze_level(&frame);
                        analysis.voice_active |= old_analysis.voice_active;
                        transition += 1;
                        if transition == PROCESSOR_TRANSITION_FRAMES {
                            previous = None;
                        }
                    }
                    let eligible = worker_speech_analysis.load(Ordering::Acquire)
                        && muted.load(Ordering::Acquire)
                        && !input.transmission_allowed
                        && (input_mode != InputMode::PushToTalk || input.ptt_pressed);
                    let speech = if eligible {
                        speech_probe
                            .get_or_insert_with(super::muted_speech::LocalSpeechProbe::new)
                            .is_speech(&frame, analysis)
                    } else {
                        speech_probe = None;
                        false
                    };
                    if speech_reminder.observe(eligible, speech, input.captured_at) {
                        let _ = events.try_send(AudioEvent::MutedSpeech(input.captured_at));
                    }
                    if let Some(error) = processor.take_error() {
                        if frame.iter().all(|sample| *sample == 0.0) {
                            silence_frames += 1;
                        }
                        if !reported_error {
                            let _ = events.try_send(AudioEvent::DspError(error));
                            reported_error = true;
                        }
                    } else {
                        reported_error = false;
                    }
                    let mut status = CaptureStatus::new(&processor, generation);
                    status.silence_frames = silence_frames;
                    status.aec_delay_ms = delay.as_secs_f64() * 1000.0;
                    let result = ProcessedCapture {
                        original: input.samples,
                        frame,
                        transmit_frame,
                        analysis,
                        input_mode,
                        ptt_pressed: permitted,
                        captured_at: captured_at.unwrap_or(input.captured_at),
                        queued_at: queued_at.unwrap_or(input.queued_at),
                        processing_time: started.elapsed(),
                        status,
                    };
                    if send_latest(&tx, &stale_results, result) {
                        worker_drops.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })?;
        Ok(Self {
            results,
            replacements,
            stale_replacements,
            unused,
            muted_speech_analysis,
            stale_drops,
            reference_drops,
        })
    }

    pub(super) fn replace(&self, processor: CaptureAudioProcessor, mode: InputMode) {
        send_latest(
            &self.replacements,
            &self.stale_replacements,
            (processor, mode),
        );
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioTiming {
    pub capture_age_ms: f64,
    pub playback_queue_ms: f64,
    pub actual_jitter_ms: f64,
    pub worker_p95_ms: f64,
    pub worker_p99_ms: f64,
    pub playout_lateness_p99_ms: f64,
    pub sent_fec_percent: f64,
    pub capture_processing_p95_ms: f64,
    pub capture_stale_drops: u64,
    pub receive_stale_drops: u64,
    pub dsp_silence_frames: u64,
    pub render_reference_drops: u64,
    pub aec_delay_ms: f64,
}

pub(super) struct TimingHistogram {
    pub(super) buckets: [u64; 201],
    pub(super) total: u64,
}
impl Default for TimingHistogram {
    fn default() -> Self {
        Self {
            buckets: [0; 201],
            total: 0,
        }
    }
}
impl TimingHistogram {
    pub(super) fn record(&mut self, duration: Duration) {
        let bucket = duration.as_micros().div_ceil(1000).min(200) as usize;
        self.buckets[bucket] += 1;
        self.total += 1;
    }
    pub(super) fn percentile(&self, percent: u64) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        let threshold = (self.total * percent).div_ceil(100);
        let mut count = 0;
        for (ms, samples) in self.buckets.iter().enumerate() {
            count += samples;
            if count >= threshold {
                return ms as f64;
            }
        }
        200.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queues() -> (Sender<AudioFrame>, OutputQueue) {
        let (frames, rx) = bounded(1);
        let (references, stale_references) = bounded(16);
        (
            frames,
            OutputQueue {
                frames: rx,
                references,
                stale_references,
                reference_drops: Arc::new(AtomicU64::new(0)),
            },
        )
    }

    #[test]
    fn reference_tracks_consumed_frames_and_underrun_not_discarded_mix() {
        let (tx, queue) = queues();
        tx.send([0.1; FRAME_SAMPLES]).unwrap();
        send_latest(&tx, &queue.frames, [0.2; FRAME_SAMPLES]);
        assert!(queue.stale_references.is_empty());
        let underruns = AtomicU64::new(0);
        let at = Instant::now() + Duration::from_millis(40);
        let played = queue.take_frame(at, 0.0, &underruns);
        let reference = queue.stale_references.try_recv().unwrap();
        assert_eq!(reference.samples, played);
        assert_eq!(reference.samples[0], 0.2);
        assert_eq!(reference.plays_at, at);
        let silence = queue.take_frame(at, 0.2, &underruns);
        assert_eq!(queue.stale_references.try_recv().unwrap().samples, silence);
        assert_eq!(underruns.load(Ordering::Relaxed), 1);
        assert_eq!(silence[FRAME_SAMPLES - 1], 0.0);
    }

    #[test]
    fn timing_percentiles_include_tail_stalls() {
        let mut histogram = TimingHistogram::default();
        for _ in 0..94 {
            histogram.record(Duration::from_micros(900));
        }
        for _ in 0..5 {
            histogram.record(Duration::from_millis(12));
        }
        histogram.record(Duration::from_millis(80));
        assert_eq!(histogram.percentile(95), 12.0);
        assert_eq!(histogram.percentile(99), 12.0);
        assert_eq!(histogram.percentile(100), 80.0);
    }

    #[test]
    fn blocked_capture_dsp_cannot_block_playout_worker() {
        let (input, capture) = bounded(8);
        let (playback, played) = bounded(16);
        let (commands, command_rx) = bounded(4);
        let (events, _) = bounded(128);
        let (_, output_queue) = queues();
        let alive = Arc::new(AtomicBool::new(true));
        let (entered, enter_rx) = bounded(1);
        let (release, release_rx) = bounded(1);
        commands
            .send(WorkerCommand::ReplaceCaptureProcessor {
                processor: CaptureAudioProcessor::blocked_for_test(entered, release_rx),
                input_mode: InputMode::VoiceActivity,
            })
            .unwrap();
        let settings = WorkerSettings {
            output_queue,
            commands: command_rx,
            input_gain: 1.0,
            output_volume: 1.0,
            input_mode: InputMode::VoiceActivity,
            echo_cancellation: false,
            automatic_gain: false,
            noise_suppression_mode: NoiseSuppressionMode::Off,
            noise_suppression_strength: NoiseSuppressionStrength::Medium,
            control: WorkerControl {
                alive: alive.clone(),
                muted: Arc::new(AtomicBool::new(false)),
                deafened: Arc::new(AtomicBool::new(false)),
                capture_overruns: Arc::new(AtomicU64::new(0)),
                playback_underruns: Arc::new(AtomicU64::new(0)),
            },
        };
        let stale = played.clone();
        let worker = thread::spawn(move || {
            run_worker(
                Box::new(LocalTransport),
                capture,
                playback,
                stale,
                events,
                settings,
            )
        });
        let start = Instant::now();
        let mut reached = false;
        while start.elapsed() < Duration::from_secs(1) {
            if enter_rx.try_recv().is_ok() {
                reached = true;
                break;
            }
            let _ = input.try_send(CapturedFrame {
                samples: [0.1; FRAME_SAMPLES],
                ptt_pressed: false,
                transmission_allowed: true,
                captured_at: Instant::now() - Duration::from_millis(250),
                queued_at: Instant::now(),
            });
            thread::sleep(Duration::from_millis(10));
        }
        played.try_iter().for_each(drop);
        let mut frames = 0;
        for _ in 0..4 {
            if played.recv_timeout(Duration::from_millis(60)).is_ok() {
                frames += 1;
            }
        }
        alive.store(false, Ordering::Release);
        let _ = release.try_send(());
        worker.join().unwrap();
        assert!(
            reached,
            "fresh capture with 250 ms device latency never reached DSP"
        );
        assert_eq!(frames, 4, "blocked DSP interrupted playout");
    }

    #[test]
    fn first_mix_after_a_worker_stall_cannot_replay_old_peer_audio() {
        struct BlockingSendTransport {
            incoming: Receiver<VoicePacket>,
            entered: Sender<()>,
            release: Receiver<()>,
        }
        impl VoiceTransport for BlockingSendTransport {
            fn send_audio(&self, _: &[u8]) -> std::io::Result<()> {
                let _ = self.entered.try_send(());
                let _ = self.release.recv_timeout(Duration::from_secs(2));
                Ok(())
            }
            fn try_receive(&self) -> Option<VoicePacket> {
                self.incoming.try_recv().ok()
            }
            fn session_id(&self) -> u64 {
                1
            }
        }

        let (input, capture) = bounded(1);
        let (playback, played) = bounded(8);
        let (_commands, command_rx) = bounded(1);
        let (events, _event_rx) = bounded(128);
        let (_, output_queue) = queues();
        let alive = Arc::new(AtomicBool::new(true));
        let (entered, enter_rx) = bounded(1);
        let (release, release_rx) = bounded(1);
        let (remote, incoming) = bounded(16);
        let mut encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
        let mut packet = |sequence: u32| {
            let mut samples = [0.0; FRAME_SAMPLES];
            for (offset, sample) in samples.iter_mut().enumerate() {
                let position = sequence as usize * FRAME_SAMPLES + offset;
                *sample = (position as f32 * 440.0 * std::f32::consts::TAU / SAMPLE_RATE as f32)
                    .sin()
                    * 0.2;
            }
            VoicePacket {
                kind: PacketType::Audio,
                flags: 0,
                session_id: 7,
                sequence,
                timestamp: sequence * FRAME_SAMPLES as u32,
                key_epoch: 0,
                payload: encoder.encode(&samples).unwrap().to_vec(),
            }
        };
        // Leave several voiced frames buffered after jitter playout starts.
        for sequence in 0..10 {
            remote.send(packet(sequence)).unwrap();
        }
        let settings = WorkerSettings {
            output_queue,
            commands: command_rx,
            input_gain: 1.0,
            output_volume: 1.0,
            input_mode: InputMode::PushToTalk,
            echo_cancellation: false,
            automatic_gain: false,
            noise_suppression_mode: NoiseSuppressionMode::Off,
            noise_suppression_strength: NoiseSuppressionStrength::Medium,
            control: WorkerControl {
                alive: alive.clone(),
                muted: Arc::new(AtomicBool::new(false)),
                deafened: Arc::new(AtomicBool::new(false)),
                capture_overruns: Arc::new(AtomicU64::new(0)),
                playback_underruns: Arc::new(AtomicU64::new(0)),
            },
        };
        let stale = played.clone();
        let worker = thread::spawn(move || {
            run_worker(
                Box::new(BlockingSendTransport {
                    incoming,
                    entered,
                    release: release_rx,
                }),
                capture,
                playback,
                stale,
                events,
                settings,
            )
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut heard_remote = false;
        while Instant::now() < deadline {
            if played
                .recv_timeout(Duration::from_millis(40))
                .is_ok_and(|frame| frame_rms(&frame) > 0.01)
            {
                heard_remote = true;
                break;
            }
        }
        let now = Instant::now();
        let _ = input.try_send(CapturedFrame {
            samples: [0.1; FRAME_SAMPLES],
            ptt_pressed: true,
            transmission_allowed: true,
            captured_at: now,
            queued_at: now,
        });
        let blocked = enter_rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .is_ok();
        let drained_into_peer = remote.is_empty();
        let mut resumed_frame = None;
        if blocked {
            // The worker is now blocked inside send_audio. Remove already
            // emitted output so the next received frame is its first new mix.
            played.try_iter().for_each(drop);
            thread::sleep(Duration::from_millis(150));
            // Model a producer that already evicted every expired packet:
            // only fresh ingress remains, so no Expired event can clear the
            // peer's old jitter/decoder state before the overdue mix.
            remote.send(packet(10)).unwrap();
            let _ = release.try_send(());
            resumed_frame = played
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .ok();
        }
        alive.store(false, Ordering::Release);
        let _ = release.try_send(());
        worker.join().unwrap();

        assert!(heard_remote, "remote jitter playout never started");
        assert!(blocked, "capture did not block the real audio worker");
        assert!(drained_into_peer, "old audio was not buffered in the peer");
        let resumed_frame = resumed_frame.expect("worker did not resume mixing");
        assert_eq!(
            frame_rms(&resumed_frame),
            0.0,
            "the first resumed mix replayed pre-stall peer audio"
        );
    }
}

#[cfg(test)]
mod transition_regressions {
    use super::*;
    fn output() -> OutputQueue {
        let (_, frames) = bounded(8);
        let (references, stale_references) = bounded(16);
        OutputQueue {
            frames,
            references,
            stale_references,
            reference_drops: Arc::new(AtomicU64::new(0)),
        }
    }
    fn processing() -> AudioProcessingSettings {
        AudioProcessingSettings {
            input_gain: 1.0,
            input_mode: InputMode::VoiceActivity,
            echo_cancellation: false,
            automatic_gain: false,
            noise_suppression_mode: NoiseSuppressionMode::Off,
            noise_suppression_strength: NoiseSuppressionStrength::Medium,
        }
    }

    #[test]
    fn delayed_ptt_crops_both_edges_and_preserves_preview_across_capture_gaps() {
        let mut processor =
            CaptureAudioProcessor::delayed_for_test(crate::dsp::DEEP_FILTER_TOTAL_DELAY_SAMPLES);
        let start = Instant::now();
        for index in 0..10 {
            let captured_at =
                start + Duration::from_millis(index * 20 + if index >= 4 { 200 } else { 0 });
            let mut preview = [0.1 * (index + 1) as f32; FRAME_SAMPLES];
            let timing = CaptureTiming::new(
                captured_at,
                start,
                (2..4).contains(&index),
                InputMode::PushToTalk,
                true,
            );
            let (_, output) = processor.process_capture_with_timing(&mut preview, timing);
            let mut transmit = preview;
            let permitted = output.restrict_transmission(&mut transmit, InputMode::PushToTalk);
            // 70 ms is 3.5 frames: the two halves have different source frames.
            for half in 0..2 {
                let source = index as i64 - 4 + half as i64;
                let raw = if source < 0 {
                    0.0
                } else {
                    0.1 * (source + 1) as f32
                };
                let expected = if (2..4).contains(&source) { raw } else { 0.0 };
                assert!(
                    preview[half * 480..(half + 1) * 480]
                        .iter()
                        .all(|sample| *sample == raw)
                );
                assert!(
                    transmit[half * 480..(half + 1) * 480]
                        .iter()
                        .all(|sample| *sample == expected)
                );
                if source >= 0 {
                    let source_index = source as u64;
                    let source_time = start
                        + Duration::from_millis(
                            source_index * 20 + if source_index >= 4 { 200 } else { 0 },
                        )
                        + Duration::from_millis(if half == 0 { 10 } else { 0 });
                    assert_eq!(output.chunks[half].unwrap().captured_at, source_time);
                }
            }
            assert_eq!(permitted, (5..8).contains(&index));
        }
    }

    #[test]
    fn delayed_mute_and_ptt_mode_permissions_survive_resume_and_crossfade() {
        let start = Instant::now();
        let mut old =
            CaptureAudioProcessor::delayed_for_test(crate::dsp::DEEP_FILTER_TOTAL_DELAY_SAMPLES);
        // A queued muted interval stays unauthorized after unmute, even with PTT held.
        for index in 0..8 {
            let timing = CaptureTiming::new(start, start, true, InputMode::PushToTalk, index != 2);
            let mut preview = [0.5; FRAME_SAMPLES];
            let (_, output) = old.process_capture_with_timing(&mut preview, timing);
            let mut transmit = preview;
            output.restrict_transmission(&mut transmit, InputMode::PushToTalk);
            if index == 5 {
                assert_eq!(&transmit[480..], &[0.0; 480]);
                assert_eq!(&preview[480..], &[0.5; 480]);
            }
            if index == 6 {
                assert_eq!(&transmit[..480], &[0.0; 480]);
                assert_eq!(&preview[..480], &[0.5; 480]);
            }
        }
        let mut preview = [0.7; FRAME_SAMPLES];
        let old_timing = CaptureTiming::new(start, start, false, InputMode::PushToTalk, true);
        let mut new_frame = [0.9; FRAME_SAMPLES];
        let new_timing = CaptureTiming::new(start, start, false, InputMode::VoiceActivity, true);
        // Switching to VAD cannot authorize old PTT-closed audio. Crop each side
        // before crossfading because the old and new DSP have different latency.
        old_timing.restrict_transmission(&mut preview, InputMode::VoiceActivity);
        new_timing.restrict_transmission(&mut new_frame, InputMode::VoiceActivity);
        crossfade_processor_transition(&preview, &mut new_frame, 0);
        let first_allowed_sample = 0.9 / (PROCESSOR_TRANSITION_FRAMES * FRAME_SAMPLES) as f32;
        assert!((new_frame[0] - first_allowed_sample).abs() < f32::EPSILON);
        assert!(new_frame[FRAME_SAMPLES - 1] < 0.2);
        let mut held = [0.7; FRAME_SAMPLES];
        CaptureTiming::new(start, start, false, InputMode::VoiceActivity, true)
            .restrict_transmission(&mut held, InputMode::PushToTalk);
        assert_eq!(held, [0.0; FRAME_SAMPLES]);
    }
    #[test]
    fn crossfade_keeps_audible_speech_in_the_send_gate() {
        let (tx, rx) = bounded(8);
        let alive = Arc::new(AtomicBool::new(true));
        let (events, _) = bounded(128);
        let pipe = CapturePipeline::spawn(
            processing(),
            rx,
            output(),
            alive.clone(),
            Arc::new(AtomicBool::new(false)),
            events,
        )
        .unwrap();
        tx.send(CapturedFrame {
            samples: [0.1; FRAME_SAMPLES],
            ptt_pressed: false,
            transmission_allowed: true,
            captured_at: Instant::now(),
            queued_at: Instant::now(),
        })
        .unwrap();
        assert!(
            pipe.results
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .analysis
                .voice_active
        );
        // Controlled zero output models a newly selected processor's silent warmup.
        pipe.replace(
            CaptureAudioProcessor::passthrough(0.0),
            InputMode::VoiceActivity,
        );
        thread::sleep(Duration::from_millis(10));
        tx.send(CapturedFrame {
            samples: [0.1; FRAME_SAMPLES],
            ptt_pressed: false,
            transmission_allowed: true,
            captured_at: Instant::now(),
            queued_at: Instant::now(),
        })
        .unwrap();
        let r = pipe.results.recv_timeout(Duration::from_secs(1)).unwrap();
        alive.store(false, Ordering::Release);
        assert_eq!(r.status.generation, 1);
        assert!(frame_rms(&r.frame) > 0.05);
        assert!(should_send_capture(
            false,
            r.input_mode,
            r.analysis.voice_active,
            false
        ));
        println!(
            "validated: final RMS {:.4}, analysis RMS {:.4}, voice_active {}",
            frame_rms(&r.frame),
            r.analysis.level.rms,
            r.analysis.voice_active
        );
    }
}

#[cfg(test)]
mod ptt_regressions {
    use super::*;
    struct CountTransport(Sender<()>);
    impl VoiceTransport for CountTransport {
        fn send_audio(&self, _: &[u8]) -> std::io::Result<()> {
            let _ = self.0.try_send(());
            Ok(())
        }
        fn try_receive(&self) -> Option<VoicePacket> {
            None
        }
        fn session_id(&self) -> u64 {
            123
        }
    }
    #[test]
    fn static_mute_keeps_post_dsp_meter_alive_without_any_media_sends() {
        for mode in [NoiseSuppressionMode::Off, NoiseSuppressionMode::Standard, NoiseSuppressionMode::RnNoise] {
            let (input, capture) = bounded(8);
            let (playback, played) = bounded(8);
            let (commands, command_rx) = bounded(8);
            let (events, event_rx) = bounded(256);
            let (_, frames) = bounded(8);
            let (references, stale_references) = bounded(16);
            let alive = Arc::new(AtomicBool::new(true));
            let muted = Arc::new(AtomicBool::new(true));
            let (sent, sent_rx) = bounded(128);
            commands
                .send(WorkerCommand::MutedSpeechAnalysis(true))
                .unwrap();
            let settings = WorkerSettings {
                output_queue: OutputQueue {
                    frames,
                    references,
                    stale_references,
                    reference_drops: Arc::new(AtomicU64::new(0)),
                },
                commands: command_rx,
                input_gain: 1.0,
                output_volume: 0.8,
                input_mode: InputMode::VoiceActivity,
                echo_cancellation: false,
                automatic_gain: false,
                noise_suppression_mode: mode,
                noise_suppression_strength: NoiseSuppressionStrength::Medium,
                control: WorkerControl {
                    alive: alive.clone(),
                    muted: muted.clone(),
                    deafened: Arc::new(AtomicBool::new(false)),
                    capture_overruns: Arc::new(AtomicU64::new(0)),
                    playback_underruns: Arc::new(AtomicU64::new(0)),
                },
            };
            let stale = played.clone();
            let worker = thread::spawn(move || {
                run_worker(
                    Box::new(CountTransport(sent)),
                    capture,
                    playback,
                    stale,
                    events,
                    settings,
                )
            });
            for index in 0..35 {
                let samples = std::array::from_fn(|offset| {
                    let phase =
                        (index * FRAME_SAMPLES + offset) as f32 * 330.0 * std::f32::consts::TAU
                            / SAMPLE_RATE as f32;
                    0.12 * phase.sin()
                });
                input
                    .send(CapturedFrame {
                        samples,
                        ptt_pressed: false,
                        transmission_allowed: false,
                        captured_at: Instant::now(),
                        queued_at: Instant::now(),
                    })
                    .unwrap();
                thread::sleep(Duration::from_millis(20));
            }
            alive.store(false, Ordering::Release);
            worker.join().unwrap();
            let events: Vec<_> = event_rx.try_iter().collect();
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, AudioEvent::InputLevel(level) if *level > 0.001)),
                "{mode:?}: static mute prevented post-DSP analysis"
            );
            assert!(sent_rx.is_empty(), "{mode:?}: static mute sent media");
            assert!(
                !events.iter().any(|event| matches!(
                    event,
                    AudioEvent::Talking(_) | AudioEvent::MutedSpeech(_)
                )),
                "a short tone must neither transmit nor trigger speech reminder"
            );
        }
    }

    #[test]
    fn delayed_capture_uses_its_key_snapshot_and_still_respects_live_mute() {
        for mode in [InputMode::VoiceActivity, InputMode::PushToTalk] {
            for (held_at_capture, allowed_at_capture, mute_before_send, should_send) in [
                (true, true, false, true),
                (false, true, false, false),
                (true, true, true, false),
                // Captured while muted, then processed after unmute: never replay.
                (true, false, false, false),
            ] {
                let (input, capture) = bounded(8);
                let (playback, played) = bounded(8);
                let (commands, command_rx) = bounded(8);
                let (events, _) = bounded(128);
                let (_, frames) = bounded(8);
                let (references, stale_references) = bounded(16);
                let alive = Arc::new(AtomicBool::new(true));
                let muted = Arc::new(AtomicBool::new(false));
                let (entered, enter_rx) = bounded(1);
                let (release, release_rx) = bounded(1);
                let (sent, sent_rx) = bounded(8);
                commands
                    .send(WorkerCommand::ReplaceCaptureProcessor {
                        processor: CaptureAudioProcessor::blocked_for_test(entered, release_rx),
                        input_mode: mode,
                    })
                    .unwrap();
                let settings = WorkerSettings {
                    output_queue: OutputQueue {
                        frames,
                        references,
                        stale_references,
                        reference_drops: Arc::new(AtomicU64::new(0)),
                    },
                    commands: command_rx,
                    input_gain: 1.0,
                    output_volume: 0.8,
                    input_mode: mode,
                    echo_cancellation: false,
                    automatic_gain: false,
                    noise_suppression_mode: NoiseSuppressionMode::Off,
                    noise_suppression_strength: NoiseSuppressionStrength::Medium,
                    control: WorkerControl {
                        alive: alive.clone(),
                        muted: muted.clone(),
                        deafened: Arc::new(AtomicBool::new(false)),
                        capture_overruns: Arc::new(AtomicU64::new(0)),
                        playback_underruns: Arc::new(AtomicU64::new(0)),
                    },
                };
                let stale = played.clone();
                let worker = thread::spawn(move || {
                    run_worker(
                        Box::new(CountTransport(sent)),
                        capture,
                        playback,
                        stale,
                        events,
                        settings,
                    )
                });
                thread::sleep(Duration::from_millis(20));
                input
                    .send(CapturedFrame {
                        samples: [0.1; FRAME_SAMPLES],
                        ptt_pressed: held_at_capture,
                        transmission_allowed: allowed_at_capture,
                        captured_at: Instant::now(),
                        queued_at: Instant::now(),
                    })
                    .unwrap();
                let reached = enter_rx.recv_timeout(Duration::from_secs(1)).is_ok();
                muted.store(mute_before_send, Ordering::Release);
                release.send(()).unwrap();
                let did_send = sent_rx.recv_timeout(Duration::from_millis(200)).is_ok();
                alive.store(false, Ordering::Release);
                worker.join().unwrap();
                assert!(reached, "DSP replacement was not exercised");
                assert_eq!(
                    did_send,
                    should_send
                        || (mode == InputMode::VoiceActivity
                            && allowed_at_capture
                            && !mute_before_send),
                    "mode={mode:?}, held={held_at_capture}, allowed_at_capture={allowed_at_capture}, muted={mute_before_send}"
                );
            }
        }
    }
}
