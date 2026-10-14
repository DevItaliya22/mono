// SPDX-License-Identifier: MPL-2.0

//! A row of [`Swatch`]es to choose a colour from, with an optional label.
//!
//! Every place that offers a colour draws this, so they agree on the swatch,
//! the ring that marks the chosen one, the ring that marks keyboard focus and
//! — the bug it was made to remove — **the room those rings need**. A label
//! drawn one line above the squares sat inside the selection ring's reach,
//! and the ring painted over it. Here the label is measured, then the rings'
//! reach is kept clear, then the squares are placed: nothing above them can
//! touch a ring.
//!
//! It is one inspector component, `ColorPickerRow`, with its label and each
//! `KoruSwatch` inside it.

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{inspect, scene, theme};
use vello::Scene;
use vello::kurbo::Affine;
use vello::peniko::Color;

use crate::swatch::Swatch;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorPickerRow<'a> {
    label: Option<&'a str>,
    /// One per swatch, in order. `None` is the "none"/"automatic" value,
    /// drawn hollow.
    colors: &'a [Option<Color>],
    selected: Option<usize>,
    hovered: Option<usize>,
    focused: Option<usize>,
    alpha: f32,
}

impl<'a> ColorPickerRow<'a> {
    /// Between the label's line and the top of the rings' reach.
    pub const LABEL_GAP: f32 = 4.0;

    pub fn new(colors: &'a [Option<Color>]) -> Self {
        Self {
            label: None,
            colors,
            selected: None,
            hovered: None,
            focused: None,
            alpha: 1.0,
        }
    }

    /// A quiet label above the swatches — `COLOR` in an appearance panel. A
    /// row whose label is beside it (a settings card) passes none.
    pub fn label(mut self, label: &'a str) -> Self {
        self.label = Some(label);
        self
    }

    pub fn selected(mut self, index: Option<usize>) -> Self {
        self.selected = index;
        self
    }

    pub fn hovered(mut self, index: Option<usize>) -> Self {
        self.hovered = index;
        self
    }

    pub fn focused(mut self, index: Option<usize>) -> Self {
        self.focused = index;
        self
    }

    pub fn alpha(mut self, alpha: f32) -> Self {
        self.alpha = alpha;
        self
    }

    fn label_style() -> theme::TextStyle {
        theme::role::hint()
    }

    /// The label's line, its gap and the rings' reach above the squares;
    /// nothing when there is no label.
    fn above(&self) -> f32 {
        if self.label.is_some() {
            Self::label_style().metrics.line_height + Self::LABEL_GAP + Swatch::RING_REACH
        } else {
            0.0
        }
    }

    pub fn height(&self) -> f32 {
        self.above() + Swatch::SIZE
    }

    pub fn width(&self) -> f32 {
        Swatch::row_width(self.colors.len())
    }

    /// Where each swatch sits in a row whose rect is `rect`.
    pub fn swatches(&self, rect: Rect) -> Vec<Rect> {
        Swatch::row(rect.x, rect.y + self.above(), self.colors.len())
    }

    /// Which swatch a point is on.
    pub fn hit(&self, rect: Rect, x: f32, y: f32) -> Option<usize> {
        self.swatches(rect)
            .iter()
            .position(|swatch| swatch.contains(x, y))
    }

    #[track_caller]
    pub fn draw(&self, canvas: &mut Scene, root: Affine, engine: &mut TextEngine, rect: Rect) {
        let height = self.height();
        let _row = inspect::scope(
            "ColorPickerRow",
            "component",
            root,
            scene::rect(rect.x, rect.y, self.width().max(rect.w), height),
            |r| {
                r.property(
                    "implementation",
                    "koru_ui::color_picker_row::ColorPickerRow",
                    "shared implementation",
                );
                r.property("label", self.label.unwrap_or("none"), "caller label");
                r.property("swatches", self.colors.len(), "caller colours");
                r.property(
                    "selected",
                    self.selected.map_or("none".into(), |i| i.to_string()),
                    "caller state",
                );
                r.property(
                    "label clearance",
                    Swatch::RING_REACH,
                    "Swatch::RING_REACH; rings never reach the label",
                );
                r.property("docs", "docs/features/koruui/COMPONENTS.md", "catalog");
            },
        );
        if let Some(label) = self.label {
            let style = Self::label_style();
            let layout = engine.simple_layout(label, style);
            crate::typography::line(
                "ColorPickerRow label",
                canvas,
                root,
                &layout,
                rect.x,
                rect.y,
                style,
                "theme::role::hint()",
                theme::p().row_hint.multiply_alpha(self.alpha),
            );
        }
        for (index, (swatch, color)) in self.swatches(rect).into_iter().zip(self.colors).enumerate()
        {
            Swatch::draw(
                canvas,
                root,
                swatch,
                *color,
                self.selected == Some(index),
                self.hovered == Some(index),
                self.focused == Some(index),
                self.alpha,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COLORS: [Option<Color>; 3] = [None, Some(Color::BLACK), Some(Color::WHITE)];

    #[test]
    fn the_rings_reach_is_kept_clear_of_the_label() {
        let row = ColorPickerRow::new(&COLORS).label("COLOR");
        let rect = Rect::new(10.0, 100.0, 300.0, row.height());
        let label_bottom = rect.y + ColorPickerRow::label_style().metrics.line_height;
        for swatch in row.swatches(rect) {
            assert!(
                swatch.y - Swatch::RING_REACH >= label_bottom,
                "a focus ring would reach the label"
            );
            assert!(swatch.y + swatch.h <= rect.y + row.height());
        }
    }

    #[test]
    fn without_a_label_the_row_is_just_the_swatches() {
        let row = ColorPickerRow::new(&COLORS);
        assert_eq!(row.height(), Swatch::SIZE);
        assert_eq!(row.width(), Swatch::row_width(3));
        let rect = Rect::new(0.0, 0.0, row.width(), row.height());
        assert_eq!(row.swatches(rect)[0].y, 0.0);
    }

    #[test]
    fn a_point_on_a_swatch_hits_it_and_the_gap_hits_nothing() {
        let row = ColorPickerRow::new(&COLORS).label("COLOR");
        let rect = Rect::new(0.0, 0.0, 300.0, row.height());
        let swatches = row.swatches(rect);
        let second = swatches[1];
        assert_eq!(row.hit(rect, second.x + 1.0, second.y + 1.0), Some(1));
        assert_eq!(
            row.hit(rect, second.x - Swatch::GAP * 0.5, second.y + 1.0),
            None
        );
        assert_eq!(
            row.hit(rect, second.x + 1.0, 2.0),
            None,
            "the label is not a swatch"
        );
    }

    #[test]
    fn a_drawn_row_is_one_component_around_its_label_and_swatches() {
        let mut engine = TextEngine::new();
        let mut canvas = Scene::new();
        let root = Affine::IDENTITY;
        let capture = inspect::Frame::begin(1, root, scene::rect(0.0, 0.0, 400.0, 200.0));
        let row = ColorPickerRow::new(&COLORS)
            .label("COLOR")
            .selected(Some(0))
            .focused(Some(1));
        row.draw(
            &mut canvas,
            root,
            &mut engine,
            Rect::new(10.0, 10.0, 300.0, row.height()),
        );
        let snapshot = capture.finish();
        let rows: Vec<usize> = snapshot
            .records
            .iter()
            .enumerate()
            .filter_map(|(i, r)| (r.name == "ColorPickerRow").then_some(i))
            .collect();
        assert_eq!(rows.len(), 1);
        let swatches: Vec<_> = snapshot
            .records
            .iter()
            .filter(|r| r.name == "KoruSwatch")
            .collect();
        assert_eq!(swatches.len(), 3);
        assert!(swatches.iter().all(|s| s.parent == Some(rows[0])));
    }
}
