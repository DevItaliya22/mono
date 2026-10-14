// SPDX-License-Identifier: MPL-2.0

//! **Opening a large artifact does no work proportional to its history.**
//!
//! A ratio, never a wall clock (standing requirement 3). Two families
//! differing only in how much history they have are opened, and the work
//! the column did is compared: the same number of rows, and the same number
//! of text layouts. A build that starts laying out a version per row of
//! history fails here whatever the machine.

use koru_draw::text::TextEngine;
use vello::Scene;
use vello::kurbo::Affine;

use super::*;

fn opened(versions: u32) -> ArtifactsSurface {
    let one = family(1, "Findings", 500);
    let made = history(&one, versions);
    let present = all_present(&[(one.clone(), made.clone())]);
    let mut surface = showing(Catalog::build(
        vec![(one.clone(), made)],
        Vec::new(),
        Some(present),
    ));
    surface.toggle_family(one.id);
    surface
}

/// The whole requirement, as arithmetic.
#[test]
fn opening_a_family_costs_what_is_shown_not_what_it_has_been() {
    let mut small = opened(crate::VERSIONS_SHOWN as u32);
    let mut large = opened(5_000);
    let small_rows = rows_of(&mut small).len();
    let large_rows = rows_of(&mut large).len();
    // The large one carries exactly one extra row: *N earlier versions*.
    assert_eq!(large_rows, small_rows + 1, "{small_rows} vs {large_rows}");
    assert_eq!(small.work().rows, small_rows);

    // And a family with a hundred times the history again costs the same.
    let mut vast = opened(500_000);
    assert_eq!(rows_of(&mut vast).len(), large_rows);
    assert_eq!(vast.work().rows, large.work().rows);
}

/// The same, through the draw path, which is what actually costs a frame.
#[test]
fn drawing_an_opened_family_lays_out_what_is_shown_not_what_it_has_been() {
    let mut engine = TextEngine::new();
    let mut scene = Scene::new();
    let mut small = opened(crate::VERSIONS_SHOWN as u32);
    small.draw(&mut scene, Affine::IDENTITY, &mut engine, area(), TOP);
    let small_layouts = small.work().layouts;

    let mut scene = Scene::new();
    let mut large = opened(5_000);
    large.draw(&mut scene, Affine::IDENTITY, &mut engine, area(), TOP);
    let large_layouts = large.work().layouts;

    assert!(small_layouts > 0, "the small one drew nothing");
    // One extra line for *N earlier versions*, and not one per version.
    assert!(
        large_layouts <= small_layouts + 2,
        "{small_layouts} vs {large_layouts}"
    );
}

/// Revealing more history is a page, not the whole of it — pressing the
/// row once adds exactly one page of rows.
#[test]
fn revealing_more_history_reveals_one_page() {
    let one = family(1, "Findings", 500);
    let made = history(&one, 5_000);
    let present = all_present(&[(one.clone(), made.clone())]);
    let mut surface = showing(Catalog::build(
        vec![(one.clone(), made)],
        Vec::new(),
        Some(present),
    ));
    surface.toggle_family(one.id);
    let before = rows_of(&mut surface).len();
    surface.reveal_more(one.id);
    let after = rows_of(&mut surface).len();
    assert_eq!(after - before, crate::VERSIONS_SHOWN);

    // And shutting the family and opening it again starts at the newest
    // page: the reason to open a result is its newest version.
    surface.toggle_family(one.id);
    surface.toggle_family(one.id);
    assert_eq!(rows_of(&mut surface).len(), before);
}

/// The band is the cull as well as the hit test's bounds: a short column
/// draws a screenful, not a catalog.
#[test]
fn a_short_column_lays_out_only_the_band() {
    let mut surface = opened(5_000);
    let short = koru_draw::geometry::Rect::new(0.0, 0.0, 260.0, 200.0);
    let visible = surface.rows(short, TOP).len();
    let all = rows_of(&mut surface).len();
    assert!(visible < all, "{visible} of {all}");
    // A shut column offers nothing at all.
    let shut = koru_draw::geometry::Rect::new(0.0, 0.0, 0.0, 720.0);
    assert!(surface.rows(shut, TOP).is_empty());
}
