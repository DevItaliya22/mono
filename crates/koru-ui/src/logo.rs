// SPDX-License-Identifier: MPL-2.0

//! The Koru mark as a component: one draw path for every surface, with size
//! and colour as declared properties.
//!
//! The bitmap, its mip chain and the channel tint stay in
//! [`koru_draw::brand`]; this component is the boundary that owns them in a
//! hierarchy — a pick names **KoruLogo** with its actual size and colour
//! treatment rather than an anonymous raster rect.

use koru_draw::geometry::Rect;
use koru_draw::{brand, inspect, scene};
use vello::Scene;
use vello::kurbo::Affine;
use vello::peniko::Color;

/// The product mark, square.
///
/// ```ignore
/// KoruLogo::new(34.0).draw(canvas, root, area.x + 44.0, 40.0);
/// KoruLogo::new(20.0).tint(theme::p().text).draw(canvas, root, x, y);
/// ```
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct KoruLogo {
    size: f32,
    tint: Option<Color>,
}

impl KoruLogo {
    pub const fn new(size: f32) -> Self {
        Self { size, tint: None }
    }

    /// Recolour the mark to one colour — the brand alpha is the shape, the
    /// colour the fill. Without it the mark draws as the brand bitmap, which
    /// the channel tint recolours at build time.
    pub const fn tint(mut self, color: Color) -> Self {
        self.tint = Some(color);
        self
    }

    pub const fn size(&self) -> f32 {
        self.size
    }

    #[track_caller]
    pub fn draw(&self, canvas: &mut Scene, root: Affine, x: f32, y: f32) {
        let _logo = inspect::scope(
            "KoruLogo",
            "component",
            root,
            scene::rect(x, y, self.size, self.size),
            |r| {
                r.property("implementation", "koru_ui::logo", "shared implementation");
                r.property("size", self.size, "caller property; logical units, square");
                match self.tint {
                    Some(color) => r.property(
                        "colour",
                        inspect::color(color),
                        "caller tint; monochrome recolour of the brand alpha",
                    ),
                    None => r.property(
                        "colour",
                        "brand bitmap",
                        "koru_draw::brand master; the channel tint applies at build",
                    ),
                }
                r.property("docs", "docs/features/koruui/COMPONENTS.md", "catalog");
            },
        );
        let area = Rect::new(x, y, self.size, self.size);
        match self.tint {
            Some(color) => {
                let [r, g, b, a] = color.components;
                brand::draw_logo_tinted(
                    canvas,
                    root,
                    area,
                    [
                        (r * 255.0).round() as u8,
                        (g * 255.0).round() as u8,
                        (b * 255.0).round() as u8,
                        (a * 255.0).round() as u8,
                    ],
                );
            }
            None => brand::draw_logo(canvas, root, area),
        }
    }
}

#[cfg(test)]
mod tests;
