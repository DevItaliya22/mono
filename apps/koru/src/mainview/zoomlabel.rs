// SPDX-License-Identifier: MPL-2.0

//! The transient "120%" indicator shown when the display zoom changes.
//!
//! Zoom itself is a shell-level concern — a multiplier on the root transform —
//! so this view only *displays* it: it notices the `theme::zoom()` global moved
//! and flashes the percentage centred on screen for a moment, like a TV's
//! volume bar. Modelled on [`koru_app_shell::Panel`]: a deadline drives
//! the fade so a dropped frame cannot strand it, and [`animating`](ZoomLabel::animating)
//! returns to `false` on its own (ADR-26).

use std::time::Instant;

/// How long the indicator stays at full opacity before it begins to fade.
const HOLD_SECS: f32 = 0.7;
/// Full duration from appearing to gone. The ease occupies the tail after the hold.
const TOTAL_SECS: f32 = 1.2;

pub struct ZoomLabel {
    /// When the current flash began. `None` at rest — which is what
    /// [`animating`](Self::animating) reports.
    shown: Option<Instant>,
}

impl ZoomLabel {
    pub const fn new() -> Self {
        Self { shown: None }
    }

    /// (Re)arm. Each change resets the deadline, so rapid Ctrl+= presses keep
    /// the indicator up rather than letting an earlier flash expire under it.
    pub fn show(&mut self, now: Instant) {
        self.shown = Some(now);
    }

    pub fn animating(&self) -> bool {
        self.shown.is_some()
    }

    /// Advance one frame. Returns the alpha to draw at, or `None` once the flash
    /// is over — and clears `shown` so [`animating`](Self::animating) reaches
    /// `false`. Driven off elapsed time against a fixed duration, never a
    /// per-frame decrement, so a dropped frame cannot leave it stuck on.
    pub fn tick(&mut self, now: Instant) -> Option<f32> {
        let started = self.shown?;
        let t = now.duration_since(started).as_secs_f32();
        if t >= TOTAL_SECS {
            self.shown = None;
            return None;
        }
        let a = if t < HOLD_SECS {
            1.0
        } else {
            1.0 - (t - HOLD_SECS) / (TOTAL_SECS - HOLD_SECS)
        };
        Some(a.clamp(0.0, 1.0))
    }
}

impl Default for ZoomLabel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn at_rest_it_is_not_animating() {
        let z = ZoomLabel::new();
        assert!(!z.animating());
    }

    /// **The loop-safety rule.** `animating` has to reach `false`, or the shell
    /// wakes at frame rate forever.
    #[test]
    fn a_flash_fades_out_and_stops_asking_for_frames() {
        let now = Instant::now();
        let mut z = ZoomLabel::new();
        z.show(now);
        assert!(z.animating());

        // Inside the hold: solid.
        assert_eq!(z.tick(now + Duration::from_millis(100)), Some(1.0));
        assert!(z.animating(), "still up during the hold");

        // Past the hold, partway through the fade: translucent.
        let mid = z.tick(now + Duration::from_millis(950));
        assert!(mid.is_some_and(|a| a > 0.0 && a < 1.0), "mid-fade");

        // Well past the end: gone, and no longer animating.
        assert_eq!(z.tick(now + Duration::from_secs(5)), None);
        assert!(!z.animating(), "the flash ended");
    }

    /// A change while a flash is already up restarts it, rather than letting
    /// the old one expire mid-stream.
    #[test]
    fn a_rearm_resets_the_deadline() {
        let now = Instant::now();
        let mut z = ZoomLabel::new();
        z.show(now);
        // Re-arm well past when the first would have expired.
        let restart = now + Duration::from_secs(5);
        z.show(restart);
        assert_eq!(
            z.tick(restart + Duration::from_millis(50)),
            Some(1.0),
            "re-arm restarts the hold"
        );
        assert!(z.animating());
    }
}
