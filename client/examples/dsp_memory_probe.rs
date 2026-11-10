use std::{env, hint::black_box, thread, time::Duration};

use edenvoice_client::{
    codec::FRAME_SAMPLES,
    dsp::{CaptureAudioProcessor, NoiseSuppressionStrength},
};

const PROBE_FRAMES: usize = 200;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mode = env::args().nth(1).unwrap_or_else(|| "off".into());
    let mut processor = match mode.as_str() {
        "off" => CaptureAudioProcessor::off(1.0, false, false),
        "standard" => {
            CaptureAudioProcessor::standard(1.0, true, true, NoiseSuppressionStrength::Medium)
                .map_err(|error| format!("standard processor: {error}"))?
        }
        "rnnoise" => CaptureAudioProcessor::rnnoise(1.0, true, true),
        "deep-filter-net" => CaptureAudioProcessor::deep_filter_net(1.0, true, true, false)?,
        "deep-filter-net-strong" => CaptureAudioProcessor::deep_filter_net(1.0, true, true, true)?,
        _ => return Err(
            "usage: dsp_memory_probe [off|standard|rnnoise|deep-filter-net|deep-filter-net-strong]"
                .into(),
        ),
    };
    let render = [0.0; FRAME_SAMPLES];
    let mut checksum = 0.0_f32;
    for frame_index in 0..PROBE_FRAMES {
        let mut frame = std::array::from_fn(|sample| {
            let position = frame_index * FRAME_SAMPLES + sample;
            (position as f32 * 0.037).sin() * 0.05
        });
        processor.process_render_reference(&render);
        checksum += processor.process_capture(&mut frame).level.rms;
        thread::sleep(Duration::from_millis(1));
    }
    black_box(checksum);
    println!("mode={mode} frames={PROBE_FRAMES}");
    Ok(())
}
