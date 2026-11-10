use std::time::{Duration, Instant};

use df::tract::{DfParams, DfTract, RuntimeParams};
use edenvoice_client::{
    codec::FRAME_SAMPLES,
    dsp::{CaptureAudioProcessor, NoiseSuppressionStrength},
};
use ndarray::{ArrayView2, ArrayViewMut2};

const WARMUP_FRAMES: usize = 100;
const MEASURED_FRAMES: usize = 5_000;

fn main() {
    println!("| Engine | Frames | Average ms | Peak ms | Real-time factor | >20 ms |");
    println!("|---|---:|---:|---:|---:|---:|");
    benchmark("Off", CaptureAudioProcessor::off(1.0, false, false));
    benchmark(
        "Standard + AEC + AGC",
        CaptureAudioProcessor::standard(1.0, true, true, NoiseSuppressionStrength::Medium)
            .expect("WebRTC APM should initialize"),
    );
    benchmark(
        "RNNoise + AEC + AGC",
        CaptureAudioProcessor::rnnoise(1.0, true, true),
    );
    benchmark_deep_filter_net();
}

fn benchmark_deep_filter_net() {
    let mut model = DfTract::new(DfParams::default(), &RuntimeParams::default())
        .expect("DeepFilterNet model should initialize");
    assert_eq!((model.sr, model.hop_size, model.ch), (48_000, 480, 1));
    let mut output = [0.0; FRAME_SAMPLES];
    for index in 0..WARMUP_FRAMES {
        let input = synthetic_frame(index);
        process_deep_filter_frame(&mut model, &input, &mut output);
    }

    let started = Instant::now();
    let mut peak = Duration::ZERO;
    let mut misses = 0usize;
    let mut checksum = 0.0_f32;
    for index in WARMUP_FRAMES..WARMUP_FRAMES + MEASURED_FRAMES {
        let input = synthetic_frame(index);
        let frame_started = Instant::now();
        process_deep_filter_frame(&mut model, &input, &mut output);
        let elapsed = frame_started.elapsed();
        peak = peak.max(elapsed);
        misses += usize::from(elapsed > Duration::from_millis(20));
        checksum += output[index % FRAME_SAMPLES];
    }
    let elapsed = started.elapsed();
    let average_ms = elapsed.as_secs_f64() * 1_000.0 / MEASURED_FRAMES as f64;
    let realtime_factor = elapsed.as_secs_f64() / (MEASURED_FRAMES as f64 * 0.020);
    println!(
        "| DeepFilterNet | {MEASURED_FRAMES} | {average_ms:.4} | {:.4} | {realtime_factor:.4} | {misses} |",
        peak.as_secs_f64() * 1_000.0,
    );
    std::hint::black_box(checksum);
}

fn process_deep_filter_frame(
    model: &mut DfTract,
    input: &[f32; FRAME_SAMPLES],
    output: &mut [f32; FRAME_SAMPLES],
) {
    for (input, output) in input.chunks_exact(480).zip(output.chunks_exact_mut(480)) {
        let input = ArrayView2::from_shape((1, 480), input).unwrap();
        let output = ArrayViewMut2::from_shape((1, 480), output).unwrap();
        model.process(input, output).unwrap();
    }
}

fn benchmark(name: &str, mut processor: CaptureAudioProcessor) {
    let render = [0.0; FRAME_SAMPLES];
    for index in 0..WARMUP_FRAMES {
        let mut frame = synthetic_frame(index);
        processor.process_render_reference(&render);
        processor.process_capture(&mut frame);
    }

    let started = Instant::now();
    let mut peak = Duration::ZERO;
    let mut misses = 0usize;
    let mut checksum = 0.0_f32;
    for index in WARMUP_FRAMES..WARMUP_FRAMES + MEASURED_FRAMES {
        let mut frame = synthetic_frame(index);
        let frame_started = Instant::now();
        processor.process_render_reference(&render);
        let analysis = processor.process_capture(&mut frame);
        let elapsed = frame_started.elapsed();
        peak = peak.max(elapsed);
        misses += usize::from(elapsed > Duration::from_millis(20));
        checksum += analysis.level.rms + frame[index % FRAME_SAMPLES];
    }
    let elapsed = started.elapsed();
    let average_ms = elapsed.as_secs_f64() * 1_000.0 / MEASURED_FRAMES as f64;
    let realtime_factor = elapsed.as_secs_f64() / (MEASURED_FRAMES as f64 * 0.020);
    println!(
        "| {name} | {MEASURED_FRAMES} | {average_ms:.4} | {:.4} | {realtime_factor:.4} | {misses} |",
        peak.as_secs_f64() * 1_000.0,
    );
    std::hint::black_box(checksum);
}

fn synthetic_frame(frame_index: usize) -> [f32; FRAME_SAMPLES] {
    std::array::from_fn(|sample| {
        let absolute = frame_index * FRAME_SAMPLES + sample;
        let voice = (absolute as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.08;
        let noise = (((absolute.wrapping_mul(1_103_515_245).wrapping_add(12_345) >> 16) & 0xff)
            as f32
            / 255.0
            - 0.5)
            * 0.03;
        voice + noise
    })
}
