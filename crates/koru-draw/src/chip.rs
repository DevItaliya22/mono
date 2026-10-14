// SPDX-License-Identifier: MPL-2.0

//! The attribute chip: the compact marker a set attribute puts after a row's
//! text — R0's one shared display element (roadmap A2).
//!
//! A chip is not a pill. A pill is *content*: anchored to a character, part of
//! the text, selectable and replaceable. A chip is *presentation of node
//! state*: it trails the text, is never in the layout, and disappears the
//! moment the attribute is cleared. What this module holds is the drawable —
//! the cached label layout, the recipe, the metrics. Deciding *which* chips a
//! node earns means reading the document, which is `koru-editor`'s
//! (`chips.rs` there), the same split `pill.rs` documents.
//!
//! Two kinds ship in R0: a **tag chip** (the tag's name, tinted by the tag's
//! accent tone) and a **color swatch** (a dot of the node's `koru.color`).
//! A3's date chip and A4's priority chip add [`ChipKind`] variants and reuse
//! this cache and these metrics — that is the extension slot, deliberately.

use std::collections::HashMap;

use parley::Layout;
use peniko::Color;
use vello::Scene;
use vello::kurbo::{Affine, Circle};

use crate::scene::{draw_layout, fill, rrect};
use crate::theme::{self, Brush};

/// Which recipe a chip draws with.
///
/// `Tag` is a labelled lozenge; `Swatch` is a bare dot. `Due` and `Priority`
/// (A3/A4) are labelled lozenges too, always in the quiet neutral colors —
/// a due date is not urgent-red and a priority is not colored, because color
/// stays presentation (invariant 13) and derived states like overdue are
/// computed by views, never painted here.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum ChipKind {
    Tag,
    Swatch,
    /// The `koru.due` chip: compact date text, neutral tone.
    Due,
    /// The `koru.priority` chip: the level's name, neutral tone.
    Priority,
}

/// Chip metrics, shared by measuring, drawing and hit-testing so the three
/// cannot disagree.
pub const CHIP_H: f32 = 17.0;
pub const CHIP_PAD_X: f32 = 6.0;
/// Air between the text's end and the first chip, and between chips.
pub const CHIP_GAP: f32 = 8.0;
/// The color swatch's diameter.
pub const SWATCH_D: f32 = 10.0;
/// Air between a chip's leading glyph and its label.
///
/// The lane itself is the glyph's own advance — an icon is a glyph and
/// measures like one — so only the gap is a constant here. A chip with no
/// lead reserves neither: unlike the remove lane, which is always there so a
/// hover cannot move a chip, a leading glyph is a property of the *value* and
/// two chips that differ in it are two different drawables anyway.
pub const CHIP_ICON_GAP: f32 = 4.0;
/// The lane **every** chip reserves after its label (or, for a swatch, after
/// its dot) for the remove `×`.
///
/// Reserved whether or not the glyph is inked, so a hover cannot make a row's
/// chips shuffle sideways. Wide enough that the whole lane is a comfortable
/// target at the chip's 17px height.
pub const CHIP_CLOSE_W: f32 = 15.0;
/// The `×` itself, drawn as two strokes inside [`CHIP_CLOSE_W`].
const CLOSE_ARM: f32 = 3.0;
const CLOSE_STROKE: f32 = 1.3;

/// How much wider than its label a chip of this kind is.
///
/// The one place the answer lives: `measure_attr_chip` sizes the *label*, and
/// this is what a placed chip adds on top — so the lozenge that is filled, the
/// rect that is hit-tested and the advance to the next chip are one number.
///
/// **Every kind reserves the remove lane** (amended [ADR-188], 2026-08-27).
/// It was the tag chip's alone while the other three were cleared only through
/// their picker's Clear row; a chip is the control for its attribute, and the
/// control that takes it off belongs on it.
///
/// [ADR-188]: ../../../docs/decisions/ADR-201-225.md#adr-211-chipclick--an-attribute-chip-is-the-control-for-its-attribute-and-a-tag-comes-off-at-the-chip
pub fn extra_w(kind: ChipKind) -> f32 {
    match kind {
        ChipKind::Tag | ChipKind::Swatch | ChipKind::Due | ChipKind::Priority => CHIP_CLOSE_W,
    }
}

/// The remove lane inside a placed chip — `Some` wherever [`extra_w`] gave
/// one, which is every kind.
///
/// Kept as an `Option` rather than a bare `Rect` on purpose: it is the one
/// place a kind could stop offering removal, and a caller that already asks
/// `is_some` would go on being right.
pub fn close_rect(kind: ChipKind, chip: crate::geometry::Rect) -> Option<crate::geometry::Rect> {
    let w = extra_w(kind);
    (w > 0.0).then(|| crate::geometry::Rect::new(chip.x + chip.w - w, chip.y, w, chip.h))
}

/// Everything needed to paint one chip, shaped once and retained.
pub struct ChipVisual {
    pub kind: ChipKind,
    pub width: f32,
    /// `None` for a swatch, which has no text.
    pub label: Option<Layout<Brush>>,
    /// A glyph before the label — the queue a due falls in. Shaped once here
    /// beside the label, so a frame that has drawn this chip lays out nothing.
    pub lead: Option<Layout<Brush>>,
    /// Whether the label is struck through.
    ///
    /// **A field, not `lead == Some(CHECK_SQUARE)`.** The two are equivalent
    /// only because a completed chip is given both, and a paint that inferred
    /// one state from another's glyph is a rule nobody could find later.
    ///
    /// It is deliberately **not** in [`ChipKey`]: it is a pure function of the
    /// same lifecycle that chose the lead, so no two entries can share a key
    /// and disagree about it. Anything that would let a strike vary
    /// independently of the lead has to put it in the key instead.
    pub struck: bool,
}

/// Key: the kind, the label, the accent token (which decides the baked label
/// brush) and the leading glyph. The theme is not in the key — a theme flip
/// clears the cache, the same contract the pill cache has.
///
/// **The glyph is in the key because it is in the width.** `chip_boxes`
/// hit-tests the rectangle it measured, so a chip that painted a lane it had
/// not measured would be pressable where it is not drawn. The accent is here
/// for a different reason and changes no width: it bakes the label's ink.
pub(crate) type ChipKey = (ChipKind, String, Option<String>, Option<char>);

/// Shaped chips, keyed by everything that affects shaping or ink.
#[derive(Default)]
pub struct ChipCache(pub(crate) HashMap<ChipKey, ChipVisual>);

impl ChipCache {
    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn get(
        &self,
        kind: ChipKind,
        label: &str,
        accent: Option<&str>,
        lead: Option<char>,
    ) -> Option<&ChipVisual> {
        self.0.get(&Self::key(kind, label, accent, lead))
    }

    pub(crate) fn key(
        kind: ChipKind,
        label: &str,
        accent: Option<&str>,
        lead: Option<char>,
    ) -> ChipKey {
        (kind, label.to_owned(), accent.map(str::to_owned), lead)
    }
}

/// The fill/ink pair a chip draws with: its accent tone, or the quiet default
/// chip colors for a tag with no color of its own (or a token this build does
/// not know — a peer's newer palette must still render legibly).
pub fn chip_colors(accent: Option<&str>) -> (Color, Color) {
    match accent.and_then(theme::accent_tone) {
        Some(tone) => (tone.fill, tone.ink),
        None => (theme::p().chip_bg, theme::p().text_dim),
    }
}

/// Paint a measured chip at its top-left corner.
///
/// `close` inks the remove `×` in the lane [`extra_w`] reserved — on every
/// kind, the swatch included. The lane is there whichever way this is passed —
/// only the glyph comes and goes — so a caller that passes `false` draws the
/// same shape at the same width.
pub fn draw(
    scene: &mut Scene,
    root: Affine,
    visual: &ChipVisual,
    accent: Option<&str>,
    x: f32,
    y: f32,
    close: bool,
) {
    let width = visual.width + extra_w(visual.kind);
    match visual.kind {
        // Three labelled lozenges, one recipe. Due and Priority pass
        // `accent: None` from the seam above, so `chip_colors` answers the
        // neutral pair for them by construction.
        ChipKind::Tag | ChipKind::Due | ChipKind::Priority => {
            let (fill_color, ink) = chip_colors(accent);
            fill(
                scene,
                root,
                fill_color,
                &rrect(x, y, width, CHIP_H, CHIP_H * 0.5),
            );
            if close {
                draw_close(scene, root, ink, x + width, y);
            }
            // The leading glyph, then the label past it. The glyph takes the
            // chip's own `ink` rather than a colour of its own: a tag-toned
            // chip draws its icon in that tone and a neutral one draws
            // `text_dim`, which is the standing rule that no menu or chip
            // glyph earns a `Palette` field.
            let mut text_x = x + CHIP_PAD_X;
            if let Some(lead) = &visual.lead {
                draw_layout(
                    scene,
                    root,
                    lead,
                    text_x,
                    y + (CHIP_H - lead.height()) * 0.5,
                    Some(ink),
                );
                text_x += lead.width() + CHIP_ICON_GAP;
            }
            if let Some(label) = &visual.label {
                draw_layout(
                    scene,
                    root,
                    label,
                    text_x,
                    y + (CHIP_H - label.height()) * 0.5,
                    // The ink was baked into the layout at measure time; None
                    // keeps it.
                    None,
                );
                // **Struck across the label's own advance, not the lozenge.**
                // A line through the whole chip would cross the glyph and the
                // remove lane, which are controls rather than the thing that
                // is done. Its own thin stroke in the label's ink, centred on
                // the line box the way a text decoration is.
                if visual.struck {
                    let mid = y + CHIP_H * 0.5;
                    crate::scene::stroke(
                        scene,
                        root,
                        ink,
                        1.0,
                        &vello::kurbo::Line::new(
                            (text_x as f64, mid as f64),
                            ((text_x + label.width()) as f64, mid as f64),
                        ),
                    );
                }
            }
        }
        ChipKind::Swatch => {
            let dot = accent
                .and_then(theme::accent_tone)
                .map(|tone| tone.swatch)
                .unwrap_or(theme::p().text_dim);
            let r = SWATCH_D * 0.5;
            fill(
                scene,
                root,
                dot,
                &Circle::new(((x + r) as f64, (y + CHIP_H * 0.5) as f64), r as f64),
            );
            // **To the right of the dot**, in the lane [`extra_w`] reserved —
            // the same place, measured the same way, as the `×` on the three
            // labelled lozenges. The swatch has no fill behind it to carry a
            // tone's ink, so the glyph takes the neutral dim text colour
            // rather than the hue: an `×` in the swatch's own saturated colour
            // reads as part of the dot rather than as a control on it.
            if close {
                draw_close(scene, root, theme::p().text_dim, x + width, y);
            }
        }
    }
}

/// The remove `×`, drawn as two strokes centred in the [`CHIP_CLOSE_W`] lane
/// that ends at `right`. One body, so the swatch's glyph and the lozenge's are
/// the same mark in the same place inside their chip.
fn draw_close(scene: &mut Scene, root: Affine, ink: Color, right: f32, y: f32) {
    let cx = right - CHIP_CLOSE_W * 0.5 - 1.0;
    let cy = y + CHIP_H * 0.5;
    for (dx, dy) in [(CLOSE_ARM, CLOSE_ARM), (CLOSE_ARM, -CLOSE_ARM)] {
        crate::scene::stroke(
            scene,
            root,
            ink,
            CLOSE_STROKE as f64,
            &vello::kurbo::Line::new(
                ((cx - dx) as f64, (cy - dy) as f64),
                ((cx + dx) as f64, (cy + dy) as f64),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::TextEngine;

    /// **A leading glyph is a lane in the width, and the width is what is
    /// hit-tested.** `chip_boxes` measures a chip and then presses the
    /// rectangle it measured, so a glyph the paint knew about and the measure
    /// did not would make the chip pressable where it is not drawn.
    #[test]
    fn a_leading_glyph_widens_the_chip_by_its_own_lane() {
        let mut e = TextEngine::new();
        let plain = e.measure_attr_chip(ChipKind::Due, "Wed", None, None, false);
        let led = e.measure_attr_chip(ChipKind::Due, "Wed", None, Some(crate::icons::CLOCK), false);
        assert!(led > plain, "the lane is in the width: {plain} -> {led}");

        // And it is exactly the glyph's advance plus the gap — no rounding,
        // no second constant.
        let visual = e
            .attr_chips
            .get(ChipKind::Due, "Wed", None, Some(crate::icons::CLOCK))
            .expect("the led chip was cached");
        let lane = visual.lead.as_ref().expect("it has a lead").width() + CHIP_ICON_GAP;
        assert!(
            (led - plain - lane).abs() < 0.01,
            "{led} - {plain} should be the lane {lane}"
        );
    }

    /// Two chips reading the same word with different glyphs are two
    /// drawables, so they are two cache entries. This is what makes the key's
    /// fourth element load-bearing rather than decorative.
    #[test]
    fn the_cache_holds_a_led_chip_and_an_unled_one_apart() {
        let mut e = TextEngine::new();
        e.measure_attr_chip(ChipKind::Due, "Wed", None, None, false);
        e.measure_attr_chip(ChipKind::Due, "Wed", None, Some(crate::icons::CLOCK), false);
        assert!(
            e.attr_chips.get(ChipKind::Due, "Wed", None, None).is_some(),
            "the unled one is still there"
        );
        assert!(
            e.attr_chips
                .get(ChipKind::Due, "Wed", None, Some(crate::icons::CLOCK))
                .is_some(),
            "and the led one is its own entry"
        );
    }

    /// **The strike is paint, not width.** A completed chip is exactly as wide
    /// as an open one with the same label and lead: the line is drawn across
    /// an advance that is already there. If it ever changes the width it has
    /// to enter [`ChipKey`], which is the invariant `struck`'s doc names.
    #[test]
    fn a_struck_chip_measures_the_same_as_an_unstruck_one() {
        let mut e = TextEngine::new();
        let lead = Some(crate::icons::CHECK_SQUARE);
        let open = e.measure_attr_chip(ChipKind::Due, "Wed", None, lead, false);
        // Same key by construction — `struck` is deliberately not in it — so
        // this reads the cached entry back rather than shaping a second one.
        let done = e.measure_attr_chip(ChipKind::Due, "Wed", None, lead, true);
        assert!((open - done).abs() < f32::EPSILON, "{open} vs {done}");
    }

    /// The remove lane is the trailing edge and knows nothing about a lead:
    /// it is `extra_w`, added past the measured width by both the geometry and
    /// the paint, so a glyph at the other end cannot move it.
    #[test]
    fn the_remove_lane_is_the_same_rectangle_with_or_without_a_lead() {
        let r = crate::geometry::Rect::new(10.0, 20.0, 80.0, CHIP_H);
        assert_eq!(
            close_rect(ChipKind::Due, r),
            close_rect(ChipKind::Tag, r),
            "one lane, one place, whatever the chip carries"
        );
        let lane = close_rect(ChipKind::Due, r).expect("every kind reserves one");
        assert!(
            (lane.x + lane.w - (r.x + r.w)).abs() < f32::EPSILON,
            "it ends where the chip ends"
        );
    }

    /// **The swatch takes neither.** It has no label to strike and no text
    /// lane to put a glyph in, so a lead passed for one changes nothing.
    #[test]
    fn a_swatch_ignores_a_lead_and_a_strike() {
        let mut e = TextEngine::new();
        let plain = e.measure_attr_chip(ChipKind::Swatch, "", Some("red"), None, false);
        let led = e.measure_attr_chip(
            ChipKind::Swatch,
            "",
            Some("red"),
            Some(crate::icons::CLOCK),
            true,
        );
        assert!((plain - led).abs() < f32::EPSILON, "a dot is a dot");
        assert!((plain - SWATCH_D).abs() < f32::EPSILON);
    }
}
