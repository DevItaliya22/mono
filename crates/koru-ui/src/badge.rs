// SPDX-License-Identifier: MPL-2.0

//! A compact numeric badge whose measured bounds are also its paint bounds.

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{scene, theme};
use parley::Layout;
use vello::Scene;
use vello::kurbo::Affine;

/// A measured badge. `None` is the canonical representation of no badge.
pub struct NumericBadge {
    rect: Rect,
    label: Layout<theme::Brush>,
}

impl NumericBadge {
    const HEIGHT: f32 = 16.0;
    const MIN_WIDTH: f32 = 16.0;
    const PAD_X: f32 = 4.0;

    /// Measure `label` and place the result inside the top-right of `anchor`.
    ///
    /// An empty label means there is no semantic count and therefore no badge.
    pub fn measure(engine: &mut TextEngine, anchor: Rect, label: &str) -> Option<Self> {
        if label.is_empty() {
            return None;
        }
        let layout = engine.simple_layout(label, theme::role::hint());
        let width = (layout.width() + Self::PAD_X * 2.0).max(Self::MIN_WIDTH);
        Some(Self {
            rect: Rect::new(
                anchor.x + anchor.w - width - 1.0,
                anchor.y + 1.0,
                width,
                Self::HEIGHT,
            ),
            label: layout,
        })
    }

    pub const fn rect(&self) -> Rect {
        self.rect
    }

    pub fn draw(&self, canvas: &mut Scene, root: Affine) {
        scene::fill(
            canvas,
            root,
            theme::p().accent,
            &scene::rrect(
                self.rect.x,
                self.rect.y,
                self.rect.w,
                self.rect.h,
                Self::HEIGHT * 0.5,
            ),
        );
        scene::draw_layout(
            canvas,
            root,
            &self.label,
            self.rect.x + (self.rect.w - self.label.width()) * 0.5,
            self.rect.y + (self.rect.h - self.label.height()) * 0.5,
            Some(theme::p().on_accent),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_label_is_no_badge() {
        let mut engine = TextEngine::new();
        assert!(NumericBadge::measure(&mut engine, Rect::new(0.0, 0.0, 36.0, 36.0), "",).is_none());
    }

    #[test]
    fn measured_badge_stays_inside_its_anchor() {
        let mut engine = TextEngine::new();
        let anchor = Rect::new(20.0, 10.0, 44.0, 36.0);
        let badge = NumericBadge::measure(&mut engine, anchor, "999+").unwrap();
        assert!(badge.rect().x >= anchor.x);
        assert!(badge.rect().y >= anchor.y);
        assert!(badge.rect().x + badge.rect().w <= anchor.x + anchor.w);
        assert!(badge.rect().y + badge.rect().h <= anchor.y + anchor.h);
    }
}
