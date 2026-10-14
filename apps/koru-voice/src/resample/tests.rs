// SPDX-License-Identifier: MPL-2.0

use super::*;

fn resample(input_rate: u32, output_rate: u32, samples: &[f32]) -> Vec<f32> {
    let mut resampler = Resampler::new(input_rate, output_rate);
    let mut out = Vec::new();
    for sample in samples {
        resampler.push(*sample, |s| out.push(s));
    }
    out
}

/// A device already at the engine's rate must not be touched at all — this is
/// the only path with no aliasing, and it is common enough (a headset that
/// reports 16 kHz) to be worth a test of its own.
#[test]
fn a_matching_rate_passes_through_unchanged() {
    let mut resampler = Resampler::new(16_000, 16_000);
    assert!(resampler.is_identity());
    let input: Vec<f32> = (0..10).map(|n| n as f32).collect();
    let mut out = Vec::new();
    for sample in &input {
        resampler.push(*sample, |s| out.push(s));
    }
    // The first sample is the anchor, so the output trails by one and is
    // otherwise identical.
    assert_eq!(out, input[..input.len() - 1].to_vec());
}

/// Three to one, which is 48 kHz to 16 kHz — the case nearly every desktop
/// takes.
#[test]
fn forty_eight_to_sixteen_keeps_about_a_third_of_the_samples() {
    let input: Vec<f32> = (0..3_000).map(|n| n as f32).collect();
    let out = resample(48_000, 16_000, &input);
    let expected = input.len() / 3;
    assert!(
        out.len().abs_diff(expected) <= 2,
        "{} samples from {}, expected about {expected}",
        out.len(),
        input.len()
    );
}

/// 44.1 kHz is the other common one, and its ratio is not an integer — which is
/// the case a naive "take every Nth sample" would get wrong by a slowly
/// accumulating drift rather than by an obvious error.
#[test]
fn a_fractional_ratio_does_not_drift() {
    let seconds = 10;
    let input: Vec<f32> = (0..44_100 * seconds).map(|n| n as f32).collect();
    let out = resample(44_100, 16_000, &input);
    let expected = 16_000 * seconds as usize;
    assert!(
        out.len().abs_diff(expected) <= 2,
        "{} samples over {seconds}s, expected {expected}",
        out.len()
    );
}

/// Upsampling emits more than one sample per input, which is the branch the
/// `while` loop exists for.
#[test]
fn upsampling_emits_several_samples_per_input() {
    let out = resample(8_000, 16_000, &[0.0, 1.0, 2.0, 3.0]);
    assert_eq!(out.len(), 6);
    // Interpolated halfway between each pair.
    assert_eq!(out, vec![0.0, 0.5, 1.0, 1.5, 2.0, 2.5]);
}

/// A ramp stays a ramp: linear interpolation over a linear signal is exact, so
/// this catches a phase that is off by a sample without needing a tolerance.
#[test]
fn interpolation_over_a_ramp_is_exact() {
    let input: Vec<f32> = (0..12).map(|n| n as f32).collect();
    let out = resample(24_000, 16_000, &input);
    for (at, value) in out.iter().enumerate() {
        let expected = at as f32 * 1.5;
        assert!(
            (value - expected).abs() < 1e-4,
            "sample {at} is {value}, expected {expected}"
        );
    }
}

/// The phase survives between buffers. A resampler restarted per callback
/// produces a discontinuity at exactly the callback rate, which is a tone.
#[test]
fn the_phase_survives_between_buffers() {
    let whole: Vec<f32> = (0..30).map(|n| n as f32).collect();
    let one_go = resample(48_000, 16_000, &whole);

    let mut resampler = Resampler::new(48_000, 16_000);
    let mut piecewise = Vec::new();
    for chunk in whole.chunks(7) {
        for sample in chunk {
            resampler.push(*sample, |s| piecewise.push(s));
        }
    }
    assert_eq!(one_go, piecewise);
}

#[test]
fn a_stereo_frame_is_averaged_rather_than_halved() {
    let mut out = Vec::new();
    Downmix::new(2).frames(&[1.0, 3.0, -1.0, 1.0], |s| out.push(s));
    assert_eq!(out, vec![2.0, 0.0]);
}

/// A trailing partial frame is dropped rather than padded: a padded frame is a
/// click, a dropped one is nothing.
#[test]
fn a_partial_trailing_frame_is_dropped() {
    let mut out = Vec::new();
    Downmix::new(2).frames(&[1.0, 1.0, 5.0], |s| out.push(s));
    assert_eq!(out, vec![1.0]);
}

/// A backend claiming no channels is broken, and refusing to open a microphone
/// over it would be a worse answer than treating it as mono.
#[test]
fn zero_channels_is_treated_as_mono() {
    let mix = Downmix::new(0);
    assert_eq!(mix.channels(), 1);
    let mut out = Vec::new();
    mix.frames(&[1.0, 2.0], |s| out.push(s));
    assert_eq!(out, vec![1.0, 2.0]);
    assert!(Resampler::new(0, 16_000).is_identity());
}
