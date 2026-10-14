// SPDX-License-Identifier: MPL-2.0

//! A screen's title: the one line that says where you are, at
//! [`theme::role::title`] (24/32).
//!
//! It is [`KoruTextParagraph`]'s geometry — wrap into the caller's width,
//! draw from the rectangle's top-left, return the height used — with the
//! style fixed, so every title is the same size and none is a paragraph that
//! happens to be large. The inspector names it `KoruTextTitle`, and the
//! wrapped line hangs inside it.

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{inspect, scene, theme};
use vello::Scene;
use vello::kurbo::Affine;
use vello::peniko::Color;

use crate::text_paragraph::KoruTextParagraph;

pub struct KoruTextTitle<'a> {
    text: &'a str,
    color: Color,
}

impl<'a> KoruTextTitle<'a> {
    /// A title in the palette's text colour.
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            color: theme::p().text,
        }
    }

    pub fn with_color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    fn paragraph(&self) -> KoruTextParagraph<'a> {
        KoruTextParagraph::new(
            self.text,
            theme::role::title(),
            "theme::role::title()",
            self.color,
        )
    }

    /// The height the title takes wrapped to `width`, without drawing it.
    pub fn measure(&self, engine: &mut TextEngine, width: f32) -> f32 {
        self.paragraph().measure(engine, width)
    }

    /// Wrap to `rect.w` and draw from the rect's top-left; returns the
    /// height used.
    #[track_caller]
    pub fn draw(
        &self,
        canvas: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        rect: Rect,
    ) -> f32 {
        let height = self.measure(engine, rect.w);
        let _title = inspect::scope(
            "KoruTextTitle",
            "component",
            root,
            scene::rect(rect.x, rect.y, rect.w, height),
            |r| {
                r.property(
                    "implementation",
                    "koru_ui::text_title",
                    "shared implementation",
                );
                r.property("text", self.text, "caller");
                r.property("docs", "docs/features/koruui/COMPONENTS.md", "catalog");
            },
        );
        self.paragraph().draw(canvas, root, engine, rect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_is_the_title_role_and_wraps_rather_than_overflowing() {
        let mut engine = TextEngine::new();
        let style = theme::role::title();
        assert_eq!(style.metrics.size, 24.0);
        assert_eq!(style.metrics.line_height, 32.0);
        let title = KoruTextTitle::new("Welcome to Koru");
        assert_eq!(title.measure(&mut engine, 600.0), 32.0);
        assert!(
            title.measure(&mut engine, 80.0) > 32.0,
            "a narrow column wraps it"
        );
    }

    #[test]
    fn a_drawn_title_is_one_inspector_component_around_its_text() {
        let mut engine = TextEngine::new();
        let mut canvas = Scene::new();
        let root = Affine::IDENTITY;
        let capture = inspect::Frame::begin(1, root, scene::rect(0.0, 0.0, 600.0, 200.0));
        let h = KoruTextTitle::new("Welcome to Koru").draw(
            &mut canvas,
            root,
            &mut engine,
            Rect::new(10.0, 10.0, 500.0, 0.0),
        );
        let snapshot = capture.finish();
        assert_eq!(h, 32.0);
        let titles: Vec<usize> = snapshot
            .records
            .iter()
            .enumerate()
            .filter_map(|(i, r)| (r.name == "KoruTextTitle").then_some(i))
            .collect();
        assert_eq!(titles.len(), 1);
        let paragraph = snapshot
            .records
            .iter()
            .find(|r| r.name == "KoruTextParagraph")
            .expect("its text");
        assert_eq!(paragraph.parent, Some(titles[0]));
    }
}
