// SPDX-License-Identifier: MPL-2.0

//! A fixed-geometry pending indicator whose three dots breathe in sequence.

use std::f32::consts::PI;
use std::time::Instant;

use koru_draw::geometry::Rect;
use koru_draw::{anim, scene, theme};
use vello::Scene;
use vello::kurbo::{Affine, Circle};
use vello::peniko::Color;

const DOT_R: f32 = 2.5;
const GAP: f32 = 5.0;
const STAGGER: f32 = 0.15;
const WAVE: f32 = 0.55;
const MIN_ALPHA: f32 = 0.30;

/// Three travelling-wave dots for work whose completion cannot be estimated.
pub struct PulsingDots {
    started: Instant,
}

impl PulsingDots {
    pub fn new(now: Instant) -> Self {
        Self { started: now }
    }

    /// Pulses until its owner drops it after the pending work changes state.
    pub fn animating(&self) -> bool {
        true
    }

    pub fn draw(&self, canvas: &mut Scene, root: Affine, rect: Rect, now: Instant, color: Color) {
        let phase = anim::loop_phase(self.started, now, theme::PULSE_LOOP_MS);
        let center_y = rect.y + rect.h * 0.5;
        for n in 0..3 {
            let center_x = rect.x + DOT_R + n as f32 * (DOT_R * 2.0 + GAP);
            scene::fill(
                canvas,
                root,
                color.multiply_alpha(dot_alpha(phase, n)),
                &Circle::new((center_x as f64, center_y as f64), DOT_R as f64),
            );
        }
    }
}

fn dot_alpha(phase: f32, n: usize) -> f32 {
    let local = (phase - n as f32 * STAGGER).rem_euclid(1.0);
    let pulse = if local < WAVE {
        (PI * local / WAVE).sin()
    } else {
        0.0
    };
    MIN_ALPHA + (1.0 - MIN_ALPHA) * pulse
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_stays_inside_the_visible_range() {
        for step in 0..=1000 {
            let phase = step as f32 / 1000.0;
            for n in 0..3 {
                let alpha = dot_alpha(phase, n);
                assert!(
                    (MIN_ALPHA..=1.0).contains(&alpha),
                    "dot {n} at phase {phase} produced alpha {alpha}"
                );
            }
        }
    }

    #[test]
    fn dot_peaks_are_staggered() {
        let dot_zero_peak = WAVE * 0.5;
        let dot_one_peak = STAGGER + WAVE * 0.5;
        assert!(dot_alpha(dot_zero_peak, 0) > dot_alpha(dot_zero_peak, 1));
        assert!(dot_alpha(dot_one_peak, 1) > dot_alpha(dot_one_peak, 0));
    }

    #[test]
    fn indicator_animates_while_owned() {
        assert!(PulsingDots::new(Instant::now()).animating());
    }
}
