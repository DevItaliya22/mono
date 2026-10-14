// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn clipped_parent_edges_are_not_presented_as_visible_endpoints() {
    let clip = BoxRect::new(0.0, 0.0, 400.0, 300.0);
    assert!(endpoint_visible(Point::new(400.0, 100.0), clip));
    assert!(endpoint_visible(Point::new(100.0, 300.0), clip));
    assert!(!endpoint_visible(Point::new(500.0, 100.0), clip));
    assert!(!endpoint_visible(Point::new(-10.0, 100.0), clip));
}

#[test]
fn rays_pass_through_center_but_measure_component_edge_gaps() {
    let selected = BoxRect::new(80.0, 60.0, 180.0, 100.0);
    let parent = BoxRect::new(20.0, 10.0, 500.0, 400.0);
    let rays = rays(selected, parent);
    assert_eq!(rays.map(|r| r.gap), [60.0, 50.0, 320.0, 300.0]);
    let rays = super::rays(selected, parent);
    assert_eq!(rays[0].end, Point::new(20.0, 80.0));
    assert_eq!(rays[1].end, Point::new(130.0, 10.0));
    assert_eq!(rays[2].end, Point::new(500.0, 80.0));
    assert_eq!(rays[3].end, Point::new(130.0, 400.0));
}

#[test]
fn overflow_and_touching_edges_are_not_fabricated_positive_spacing() {
    let rays = rays(
        BoxRect::new(-10.0, 0.0, 90.0, 110.0),
        BoxRect::new(0.0, 0.0, 100.0, 100.0),
    );
    assert_eq!(rays.map(|r| r.gap), [-10.0, 0.0, 10.0, -10.0]);
}
