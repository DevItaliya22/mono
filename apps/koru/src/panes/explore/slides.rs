// SPDX-License-Identifier: MPL-2.0

//! The vertical slide an explore row makes when the order changes under it.
//!
//! This is the row-tree counterpart of the tab strip's `TabSlides`, and it
//! works the same way for the same reason: [`ExploreView::rows`] stays the
//! live display truth, and what animates is only how far from its slot each
//! row is **drawn**. A click during the slide therefore lands on the row the
//! cache says is there, not on the one the eye has not caught up with — which
//! is the behaviour you want when a swap happens under a moving pointer.
//!
//! ## Why the caller computes the offsets
//!
//! A tab strip is linear: a reorder displaces its neighbours by exactly one
//! slot, so the slide math can assume ±`step`. A tree is not. Reparenting a
//! document moves its block many slots and shifts every row it passes, and a
//! multi-select carried block displaces its neighbours by the block's height
//! rather than by one row. So [`RowSlides`] takes the per-row offsets
//! **already computed** by the caller — [`relocate`] is given a `from` value
//! for every row, recorded against its *current* index, and it simply eases
//! each one to zero. The caller snapshots each row's drawn Y before the
//! reorder and subtracts its new slot Y; the difference is what goes in here.
//!
//! ## Why the row you are dragging does not slide
//!
//! It is already where the pointer put it — the drag state decides where it
//! draws, and this decides where everything *else* does. Easing it as well
//! would make it trail the cursor, which reads as lag rather than motion. The
//! carried row gets exactly one easing, on release: the caller folds its
//! pointer-delta into the `relocate` it starts then.
//!
//! ## Why a deadline rather than a per-frame step
//!
//! The same reason as the tab slide and the side columns: `View::animating`
//! must return to `false` or the loop spins at frame rate forever, and a
//! per-frame decrement can be stranded by a dropped frame where elapsed time
//! cannot.
//!
//! [`ExploreView::rows`]: super::ExploreView
//! [`relocate`]: RowSlides::relocate

use std::time::Instant;

use koru_draw::{anim::ease_out_cubic as ease, theme};

#[derive(Default)]
pub struct RowSlides {
    /// Per row, in current index order: how far from its slot it was when the
    /// current slide began, in logical pixels (positive = drawn below its
    /// slot). Empty at rest.
    from: Vec<f32>,
    /// How much of `from` is still owed, 1.0 at the start and 0.0 at rest.
    /// Advanced once per frame so every row in one frame agrees on the time.
    remaining: f32,
    /// When the current slide began. `None` at rest, and that is what
    /// [`animating`](Self::animating) reports.
    started: Option<Instant>,
}

impl RowSlides {
    /// Begin a slide from the given per-row offsets, recorded against each
    /// row's **current** index.
    ///
    /// `from_offsets[i]` is how far below its new slot the row now at `i`
    /// should be drawn, so that it eases up into place. A row that did not
    /// move takes `0.0` and is drawn at its slot throughout.
    ///
    /// A relocation mid-slide has to carry on from where the rows actually
    /// are rather than snapping back to their slots first, which is why the
    /// caller folds any offset still owed into the values it passes here.
    pub fn relocate(&mut self, from_offsets: Vec<f32>, now: Instant) {
        if from_offsets.iter().all(|&o| o == 0.0) {
            self.clear();
            return;
        }
        self.from = from_offsets;
        self.remaining = 1.0;
        self.started = Some(now);
    }

    /// Forget every slide.
    ///
    /// A rebuild renumbers the rows, at which point an offset recorded against
    /// an old index would be applied to a different row — one document
    /// visibly sliding because a *different* one moved. The same reason the
    /// tab slide clears on any structural change.
    pub fn clear(&mut self) {
        self.from.clear();
        self.remaining = 0.0;
        self.started = None;
    }

    pub fn animating(&self) -> bool {
        self.started.is_some()
    }

    /// Advance the slide. Returns whether anything moved.
    pub fn advance(&mut self, now: Instant) -> bool {
        let Some(started) = self.started else {
            return false;
        };
        let secs = (theme::ROW_SLIDE_MS / 1000.0).max(f32::EPSILON);
        let t = (now.duration_since(started).as_secs_f32() / secs).clamp(0.0, 1.0);
        self.remaining = 1.0 - ease(t);
        if t >= 1.0 {
            self.clear();
        }
        true
    }

    /// How far below its slot the row at `i` should be drawn.
    pub fn offset(&self, i: usize) -> f32 {
        self.from.get(i).copied().unwrap_or(0.0) * self.remaining
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(slides: &mut RowSlides, count: usize, step_secs: f32) {
        // Step the clock forward in fixed increments so the tests are
        // deterministic without sleeping.
        let mut t = Instant::now();
        for _ in 0..count {
            t += std::time::Duration::from_secs_f32(step_secs);
            slides.advance(t);
        }
    }

    #[test]
    fn at_rest_nothing_animates() {
        let slides = RowSlides::default();
        assert!(!slides.animating());
        assert_eq!(slides.offset(0), 0.0);
    }

    #[test]
    fn a_relocate_eases_to_zero_over_the_duration() {
        let mut slides = RowSlides::default();
        let now = Instant::now();
        slides.relocate(vec![24.0, 0.0, -24.0], now);
        assert!(slides.animating());
        // Exactly at the start the full offset is owed.
        assert_eq!(slides.offset(0), 24.0);
        assert_eq!(slides.offset(2), -24.0);
        // Past the duration it has settled and is no longer animating.
        let n = (theme::ROW_SLIDE_MS / 1000.0 / 0.005).ceil() as usize + 1;
        steps(&mut slides, n, 0.005);
        assert!(!slides.animating());
        assert_eq!(slides.offset(0), 0.0);
        assert_eq!(slides.offset(2), 0.0);
    }

    #[test]
    fn an_all_zero_relocate_is_a_no_op() {
        let mut slides = RowSlides::default();
        slides.relocate(vec![0.0, 0.0], Instant::now());
        assert!(!slides.animating());
    }

    #[test]
    fn clear_resets_everything() {
        let mut slides = RowSlides::default();
        slides.relocate(vec![24.0], Instant::now());
        assert!(slides.animating());
        slides.clear();
        assert!(!slides.animating());
        assert_eq!(slides.offset(0), 0.0);
    }
}
