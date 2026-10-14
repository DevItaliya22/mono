// SPDX-License-Identifier: MPL-2.0

//! A shortcut or a command in a small outlined chip — `Ctrl+Shift+D`,
//! `koru agent`.
//!
//! One look wherever a chord is named beside what it does: the Welcome page's
//! rows, a settings row's label. The text is the caller's, and a chord should
//! come from the shortcut registry rather than be typed, so the chip never
//! names a key the app does not bind.

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{inspect, scene, theme};
use vello::Scene;
use vello::kurbo::Affine;

pub struct KeyChip;

impl KeyChip {
    pub const H: f32 = 22.0;
    const PAD_X: f32 = 7.0;
    const RADIUS: f32 = 5.0;

    fn style() -> theme::TextStyle {
        theme::role::meta()
    }

    /// The chip's width for `text`.
    pub fn width(engine: &mut TextEngine, text: &str) -> f32 {
        engine.simple_layout(text, Self::style()).width() + Self::PAD_X * 2.0
    }

    /// Draw `text` in a chip at `rect` (its height is [`Self::H`]).
    #[track_caller]
    pub fn draw(canvas: &mut Scene, root: Affine, engine: &mut TextEngine, rect: Rect, text: &str) {
        let _chip = inspect::scope(
            "KoruKeyChip",
            "component",
            root,
            scene::rect(rect.x, rect.y, rect.w, rect.h),
            |r| {
                r.property(
                    "implementation",
                    "koru_ui::key_chip::KeyChip",
                    "shared implementation",
                );
                r.property("text", text, "caller (a chord from the shortcut registry)");
                r.property("docs", "docs/features/koruui/COMPONENTS.md", "catalog");
            },
        );
        let p = theme::p();
        let shape = scene::rrect(rect.x, rect.y, rect.w, rect.h, Self::RADIUS);
        scene::fill(canvas, root, p.chip_bg, &shape);
        scene::stroke(
            canvas,
            root,
            p.control_border,
            theme::CONTROL_BORDER_W,
            &shape,
        );
        let style = Self::style();
        crate::text_paragraph::draw_line(
            canvas,
            root,
            engine,
            text,
            rect.x + Self::PAD_X,
            rect.y + (rect.h - style.metrics.line_height) * 0.5,
            style,
            "theme::role::meta()",
            p.text_dim,
        );
    }
}
