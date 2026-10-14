// SPDX-License-Identifier: MPL-2.0

//! Leading emoji in a document title, used as the page icon in Explore and tabs.

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{scene, theme};
use vello::Scene;
use vello::kurbo::Affine;

/// Colour emoji fill their em box more than outline icons, so the page emoji
/// keeps a smaller nominal size than a text tier.
pub const PAGE_EMOJI_SIZE: f32 = 10.0;

/// Slot width reserved for a page emoji (or kind glyph) before the label.
pub const PAGE_ICON_SLOT: f32 = 18.0;

// One rule, one place — draw crate owns the split so pills can ask too.
pub use koru_draw::split_leading_emoji;

/// Draw a page emoji centred in an [`PAGE_ICON_SLOT`]-wide slot at `icon_x`.
///
/// When `label_baseline` is set (y of the label's baseline), the emoji is
/// baseline-aligned to it; otherwise it is vertically centred in `row`.
pub fn draw_page_emoji(
    scene: &mut Scene,
    root: Affine,
    engine: &mut TextEngine,
    icon: &str,
    icon_x: f32,
    row: Rect,
    label_baseline: Option<f32>,
) {
    let emoji_style = theme::TextStyle::new(
        theme::TextMetrics::new(PAGE_EMOJI_SIZE, PAGE_EMOJI_SIZE * 1.25),
        theme::FontWeight::Medium,
    );
    let glyph = engine.simple_layout(icon, emoji_style);
    let glyph_y = match (
        label_baseline,
        glyph.lines().next().map(|line| line.metrics().baseline),
    ) {
        (Some(label_bl), Some(emoji_bl)) => label_bl - emoji_bl,
        _ => row.y + (row.h - glyph.height()) * 0.5,
    };
    scene::draw_layout(
        scene,
        root,
        &glyph,
        icon_x + (PAGE_ICON_SLOT - glyph.width()) * 0.5,
        glyph_y,
        None,
    );
}
