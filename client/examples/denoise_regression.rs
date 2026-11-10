//! Development-only offline regression runner for EdenVoice's microphone DSP.
//!
//! Usage:
//!   cargo run --release --example denoise_regression -- input-48k-mono.wav output-dir
//!
//! The input must be mono, 48 kHz PCM16 or IEEE-float WAV. The runner bypasses
//! devices, VAD/DTX, Opus, networking, playback, and UI state. AEC is disabled
//! because an offline capture file has no time-aligned render reference.

use std::{
    env,
    fs::{self, File},
    io::{Read, Write},
    path::Path,
    time::{Duration, Instant},
};

use edenvoice_client::{
    codec::{FRAME_SAMPLES, SAMPLE_RATE},
    dsp::{CaptureAudioProcessor, DEEP_FILTER_TOTAL_DELAY_SAMPLES, NoiseSuppressionStrength},
};

#[derive(Clone, Copy)]
enum Mode {
    Original,
    WebRtc,
    RnNoise,
    DeepFilterNet,
    DeepFilterNetStrong,
}

impl Mode {
    const ALL: [Self; 5] = [
        Self::Original,
        Self::WebRtc,
        Self::RnNoise,
        Self::DeepFilterNet,
        Self::DeepFilterNetStrong,
    ];

    fn suffix(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::WebRtc => "webrtc-apm",
            Self::RnNoise => "rnnoise",
            Self::DeepFilterNet => "deepfilternet",
            Self::DeepFilterNetStrong => "deepfilternet-strong",
        }
    }

    fn processor(self) -> Result<CaptureAudioProcessor, String> {
        match self {
            Self::Original => Ok(CaptureAudioProcessor::passthrough(1.0)),
            // AEC requires a render reference and is intentionally disabled.
            Self::WebRtc => {
                CaptureAudioProcessor::standard(1.0, false, true, NoiseSuppressionStrength::Medium)
            }
            Self::RnNoise => Ok(CaptureAudioProcessor::rnnoise(1.0, false, false)),
            Self::DeepFilterNet => CaptureAudioProcessor::deep_filter_net(1.0, false, false, false),
            Self::DeepFilterNetStrong => {
                CaptureAudioProcessor::deep_filter_net(1.0, false, false, true)
            }
        }
    }
}

#[derive(Clone, Copy)]
struct SignalStats {
    rms: f64,
    peak: f32,
    clipped: usize,
    non_finite: usize,
}

fn main() -> Result<(), String> {
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() == 2 && args[0] == "--generate-fixtures" {
        generate_fixtures(Path::new(&args[1]))?;
        return Ok(());
    }
    if args.len() == 2 && args[0] == "--stats" {
        let samples = read_wav(Path::new(&args[1]))?;
        let stats = signal_stats(&samples);
        println!(
            "samples={} duration_s={:.6} rms={:.8} peak={:.8} clipped={} non_finite={}",
            samples.len(),
            samples.len() as f64 / SAMPLE_RATE as f64,
            stats.rms,
            stats.peak,
            stats.clipped,
            stats.non_finite,
        );
        return Ok(());
    }
    if args.len() == 3 && args[0] == "--from-raw-f32" {
        let samples = read_raw_f32(Path::new(&args[1]))?;
        write_wav_f32(Path::new(&args[2]), &samples)?;
        return Ok(());
    }
    if args.len() != 2 {
        return Err("usage: denoise_regression <48k-mono.wav> <output-dir>\n       denoise_regression --generate-fixtures <output-dir>\n       denoise_regression --stats <input.wav>\n       denoise_regression --from-raw-f32 <input.raw> <output.wav>".into());
    }

    let input_path = Path::new(&args[0]);
    let output_dir = Path::new(&args[1]);
    fs::create_dir_all(output_dir)
        .map_err(|error| format!("create {}: {error}", output_dir.display()))?;
    let input = read_wav(input_path)?;
    let stem = input_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("denoise");
    let mut report = String::from(
        "mode\tsamples\tduration_s\trms\tpeak\tclipped\tnon_finite\tprocess_ms\trtf\n",
    );

    for mode in Mode::ALL {
        let (output, elapsed) = process(mode, &input)?;
        let stats = signal_stats(&output);
        let duration = output.len() as f64 / SAMPLE_RATE as f64;
        let rtf = elapsed.as_secs_f64() / duration.max(f64::EPSILON);
        let output_path = output_dir.join(format!("{stem}-{}.wav", mode.suffix()));
        write_wav_f32(&output_path, &output)?;
        let row = format!(
            "{}\t{}\t{duration:.6}\t{:.8}\t{:.8}\t{}\t{}\t{:.3}\t{rtf:.6}\n",
            mode.suffix(),
            output.len(),
            stats.rms,
            stats.peak,
            stats.clipped,
            stats.non_finite,
            elapsed.as_secs_f64() * 1000.0,
        );
        print!("{row}");
        report.push_str(&row);
    }
    fs::write(output_dir.join(format!("{stem}-metrics.tsv")), report)
        .map_err(|error| format!("write metrics: {error}"))?;
    Ok(())
}

fn process(mode: Mode, input: &[f32]) -> Result<(Vec<f32>, Duration), String> {
    let mut processor = mode.processor()?;
    let frame_count = input.len().div_ceil(FRAME_SAMPLES);
    let flush_frames = matches!(mode, Mode::DeepFilterNet | Mode::DeepFilterNetStrong)
        .then_some(DEEP_FILTER_TOTAL_DELAY_SAMPLES.div_ceil(FRAME_SAMPLES))
        .unwrap_or_default();
    let started = Instant::now();
    let mut framed = Vec::with_capacity((frame_count + flush_frames) * FRAME_SAMPLES);

    for frame_index in 0..frame_count + flush_frames {
        let mut frame = [0.0; FRAME_SAMPLES];
        if frame_index < frame_count {
            let start = frame_index * FRAME_SAMPLES;
            let end = (start + FRAME_SAMPLES).min(input.len());
            frame[..end - start].copy_from_slice(&input[start..end]);
        }
        processor.process_capture(&mut frame);
        // Unit tests may run beside CPU-heavy DSP tests and otherwise feed the
        // asynchronous DeepFilterNet worker much faster than a real capture
        // device can. Give that worker a scheduling opportunity without
        // changing the release regression runner or its timing measurements.
        #[cfg(test)]
        if matches!(mode, Mode::DeepFilterNet | Mode::DeepFilterNetStrong) {
            std::thread::sleep(Duration::from_millis(1));
        }
        if let Some(error) = processor.take_error() {
            return Err(format!("{} frame {frame_index}: {error}", mode.suffix()));
        }
        framed.extend_from_slice(&frame);
    }
    let elapsed = started.elapsed();

    // Compensate both the worker's 40 ms queue and the bundled model's 30 ms
    // STFT/lookahead delay. Flush with silence so blind-listening files retain
    // the same sample-zero alignment and exact length as the input.
    let start = if matches!(mode, Mode::DeepFilterNet | Mode::DeepFilterNetStrong) {
        DEEP_FILTER_TOTAL_DELAY_SAMPLES
    } else {
        0
    };
    Ok((framed[start..start + input.len()].to_vec(), elapsed))
}

fn signal_stats(samples: &[f32]) -> SignalStats {
    let mut sum = 0.0_f64;
    let mut peak = 0.0_f32;
    let mut clipped = 0usize;
    let mut non_finite = 0usize;
    for &sample in samples {
        if !sample.is_finite() {
            non_finite += 1;
            continue;
        }
        sum += f64::from(sample) * f64::from(sample);
        peak = peak.max(sample.abs());
        clipped += usize::from(sample.abs() >= 1.0);
    }
    SignalStats {
        rms: (sum / samples.len().max(1) as f64).sqrt(),
        peak,
        clipped,
        non_finite,
    }
}

fn generate_fixtures(directory: &Path) -> Result<(), String> {
    fs::create_dir_all(directory)
        .map_err(|error| format!("create {}: {error}", directory.display()))?;
    let background = background_noise();
    let cup = cup_noise();
    let keyboard = keyboard_noise();
    let voice = voice_signal(false);
    let voice_with_gap = voice_signal(true);
    let cases = [
        ("01-background-only", background),
        ("02-cup-only", cup.clone()),
        ("03-keyboard-only", keyboard.clone()),
        ("04-voice-only", voice.clone()),
        ("05-voice-plus-cup", mix(&voice, &cup)),
        ("06-voice-plus-keyboard", mix(&voice, &keyboard)),
        ("07-transient-in-speech-gap", mix(&voice_with_gap, &cup)),
    ];
    for (name, samples) in cases {
        write_wav_f32(&directory.join(format!("{name}.wav")), &samples)?;
    }
    Ok(())
}

fn fixture_len() -> usize {
    SAMPLE_RATE as usize * 3
}

fn background_noise() -> Vec<f32> {
    let mut state = 0x4d59_5df4_u32;
    let mut low = 0.0_f32;
    (0..fixture_len())
        .map(|index| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let white = ((state >> 8) as f32 / 8_388_608.0 - 1.0) * 0.012;
            low = low * 0.995 + white * 0.005;
            let hum =
                (index as f32 * 120.0 * std::f32::consts::TAU / SAMPLE_RATE as f32).sin() * 0.006;
            (white * 0.45 + low * 4.0 + hum).clamp(-1.0, 1.0)
        })
        .collect()
}

fn cup_noise() -> Vec<f32> {
    let mut output = vec![0.0; fixture_len()];
    for start_ms in [350usize, 820, 1_310, 1_850, 2_330] {
        let start = start_ms * SAMPLE_RATE as usize / 1_000;
        let length = SAMPLE_RATE as usize / 7;
        for offset in 0..length.min(output.len() - start) {
            let time = offset as f32 / SAMPLE_RATE as f32;
            let envelope = (1.0 - offset as f32 / length as f32).powi(2);
            let scrape = (time * (1_900.0 + time * 3_200.0) * std::f32::consts::TAU).sin();
            let ring = (time * 4_700.0 * std::f32::consts::TAU).sin();
            output[start + offset] += (scrape * 0.12 + ring * 0.035) * envelope;
        }
    }
    output
}

fn keyboard_noise() -> Vec<f32> {
    let mut output = vec![0.0; fixture_len()];
    for start_ms in [260usize, 510, 765, 1_020, 1_560, 1_820, 2_080, 2_590] {
        let start = start_ms * SAMPLE_RATE as usize / 1_000;
        let length = SAMPLE_RATE as usize / 80;
        for offset in 0..length.min(output.len() - start) {
            let envelope = (-8.0 * offset as f32 / length as f32).exp();
            let alternating = if offset & 1 == 0 { 1.0 } else { -1.0 };
            output[start + offset] += alternating * envelope * 0.16;
        }
    }
    output
}

fn voice_signal(with_middle_gap: bool) -> Vec<f32> {
    (0..fixture_len())
        .map(|index| {
            let time = index as f32 / SAMPLE_RATE as f32;
            let syllable = (time * 3.2 * std::f32::consts::TAU).sin().max(0.0).powi(2);
            let middle_gate = if with_middle_gap && (1.15..1.85).contains(&time) {
                0.0
            } else {
                1.0
            };
            let pitch = 145.0 + (time * 1.7).sin() * 22.0;
            let phase = time * pitch * std::f32::consts::TAU;
            let voiced =
                phase.sin() * 0.10 + (phase * 2.0).sin() * 0.045 + (phase * 3.0).sin() * 0.022;
            (voiced * syllable * middle_gate).clamp(-1.0, 1.0)
        })
        .collect()
}

fn mix(left: &[f32], right: &[f32]) -> Vec<f32> {
    left.iter()
        .zip(right)
        .map(|(left, right)| (left + right).clamp(-1.0, 1.0))
        .collect()
}

fn read_raw_f32(path: &Path) -> Result<Vec<f32>, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    if bytes.len() % 4 != 0 {
        return Err("raw f32 input length is not divisible by four".into());
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
        .collect())
}

fn read_wav(path: &Path) -> Result<Vec<f32>, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("input is not a RIFF/WAVE file".into());
    }
    let mut format = None;
    let mut data = None;
    let mut cursor = 12usize;
    while cursor + 8 <= bytes.len() {
        let id = &bytes[cursor..cursor + 4];
        let size = u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
        let start = cursor + 8;
        let end = start.checked_add(size).ok_or("invalid WAV chunk size")?;
        if end > bytes.len() {
            return Err("truncated WAV chunk".into());
        }
        if id == b"fmt " {
            format = Some(&bytes[start..end]);
        } else if id == b"data" {
            data = Some(&bytes[start..end]);
        }
        cursor = end + (size & 1);
    }
    let format = format.ok_or("WAV has no fmt chunk")?;
    let data = data.ok_or("WAV has no data chunk")?;
    if format.len() < 16 {
        return Err("WAV fmt chunk is too short".into());
    }
    let encoding = u16::from_le_bytes(format[0..2].try_into().unwrap());
    let channels = u16::from_le_bytes(format[2..4].try_into().unwrap());
    let sample_rate = u32::from_le_bytes(format[4..8].try_into().unwrap());
    let bits = u16::from_le_bytes(format[14..16].try_into().unwrap());
    if channels != 1 || sample_rate != SAMPLE_RATE {
        return Err(format!(
            "input must be mono {SAMPLE_RATE} Hz, got {channels} channel(s) at {sample_rate} Hz"
        ));
    }
    match (encoding, bits) {
        (1, 16) => Ok(data
            .chunks_exact(2)
            .map(|bytes| i16::from_le_bytes(bytes.try_into().unwrap()) as f32 / 32_768.0)
            .collect()),
        (3, 32) => Ok(data
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
            .collect()),
        _ => Err(format!(
            "unsupported WAV format tag {encoding}, {bits} bits; use PCM16 or float32"
        )),
    }
}

fn write_wav_f32(path: &Path, samples: &[f32]) -> Result<(), String> {
    let data_bytes = samples
        .len()
        .checked_mul(4)
        .and_then(|size| u32::try_from(size).ok())
        .ok_or("WAV is too large")?;
    let riff_bytes = 36u32.checked_add(data_bytes).ok_or("WAV is too large")?;
    let mut file =
        File::create(path).map_err(|error| format!("create {}: {error}", path.display()))?;
    file.write_all(b"RIFF")
        .and_then(|_| file.write_all(&riff_bytes.to_le_bytes()))
        .and_then(|_| file.write_all(b"WAVEfmt "))
        .and_then(|_| file.write_all(&16u32.to_le_bytes()))
        .and_then(|_| file.write_all(&3u16.to_le_bytes()))
        .and_then(|_| file.write_all(&1u16.to_le_bytes()))
        .and_then(|_| file.write_all(&SAMPLE_RATE.to_le_bytes()))
        .and_then(|_| file.write_all(&(SAMPLE_RATE * 4).to_le_bytes()))
        .and_then(|_| file.write_all(&4u16.to_le_bytes()))
        .and_then(|_| file.write_all(&32u16.to_le_bytes()))
        .and_then(|_| file.write_all(b"data"))
        .and_then(|_| file.write_all(&data_bytes.to_le_bytes()))
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    for &sample in samples {
        let sample = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        file.write_all(&sample.to_le_bytes())
            .map_err(|error| format!("write {}: {error}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_round_trip_preserves_f32_samples() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("roundtrip.wav");
        let input = vec![-1.0, -0.25, 0.0, 0.5, 1.0];
        write_wav_f32(&path, &input).unwrap();
        assert_eq!(read_wav(&path).unwrap(), input);
    }

    #[test]
    fn passthrough_is_sample_exact_and_length_preserving() {
        let input = (0..FRAME_SAMPLES * 3 + 17)
            .map(|index| (index as f32 * 0.013).sin() * 0.2)
            .collect::<Vec<_>>();
        let (output, _) = process(Mode::Original, &input).unwrap();
        assert_eq!(output, input);
    }

    #[test]
    fn regression_fixtures_cover_all_seven_required_scenarios() {
        let directory = tempfile::tempdir().unwrap();
        generate_fixtures(directory.path()).unwrap();
        let mut names = fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names.len(), 7);
        assert_eq!(names.first().unwrap(), "01-background-only.wav");
        assert_eq!(names.last().unwrap(), "07-transient-in-speech-gap.wav");
        for name in names {
            let samples = read_wav(&directory.path().join(name)).unwrap();
            assert_eq!(samples.len(), fixture_len());
            assert!(samples.iter().all(|sample| sample.is_finite()));
        }
    }

    #[test]
    #[ignore = "real neural-model regression; run this example test in release mode"]
    fn all_modes_are_length_safe_finite_and_distinct() {
        let voice = voice_signal(false);
        let input = mix(&voice, &background_noise());
        let mut outputs = Vec::new();
        for mode in Mode::ALL {
            let (output, _) = process(mode, &input).unwrap();
            assert_eq!(output.len(), input.len(), "{}", mode.suffix());
            assert!(output.iter().all(|sample| sample.is_finite()));
            assert!(output.iter().all(|sample| sample.abs() <= 1.0));
            outputs.push(output);
        }
        assert_eq!(outputs[0], input);
        for left in 0..outputs.len() {
            for right in left + 1..outputs.len() {
                let difference_rms = outputs[left]
                    .iter()
                    .zip(&outputs[right])
                    .map(|(raw, processed)| {
                        let difference = raw - processed;
                        difference * difference
                    })
                    .sum::<f32>()
                    / input.len() as f32;
                assert!(
                    difference_rms.sqrt() > 1.0e-4,
                    "{} unexpectedly matched {}",
                    Mode::ALL[left].suffix(),
                    Mode::ALL[right].suffix()
                );
            }
        }
    }
}
