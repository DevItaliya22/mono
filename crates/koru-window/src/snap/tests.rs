// SPDX-License-Identifier: MPL-2.0

use super::*;

/// A 1920x1080 screen with a 40px taskbar at the bottom.
const WORK: (i32, i32, i32, i32) = (0, 0, 1920, 1040);

#[test]
fn the_two_halves_tile_the_work_area_exactly() {
    let (lx, ly, lw, lh) = half(WORK, Side::Left);
    let (rx, ry, rw, rh) = half(WORK, Side::Right);

    assert_eq!((lx, ly), (0, 0));
    assert_eq!((lh, rh), (1040, 1040), "both fill the work area's height");
    assert_eq!(ly, ry);
    assert_eq!(
        lx + lw as i32,
        rx,
        "the right half starts where the left one ends — no gutter, no overlap"
    );
    assert_eq!(rx + rw as i32, WORK.2, "and reaches the screen's edge");
}

/// The work area, not the monitor: the bottom stops above the taskbar.
#[test]
fn a_snap_stays_out_of_the_taskbar() {
    let (_, y, _, h) = half(WORK, Side::Left);
    assert_eq!(y + h as i32, 1040, "not 1080");
}

/// An odd width has to go somewhere, and a one-pixel gutter down the middle of
/// the screen is the one outcome nobody wants.
#[test]
fn an_odd_width_leaves_no_gap() {
    let odd = (0, 0, 1921, 1040);
    let (lx, _, lw, _) = half(odd, Side::Left);
    let (rx, _, rw, _) = half(odd, Side::Right);
    assert_eq!(lx + lw as i32, rx);
    assert_eq!(rx + rw as i32, 1921);
    assert_eq!(lw, 961, "the left half takes the odd pixel");
}

/// A monitor whose work area is off the origin — a second screen to the right,
/// or one above — snaps within *itself*, not to the desktop's origin.
#[test]
fn a_second_monitor_snaps_within_its_own_bounds() {
    let right_screen = (1920, 0, 3840, 1040);
    let (lx, _, lw, _) = half(right_screen, Side::Left);
    let (rx, _, rw, _) = half(right_screen, Side::Right);
    assert_eq!(lx, 1920);
    assert_eq!(rx + rw as i32, 3840);
    assert_eq!(lw, 960);
}

/// A degenerate work area produces nothing rather than a negative size, which
/// would be a panic on the way into `PhysicalSize`.
#[test]
fn an_empty_work_area_yields_an_empty_rect() {
    for side in [Side::Left, Side::Right] {
        let (_, _, w, h) = half((100, 100, 100, 100), side);
        assert_eq!((w, h), (0, 0));
        let (_, _, w, h) = half((100, 100, 0, 0), side);
        assert_eq!((w, h), (0, 0), "inverted, too");
    }
}

// ------------------------------------------------------------- reachability

/// One 1920x1080 screen at the desktop origin.
const SCREEN: (i32, i32, i32, i32) = (0, 0, 1920, 1080);
/// The default window, in physical pixels.
const WINDOW: (u32, u32) = (940, 620);

#[test]
fn a_window_on_the_screen_is_reachable() {
    assert!(reachable(&[SCREEN], (100, 100), WINDOW));
    assert!(reachable(&[SCREEN], (0, 0), WINDOW), "the corner, too");
}

/// The reported bug: a position remembered while a second monitor sat left of
/// the primary, restored after that monitor was unplugged. Nothing of the
/// window is on the screen that remains, so it is the OS that places the
/// window, not the file.
#[test]
fn a_position_from_a_monitor_that_is_gone_is_not_reachable() {
    assert!(!reachable(&[SCREEN], (-1920, 0), WINDOW));
    // Above the primary is the same failure with the other axis.
    assert!(!reachable(&[SCREEN], (0, -1080), WINDOW));
    // And so is a resolution drop past the old right edge, window and all.
    assert!(!reachable(&[SCREEN], (1920, 0), WINDOW));
    assert!(!reachable(&[SCREEN], (500, 1080), WINDOW));
}

/// A window hanging off an edge is a placement, not a loss: enough of the
/// caption strip is on-screen to grab and drag it back.
#[test]
fn a_window_hanging_off_the_edge_is_still_grabbable() {
    assert!(reachable(&[SCREEN], (1500, 100), WINDOW));
    // Pushed up until only a caption's worth shows below the top edge.
    assert!(reachable(&[SCREEN], (100, -40), WINDOW));
}

/// A sliver is not a grip. A window all but submerged past an edge — above
/// the top here, leaving less than a caption's height — cannot be rescued by
/// dragging, so the position is not honoured.
#[test]
fn a_sliver_of_window_is_not_a_grip() {
    // The strip is 64 tall; 40 above the screen leaves 24 exactly — a grip.
    assert!(reachable(&[SCREEN], (100, -40), WINDOW));
    // One pixel higher and the visible band is too thin to hold.
    assert!(!reachable(&[SCREEN], (100, -41), WINDOW));
    // And 95px of width at the right edge is below what a grip needs.
    assert!(!reachable(&[SCREEN], (1825, 100), WINDOW));
    assert!(reachable(&[SCREEN], (1824, 100), WINDOW));
}

/// Reachability is a question about the whole desktop, not one monitor: a
/// window sitting wholly on the second screen is fine while that screen is
/// connected, however negative its coordinates.
#[test]
fn any_monitor_counts() {
    let left_screen = (-1920, 0, 0, 1080);
    assert!(reachable(&[SCREEN, left_screen], (-1920, 0), WINDOW));
    // The same position with only the primary connected is not — the desktop
    // changed, the remembered hint did not.
    assert!(!reachable(&[SCREEN], (-1920, 0), WINDOW));
}

/// An empty monitor list answers nothing: better the OS's default placement
/// than a position nothing vouches for.
#[test]
fn no_monitors_means_not_reachable() {
    assert!(!reachable(&[], (100, 100), WINDOW));
}

// ----------------------------------------------------------- nudge_onscreen

/// A window already on the work area is left exactly where it is.
#[test]
fn an_onscreen_position_is_untouched() {
    assert_eq!(nudge_onscreen(WORK, (300, 200), WINDOW), (300, 200));
    assert_eq!(
        nudge_onscreen(WORK, (0, 0), WINDOW),
        (0, 0),
        "the corner, too"
    );
}

/// A position that would strand the window past an edge is pulled back until
/// a grip shows — never by shrinking the window.
#[test]
fn an_offscreen_position_is_nudged_back_to_a_grip() {
    // 1000px past the right edge of a 1920 work area leaves nothing to grab.
    let (x, _) = nudge_onscreen(WORK, (3000, 200), WINDOW);
    assert_eq!(x, 1920 - 96, "a grip's width of window stays on");
    // Wholly off the left edge: the grip comes back on the left.
    let (x, _) = nudge_onscreen(WORK, (-1500, 200), WINDOW);
    assert_eq!(x, 0 + 96 - 940);
    // Above the top: the caption returns to the work area's top edge.
    let (_, y) = nudge_onscreen(WORK, (300, -300), WINDOW);
    assert_eq!(y, 0);
    // Below the bottom: the caption, not the window, is what comes back.
    let (_, y) = nudge_onscreen(WORK, (300, 1200), WINDOW);
    assert_eq!(y, 1040 - 24);
}

/// The recording case: a portrait preset taller than the work area is left
/// exactly where it sits — height is never consulted, let alone clamped.
#[test]
fn a_tall_preset_is_never_shrunk() {
    let portrait = (1080, 1920);
    let (x, y) = nudge_onscreen(WORK, (400, 600), portrait);
    assert_eq!(x, 400, "width fits; horizontal position untouched");
    assert_eq!(
        y, 600,
        "the caption is on the work area, so the 1920px height simply hangs below"
    );
}

/// A work area too short to hold a caption grip cannot satisfy the clamp;
/// its top edge is the answer rather than an inverted range.
#[test]
fn a_degenerate_work_area_anchors_to_its_top() {
    let taskbar_strip = (0, 0, 1920, 10);
    let (_, y) = nudge_onscreen(taskbar_strip, (500, 100), WINDOW);
    assert_eq!(y, 0);
}
