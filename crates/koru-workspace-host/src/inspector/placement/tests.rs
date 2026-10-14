// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn native_geometry_accepts_monitors_left_of_primary_and_pin_state() {
    let g = parse("v2 -1800 -120 520 720 1").unwrap();
    assert_eq!(g.position, Some((-1800, -120)));
    assert_eq!(g.size, (520.0, 720.0));
    assert!(g.pinned);
    assert!(!parse("v2 10 20 600 800 0").unwrap().pinned);
}

#[test]
fn legacy_panel_positions_and_invalid_sizes_are_not_native_geometry() {
    for row in [
        "v1 300 40",
        "v2 1 2 NaN 720 0",
        "v2 1 2 520 inf 0",
        "v2 1 2 12 720 0",
        "v2 1 2 520 720 2",
        "v2 1 2 520 720 0 extra",
    ] {
        assert!(parse(row).is_none(), "{row}");
    }
}
