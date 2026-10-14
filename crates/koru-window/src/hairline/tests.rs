// SPDX-License-Identifier: MPL-2.0

//! An edge is flush when there is nothing left between it and the screen —
//! which has to include a maximised window's overhang, exclude a window one
//! pixel inside, and be measured against the *work area* rather than the
//! monitor.

use super::*;

/// A 2560 × 1440 monitor with a 40px taskbar along the bottom. The taskbar is
/// the point: a test that used the monitor's own bottom would pass while the
/// app drew its bottom hairline behind the taskbar.
const WORK: Bounds = Bounds {
    left: 0,
    top: 0,
    right: 2560,
    bottom: 1400,
};

const NONE: Edges = Edges {
    left: false,
    top: false,
    right: false,
    bottom: false,
};

fn win(left: i32, top: i32, right: i32, bottom: i32) -> Edges {
    flush(
        Bounds {
            left,
            top,
            right,
            bottom,
        },
        WORK,
    )
}

#[test]
fn a_floating_window_is_flush_on_nothing() {
    assert_eq!(win(100, 100, 1400, 900), NONE);
    assert!(!win(100, 100, 1400, 900).any());
}

/// The bug this module exists for. winit gives a maximised window's *client*
/// area the work area, but its window rect still overhangs by the invisible
/// frame — which is where the compositor paints the border.
#[test]
fn a_maximised_window_overhangs_and_is_flush_on_all_four() {
    let e = win(-8, -8, 2568, 1408);
    assert_eq!(
        e,
        Edges {
            left: true,
            top: true,
            right: true,
            bottom: true
        }
    );
}

#[test]
fn a_left_snapped_window_is_flush_on_three() {
    assert_eq!(
        win(0, 0, 1280, 1400),
        Edges {
            left: true,
            top: true,
            right: false,
            bottom: true
        }
    );
}

#[test]
fn a_top_right_quarter_is_flush_on_two() {
    assert_eq!(
        win(1280, 0, 2560, 700),
        Edges {
            left: false,
            top: true,
            right: true,
            bottom: false
        }
    );
}

/// `<=` and `>=`, not `==`: a snapped window lands exactly on the boundary, and
/// a maximised one lands past it. Both have to count.
#[test]
fn an_edge_exactly_on_the_boundary_counts_as_flush() {
    assert!(win(0, 100, 1400, 900).left);
    assert!(win(100, 0, 1400, 900).top);
    assert!(win(100, 100, 2560, 900).right);
    assert!(win(100, 100, 1400, 1400).bottom);
}

/// The other half of that boundary. One pixel inside, the compositor's border
/// is visible and ours must not be drawn over it.
#[test]
fn one_pixel_inside_is_not_flush() {
    assert!(!win(1, 100, 1400, 900).left);
    assert!(!win(100, 1, 1400, 900).top);
    assert!(!win(100, 100, 2559, 900).right);
    assert!(!win(100, 100, 1400, 1399).bottom);
}

/// This is the one that fails if someone "simplifies" `GetMonitorInfoW` away
/// in favour of `MonitorHandle::size()`, which reports the whole monitor.
#[test]
fn the_taskbar_edge_is_the_work_area_not_the_monitor() {
    // Flush with the work area, well above the monitor's own bottom of 1440.
    assert!(win(100, 100, 1400, 1400).bottom);
    // And one pixel above that is not flush, even though it is nowhere near
    // the monitor's bottom either.
    assert!(!win(100, 100, 1400, 1399).bottom);
}

/// A minimised window reports the (-32000, -32000) sentinel. Without the
/// off-work-area guard it would read as flush on the left and the top, and the
/// window would come back from the taskbar wearing half an outline.
#[test]
fn a_minimised_window_is_flush_on_nothing() {
    assert_eq!(win(-32000, -32000, -30800, -31300), NONE);
}

#[test]
fn a_degenerate_window_is_flush_on_nothing() {
    assert_eq!(win(0, 0, 0, 0), NONE);
    assert_eq!(win(100, 100, 100, 900), NONE);
    assert_eq!(win(100, 100, 1400, 100), NONE);
    // Backwards, which a bad conversion could produce.
    assert_eq!(win(1400, 900, 100, 100), NONE);
}

/// A window on the monitor to the left: entirely outside this work area, so it
/// has no flush edges *here* even though its right edge touches this one's left.
#[test]
fn a_window_on_another_monitor_is_flush_on_nothing() {
    assert_eq!(win(-2560, 0, 0, 1400), NONE);
}
