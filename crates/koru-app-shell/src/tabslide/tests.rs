// SPDX-License-Identifier: MPL-2.0

//! The reorder slide, driven on a fake clock — the same way `panel::tests`
//! does it, because a real one makes the assertions timing-dependent.

use super::*;
use std::time::Duration;

const STEP: f32 = 100.0;

fn at(t0: Instant, ms: u64) -> Instant {
    t0 + Duration::from_millis(ms)
}

/// Uniform slot left edges for `n` tabs of width `STEP`.
fn xs(n: usize) -> Vec<f32> {
    (0..n).map(|i| i as f32 * STEP).collect()
}

/// A tab dragged one slot left: the tab it passed has to be drawn where it
/// *was*, so the eye sees it move out of the way.
#[test]
fn the_displaced_tab_starts_where_it_was_and_arrives_at_its_slot() {
    let t0 = Instant::now();
    let mut s = TabSlides::new();
    let old = xs(4);
    let new = xs(4);
    s.moved(&old, &new, 3, 2, t0);

    assert_eq!(s.offset(3), -STEP, "the displaced tab starts a slot back");
    assert_eq!(s.offset(2), 0.0, "the dragged tab does not slide");
    assert_eq!(s.offset(0), 0.0);
    assert_eq!(s.offset(1), 0.0);

    s.advance(at(t0, 55));
    let mid = s.offset(3);
    assert!(
        mid > -STEP && mid < 0.0,
        "half way through it is between the two: {mid}"
    );

    s.advance(at(t0, theme::TAB_SLIDE_MS as u64 + 1));
    assert_eq!(s.offset(3), 0.0, "it arrives");
    assert!(!s.animating(), "and stops asking for frames");
}

/// The other direction, which is a different arm of `old_index`.
#[test]
fn dragging_a_tab_right_slides_the_one_it_passed_the_other_way() {
    let t0 = Instant::now();
    let mut s = TabSlides::new();
    let old = xs(4);
    let new = xs(4);
    s.moved(&old, &new, 1, 3, t0);
    assert_eq!(s.offset(1), STEP);
    assert_eq!(s.offset(2), STEP);
    assert_eq!(s.offset(3), 0.0, "the dragged tab");
    assert_eq!(s.offset(0), 0.0, "untouched");
}

/// **`animating` has to return to `false`.** ADR-26: it is the one exception to
/// "a frame only when something changed", so a slide that never finished would
/// spin the loop at frame rate for the rest of the session.
#[test]
fn a_slide_stops_asking_for_frames() {
    let t0 = Instant::now();
    let mut s = TabSlides::new();
    let old = xs(3);
    let new = xs(3);
    s.moved(&old, &new, 0, 2, t0);
    assert!(s.animating());
    s.advance(at(t0, 10_000));
    assert!(!s.animating());
    for i in 0..3 {
        assert_eq!(s.offset(i), 0.0);
    }
}

/// A second drag while the first is still settling must not snap the tabs back
/// to their slots before starting again.
#[test]
fn a_reorder_mid_slide_carries_on_from_where_the_tabs_are() {
    let t0 = Instant::now();
    let mut s = TabSlides::new();
    let old = xs(4);
    let new = xs(4);
    s.moved(&old, &new, 3, 2, t0);
    s.advance(at(t0, 55));
    let carried = s.offset(3);
    assert!(carried < 0.0, "still owed some of the first slide");

    // Now drag it one further left. The tab at 3 is not displaced again, so all
    // it should have is what it was already owed.
    s.moved(&old, &new, 2, 1, at(t0, 55));
    assert!(
        (s.offset(3) - carried).abs() < 0.01,
        "carried {carried} became {}",
        s.offset(3)
    );
}

/// Offsets are recorded against an index. Renumbering the list — a tab opened
/// or closed — makes them belong to somebody else.
#[test]
fn clearing_drops_every_offset() {
    let t0 = Instant::now();
    let mut s = TabSlides::new();
    let old = xs(4);
    let new = xs(4);
    s.moved(&old, &new, 3, 2, t0);
    s.clear();
    assert!(!s.animating());
    assert_eq!(s.offset(3), 0.0);
}

/// Nothing to slide past, nothing to animate — and no frames asked for.
#[test]
fn a_move_that_is_not_a_move_does_nothing() {
    let t0 = Instant::now();
    let mut s = TabSlides::new();
    let old = xs(4);
    let new = xs(4);
    s.moved(&old, &new, 2, 2, t0);
    assert!(!s.animating(), "same index");
    s.moved(&[], &[], 0, 1, t0);
    assert!(!s.animating(), "no tabs");
}

/// An index past the end is a question about a tab that is not there, and the
/// answer is "where it belongs" rather than a panic.
#[test]
fn an_offset_past_the_end_is_zero() {
    let s = TabSlides::new();
    assert_eq!(s.offset(99), 0.0);
}
