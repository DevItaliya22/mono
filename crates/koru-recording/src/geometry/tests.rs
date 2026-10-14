// SPDX-License-Identifier: MPL-2.0

use super::*;

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
    Rect {
        left,
        top,
        right,
        bottom,
    }
}

#[test]
fn capture_local_subtracts_the_window_origin() {
    let local = to_capture_local(rect(140, 90, 340, 122), rect(100, 50, 1100, 850));
    assert_eq!(local, rect(40, 40, 240, 72));
    assert_eq!(center(local), (140, 56));
}

#[test]
fn negative_monitor_coordinates_stay_capture_local() {
    let window = rect(-400, 100, 1520, 1180);
    let target = rect(-200, 180, 0, 212);
    assert_eq!(to_capture_local(target, window), rect(200, 80, 400, 112));
}

#[test]
fn fractional_dpi_is_not_applied_a_second_time() {
    let geometry = CaptureGeometry {
        window: rect(100, 50, 1100, 850),
        width: 1000,
        height: 800,
        title_bar_height: 32,
        dpi_scale_milli: 1250,
    };
    let local = to_capture_local(rect(140, 90, 340, 122), geometry.window);
    assert_eq!(
        local,
        rect(40, 40, 240, 72),
        "UIA bounds are already physical; 125% must not scale them again"
    );
}

#[test]
fn scrolling_records_bounds_at_delivery_not_at_origin() {
    let window = rect(0, 0, 800, 600);
    let before = to_capture_local(rect(40, 500, 200, 524), window);
    let after = to_capture_local(rect(40, 40, 200, 64), window);
    assert_eq!(before.top, 500);
    assert_eq!(after.top, 40);
}

#[test]
fn a_resized_frame_is_not_the_same_capture() {
    let geometry = CaptureGeometry::physical(rect(0, 0, 800, 600), 800, 600, 0);
    assert!(same_capture_size(geometry, 800, 600));
    assert!(!same_capture_size(geometry, 1024, 768));
}

#[test]
fn an_odd_window_height_matches_the_even_encoder_size() {
    let geometry = CaptureGeometry::physical(rect(533, 227, 1397, 894), 864, 666, 0);
    assert!(
        same_capture_size(geometry, 864, 667),
        "WGC 864x667 vs H.264 864x666 is the same capture, not a resize"
    );
    assert!(!same_capture_size(geometry, 864, 768));
}

#[test]
fn title_bar_height_is_not_applied_as_a_client_offset() {
    let geometry = CaptureGeometry::physical(rect(100, 50, 1100, 850), 1000, 800, 32);
    let local = to_capture_local(rect(140, 90, 340, 122), geometry.window);
    assert_eq!(
        local.top, 40,
        "client/capture offset is recorded, not subtracted"
    );
}
