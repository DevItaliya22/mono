// SPDX-License-Identifier: MPL-2.0

//! **Layout coordinates to pane coordinates, then one explicit pane translation.**
//!
//! One sentence, and what it excludes: *this module turns the geometry that is
//! already in a row's parley layout into rectangles in the editor pane's **logical**
//! coordinate space, and applies no scale of its own.*
//!
//! ## The three spaces, and who converts between which
//!
//! §5 of the roadmap requires the transforms to be explicit and applied
//! *exactly once*. These spaces have explicit conversions, and they have
//! different owners on purpose:
//!
//! | From | To | Who | What it accounts for |
//! | --- | --- | --- | --- |
//! | a row's own parley layout | the editor pane's logical pixels | **here** | content column, row `y`, scroll |
//! | editor pane | window logical pixels | `projection::ViewGeometry::origin` | actual pane draw/hit-test origin |
//! | the window's logical pixels | physical, client-relative pixels | `koru_workspace_host::accessibility::projection::build` | the display scale factor |
//! | physical, client-relative | the screen | `accesskit_windows` | `ClientToScreen(hwnd)` |
//!
//! So **nothing here multiplies by a scale factor**, and nothing here knows
//! where the window is on a desktop. A second scale applied here would be
//! invisible at 100% and wrong on every other display — the shape of bug that
//! only appears on somebody else's machine.
//!
//! ## Why a translation exists at all
//!
//! parley's `build_nodes` bakes the origin into every run's `bounds`, so a
//! scroll changes every published rectangle. Re-running `build_nodes` to move a
//! row would be O(clusters) to change two numbers, which is the exact cost this
//! plan exists to remove — so [`translate`] moves a cached projection instead.
//! `character_positions` are relative to the run's own `bounds.x0`
//! (`guide/contributing/deps/ACCESSKIT.md`), so they do not move with it and are deliberately
//! left alone.

use accesskit::{Node, NodeId, Rect as AccessRect};
use koru_draw::geometry::Rect;

use crate::paragraph::Paragraph;

/// Where this row's parley layout starts, in the editor pane's logical coordinates.
///
/// **The one place this arithmetic is written.** It is exactly what
/// `Editing::selection_rects` does, which is what stops a caret rectangle and a
/// text-range rectangle from drifting apart: `content_x + row.text_x()` is the
/// left edge of the text column for this row's kind and depth, and
/// `row.y - scroll` is where the first glyph's top has been scrolled to.
pub fn row_origin(content_x: f32, scroll: f32, row: &Paragraph) -> (f32, f32) {
    (content_x + row.text_x(), row.y - scroll)
}

/// Where this row's checkbox is, in the editor pane's logical coordinates.
///
/// `None` on a row that draws none. The rectangle is
/// [`rowlayout::checkbox_band`](crate::rowlayout::checkbox_band) — **the band a
/// press is resolved against**, which is the drawn square plus the three pixels
/// of slack around it, and is what `Editor::checkbox_rect` answers — shifted by
/// exactly the `content_x`/`scroll` pair [`row_origin`] applies and by nothing
/// else. The band rather than the ink, for the reason the explore column
/// publishes its settled rect rather than its animated one: a control has to be
/// findable where it *answers*.
///
/// It is deliberately **not** derived from [`row_bounds`]: the box sits in the
/// gutter, *left* of the text column, so a rectangle worked out from the runs
/// would put it inside the words.
pub fn checkbox_bounds(content_x: f32, scroll: f32, row: &Paragraph) -> Option<Rect> {
    let band = crate::rowlayout::checkbox_band(row, row.y)?;
    Some(Rect::new(
        content_x + band.x,
        band.y - scroll,
        band.w,
        band.h,
    ))
}

/// The paragraph control's own rectangle, from the runs already projected.
///
/// The right edge comes from the runs rather than from the viewport, so a short
/// paragraph is a short rectangle and a screen magnifier following the element
/// does not jump to the window's width. Both extents are floored at one logical
/// pixel: an empty paragraph is a real place a caret can be, and a zero-area
/// rectangle is one a hit test can never land in.
pub fn row_bounds(runs: &[(NodeId, Node)], origin: (f32, f32), height: f32) -> Rect {
    let right = runs
        .iter()
        .filter_map(|(_, node)| node.bounds())
        .fold(f64::from(origin.0), |acc, bounds| acc.max(bounds.x1));
    Rect::new(
        origin.0,
        origin.1,
        (right as f32 - origin.0).max(1.0),
        height.max(1.0),
    )
}

/// Move an already-built projection to a new origin.
///
/// Used when a row's layout is unchanged but the view scrolled under it —
/// O(runs) additions instead of O(clusters) of shaping. `character_positions`
/// and `character_widths` are relative to the run's own box and are untouched.
pub fn translate(runs: &mut [(NodeId, Node)], dx: f32, dy: f32) {
    if dx == 0.0 && dy == 0.0 {
        return;
    }
    let (dx, dy) = (f64::from(dx), f64::from(dy));
    for (_, node) in runs {
        if let Some(bounds) = node.bounds() {
            node.set_bounds(AccessRect::new(
                bounds.x0 + dx,
                bounds.y0 + dy,
                bounds.x1 + dx,
                bounds.y1 + dy,
            ));
        }
    }
}

/// Where an inline object sits, in the window's logical coordinates.
///
/// parley places an inline box on the baseline and reports it already
/// positioned within the layout (`PositionedInlineBox`), so this is the same
/// origin shift every run gets and nothing more. It is here rather than beside
/// the pill code for the reason the module exists: there is one place that
/// turns layout coordinates into window coordinates.
pub fn box_bounds(x: f32, y: f32, w: f32, h: f32, origin: (f32, f32)) -> Rect {
    Rect::new(origin.0 + x, origin.1 + y, w.max(1.0), h.max(1.0))
}

/// Publish a row's text without saying where it is.
///
/// **Not a hidden node and not a missing one.** A node with no bounds is legal
/// — the adapter answers an empty rectangle and `GetBoundingRectangles` returns
/// nothing — and it is the honest report for a paragraph that is nowhere on
/// screen: its text is still readable, its position is not a fact yet. It is
/// also what keeps a scroll's update proportional to the viewport rather than
/// to the document: a run with no bounds does not move when the view does, so
/// it is not republished.
pub fn clear_geometry(runs: &mut [(NodeId, Node)]) {
    for (_, node) in runs {
        node.clear_bounds();
    }
}

/// Move a rectangle by the same delta [`translate`] moves a run by.
pub fn shift(rect: Rect, dx: f32, dy: f32) -> Rect {
    Rect::new(rect.x + dx, rect.y + dy, rect.w, rect.h)
}

/// Whether a row is close enough to the viewport for its geometry to be worth
/// publishing.
///
/// The band is the viewport plus one viewport of margin on each side — inside
/// `rowlayout`'s own shape band, so a row that answers `true` here has already
/// been laid out for real rather than estimated. The margin is what makes a
/// small scroll cost nothing: rows just off screen already carry their
/// rectangles, so they do not have to be discovered on the way in.
pub fn near_viewport(row: &Paragraph, scroll: f32, viewport_h: f32) -> bool {
    if !viewport_h.is_finite() || viewport_h <= 0.0 {
        // A viewport nothing has sized yet — a test, or a window before its
        // first layout. Everything counts as near, which is what the
        // whole-document assertions in this crate's tests expect.
        return true;
    }
    let margin = viewport_h;
    row.y + row.height >= scroll - margin && row.y <= scroll + viewport_h + margin
}
