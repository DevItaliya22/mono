// SPDX-License-Identifier: MPL-2.0

//! The border has to be reachable on every side, absent in the middle, and
//! absent altogether when the window is maximised.

use super::*;

const W: f32 = 1200.0;
const H: f32 = 700.0;

fn at(x: f32, y: f32) -> Option<ResizeDirection> {
    edge(W, H, x, y, false)
}

#[test]
fn every_edge_is_reachable() {
    assert_eq!(at(0.0, H / 2.0), Some(ResizeDirection::West));
    assert_eq!(at(W, H / 2.0), Some(ResizeDirection::East));
    assert_eq!(at(W / 2.0, 0.0), Some(ResizeDirection::North));
    assert_eq!(at(W / 2.0, H), Some(ResizeDirection::South));
}

#[test]
fn every_corner_is_reachable_and_beats_the_edges_it_meets() {
    assert_eq!(at(0.0, 0.0), Some(ResizeDirection::NorthWest));
    assert_eq!(at(W, 0.0), Some(ResizeDirection::NorthEast));
    assert_eq!(at(0.0, H), Some(ResizeDirection::SouthWest));
    assert_eq!(at(W, H), Some(ResizeDirection::SouthEast));
}

/// A corner is a wider target than an edge on purpose — otherwise it is a
/// pixel-hunt where two 6px bands happen to cross.
#[test]
fn a_corner_is_a_wider_target_than_an_edge() {
    // Along the top, well inside the edge band but still near the corner.
    assert_eq!(at(2.0, CORNER - 1.0), Some(ResizeDirection::NorthWest));
    assert_eq!(at(CORNER - 1.0, 2.0), Some(ResizeDirection::NorthWest));
    // Past the corner's reach it is a plain edge again.
    assert_eq!(at(CORNER + 20.0, 2.0), Some(ResizeDirection::North));
}

#[test]
fn the_middle_of_the_window_is_not_a_border() {
    assert_eq!(at(W / 2.0, H / 2.0), None);
    assert_eq!(
        at(BORDER + 1.0, BORDER + 1.0),
        None,
        "just inside the corner"
    );
}

/// **The reason `maximized` is a parameter.** A maximised window cannot be
/// resized, so offering the cursor for it would be a lie — and it is exactly
/// when the window fills the screen that throwing the pointer into the top-right
/// corner has to reach the close button.
#[test]
fn a_maximised_window_has_no_border_at_all() {
    for (x, y) in [(0.0, 0.0), (W, 0.0), (0.0, H), (W, H), (W / 2.0, 0.0)] {
        assert_eq!(edge(W, H, x, y, true), None, "({x}, {y}) while maximised");
    }
}

/// A `Resized(1, 1)` arrives mid-drag and while restoring from minimised.
///
/// A window this small is *all* border — every point of it is within a corner's
/// reach, which is the honest answer rather than a bug. What still has to hold
/// is that the bands do not swallow each other: the left of it must not resize
/// eastward.
#[test]
fn a_tiny_window_is_all_border_but_still_has_two_sides() {
    let (w, h) = (10.0, 10.0);
    let westward = [
        ResizeDirection::West,
        ResizeDirection::NorthWest,
        ResizeDirection::SouthWest,
    ];
    let eastward = [
        ResizeDirection::East,
        ResizeDirection::NorthEast,
        ResizeDirection::SouthEast,
    ];

    let left = edge(w, h, 0.0, h / 2.0, false).expect("the left side resizes");
    assert!(westward.contains(&left), "left gave {left:?}");
    let right = edge(w, h, w, h / 2.0, false).expect("the right side resizes");
    assert!(eastward.contains(&right), "right gave {right:?}");
}

/// A zero-sized window answers nothing rather than dividing by zero.
#[test]
fn a_window_with_no_size_has_no_border() {
    assert_eq!(edge(0.0, 0.0, 0.0, 0.0, false), None);
    assert_eq!(edge(-5.0, -5.0, 0.0, 0.0, false), None);
}

/// The cursor comes from winit's own mapping, so a direction and the shape the
/// pointer takes can never disagree.
#[test]
fn every_direction_has_a_cursor() {
    use winit::window::CursorIcon;
    for (x, y) in [(0.0, 0.0), (W, H), (W / 2.0, 0.0), (0.0, H / 2.0)] {
        let dir = at(x, y).expect("a border");
        assert_ne!(CursorIcon::from(dir), CursorIcon::Default);
    }
}

/// **The band is ours only where winit can act on it.**
///
/// winit's macOS backend returns `NotSupported` from `drag_resize_window`
/// unconditionally, and the call site used to discard that error and then claim
/// the press anyway — so the outermost 6 px of every edge swallowed mouse-downs
/// and did nothing. The tab strip begins at `y = 0`, so the top band lay across
/// it and dragging the window by the caption could not work.
///
/// Asserted in both directions: a constant that was `true` everywhere would
/// pass a one-sided check, and that is exactly the bug.
#[test]
fn the_border_is_ours_everywhere_except_macos() {
    if cfg!(target_os = "macos") {
        assert!(
            !is_ours(),
            "winit cannot perform a resize drag on macOS, so the band must not \
             be hit-tested — it would only swallow the press"
        );
    } else {
        assert!(is_ours());
    }
}

/// The geometry itself is platform-neutral and stays tested everywhere: it is
/// correct, it is simply not consulted on macOS. Pinned so the gate is not
/// "fixed" later by gutting [`edge`], which would take the border off Windows.
#[test]
fn the_geometry_is_not_what_the_platform_gate_turns_off() {
    assert_eq!(at(0.0, H / 2.0), Some(ResizeDirection::West));
    assert_eq!(at(W / 2.0, 0.0), Some(ResizeDirection::North));
}
