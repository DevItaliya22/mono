// SPDX-License-Identifier: MPL-2.0

//! What the result tab puts on screen.
//!
//! Every rect comes from [`ArtifactTab::relayout`] — the same list the hit
//! test walks — so a control cannot be drawn anywhere other than where it is
//! pressable (`DESIGN.md`'s first rule for chrome). Nothing here measures
//! anything the layout does not already know.

use koru_draw::geometry::Rect;
use koru_draw::{icons, scene, theme};
use koru_i18n::{Key, t};
use koru_shell_chrome::tabs::draw_icon;
use vello::Scene;
use vello::kurbo::Affine;
use vello::peniko::Color;

use super::{ArtifactTab, TabRow};

impl ArtifactTab {
    pub(super) fn draw_row(
        &mut self,
        scene_out: &mut Scene,
        root: Affine,
        row: TabRow,
        rect: Rect,
        p: &theme::Palette,
    ) {
        // The focus ring and the hover tint, on the rows that answer to them.
        // A ring says *here, not yet chosen*; a fill would read as a second
        // thing open.
        if row.pressable() {
            if self.hover == Some(row) {
                scene::fill(scene_out, root, p.hover_tint, &chip(rect));
            }
            if self.focus == Some(row) {
                scene::stroke(scene_out, root, p.accent, theme::BORDER_W, &chip(rect));
            }
        }
        let lang = self.lang;
        match row {
            TabRow::Title => {
                let Some(title) = self.title().map(str::to_owned) else {
                    return;
                };
                self.text(
                    scene_out,
                    root,
                    rect,
                    &title,
                    theme::role::subtitle(),
                    p.text,
                );
            }
            TabRow::Meta => {
                let text = self.meta_line();
                self.text(
                    scene_out,
                    root,
                    rect,
                    &text,
                    theme::role::meta(),
                    p.row_hint,
                );
            }
            TabRow::Run => draw_icon(
                scene_out,
                root,
                &mut self.engine,
                icons::ROBOT,
                rect,
                p.accent,
            ),
            TabRow::Edit => self.text(
                scene_out,
                root,
                rect,
                t(lang, Key::ArtifactsEdit),
                theme::role::row(),
                p.accent,
            ),
            TabRow::Save => {
                // Present and dim until something is typed: a Save offered
                // over an untouched version is a Save that would make a
                // second copy of it. What "typed" means is the host's page,
                // which is why the tab is told rather than asking — asking
                // would mean exporting a document every frame.
                let ink = if self.document_edited() {
                    p.accent
                } else {
                    p.text_dim
                };
                let text = self.save_label();
                self.text(scene_out, root, rect, &text, theme::role::row(), ink);
            }
            TabRow::StopEditing => self.text(
                scene_out,
                root,
                rect,
                t(lang, Key::ArtifactsStopEditing),
                theme::role::row(),
                p.text_dim,
            ),
            TabRow::Note(index) => {
                let Some(text) = self.notes().into_iter().nth(index) else {
                    return;
                };
                self.text(
                    scene_out,
                    root,
                    rect,
                    &text,
                    theme::role::meta(),
                    p.text_dim,
                );
            }
            TabRow::Document => {
                // **Nothing is measured here.** The document laid itself out
                // in `relayout`, on this surface's engine, at this width.
                let mut view = std::mem::take(&mut self.view);
                let mut engine =
                    std::mem::replace(&mut self.engine, koru_draw::text::TextEngine::new());
                view.draw(scene_out, root, &mut engine, rect.x, rect.y);
                self.view = view;
                self.engine = engine;
            }
        }
    }

    /// One line of text, elided rather than wrapped: a result called
    /// `2026-Q3-consolidated-findings.md` has no word boundary in it, and a
    /// wrapped line would run past the column with nothing to say it had
    /// been cut.
    fn text(
        &mut self,
        scene_out: &mut Scene,
        root: Affine,
        rect: Rect,
        text: &str,
        style: theme::TextStyle,
        ink: Color,
    ) {
        let text = self.engine.elided(text, style, rect.w);
        let layout = self.engine.simple_layout(&text, style);
        self.work.layouts += 1;
        scene::draw_layout_first_line(
            scene_out,
            root,
            &layout,
            rect.x,
            rect.y + (rect.h - scene::first_line_height(&layout)) * 0.5,
            Some(ink),
        );
    }
}

/// The shape a hover tint and a focus ring are drawn on.
fn chip(rect: Rect) -> vello::kurbo::RoundedRect {
    scene::rrect(
        rect.x - 6.0,
        rect.y + 1.0,
        (rect.w + 12.0).max(0.0),
        (rect.h - 2.0).max(0.0),
        6.0,
    )
}
