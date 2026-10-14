// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn two_side_by_side_read_left_then_right() {
    // The right-hand one is a little higher: still one row.
    assert_eq!(reading_order(&[(300, 10), (0, 45)]), vec![1, 0]);
}

#[test]
fn rows_read_top_to_bottom() {
    let positions = [(0, 500), (400, 0), (0, 0), (400, 520)];
    assert_eq!(reading_order(&positions), vec![2, 1, 0, 3]);
}

/// The band is measured from the row's first object, not from the last one
/// that joined — or a staircase of cards would read as one endless row.
#[test]
fn the_band_is_anchored_at_the_rows_first_object() {
    // 60 is past 0 + 40, so the third starts a new row even though it is
    // within 40 of the second. Rows: {0, 1} then {2, 3}, each left to right.
    let staircase = [(300, 0), (200, 30), (100, 60), (0, 90)];
    assert_eq!(reading_order(&staircase), vec![1, 0, 3, 2]);
}

#[test]
fn two_columns_read_row_by_row_interleaved() {
    let left = [(0, 0), (0, 200), (0, 400)];
    let right = [(500, 10), (500, 210), (500, 410)];
    let positions: Vec<(i32, i32)> = left.iter().chain(right.iter()).copied().collect();
    assert_eq!(reading_order(&positions), vec![0, 3, 1, 4, 2, 5]);
}

#[test]
fn extreme_positions_do_not_overflow() {
    let positions = [(i32::MAX, i32::MAX), (i32::MIN, i32::MIN), (0, 0)];
    assert_eq!(reading_order(&positions), vec![1, 2, 0]);
}
