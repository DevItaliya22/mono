// SPDX-License-Identifier: MPL-2.0

//! Fold or unfold a whole tree: two icon buttons, right-aligned in a section
//! heading.
//!
//! ```text
//! LIBRARY                        ⇥⇤  ⇤⇥
//!                                 ^    ^
//!                       Collapse all  Expand all
//! ```
//!
//! Its own file rather than an arm of [`super::explore`] because the explore
//! pane is not the only tree this app will grow — the context column's sections
//! and any future outline want the same control, and a second hand-rolled copy
//! is how two of them end up a pixel apart.
//!
//! One function produces the rects, one reads them back, and drawing asks the
//! first — the crate's rule, so what is clickable and what is drawn cannot
//! disagree.

use koru_draw::geometry::Rect;
use koru_draw::{icons, theme};

use vello::Scene;
use vello::kurbo::Affine;

use koru_draw::text::TextEngine;

/// Which of the two was pressed.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Fold {
    /// Shut every node that could show a caret.
    Collapse,
    /// Open all of them.
    Expand,
}

/// How wide each button is. Square and the height of the band, so the two sit on
/// the same optical grid as every other icon button in the chrome.
// Unconstructed since the ADR-262 move made dead code visible; a later wave of
// the app-shell migration deletes or revives it.
#[allow(dead_code)]
fn button_w(band: Rect) -> f32 {
    band.h.min(theme::CAPTION_H)
}

/// The two buttons, right-aligned, collapse first.
///
/// A band too narrow for both gets **zero-width rects** rather than an absence,
/// the same idiom [`koru_shell_chrome::caption::context_toggle`] uses: the caller draws and
/// hit-tests either way, with no second code path. Zero-width has to be excluded
/// from hit-testing explicitly, because `Rect::contains` is inclusive on every
/// edge — which is what [`at`] does.
pub fn buttons(band: Rect) -> [(Fold, Rect); 2] {
    let w = button_w(band);
    let w = if band.w >= w * 2.0 { w } else { 0.0 };
    let right = band.x + band.w;
    [
        (
            Fold::Collapse,
            Rect::new(right - w * 2.0, band.y, w, band.h),
        ),
        (Fold::Expand, Rect::new(right - w, band.y, w, band.h)),
    ]
}

/// Which button `(x, y)` is over, if either.
// Unconstructed since the ADR-262 move made dead code visible; a later wave of
// the app-shell migration deletes or revives it.
#[allow(dead_code)]
pub fn at(band: Rect, x: f32, y: f32) -> Option<Fold> {
    buttons(band)
        .into_iter()
        .find(|(_, r)| r.w > 0.0 && r.contains(x, y))
        .map(|(fold, _)| fold)
}

/// Both glyphs, the hovered one at full strength.
// Unconstructed since the ADR-262 move made dead code visible; a later wave of
// the app-shell migration deletes or revives it.
#[allow(dead_code)]
pub fn draw(
    scene: &mut Scene,
    root: Affine,
    engine: &mut TextEngine,
    band: Rect,
    hover: Option<Fold>,
) {
    let p = theme::p();
    for (fold, rect) in buttons(band) {
        if rect.w <= 0.0 {
            continue;
        }
        let glyph = match fold {
            Fold::Collapse => icons::FOLD_ALL,
            Fold::Expand => icons::UNFOLD_ALL,
        };
        let colour = if hover == Some(fold) {
            p.text
        } else {
            p.text_dim
        };
        koru_shell_chrome::tabs::draw_icon(scene, root, engine, glyph, rect, colour);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band() -> Rect {
        Rect::new(0.0, 100.0, 240.0, 24.0)
    }

    /// Right-aligned, collapse then expand, and never overlapping — two buttons
    /// sharing a pixel is one that cannot be pressed.
    #[test]
    fn the_two_buttons_sit_side_by_side_at_the_right_edge() {
        let [(a, ar), (b, br)] = buttons(band());
        assert_eq!((a, b), (Fold::Collapse, Fold::Expand));
        assert!(
            ar.x + ar.w <= br.x + f32::EPSILON,
            "collapse is left of expand"
        );
        assert_eq!(
            br.x + br.w,
            band().x + band().w,
            "flush with the right edge"
        );
    }

    /// Every pixel of a button answers for it, and the space to their left
    /// answers for neither — that space is the heading's own.
    #[test]
    fn hit_testing_reads_back_the_same_rects() {
        let band = band();
        for (fold, r) in buttons(band) {
            assert_eq!(at(band, r.x + r.w * 0.5, r.y + r.h * 0.5), Some(fold));
        }
        assert_eq!(at(band, band.x + 4.0, band.y + 12.0), None);
    }

    /// A band with no room offers nothing rather than a sliver that can be hit
    /// by accident. `Rect::contains` is inclusive, so a zero-width rect would
    /// otherwise still answer for its own edge.
    #[test]
    fn a_band_too_narrow_offers_nothing() {
        let narrow = Rect::new(0.0, 0.0, 8.0, 24.0);
        for (_, r) in buttons(narrow) {
            assert_eq!(r.w, 0.0);
        }
        assert_eq!(at(narrow, narrow.x + narrow.w, 12.0), None);
    }
}
