// SPDX-License-Identifier: MPL-2.0

//! A value that travels from 0 to 1 and back over a fixed duration.
//!
//! ## Why a deadline rather than a per-frame step
//!
//! This is the rule `crates/koru-app-shell/src/panel.rs` states and this module
//! exists to share: `View::animating` must return to `false` or the loop spins
//! at frame rate forever. A per-frame decrement can be stranded by a dropped
//! frame — it never reaches its end, so `animating` never goes quiet. Elapsed
//! time against a fixed duration cannot be stranded, because the clock keeps
//! moving whether or not a frame was drawn.
//!
//! ## Why it is here and not beside the thing that animates
//!
//! Both windows morph the caption's window-menu button into an ✕, and `apps/koru` and
//! `apps/koru-document` may not depend on each other. `koru-draw` is the crate
//! they share, and a tween names no node, no document and no view — which is
//! also why it is not a second copy inside `Panel`. `Panel` keeps its own
//! progress because it animates a *width* and carries two of them; this animates
//! a bare number.

use std::time::Instant;

/// A 0→1 value with a duration, at rest whenever it has arrived.
#[derive(Clone, Debug)]
pub struct Tween {
    /// How long a full traverse takes, in milliseconds.
    duration_ms: f32,
    /// Where it is now, 0.0 to 1.0.
    progress: f32,
    /// Whether it is heading for 1.0 rather than 0.0.
    forward: bool,
    /// When the current run began, and the progress it began from. `None` at
    /// rest — and that is what [`animating`](Self::animating) reports.
    run: Option<(Instant, f32)>,
}

impl Tween {
    /// A tween sitting at one end, with nothing in flight.
    pub fn new(duration_ms: f32, on: bool) -> Self {
        Self {
            duration_ms,
            progress: if on { 1.0 } else { 0.0 },
            forward: on,
            run: None,
        }
    }

    /// How far through it is: 0 at the start, 1 arrived.
    pub fn progress(&self) -> f32 {
        self.progress
    }

    /// Whether it is heading for — or resting at — 1.0.
    pub fn is_on(&self) -> bool {
        self.forward
    }

    pub fn animating(&self) -> bool {
        self.run.is_some()
    }

    /// Send it to one end. Returns whether that changed the direction.
    ///
    /// Re-aiming at the end it is already heading for does nothing, so a stream
    /// of identical calls — one per frame, say — cannot restart the run and pin
    /// it short of arriving.
    pub fn set(&mut self, on: bool, now: Instant) -> bool {
        if self.forward == on {
            return false;
        }
        self.forward = on;
        // From wherever it actually is, not from the far end: reversing halfway
        // must turn round on the spot rather than jump.
        self.run = Some((now, self.progress));
        true
    }

    /// Put it at one end with no animation.
    ///
    /// For `restore`: a window coming back with no menu open must not turn a
    /// glyph nobody touched, and `animating` would be true before the first
    /// frame had been drawn.
    pub fn set_immediately(&mut self, on: bool) {
        self.forward = on;
        self.progress = if on { 1.0 } else { 0.0 };
        self.run = None;
    }

    /// Advance to `now`. Returns whether anything moved.
    pub fn advance(&mut self, now: Instant) -> bool {
        let Some((started, from)) = self.run else {
            return false;
        };
        // A duration of zero arrives at once rather than never. Dividing by a
        // clamped epsilon would give `t == 0` for the first frame and leave it
        // stuck there, which is the opposite of what "no animation" means.
        let secs = self.duration_ms / 1000.0;
        let t = if secs <= 0.0 {
            1.0
        } else {
            (now.duration_since(started).as_secs_f32() / secs).clamp(0.0, 1.0)
        };
        let to = if self.forward { 1.0 } else { 0.0 };
        self.progress = from + (to - from) * crate::anim::ease_out_cubic(t);
        if t >= 1.0 {
            self.progress = to;
            self.run = None;
        }
        true
    }
}

#[cfg(test)]
mod tests;
