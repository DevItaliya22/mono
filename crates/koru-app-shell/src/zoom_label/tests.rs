// SPDX-License-Identifier: MPL-2.0

use std::time::Duration;

use super::*;

fn armed(now: Instant) -> ZoomLabel {
    let mut label = ZoomLabel::new();
    label.show(now);
    label
}

/// **The one that matters for the frame budget.** A window nobody has zoomed
/// must not be a window the shell keeps waking up.
#[test]
fn at_rest_it_draws_nothing_and_asks_for_nothing() {
    let label = ZoomLabel::new();
    let now = Instant::now();
    assert!(!label.showing());
    assert!(!label.animating(now));
    assert_eq!(label.alpha(now), None);
    assert_eq!(label.maintenance_deadline(), None);
}

#[test]
fn a_zoom_arms_it_at_full_opacity() {
    let now = Instant::now();
    let label = armed(now);
    assert!(label.showing());
    assert_eq!(label.alpha(now), Some(1.0));
    assert_eq!(label.alpha(now + Duration::from_millis(100)), Some(1.0));
}

/// The toast rule, stated as a test: **the hold is a deadline, the fade is
/// frames**. Koru's own indicator fails this one on purpose (see the module
/// note); the shell's does not.
#[test]
fn the_hold_asks_for_a_wake_and_the_fade_asks_for_frames() {
    let now = Instant::now();
    let label = armed(now);

    // Inside the hold: drawn, but nothing to animate yet.
    assert!(!label.animating(now + Duration::from_millis(100)));
    assert_eq!(
        label.maintenance_deadline(),
        Some(now + Duration::from_millis(700)),
        "the wake is the moment the fade starts"
    );

    // Inside the fade: frames.
    let fading = now + Duration::from_millis(950);
    assert!(label.animating(fading));
    let alpha = label.alpha(fading).expect("mid-fade");
    assert!(
        alpha > 0.0 && alpha < 1.0,
        "half way down the ramp, got {alpha}"
    );
}

/// Driven off elapsed time against a fixed total, so a frame dropped in the
/// middle of the ramp cannot strand the pill on screen.
#[test]
fn it_expires_by_its_deadline_and_stops_asking_for_frames() {
    let now = Instant::now();
    let mut label = armed(now);
    let after = now + Duration::from_millis(1_200);

    assert_eq!(label.alpha(after), None, "1.2 s is the whole life");
    assert!(!label.animating(after));
    assert!(label.expire(after), "the flash is dropped");
    assert!(!label.showing());
    assert!(
        !label.expire(after),
        "and dropping it twice is not a second change"
    );

    // The frame that arrived a whole second late finds it over, not stuck.
    let mut late = armed(now);
    assert!(late.expire(now + Duration::from_secs(5)));
    assert!(!late.showing());
}

#[test]
fn expiring_is_not_a_change_while_it_is_still_showing() {
    let now = Instant::now();
    let mut label = armed(now);
    assert!(!label.expire(now + Duration::from_millis(100)));
    assert!(label.showing());
}

/// Holding the chord down keeps the pill up rather than letting the first press
/// expire under the fifth.
#[test]
fn a_second_zoom_resets_the_deadline() {
    let now = Instant::now();
    let mut label = armed(now);
    let again = now + Duration::from_secs(5);
    label.show(again);
    assert_eq!(label.alpha(again + Duration::from_millis(50)), Some(1.0));
    assert_eq!(label.maintenance_deadline(), Some(again + HOLD));
}

/// Koru's copy, to the character: integer percent, no decimals, at both ends of
/// the range the shell clamps to.
#[test]
fn the_copy_is_the_percentage_koru_shows() {
    assert_eq!(ZoomLabel::text(1.0), "100%");
    assert_eq!(ZoomLabel::text(1.2), "120%");
    assert_eq!(ZoomLabel::text(crate::state::ZOOM_MIN), "50%");
    assert_eq!(ZoomLabel::text(crate::state::ZOOM_MAX), "200%");
    // The step grid is 10%, so the rounding is only ever tidying float dust.
    assert_eq!(ZoomLabel::text(1.100_000_1), "110%");
}

/// Centred on the **window**, not on the content rect: the pane and the caption
/// do not shift it, because it is a readout about the glass.
#[test]
fn the_pill_is_centred_on_the_whole_window_at_true_pixels() {
    let viewport = Rect::new(0.0, 0.0, 500.0, 400.0);
    let pill = ZoomLabel::pill((40.0, 18.0), viewport, 2.0);
    assert_eq!((pill.w, pill.h), (40.0 + 18.0, 18.0 + 18.0));
    // The viewport is the zoomed-down space, so the true window is 1000x800.
    assert_eq!(pill.x, (1000.0 - pill.w) * 0.5);
    assert_eq!(pill.y, (800.0 - pill.h) * 0.5);
}

/// The pill is a fixed physical size, so the same label at 50% and at 200%
/// occupies the same number of real pixels — only its centre moves.
#[test]
fn zoom_moves_the_pill_without_resizing_it() {
    let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
    let small = ZoomLabel::pill((40.0, 18.0), viewport, 0.5);
    let large = ZoomLabel::pill((40.0, 18.0), viewport, 2.0);
    assert_eq!((small.w, small.h), (large.w, large.h));
    assert!(large.x > small.x && large.y > small.y);
}
