// SPDX-License-Identifier: MPL-2.0

//! **The filter chip**: a pill that says what a list is filtered to, and
//! whether you can change it.
//!
//! The row over the Tasks page — *Priority ≥ High*, *Open ⌄*, *Any date ⌄* —
//! and any other surface that filters a list the same way. A chip is a label,
//! optionally with a **trailing mark**: a caret when it is a selector whose
//! label *is* its current value, or an `×` when it is a filter that can be
//! switched off again.
//!
//! **The padding is symmetric, and that is the whole reason this is a
//! component.** It was drawn by hand on the Tasks page with the trailing
//! glyph in a lane flush against the chip's right edge, so a chip with a
//! caret had ten pixels of air on the left and about two on the right; the
//! owner saw it (round nine, finding 45). Here the label's box starts
//! [`PAD_X`] from the left border and the mark's box ends [`PAD_X`] from the
//! right one, whether or not there is a mark, and the height is the label's
//! own line plus [`PAD_Y`] above and below. One arithmetic, used by drawing
//! and by measuring, so a chip cannot be wider than it looks.
//!
//! It is **not** [`chip`](koru_draw::chip), which is the *attribute* chip a
//! row wears (a tag, a due, a priority — a value on a node), and not
//! [`choice_chip`](crate::choice_chip), which is the squared-off segment a
//! settings row offers. Three families, three shapes, named apart.

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{icons, scene, theme};
use vello::Scene;
use vello::kurbo::Affine;

/// Air either side of the chip's contents — the label's left and the mark's
/// right, so the two read as one inset.
pub const PAD_X: f32 = 11.0;
/// Air above and below the label's line.
pub const PAD_Y: f32 = 5.0;
/// Between the label and the trailing mark.
pub const GAP: f32 = 5.0;
/// The trailing mark's own box. Square, so the caret and the `×` centre in
/// the same place and a chip does not change width when it changes mark.
pub const MARK: f32 = 12.0;

/// What a chip's trailing mark says, if anything.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Mark {
    /// A plain chip: a label and nothing else.
    #[default]
    None,
    /// A **selector**: the label is its current value, and the caret says
    /// pressing it offers the others.
    Caret,
    /// A filter that is **switched on** and can be switched off again.
    Clear,
}

impl Mark {
    /// Whether this mark takes room at the chip's trailing end.
    pub fn shown(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// The chip's height: the label's own line, plus [`PAD_Y`] above and below.
///
/// Derived rather than a constant, so a density change moves the row rather
/// than clipping it.
pub fn height() -> f32 {
    theme::role::hint().metrics.line_height + PAD_Y * 2.0
}

/// How wide a chip has to be for `label` and `mark`.
pub fn width(engine: &mut TextEngine, label: &str, mark: Mark) -> f32 {
    let text = engine.simple_layout(label, theme::role::hint()).width();
    let trailing = if mark.shown() { GAP + MARK } else { 0.0 };
    PAD_X * 2.0 + text + trailing
}

/// Where the trailing mark sits inside `rect`, or `None` when there is none.
///
/// **Public so a test can assert the symmetry** the module doc promises: this
/// box's right edge is [`PAD_X`] from the chip's right border, exactly as the
/// label's left edge is from its left one.
pub fn mark_rect(rect: Rect, mark: Mark) -> Option<Rect> {
    mark.shown().then(|| {
        Rect::new(
            rect.x + rect.w - PAD_X - MARK,
            rect.y + (rect.h - MARK) * 0.5,
            MARK,
            MARK,
        )
    })
}

/// The fill and the ink a chip wears.
///
/// **On is the one inverted surface** — dark in both themes — and off is the
/// chip family's own neutral pair, spelled from the palette rather than by
/// asking the theme a second time.
pub fn tokens(active: bool, p: &theme::Palette) -> (vello::peniko::Color, vello::peniko::Color) {
    if active {
        (p.bubble_bg, p.bubble_fg)
    } else {
        (p.chip_bg, p.text_dim)
    }
}

/// Paint one chip into `rect`.
///
/// `active` is *this dimension is set*, which inverts the chip; `hovered` is
/// the pointer, tinted over whichever surface is underneath — `bubble_hover`
/// over the inverted one, because `hover_tint` is the text colour at low
/// alpha and vanishes against `bubble_bg`.
pub fn draw(
    scene: &mut Scene,
    root: Affine,
    engine: &mut TextEngine,
    rect: Rect,
    label: &str,
    mark: Mark,
    active: bool,
    hovered: bool,
) {
    let p = theme::p();
    let (fill, ink) = tokens(active, &p);
    let pill = scene::rrect(rect.x, rect.y, rect.w, rect.h, rect.h * 0.5);
    scene::fill(scene, root, fill, &pill);
    if hovered {
        let tint = if active { p.bubble_hover } else { p.hover_tint };
        scene::fill(scene, root, tint, &pill);
    }
    let laid = engine.simple_layout(label, theme::role::hint());
    scene::draw_layout_first_line(
        scene,
        root,
        &laid,
        rect.x + PAD_X,
        scene::centre_y_in(&laid, rect),
        Some(ink),
    );
    let Some(box_) = mark_rect(rect, mark) else {
        return;
    };
    let (cx, cy) = (box_.x + box_.w * 0.5, box_.y + box_.h * 0.5);
    match mark {
        Mark::Caret => {
            let glyph = engine.icon_layout(icons::CARET_DOWN, MARK);
            scene::draw_layout(
                scene,
                root,
                &glyph,
                cx - glyph.width() * 0.5,
                cy - glyph.height() * 0.5,
                Some(ink),
            );
        }
        // Two strokes, the way the attribute chip's own `×` is drawn — a
        // glyph would be a second mark for the same gesture, and Phosphor has
        // no `×` at this weight that matches the one on every tag chip.
        Mark::Clear => {
            let arm = (MARK * 0.25) as f64;
            for (dx, dy) in [(arm, arm), (arm, -arm)] {
                scene::stroke(
                    scene,
                    root,
                    ink,
                    1.3,
                    &vello::kurbo::Line::new(
                        (cx as f64 - dx, cy as f64 - dy),
                        (cx as f64 + dx, cy as f64 + dy),
                    ),
                );
            }
        }
        Mark::None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The air is the same on both sides.** The label's left edge sits
    /// `PAD_X` from the chip's left border and the mark's box ends `PAD_X`
    /// from its right one — the asymmetry the owner saw, stated as a test so
    /// it cannot come back.
    #[test]
    fn the_label_and_the_mark_have_the_same_air_at_their_own_edges() {
        let mut engine = TextEngine::new();
        for mark in [Mark::Caret, Mark::Clear] {
            let w = width(&mut engine, "Any date", mark);
            let rect = Rect::new(20.0, 8.0, w, height());
            let box_ = mark_rect(rect, mark).expect("a mark takes room");
            let left = (rect.x + PAD_X) - rect.x;
            let right = (rect.x + rect.w) - (box_.x + box_.w);
            assert_eq!(left, right, "{mark:?}: one inset, both ends");
            assert!(
                box_.x > rect.x + PAD_X,
                "and the mark sits past the label's edge"
            );
        }
    }

    /// The vertical air is the same above and below, and the height is the
    /// label's line plus both.
    #[test]
    fn the_vertical_air_is_the_same_above_and_below() {
        let rect = Rect::new(0.0, 30.0, 120.0, height());
        let box_ = mark_rect(rect, Mark::Caret).expect("a mark");
        let above = box_.y - rect.y;
        let below = (rect.y + rect.h) - (box_.y + box_.h);
        assert!((above - below).abs() < 0.01, "the mark is centred");
        assert_eq!(
            height(),
            theme::role::hint().metrics.line_height + PAD_Y * 2.0
        );
    }

    /// A mark costs width, and the two marks cost the same — so switching a
    /// selector's caret for a clear `×` does not shuffle the chips beside it.
    #[test]
    fn a_mark_costs_width_and_both_marks_cost_the_same() {
        let mut engine = TextEngine::new();
        let plain = width(&mut engine, "Open", Mark::None);
        let caret = width(&mut engine, "Open", Mark::Caret);
        let clear = width(&mut engine, "Open", Mark::Clear);
        assert_eq!(caret, clear);
        assert_eq!(caret - plain, GAP + MARK);
        assert!(mark_rect(Rect::new(0.0, 0.0, plain, height()), Mark::None).is_none());
    }

    #[test]
    fn on_is_the_inverted_pair_and_off_the_neutral_one() {
        let p = theme::p();
        assert_eq!(tokens(false, &p), (p.chip_bg, p.text_dim));
        assert_eq!(tokens(true, &p), (p.bubble_bg, p.bubble_fg));
    }
}
