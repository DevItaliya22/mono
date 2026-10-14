// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn a_segment_runs_edge_to_edge_along_the_centres() {
    let a = Rect::new(0.0, 0.0, 100.0, 50.0);
    let b = Rect::new(300.0, 0.0, 400.0, 50.0);
    let (p0, p1) = segment(a, b).expect("apart");
    assert_eq!(p0, Point::new(100.0, 25.0));
    assert_eq!(p1, Point::new(300.0, 25.0));
}

#[test]
fn a_diagonal_segment_leaves_through_the_nearer_side() {
    let a = Rect::new(0.0, 0.0, 100.0, 100.0);
    let b = Rect::new(200.0, 400.0, 300.0, 500.0);
    let (p0, p1) = segment(a, b).expect("apart");
    // Steeper than 45°, so it leaves through the bottom and enters the top.
    assert!((p0.y - 100.0).abs() < 1e-9, "{p0:?}");
    assert!((p1.y - 400.0).abs() < 1e-9, "{p1:?}");
}

#[test]
fn overlapping_objects_have_no_segment() {
    let a = Rect::new(0.0, 0.0, 100.0, 100.0);
    let b = Rect::new(50.0, 50.0, 150.0, 150.0);
    assert!(segment(a, b).is_none());
}

#[test]
fn distance_is_to_the_segment_not_the_line() {
    let (a, b) = (Point::new(0.0, 0.0), Point::new(10.0, 0.0));
    assert_eq!(distance(Point::new(5.0, 3.0), a, b), 3.0);
    assert_eq!(distance(Point::new(14.0, 3.0), a, b), 5.0);
    assert_eq!(distance(Point::new(0.0, 0.0), a, a), 0.0);
}
