// SPDX-License-Identifier: MPL-2.0

//! Pure geometry — no font, no document, no theme mutex.
//!
//! The house style for chrome: draw it, ask where a thing went, ask what a click
//! there hits, and assert the two agree. Asserting the arithmetic back to itself
//! would pass while the table was unusable.

use super::*;

fn table() -> Table {
    Table::new(Rect::new(0.0, 0.0, 800.0, 600.0), Metrics::manager(), 0.0)
}

fn bands() -> Vec<Band> {
    vec![
        Band::Section,
        Band::Row,
        Band::Row,
        Band::Section,
        Band::Row,
    ]
}

/// The round trip. If this ever fails, something is clickable where it is not
/// drawn — the bug the single-source-of-geometry rule exists to make impossible.
#[test]
fn every_band_hit_tests_back_to_where_it_is_drawn() {
    let t = table();
    let bands = bands();
    for i in 0..bands.len() {
        let r = t.row_rect(&bands, i);
        let (x, y) = (r.x + r.w * 0.5, r.y + r.h * 0.5);
        assert_eq!(t.row_at(&bands, x, y), Some(i), "band {i}");
    }
}

/// A section heading is shorter than a row, and the walk knows it. Multiplying
/// instead would drift by a heading's worth per section.
#[test]
fn a_section_is_shorter_than_a_row_and_the_walk_knows_it() {
    let t = table();
    let bands = bands();
    let m = t.metrics;
    assert_eq!(t.row_rect(&bands, 0).h, m.section_h);
    assert_eq!(t.row_rect(&bands, 1).h, m.row_h);

    // Band 3 starts after one section and two rows, not after three rows.
    assert_eq!(
        t.row_rect(&bands, 3).y,
        t.top() + m.section_h + m.row_h * 2.0
    );
    assert_eq!(t.total_h(&bands), m.section_h * 2.0 + m.row_h * 3.0);
}

#[test]
fn scrolling_moves_the_bands_and_nothing_else() {
    let bands = bands();
    let a = table();
    let b = Table::new(a.area, a.metrics, 40.0);

    assert_eq!(b.row_rect(&bands, 0).y, a.row_rect(&bands, 0).y - 40.0);
    assert_eq!(b.top(), a.top(), "the band's origin does not move");
    assert_eq!(b.columns_rect(), a.columns_rect());
    assert_eq!(b.body(), a.body());
}

#[test]
fn the_visible_set_is_exactly_the_bands_inside_the_body() {
    let t = table();
    // Far more rows than fit, so something has to be culled.
    let bands = vec![Band::Row; 60];
    let visible = t.visible(&bands);

    assert!(!visible.is_empty());
    assert!(visible.len() < bands.len(), "something was culled");
    let body = t.body();
    for i in &visible {
        let r = t.row_rect(&bands, *i);
        assert!(r.y + r.h > body.y && r.y < body.y + body.h, "band {i}");
    }
    // And nothing outside it is clickable.
    let last = *visible.last().unwrap();
    let below = t.row_rect(&bands, last + 1);
    assert_eq!(t.row_at(&bands, 10.0, below.y + below.h * 0.5), None);
}

#[test]
fn max_scroll_is_zero_when_everything_fits() {
    let t = table();
    assert_eq!(t.max_scroll(&[Band::Row, Band::Row]), 0.0);
    let many = vec![Band::Row; 60];
    assert!(t.max_scroll(&many) > 0.0);
}

/// Above the first band and below the last is not a row.
#[test]
fn a_point_outside_the_body_is_not_a_row() {
    let t = table();
    let bands = bands();
    assert_eq!(
        t.row_at(&bands, 10.0, t.top() - 4.0),
        None,
        "in the columns"
    );
    assert_eq!(
        t.row_at(&bands, 10.0, t.area.h - 4.0),
        None,
        "in the footer"
    );
}

// ------------------------------------------------------------- the lane

fn cols() -> [Column; 3] {
    [
        Column::new(16.0, "NAME"),
        Column::new(360.0, "ORIGINAL LOCATION"),
        Column::new(600.0, "DELETED"),
    ]
}

const LANE: f32 = 120.0;

/// **The reflow guard.** A hovered row and an idle row must give identical cell
/// widths, or the name shifts sideways as the pointer travels down the list. The
/// lane is reserved always; only what is *drawn* in it changes.
#[test]
fn the_trailing_lane_is_reserved_whether_or_not_a_control_shows() {
    let t = table();
    let bands = vec![Band::Row, Band::Row];
    let idle = t.cell(&cols(), LANE, 2, t.row_rect(&bands, 0));
    let hovered = t.cell(&cols(), LANE, 2, t.row_rect(&bands, 1));
    assert_eq!(idle.w, hovered.w);

    // And the last cell stops before the lane rather than running under it.
    let r = t.row_rect(&bands, 0);
    assert!(idle.x + idle.w <= t.trailing_lane(r, LANE).x);
}

#[test]
fn a_cell_never_runs_into_the_next_column() {
    let t = table();
    let c = cols();
    let r = t.row_rect(&[Band::Row], 0);
    for i in 0..c.len() - 1 {
        let cell = t.cell(&c, LANE, i, r);
        assert!(
            cell.x + cell.w <= c[i + 1].x,
            "cell {i} spills into the next column"
        );
    }
}

/// A control is pressable exactly where it is drawn, and the rightmost one owns
/// the right-hand end of the lane.
#[test]
fn a_trailing_control_is_pressable_where_it_is_drawn() {
    let t = table();
    let r = t.row_rect(&[Band::Row], 0);
    let rects = t.trailing_rects(r, LANE, &[70.0, 24.0], 8.0);

    assert_eq!(rects.len(), 2);
    let lane = t.trailing_lane(r, LANE);
    assert_eq!(rects[1].x + rects[1].w, lane.x + lane.w, "flush right");
    assert_eq!(rects[0].x + rects[0].w + 8.0, rects[1].x, "the gap");
    for c in &rects {
        assert!(c.y >= r.y && c.y + c.h <= r.y + r.h, "inside the row");
    }
}

/// A lane wider than the row it is in must not push its controls off the left.
#[test]
fn an_absurd_lane_is_clamped_to_the_row() {
    let t = table();
    let r = t.row_rect(&[Band::Row], 0);
    let lane = t.trailing_lane(r, 9_000.0);
    assert!(lane.x >= r.x);
    assert!(lane.w <= r.w);
}

/// A taller header pushes the whole table down and takes room off the body,
/// which is the only reason `header_h` is a field rather than a constant.
#[test]
fn the_header_and_footer_are_reserved_out_of_the_body() {
    let base = table();
    let tall = Table::new(base.area, Metrics::manager().with_header(76.0), 0.0);
    assert_eq!(tall.top(), base.top() + 24.0);
    assert_eq!(tall.body().h, base.body().h - 24.0);

    let no_footer = Table::new(base.area, Metrics::manager().with_footer(0.0), 0.0);
    assert_eq!(
        no_footer.body().h,
        base.body().h + Metrics::manager().footer_h
    );
}

// ------------------------------------------------------- how a row fills

/// **The fill is padding around the content, not an inset from the row.** Get
/// that backwards and a wide page margin puts the highlight *inside* the text
/// it is meant to be highlighting.
#[test]
fn a_padded_row_fill_contains_the_text_it_highlights() {
    let m = Metrics::manager().with_pad(40.0).roomy();
    let t = Table::new(Rect::new(0.0, 0.0, 800.0, 600.0), m, 0.0);
    let r = t.row_rect(&[Band::Row], 0);
    let cols = [Column::new(40.0, "Name"), Column::new(400.0, "Deleted")];

    // The first cell starts at the page margin; the fill starts before it.
    let cell = t.cell(&cols, 100.0, 0, r);
    let fill_x = r.x + m.pad - m.row_pad_x;
    assert!(fill_x < cell.x, "the fill starts left of the text");
    assert_eq!(cell.x - fill_x, m.row_pad_x, "by exactly its padding");

    assert_eq!(m.row_pad_x, 8.0);
}

/// **One pixel between rows**, not none and not a gap. Half a pixel is taken
/// off each side of the fill, so two neighbouring rows are separated by exactly
/// one — enough to read as separate, not enough to read as spaced out.
#[test]
fn neighbouring_rows_are_one_pixel_apart() {
    let m = Metrics::manager().with_pad(40.0).roomy();
    let t = Table::new(Rect::new(0.0, 0.0, 800.0, 600.0), m, 0.0);
    let bands = vec![Band::Row, Band::Row];
    let (a, b) = (t.row_rect(&bands, 0), t.row_rect(&bands, 1));

    let a_bottom = a.y + a.h - m.row_pad_y;
    let b_top = b.y + m.row_pad_y;
    assert_eq!(b_top - a_bottom, 1.0);
}

/// The manager's rows still bleed to both edges — `roomy` is opt-in, and
/// changing the page margin must not quietly change what full-bleed means.
#[test]
fn an_unpadded_row_fill_still_reaches_both_edges() {
    for pad in [16.0, 40.0] {
        let m = Metrics::manager().with_pad(pad);
        assert_eq!(m.row_pad_x, pad, "full bleed at pad {pad}");
        assert_eq!(m.row_pad_y, 0.0);
        assert_eq!(m.row_radius, 0.0);
    }
}

/// The gap goes **below the rule**, not inside the column strip.
///
/// Folding it into `columns_h` would push the rule down with the rows and close
/// the gap again — the rule is drawn at that strip's bottom edge.
#[test]
fn the_column_gap_moves_the_rows_and_leaves_the_rule() {
    let base = Table::new(Rect::new(0.0, 0.0, 800.0, 600.0), Metrics::manager(), 0.0);
    let gapped = Table::new(base.area, Metrics::manager().roomy(), 0.0);

    assert_eq!(gapped.metrics.column_gap, 2.0);
    assert_eq!(
        gapped.columns_rect(),
        base.columns_rect(),
        "the rule has not moved"
    );
    assert_eq!(gapped.top(), base.top() + 2.0, "the rows have");
    assert_eq!(
        gapped.row_rect(&[Band::Row], 0).y,
        base.row_rect(&[Band::Row], 0).y + 2.0
    );
    // ...and the body loses exactly what the rows gained.
    assert_eq!(gapped.body().h, base.body().h - 2.0);
}

/// A table that has not asked for the roomy look keeps its rows flush with the
/// rule, which is what the workspace manager has always drawn.
#[test]
fn a_plain_table_has_no_column_gap() {
    assert_eq!(Metrics::manager().column_gap, 0.0);
}

/// **A trailing control never stands proud of its row.** It was a fixed 24,
/// which fitted a 40-tall row and overflowed the moment the rows were tightened
/// to match the explore column. Deriving it means a row height can change
/// without somebody remembering that a second number depended on it.
#[test]
fn a_trailing_control_fits_inside_the_row_fill() {
    for row_h in [24.0_f32, 28.0, 40.0] {
        let m = Metrics::manager().with_pad(40.0).with_row_h(row_h).roomy();
        let t = Table::new(Rect::new(0.0, 0.0, 800.0, 600.0), m, 0.0);
        let r = t.row_rect(&[Band::Row], 0);
        let c = t.trailing_rects(r, 120.0, &[70.0, 24.0], 8.0);

        let fill_top = r.y + m.row_pad_y;
        let fill_bottom = r.y + r.h - m.row_pad_y;
        for b in &c {
            assert!(b.y >= fill_top, "row_h {row_h}: top {} < {fill_top}", b.y);
            assert!(
                b.y + b.h <= fill_bottom,
                "row_h {row_h}: bottom {} > {fill_bottom}",
                b.y + b.h
            );
        }
    }
}

/// ...but never so short it stops reading as a button.
#[test]
fn a_trailing_control_has_a_floor() {
    let m = Metrics::manager().with_row_h(12.0).roomy();
    let t = Table::new(Rect::new(0.0, 0.0, 800.0, 600.0), m, 0.0);
    assert_eq!(t.control_h(), CONTROL_MIN_H);
}
