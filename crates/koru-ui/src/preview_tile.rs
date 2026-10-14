// SPDX-License-Identifier: MPL-2.0

//! A tile that shows what a choice looks like above its name — a theme, a
//! layout — for the few settings where a picture says more than a word.
//!
//! The preview is the caller's to paint (the tile hands it the rect and a
//! clip), so one component serves a theme swatch today and a page-width
//! diagram tomorrow without learning about either. The tile owns the frame,
//! the name, and the chosen ring.

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{inspect, scene, theme};
use vello::Scene;
use vello::kurbo::Affine;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PreviewTile<'a> {
    pub label: &'a str,
    pub selected: bool,
}

impl<'a> PreviewTile<'a> {
    pub const PREVIEW_H: f32 = 64.0;
    pub const LABEL_H: f32 = 34.0;
    pub const H: f32 = Self::PREVIEW_H + Self::LABEL_H;
    pub const GAP: f32 = 10.0;
    const RADIUS: f32 = 10.0;
    const LABEL_PAD_X: f32 = 11.0;

    pub const fn new(label: &'a str, selected: bool) -> Self {
        Self { label, selected }
    }

    /// `count` equal tiles across `w`, starting at (`x`, `y`).
    pub fn row(x: f32, y: f32, w: f32, count: usize) -> Vec<Rect> {
        if count == 0 {
            return Vec::new();
        }
        let tile_w = (w - Self::GAP * (count - 1) as f32) / count as f32;
        (0..count)
            .map(|i| Rect::new(x + i as f32 * (tile_w + Self::GAP), y, tile_w, Self::H))
            .collect()
    }

    /// The area the caller's preview is painted into.
    pub fn preview_rect(rect: Rect) -> Rect {
        Rect::new(rect.x, rect.y, rect.w, Self::PREVIEW_H)
    }

    #[allow(clippy::too_many_arguments)]
    #[track_caller]
    pub fn draw(
        self,
        canvas: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        rect: Rect,
        hovered: bool,
        alpha: f32,
        preview: impl FnOnce(&mut Scene, Affine, Rect),
    ) {
        let _component = inspect::scope(
            "KoruPreviewTile",
            "component",
            root,
            scene::rect(rect.x, rect.y, rect.w, rect.h),
            |r| {
                r.property(
                    "implementation",
                    "koru_ui::preview_tile::PreviewTile",
                    "shared implementation",
                );
                r.property("label", self.label, "caller label");
                r.property(
                    "selected / hovered",
                    format!("{} / {}", self.selected, hovered),
                    "instance state",
                );
                r.property("docs", "docs/features/koruui/COMPONENTS.md", "catalog");
            },
        );
        let p = theme::p();
        let shape = scene::rrect(rect.x, rect.y, rect.w, rect.h, Self::RADIUS);
        scene::fill(canvas, root, p.panel_bg.multiply_alpha(alpha), &shape);
        {
            let area = Self::preview_rect(rect);
            let _preview = inspect::scope(
                "Tile preview",
                "part",
                root,
                scene::rect(area.x, area.y, area.w, area.h),
                |_| {},
            );
            canvas.push_clip_layer(vello::peniko::Fill::NonZero, root, &shape);
            preview(canvas, root, area);
            canvas.pop_layer();
        }
        scene::fill(
            canvas,
            root,
            p.panel_border.multiply_alpha(alpha),
            &scene::rect(rect.x, rect.y + Self::PREVIEW_H, rect.w, 1.0),
        );
        let label = engine.simple_layout(self.label, theme::role::ui());
        crate::typography::line(
            "Tile label",
            canvas,
            root,
            &label,
            rect.x + Self::LABEL_PAD_X,
            rect.y + Self::PREVIEW_H + (Self::LABEL_H - scene::first_line_height(&label)) * 0.5,
            theme::role::ui(),
            "theme::role::ui()",
            p.text.multiply_alpha(alpha),
        );
        let (ring, width) = if self.selected {
            (p.accent, 2.0)
        } else if hovered {
            (p.focus_ring, theme::BORDER_W)
        } else {
            (p.panel_border, theme::BORDER_W)
        };
        scene::stroke(canvas, root, ring.multiply_alpha(alpha), width, &shape);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_share_the_row_evenly() {
        let tiles = PreviewTile::row(0.0, 0.0, 320.0, 3);
        assert_eq!(tiles.len(), 3);
        let last = tiles[2];
        assert!((last.x + last.w - 320.0).abs() < 0.01);
        assert!((tiles[0].w - tiles[1].w).abs() < 0.01);
        assert_eq!(tiles[1].x - (tiles[0].x + tiles[0].w), PreviewTile::GAP);
    }
}
