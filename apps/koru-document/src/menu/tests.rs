// SPDX-License-Identifier: MPL-2.0

//! The geometry round-trips, carried over from the list flyout this replaced.
//!
//! Every one of them asks the same question in a different place: is a row
//! clickable exactly where it is drawn? That is the rule `docs/features/design/DESIGN.md`
//! states, and the reason four menus share one `Menu` rather than four copies
//! of the arithmetic.

use super::*;

const VP: (f32, f32) = (940.0, 600.0);
const W: f32 = 220.0;

/// A menu with one of every row kind, so the mixed heights are exercised.
fn rows() -> Vec<Row> {
    vec![
        Row::Heading("SECTION"),
        Row::item("First").checked(true),
        Row::item("Second"),
        Row::Divider,
        Row::item("Third").with_trailing(Trailing::Swatches(1)),
        Row::item("Dead")
            .with_trailing(Trailing::Toggle(false))
            .enabled(false),
    ]
}

fn anchor() -> Rect {
    Rect::new(120.0, 60.0, theme::AFFORDANCE_BTN, theme::AFFORDANCE_BTN)
}

/// The round-trip that matters: every selectable row hit-tests back to itself,
/// centre and both vertical edges.
#[test]
fn every_selectable_row_hit_tests_back_to_itself() {
    let rows = rows();
    let m = Menu::new(anchor(), W);
    for (i, row) in rows.iter().enumerate() {
        if !row.selectable() {
            continue;
        }
        let r = m.row_rect(&rows, VP, i);
        assert_eq!(
            m.item_at(&rows, VP, r.x + r.w * 0.5, r.y + r.h * 0.5),
            Some(i)
        );
        assert_eq!(m.item_at(&rows, VP, r.x + 2.0, r.y + 0.5), Some(i));
        assert_eq!(m.item_at(&rows, VP, r.x + 2.0, r.y + r.h - 0.5), Some(i));
    }
}

/// A heading, a hairline and a disabled row all hit nothing — which is what
/// stops a click on "Focus mode  Coming soon" doing anything.
#[test]
fn furniture_and_disabled_rows_are_not_clickable() {
    let rows = rows();
    let m = Menu::new(anchor(), W);
    for i in [0, 3, 5] {
        let r = m.row_rect(&rows, VP, i);
        assert_eq!(
            m.item_at(&rows, VP, r.centre().0, r.centre().1),
            None,
            "row {i}"
        );
    }
}

/// Rows tile the panel's inside with no gap and no overlap: the sum of their
/// heights is the panel less its padding.
#[test]
fn the_rows_tile_the_panel() {
    let rows = rows();
    let m = Menu::new(anchor(), W);
    let p = m.panel(&rows, VP);
    let first = m.row_rect(&rows, VP, 0);
    assert_eq!(first.y, p.y + PAD);
    for i in 1..rows.len() {
        let above = m.row_rect(&rows, VP, i - 1);
        let here = m.row_rect(&rows, VP, i);
        assert_eq!(here.y, above.y + above.h, "row {i} follows row {}", i - 1);
    }
    let last = m.row_rect(&rows, VP, rows.len() - 1);
    assert!((last.y + last.h + PAD - (p.y + p.h)).abs() < 0.01);
}

/// A point outside the panel hits nothing — which is what dismisses the menu
/// and lets the click fall through to the document.
#[test]
fn a_point_outside_the_panel_hits_nothing() {
    let rows = rows();
    let m = Menu::new(anchor(), W);
    let p = m.panel(&rows, VP);
    assert_eq!(m.item_at(&rows, VP, p.x - 1.0, p.y + 30.0), None);
    assert_eq!(m.item_at(&rows, VP, p.x + p.w + 1.0, p.y + 30.0), None);
    assert_eq!(m.item_at(&rows, VP, p.x + 10.0, p.y - 1.0), None);
}

/// The panel clamps inside the viewport rather than running off the right.
#[test]
fn the_panel_clamps_inside_the_viewport() {
    let rows = rows();
    let far_right = Rect::new(VP.0 - 10.0, 60.0, 20.0, 20.0);
    let p = Menu::new(far_right, W).panel(&rows, VP);
    assert!(p.x + p.w <= VP.0 - EDGE_MARGIN);
    assert!(p.x >= EDGE_MARGIN);
}

/// An opener near the floor flips the panel above it.
#[test]
fn an_opener_near_the_floor_flips_above() {
    let rows = rows();
    let low = Rect::new(60.0, VP.1 - 10.0, 20.0, 20.0);
    let p = Menu::new(low, W).panel(&rows, VP);
    assert!(p.y + p.h <= low.y, "panel sits above the opener");
}

/// Every swatch hit-tests back to itself, and the gaps between them do not —
/// a press that misses a circle cycles instead, which is a different meaning.
#[test]
fn every_swatch_hit_tests_back_to_itself() {
    let rows = rows();
    let m = Menu::new(anchor(), W);
    let r = m.row_rect(&rows, VP, 4);
    for n in 0..4 {
        let sr = swatch_rect(r, 4, n);
        assert_eq!(m.swatch_at(&rows, VP, 4, 4, sr.centre().0), Some(n));
    }
    assert_eq!(m.swatch_at(&rows, VP, 4, 4, r.x + 10.0), None);
}

/// The keyboard steps over headings, hairlines and disabled rows, and wraps.
#[test]
fn the_keyboard_walks_only_the_selectable_rows() {
    let rows = rows();
    let mut m = Menu::new(anchor(), W);
    m.step(&rows, 1);
    assert_eq!(m.hover, Some(1));
    m.step(&rows, 1);
    assert_eq!(m.hover, Some(2));
    // 3 is the hairline and 5 is disabled: the next one is 4, then it wraps.
    m.step(&rows, 1);
    assert_eq!(m.hover, Some(4));
    m.step(&rows, 1);
    assert_eq!(m.hover, Some(1));
    m.step(&rows, -1);
    assert_eq!(m.hover, Some(4));
}

/// Arrow-up into a fresh menu lands on the last row, not the first.
#[test]
fn stepping_backwards_from_nothing_starts_at_the_end() {
    let rows = rows();
    let mut m = Menu::new(anchor(), W);
    m.step(&rows, -1);
    assert_eq!(m.hover, Some(4));
}
