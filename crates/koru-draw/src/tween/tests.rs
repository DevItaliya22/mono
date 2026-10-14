// SPDX-License-Identifier: MPL-2.0

use std::time::{Duration, Instant};

use super::*;

const MS: f32 = 100.0;

#[test]
fn it_rests_at_the_end_it_was_built_at() {
    let off = Tween::new(MS, false);
    assert_eq!(off.progress(), 0.0);
    assert!(!off.animating());

    let on = Tween::new(MS, true);
    assert_eq!(on.progress(), 1.0);
    assert!(!on.animating());
}

#[test]
fn it_reaches_the_end_and_stops_animating() {
    let now = Instant::now();
    let mut t = Tween::new(MS, false);
    assert!(t.set(true, now));
    assert!(t.animating());

    // Halfway: moving, arrived at neither end.
    t.advance(now + Duration::from_millis(50));
    assert!(t.progress() > 0.0 && t.progress() < 1.0);
    assert!(t.animating());

    t.advance(now + Duration::from_millis(100));
    assert_eq!(t.progress(), 1.0);
    assert!(
        !t.animating(),
        "`View::animating` must return to false or the loop spins at frame rate"
    );
}

/// The whole reason this is a deadline rather than a per-frame decrement: a
/// frame that never arrived must not strand it short of its end.
#[test]
fn a_skipped_frame_still_lands_exactly_on_the_end() {
    let now = Instant::now();
    let mut t = Tween::new(MS, false);
    t.set(true, now);
    // One advance, long after it was due — every frame in between dropped.
    t.advance(now + Duration::from_secs(5));
    assert_eq!(t.progress(), 1.0);
    assert!(!t.animating());
}

#[test]
fn reversing_halfway_turns_round_on_the_spot() {
    let now = Instant::now();
    let mut t = Tween::new(MS, false);
    t.set(true, now);
    let half = now + Duration::from_millis(50);
    t.advance(half);
    let midway = t.progress();

    assert!(t.set(false, half));
    // It carries on from where it was rather than jumping to the far end.
    assert_eq!(t.progress(), midway);
    t.advance(half + Duration::from_millis(1));
    assert!(t.progress() < midway);
}

#[test]
fn aiming_at_the_end_it_is_already_heading_for_does_nothing() {
    let now = Instant::now();
    let mut t = Tween::new(MS, false);
    assert!(t.set(true, now));
    t.advance(now + Duration::from_millis(50));
    let midway = t.progress();

    // A per-frame re-aim must not restart the run and pin it short of arriving.
    assert!(!t.set(true, now + Duration::from_millis(50)));
    t.advance(now + Duration::from_millis(100));
    assert!(t.progress() > midway);
    assert_eq!(t.progress(), 1.0);
}

#[test]
fn set_immediately_leaves_nothing_in_flight() {
    let now = Instant::now();
    let mut t = Tween::new(MS, false);
    t.set(true, now);
    assert!(t.animating());

    t.set_immediately(false);
    assert_eq!(t.progress(), 0.0);
    assert!(!t.animating());
    assert!(!t.advance(now + Duration::from_secs(1)));
}

#[test]
fn a_zero_duration_arrives_on_the_first_advance() {
    let now = Instant::now();
    let mut t = Tween::new(0.0, false);
    t.set(true, now);
    t.advance(now);
    assert_eq!(t.progress(), 1.0);
    assert!(!t.animating());
}
