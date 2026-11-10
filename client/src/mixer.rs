pub fn mix(frames: &[&[f32]], output: &mut [f32], volume: f32) {
    mix_iter(frames.iter().copied(), output, volume);
}

/// Mix borrowed frames without collecting a temporary vector of references.
pub fn mix_iter<'a>(frames: impl IntoIterator<Item = &'a [f32]>, output: &mut [f32], volume: f32) {
    output.fill(0.0);
    for frame in frames {
        for (target, sample) in output.iter_mut().zip(frame.iter()) {
            *target += *sample;
        }
    }
    for sample in output {
        *sample = soft_limit(*sample * volume);
    }
}

const LIMITER_THRESHOLD: f32 = 0.85;

fn soft_limit(sample: f32) -> f32 {
    if !sample.is_finite() {
        return 0.0;
    }
    let magnitude = sample.abs();
    if magnitude <= LIMITER_THRESHOLD {
        return sample;
    }
    let headroom = 1.0 - LIMITER_THRESHOLD;
    let limited =
        LIMITER_THRESHOLD + headroom * (1.0 - (-(magnitude - LIMITER_THRESHOLD) / headroom).exp());
    sample.signum() * limited.min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sums_and_soft_limits_without_overflow() {
        let a = [0.75, -0.75, 0.25];
        let b = [0.75, -0.75, -0.5];
        let mut output = [0.0; 3];
        mix(&[&a, &b], &mut output, 1.0);
        assert!(output[0] > 0.85 && output[0] < 1.0);
        assert!(output[1] < -0.85 && output[1] > -1.0);
        assert_eq!(output[2], -0.25);
    }
    #[test]
    fn handles_different_frame_lengths() {
        let a = [0.5];
        let b = [0.25, 1.0];
        let mut output = [0.0; 2];
        mix(&[&a, &b], &mut output, 0.5);
        assert_eq!(output, [0.375, 0.5]);
    }

    #[test]
    fn mixes_five_participants() {
        let frames = [[0.1; 960], [0.2; 960], [-0.1; 960], [0.3; 960], [0.05; 960]];
        let refs: Vec<&[f32]> = frames.iter().map(|frame| frame.as_slice()).collect();
        let mut output = [0.0; 960];
        mix(&refs, &mut output, 1.0);
        assert!(output.iter().all(|sample| (*sample - 0.55).abs() < 0.0001));
    }

    #[test]
    fn reusable_frames_match_original_mix_across_room_sizes() {
        let mut reusable = Vec::<[f32; 960]>::with_capacity(6);
        for count in [0, 1, 5, 6, 12, 32, 2, 0, 32] {
            let capacity = reusable.capacity();
            let allocation = reusable.as_ptr();
            reusable.clear();
            reusable.extend((0..count).map(|peer| {
                std::array::from_fn(|sample| ((peer * 17 + sample) % 31) as f32 / 10.0 - 1.5)
            }));
            if count <= capacity {
                assert_eq!(
                    reusable.as_ptr(),
                    allocation,
                    "steady-state buffer reallocated"
                );
                assert_eq!(reusable.capacity(), capacity);
            }
            for volume in [0.0, 0.01, 1.0, 1.6, 2.0] {
                // Independent reference preserves the pre-change summation order,
                // gain and limiter. The wrapper itself now uses mix_iter too.
                let mut expected = [0.0; 960];
                for frame in &reusable {
                    for (target, sample) in expected.iter_mut().zip(frame) {
                        *target += *sample;
                    }
                }
                for sample in &mut expected {
                    *sample = soft_limit(*sample * volume);
                }
                let mut actual = [123.0; 960];
                mix_iter(
                    reusable.iter().map(|frame| frame.as_slice()),
                    &mut actual,
                    volume,
                );
                assert_eq!(actual, expected);
            }
        }
    }

    #[test]
    fn limiter_is_transparent_below_threshold_and_sanitizes_non_finite_input() {
        assert_eq!(soft_limit(0.5), 0.5);
        assert_eq!(soft_limit(-0.85), -0.85);
        assert_eq!(soft_limit(f32::NAN), 0.0);
        assert!(soft_limit(10.0) <= 1.0);
        assert!(soft_limit(-10.0) >= -1.0);
    }
}
