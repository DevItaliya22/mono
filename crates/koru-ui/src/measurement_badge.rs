// SPDX-License-Identifier: MPL-2.0

//! Read-only resize feedback. The host owns formatting, position and lifetime;
//! the badge owns the existing measured hint/code-font paint recipe.

use koru_draw::{geometry::Rect, inspect, scene, text::TextEngine, theme};
use parley::Layout;
use vello::{Scene, kurbo::Affine};

const PADDING: f32 = 7.0;
const RADIUS: f32 = 6.0;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct KoruMeasurementGeometry {
    pub badge: Rect,
    pub text: Rect,
}

pub struct KoruMeasurementBadge {
    label: Layout<theme::Brush>,
    style: theme::TextStyle,
    family: &'static str,
    text_scale: f32,
}

impl KoruMeasurementBadge {
    /// Measure host-formatted text. `text_scale` enlarges the hint metrics,
    /// not the seven-unit padding. Display zoom belongs in the draw root and
    /// must not be supplied here again. Hosts currently use 1.0.
    ///
    /// Panics for a nonpositive or nonfinite scale (a caller programming error).
    pub fn measure(engine: &mut TextEngine, text: &str, text_scale: f32) -> Self {
        assert!(
            text_scale.is_finite() && text_scale > 0.0,
            "text scale must be positive and finite"
        );
        let mut style = theme::role::hint();
        style.metrics.size *= text_scale;
        style.metrics.line_height *= text_scale;
        let family = theme::code_stack();
        Self {
            label: engine.simple_layout_in(text, style, family),
            style,
            family,
            text_scale,
        }
    }

    pub fn size(&self) -> (f32, f32) {
        (
            self.label.width() + PADDING * 2.0,
            self.label.height() + PADDING * 2.0,
        )
    }

    pub fn geometry(&self, x: f32, y: f32) -> KoruMeasurementGeometry {
        let (width, height) = self.size();
        KoruMeasurementGeometry {
            badge: Rect::new(x, y, width, height),
            text: Rect::new(
                x + PADDING,
                y + PADDING,
                self.label.width(),
                self.label.height(),
            ),
        }
    }

    /// Draw the already measured layout at the host position. No formatting,
    /// timer, hit target, additional measurement or document work happens here.
    pub fn draw(
        &self,
        canvas: &mut Scene,
        root: Affine,
        x: f32,
        y: f32,
    ) -> KoruMeasurementGeometry {
        let geometry = self.geometry(x, y);
        let bounds = geometry.badge;
        let _inspection = inspect::scope(
            "KoruMeasurementBadge",
            "component",
            root,
            scene::rect(bounds.x, bounds.y, bounds.w, bounds.h),
            |r| {
                r.property(
                    "component ID",
                    "C-MEASUREMENT-BADGE",
                    "docs/features/koruui/COMPONENTS.md",
                );
                r.property("component family", "C-STATUS", "catalog");
                r.property(
                    "variant",
                    "read-only measurement",
                    "shared resize recipe; no input or tooltip delay",
                );
                r.property("padding", PADDING, "existing resize recipe; GP-V05");
                r.property("radius", RADIUS, "existing resize recipe; GP-V05");
                r.property("elevation", "Flat", "scene::Panel::tooltip");
                r.property(
                    "background token",
                    "theme::p().bubble_bg",
                    "scene::Panel::tooltip",
                );
                r.property(
                    "foreground token",
                    "theme::p().bubble_fg",
                    "measurement label paint",
                );
                r.property(
                    "text scale",
                    self.text_scale,
                    "explicit measure input; display zoom applied separately by draw root",
                );
                r.property(
                    "formatting",
                    "host-formatted; text excluded from diagnostics",
                    "GP-F01; no locale formatter inside badge",
                );
                r.property(
                    "native semantics",
                    "noninteractive feedback; divider value and announcements are host-owned",
                    "no native nodes emitted by painter",
                );
                r.property("docs", "docs/features/koruui/COMPONENTS.md", "catalog");
            },
        );
        scene::floating_panel(
            canvas,
            root,
            bounds.x,
            bounds.y,
            bounds.w,
            bounds.h,
            &scene::Panel::tooltip(RADIUS),
        );
        let text = geometry.text;
        let _text = inspect::scope(
            "Measurement text",
            "part",
            root,
            scene::rect(text.x, text.y, text.w, text.h),
            |r| {
                r.property(
                    "typography role",
                    "theme::role::hint()",
                    "shared measurement recipe, with explicit text scale",
                );
                r.property(
                    "requested family",
                    self.family,
                    "theme::code_stack() at measurement; resolved faces on descendant runs",
                );
                r.property(
                    "requested size / line height",
                    format!(
                        "{} / {}",
                        self.style.metrics.size, self.style.metrics.line_height
                    ),
                    "hint metrics × text scale",
                );
                r.property(
                    "requested weight",
                    self.style.weight.value(),
                    "theme::role::hint().weight",
                );
            },
        );
        scene::draw_layout(
            canvas,
            root,
            &self.label,
            text.x,
            text.y,
            Some(theme::p().bubble_fg),
        );
        geometry
    }
}

#[cfg(test)]
mod tests;
