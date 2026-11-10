//! Local processing cost only, not device latency. Prefer --release for comparisons.
use std::time::Instant;
use edenvoice_client::{codec::FRAME_SAMPLES, dsp::{CaptureAudioProcessor,NoiseSuppressionStrength}};
fn main() {
    for unused in [false,true] {
        let mut processor=CaptureAudioProcessor::standard(1.0,true,true,NoiseSuppressionStrength::Medium).unwrap();
        processor.set_output_unused(unused);
        let source: [f32;FRAME_SAMPLES]=std::array::from_fn(|i|(i as f32*0.03).sin()*0.1);
        let render=[0.01;FRAME_SAMPLES];
        for _ in 0..100 { let mut frame=source;processor.process_render_reference(&render);processor.process_capture(&mut frame); }
        let started=Instant::now();
        for _ in 0..5000 { let mut frame=source;processor.process_render_reference(&render);std::hint::black_box(processor.process_capture(&mut frame)); }
        println!("muted={unused}: {:.3} us/20ms frame",started.elapsed().as_secs_f64()*1_000_000.0/5000.0);
        assert!(processor.take_error().is_none());
    }
}
