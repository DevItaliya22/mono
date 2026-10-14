// SPDX-License-Identifier: MPL-2.0

//! Stateless animation math shared by views and composed widgets.

use std::time::Instant;

/// Ease-out cubic: the app's one easing curve for motion that settles into
/// place (`docs/features/koruui/ANIMATION.md`).
pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// The repeating phase of an animation loop, from `0.0` up to (but not
/// including) `1.0`.
///
/// A non-positive duration means there is no loop, matching the zero-duration
/// guard used by [`crate::tween::Tween`].
pub fn loop_phase(started: Instant, now: Instant, loop_ms: f32) -> f32 {
    if loop_ms <= 0.0 {
        return 0.0;
    }
    (now.saturating_duration_since(started).as_secs_f32() / (loop_ms / 1000.0)).fract()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn cubic_easing_keeps_both_endpoints() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
    }

    #[test]
    fn loop_phase_starts_advances_and_wraps() {
        let started = Instant::now();
        assert_eq!(loop_phase(started, started, 1200.0), 0.0);
        assert_eq!(
            loop_phase(started, started + Duration::from_millis(600), 1200.0),
            0.5
        );
        assert_eq!(
            loop_phase(started, started + Duration::from_millis(1200), 1200.0),
            0.0
        );
    }

    #[test]
    fn non_positive_loop_duration_has_no_phase() {
        let started = Instant::now();
        let later = started + Duration::from_secs(1);
        assert_eq!(loop_phase(started, later, 0.0), 0.0);
        assert_eq!(loop_phase(started, later, -1.0), 0.0);
    }
}
