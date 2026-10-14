// SPDX-License-Identifier: MPL-2.0

//! A line that shows fonts rather than naming them — *Preview: body ·
//! **Heading** · code*, each sample set in the face it stands for.
//!
//! Drawn right-aligned against a caller's right edge, so it can sit at the end
//! of a card's caption line where an aside would. The samples carry their own
//! family stacks and weights; the lead and the separators are the chrome's.

use koru_draw::text::TextEngine;
use koru_draw::{inspect, scene, theme};
use vello::Scene;
use vello::kurbo::Affine;

/// One sample: its text, the family stack it is set in, and its weight.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FontSample<'a> {
    pub text: &'a str,
    pub stack: &'a str,
    pub weight: theme::FontWeight,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FontPreview<'a> {
    /// *Preview:* — set quiet, before the samples.
    pub lead: &'a str,
    pub samples: &'a [FontSample<'a>],
}

impl FontPreview<'_> {
    const SEPARATOR: &'static str = " · ";

    fn style() -> theme::TextStyle {
        theme::role::meta()
    }

    /// Every piece in order, laid out, with whether it is a sample.
    fn pieces(&self, engine: &mut TextEngine) -> Vec<(parley::Layout<theme::Brush>, bool)> {
        let style = Self::style();
        let mut pieces = vec![(
            engine.simple_layout(&format!("{} ", self.lead), style),
            false,
        )];
        for (i, sample) in self.samples.iter().enumerate() {
            if i > 0 {
                pieces.push((engine.simple_layout(Self::SEPARATOR, style), false));
            }
            pieces.push((
                engine.simple_layout_in(
                    sample.text,
                    theme::TextStyle::new(style.metrics, sample.weight),
                    sample.stack,
                ),
                true,
            ));
        }
        pieces
    }

    /// The width the whole line takes.
    pub fn width(&self, engine: &mut TextEngine) -> f32 {
        // A layout's width ends at its last glyph's ink, so a trailing space
        // is measured between two letters and added back.
        let space = engine.simple_layout("a a", Self::style()).width()
            - engine.simple_layout("aa", Self::style()).width();
        let pieces = self.pieces(engine);
        // One trailing space after the lead and after each separator.
        pieces.iter().map(|(l, _)| l.width()).sum::<f32>() + space * self.samples.len() as f32
    }

    /// Draw the line ending at `right`, its top at `y`.
    #[track_caller]
    pub fn draw(
        &self,
        canvas: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        right: f32,
        y: f32,
        alpha: f32,
    ) {
        let width = self.width(engine);
        let line_h = Self::style().metrics.line_height;
        let _scope = inspect::scope(
            "KoruFontPreview",
            "component",
            root,
            scene::rect(right - width, y, width, line_h),
            |r| {
                r.property(
                    "implementation",
                    "koru_ui::font_preview::FontPreview",
                    "shared implementation",
                );
                r.property(
                    "samples",
                    self.samples
                        .iter()
                        .map(|s| format!("{} in {}", s.text, s.stack))
                        .collect::<Vec<_>>()
                        .join("; "),
                    "caller family stacks",
                );
                r.property("docs", "docs/features/koruui/COMPONENTS.md", "catalog");
            },
        );
        let p = theme::p();
        let space = engine.simple_layout("a a", Self::style()).width()
            - engine.simple_layout("aa", Self::style()).width();
        let mut x = right - width;
        let pieces = self.pieces(engine);
        let count = pieces.len();
        for (i, (layout, sample)) in pieces.into_iter().enumerate() {
            scene::draw_layout_first_line(
                canvas,
                root,
                &layout,
                x,
                y,
                Some(if sample { p.text } else { p.placeholder }.multiply_alpha(alpha)),
            );
            x += layout.width();
            // The lead and every separator end with a space their ink width
            // does not count; a sample ends on its last letter.
            if !sample && i + 1 < count {
                x += space;
            }
        }
    }
}
