// SPDX-License-Identifier: MPL-2.0

//! **The one checkbox**, wherever a task is ticked.
//!
//! A rounded square, outlined when open and filled with a smaller square when
//! done. There is no checkmark glyph in it on purpose: the mark would come from
//! the icon font, and a tick is the one thing a row must still say when a font
//! has not loaded.
//!
//! It lives here because it was drawn twice and the two had already drifted —
//! a document's row drew this square, and the Tasks page drew a circle with a
//! `CHECK` glyph in it. The same task, ticked in two places, did not look like
//! the same thing. One recipe is how they stay one thing; the *size* is the
//! caller's, because a page row and a document line are not the same scale.
//!
//! What is deliberately **not** here: the hover ring and the reminder dot. Both
//! are about the surface around the box rather than about the box, and the two
//! surfaces disagree about them honestly — a document row rings the box, a
//! Tasks row lights the whole row instead.

use peniko::Color;
use vello::Scene;
use vello::kurbo::Affine;

use crate::geometry::Rect;
use crate::scene::{fill, rrect, stroke};
use crate::theme;

/// The corner radius, as a fraction of the side.
///
/// Proportional rather than a constant, because the two callers draw at 13 px
/// and 18 px: a fixed 3 px reads as a rounded square at one size and very
/// nearly a circle at the other, which is the drift this module exists to end.
const RADIUS: f32 = 0.23;
/// The stroke an open box is drawn with.
const LINE: f64 = 1.3;
/// The done mark's side and inset, as fractions of the box.
const MARK: f32 = 0.44;
const MARK_INSET: f32 = 0.28;

/// An unticked box, in `ink` — `theme::p().checkbox` for a live one, and the
/// capture row's `placeholder` for the prompt that is not a task yet.
pub fn outline(scene: &mut Scene, root: Affine, r: Rect, ink: Color) {
    stroke(scene, root, ink, LINE, &shape(r));
}

/// A ticked box: filled, with a smaller filled square inside it.
pub fn ticked(scene: &mut Scene, root: Affine, r: Rect) {
    let p = theme::p();
    fill(scene, root, p.checkbox_done, &shape(r));
    fill(
        scene,
        root,
        p.checkbox_mark,
        &rrect(
            r.x + r.w * MARK_INSET,
            r.y + r.h * MARK_INSET,
            r.w * MARK,
            r.h * MARK,
            r.w * 0.08,
        ),
    );
}

fn shape(r: Rect) -> vello::kurbo::RoundedRect {
    rrect(r.x, r.y, r.w, r.h, r.w * RADIUS)
}
