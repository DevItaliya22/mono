// SPDX-License-Identifier: MPL-2.0

use super::*;

fn stamped(index: u32, width: u32, height: u32) -> FrameStamp {
    FrameStamp {
        index,
        arrived_ms: u64::from(index) * 16,
        width: Some(width),
        height: Some(height),
        dpi_scale_milli: None,
    }
}

#[test]
fn a_take_that_kept_its_size_holds() {
    let frames = vec![
        stamped(0, 1920, 1080),
        stamped(1, 1920, 1080),
        stamped(2, 1920, 1080),
    ];
    assert!(frames_hold_size(&frames));
}

/// The OS-level resize the app lock cannot see (Win+Arrow, aero snap, a DPI
/// change on monitor move): the encoder pads the odd frame and says nothing,
/// so the stamps are the only place it shows.
#[test]
fn a_mid_clip_resize_is_seen() {
    let frames = vec![
        stamped(0, 1920, 1080),
        stamped(1, 1920, 1080),
        stamped(2, 1280, 720),
    ];
    assert!(!frames_hold_size(&frames));
}

#[test]
fn an_empty_take_holds_vacuously() {
    assert!(frames_hold_size(&[]));
}
