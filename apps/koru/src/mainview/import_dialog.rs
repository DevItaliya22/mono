// SPDX-License-Identifier: MPL-2.0

//! The legacy-workspace preview: fixed question and actions around a scrolling tree.

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{scene, theme};
use koru_ui::button::{self, ButtonRole};
use koru_ui::selectable_list::{ListScrollbar, SelectableList};
use koru_workspace_host::{ImportChoice, ImportPreview};
use vello::Scene;
use vello::kurbo::Affine;
use vello::peniko::Fill;
use winit::event::MouseScrollDelta;
use winit::keyboard::{Key, NamedKey};
use winit::window::CursorIcon;

const WIDTH: f32 = 680.0;
const HEIGHT: f32 = 620.0;
const EDGE: f32 = 16.0;
const PAD: f32 = 24.0;
const HEADER_H: f32 = 116.0;
const FOOTER_H: f32 = 72.0;
const ROW_H: f32 = 24.0;
const SCROLLBAR_W: f32 = 5.0;

const TITLE: theme::TextStyle =
    theme::TextStyle::new(theme::text_metrics::UI_BODY, theme::FontWeight::SemiBold);
const NAME: theme::TextStyle =
    theme::TextStyle::new(theme::text_metrics::SM, theme::FontWeight::SemiBold);
const BODY: theme::TextStyle =
    theme::TextStyle::new(theme::text_metrics::SM, theme::FontWeight::Medium);
const ROW: theme::TextStyle =
    theme::TextStyle::new(theme::text_metrics::SM, theme::FontWeight::Regular);

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Hit {
    Cancel,
    Keep,
    Copy,
}

pub(super) enum Outcome {
    Keep,
    Choose(ImportChoice),
}

pub(super) struct ImportDialog {
    preview: ImportPreview,
    list: SelectableList,
    focus: Hit,
    hover: Option<Hit>,
}

impl ImportDialog {
    pub(super) fn new(preview: ImportPreview) -> Self {
        Self {
            preview,
            list: SelectableList::new(ROW_H),
            // Import is not destructive, but it is still a write. A modal that
            // appears under a pending Enter must therefore arrive on Cancel.
            focus: Hit::Cancel,
            hover: None,
        }
    }

    fn item_count(&self) -> usize {
        self.preview.warnings.len() + self.preview.rows.len()
    }

    fn panel(&self, viewport: (f32, f32)) -> Rect {
        let w = WIDTH.min((viewport.0 - EDGE * 2.0).max(240.0));
        let h = HEIGHT.min((viewport.1 - EDGE * 2.0).max(260.0));
        Rect::new(
            ((viewport.0 - w) * 0.5).max(0.0),
            ((viewport.1 - h) * 0.5).max(0.0),
            w,
            h,
        )
    }

    fn preview_rect(&self, viewport: (f32, f32)) -> Rect {
        let p = self.panel(viewport);
        Rect::new(
            p.x + PAD,
            p.y + HEADER_H,
            (p.w - PAD * 2.0).max(0.0),
            (p.h - HEADER_H - FOOTER_H).max(0.0),
        )
    }

    pub(super) fn preview_contains(&self, viewport: (f32, f32), x: f32, y: f32) -> bool {
        self.preview_rect(viewport).contains(x, y)
    }

    fn hits(&self) -> Vec<Hit> {
        if self.preview.identity_conflict {
            vec![Hit::Cancel, Hit::Copy]
        } else {
            vec![Hit::Cancel, Hit::Keep, Hit::Copy]
        }
    }

    fn targets(&self, engine: &mut TextEngine, viewport: (f32, f32)) -> Vec<(Hit, Rect)> {
        let p = self.panel(viewport);
        let hits = self.hits();
        let widths: Vec<_> = hits
            .iter()
            .map(|hit| button::button_width(engine, Self::label(*hit)))
            .collect();
        let rects = button::row_from_right(p.x + p.w - PAD, p.y + p.h - PAD - button::H, &widths);
        hits.into_iter().zip(rects).collect()
    }

    fn label(hit: Hit) -> &'static str {
        match hit {
            Hit::Cancel => "Cancel",
            Hit::Keep => "Keep identity",
            Hit::Copy => "Import as copy",
        }
    }

    fn choice(hit: Hit) -> ImportChoice {
        match hit {
            Hit::Cancel => ImportChoice::Cancel,
            Hit::Keep => ImportChoice::KeepIdentity,
            Hit::Copy => ImportChoice::Copy,
        }
    }

    fn hit_at(&self, engine: &mut TextEngine, viewport: (f32, f32), x: f32, y: f32) -> Option<Hit> {
        self.targets(engine, viewport)
            .into_iter()
            .find(|(_, rect)| rect.contains(x, y))
            .map(|(hit, _)| hit)
    }

    fn scrollbar(&self, viewport: (f32, f32)) -> Option<ListScrollbar> {
        let r = self.preview_rect(viewport);
        self.list.scrollbar(self.item_count(), r.h, r.y, 28.0)
    }

    fn cycle_focus(&mut self, delta: isize) {
        let hits = self.hits();
        let at = hits.iter().position(|hit| *hit == self.focus).unwrap_or(0) as isize;
        let next = (at + delta).rem_euclid(hits.len() as isize) as usize;
        self.focus = hits[next];
    }

    pub(super) fn key(&mut self, key: &Key, viewport: (f32, f32)) -> Outcome {
        match key {
            Key::Named(NamedKey::Escape) => Outcome::Choose(ImportChoice::Cancel),
            Key::Named(NamedKey::Tab) | Key::Named(NamedKey::ArrowRight) => {
                self.cycle_focus(1);
                Outcome::Keep
            }
            Key::Named(NamedKey::ArrowLeft) => {
                self.cycle_focus(-1);
                Outcome::Keep
            }
            Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space) => {
                Outcome::Choose(Self::choice(self.focus))
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.scroll_amount(ROW_H, viewport);
                Outcome::Keep
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.scroll_amount(-ROW_H, viewport);
                Outcome::Keep
            }
            Key::Named(NamedKey::PageDown) => {
                self.scroll_amount(self.preview_rect(viewport).h * 0.85, viewport);
                Outcome::Keep
            }
            Key::Named(NamedKey::PageUp) => {
                self.scroll_amount(-self.preview_rect(viewport).h * 0.85, viewport);
                Outcome::Keep
            }
            Key::Named(NamedKey::Home) => {
                self.list.scroll = 0.0;
                Outcome::Keep
            }
            Key::Named(NamedKey::End) => {
                let r = self.preview_rect(viewport);
                self.list.scroll = self.list.max_scroll(self.item_count(), r.h);
                Outcome::Keep
            }
            _ => Outcome::Keep,
        }
    }

    pub(super) fn mouse_down(
        &mut self,
        engine: &mut TextEngine,
        viewport: (f32, f32),
        x: f32,
        y: f32,
    ) -> Outcome {
        if let Some(hit) = self.hit_at(engine, viewport, x, y) {
            return Outcome::Choose(Self::choice(hit));
        }
        let r = self.preview_rect(viewport);
        if r.contains(x, y)
            && let Some(bar) = self.scrollbar(viewport)
            && x >= r.x + r.w - 18.0
        {
            self.list.scroll = bar.scroll_for_pointer(y, bar.thumb_height * 0.5);
        }
        Outcome::Keep
    }

    pub(super) fn mouse_move(
        &mut self,
        engine: &mut TextEngine,
        viewport: (f32, f32),
        x: f32,
        y: f32,
    ) -> bool {
        let before = self.hover;
        self.hover = self.hit_at(engine, viewport, x, y);
        before != self.hover
    }

    pub(super) fn cursor(&self) -> CursorIcon {
        if self.hover.is_some() {
            koru_ui::cursor::PRESS
        } else {
            CursorIcon::Default
        }
    }

    pub(super) fn scroll_by(&mut self, delta: MouseScrollDelta, viewport: (f32, f32)) {
        let amount = match delta {
            MouseScrollDelta::LineDelta(_, y) => -y * ROW_H * 2.0,
            MouseScrollDelta::PixelDelta(p) => -p.y as f32,
        };
        self.scroll_amount(amount, viewport);
    }

    fn scroll_amount(&mut self, amount: f32, viewport: (f32, f32)) {
        let r = self.preview_rect(viewport);
        self.list.scroll_by(amount, self.item_count(), r.h);
    }

    pub(super) fn draw(
        &self,
        scene: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        viewport: (f32, f32),
    ) {
        let palette = theme::p();
        scene::fill(
            scene,
            root,
            palette.scrim,
            &scene::rect(0.0, 0.0, viewport.0, viewport.1),
        );
        let panel = self.panel(viewport);
        scene::floating_panel(
            scene,
            root,
            panel.x,
            panel.y,
            panel.w,
            panel.h,
            &scene::Panel::popover(10.0),
        );

        let title = engine.simple_layout("Import older Koru workspace", TITLE);
        scene::draw_layout_first_line(
            scene,
            root,
            &title,
            panel.x + PAD,
            panel.y + PAD,
            Some(palette.text),
        );
        let name = engine.simple_layout(&self.preview.workspace_name, NAME);
        scene::draw_layout_first_line(
            scene,
            root,
            &name,
            panel.x + PAD,
            panel.y + 52.0,
            Some(palette.text),
        );
        let summary = engine.simple_layout(
            &format!(
                "{} documents, {} nodes, {} images",
                self.preview.document_count, self.preview.node_count, self.preview.image_count
            ),
            BODY,
        );
        scene::draw_layout_first_line(
            scene,
            root,
            &summary,
            panel.x + PAD,
            panel.y + 76.0,
            Some(palette.text_dim),
        );
        if self.preview.identity_conflict {
            let conflict = engine.simple_layout(
                "This identity already exists; import it as a new copy.",
                BODY,
            );
            scene::draw_layout_first_line(
                scene,
                root,
                &conflict,
                panel.x + PAD + 260.0,
                panel.y + 76.0,
                Some(palette.close_hover),
            );
        }

        let preview = self.preview_rect(viewport);
        let well = scene::rrect(preview.x, preview.y, preview.w, preview.h, 6.0);
        scene::fill(scene, root, palette.panel_bg, &well);
        scene::stroke(scene, root, palette.panel_border, theme::BORDER_W, &well);
        let clip = vello::kurbo::Rect::new(
            preview.x as f64,
            preview.y as f64,
            (preview.x + preview.w) as f64,
            (preview.y + preview.h) as f64,
        );
        scene.push_clip_layer(Fill::NonZero, root, &clip);
        let first = (self.list.scroll / ROW_H).floor() as usize;
        let last = ((self.list.scroll + preview.h) / ROW_H).ceil() as usize;
        for index in first..last.min(self.item_count()) {
            let y = self.list.row_y(preview.y, index);
            let (text, colour, depth) = if index < self.preview.warnings.len() {
                (
                    format!("Warning: {}", self.preview.warnings[index]),
                    palette.close_hover,
                    0,
                )
            } else {
                let row = &self.preview.rows[index - self.preview.warnings.len()];
                (
                    format!(
                        "{}{} ({} {})",
                        if row.trashed { "[Trash] " } else { "" },
                        row.title,
                        row.nodes,
                        if row.nodes == 1 { "node" } else { "nodes" }
                    ),
                    if row.trashed {
                        palette.text_dim
                    } else {
                        palette.text
                    },
                    row.depth,
                )
            };
            let layout = engine.simple_layout(&text, ROW);
            scene::draw_layout_first_line(
                scene,
                root,
                &layout,
                preview.x + 12.0 + depth as f32 * 16.0,
                y + (ROW_H - scene::first_line_height(&layout)) * 0.5,
                Some(colour),
            );
        }
        scene.pop_layer();

        if let Some(bar) = self.scrollbar(viewport) {
            scene::fill(
                scene,
                root,
                palette.scrollbar_thumb,
                &scene::rrect(
                    preview.x + preview.w - 10.0,
                    bar.thumb_top,
                    SCROLLBAR_W,
                    bar.thumb_height,
                    SCROLLBAR_W * 0.5,
                ),
            );
        }

        for (hit, rect) in self.targets(engine, viewport) {
            button::draw_button(
                scene,
                root,
                engine,
                rect,
                Self::label(hit),
                if hit == Hit::Copy {
                    ButtonRole::Primary
                } else {
                    ButtonRole::Secondary
                },
                self.hover == Some(hit),
                self.focus == hit,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use koru_workspace_host::ImportPreviewRow;

    const VIEWPORT: (f32, f32) = (1000.0, 720.0);

    fn preview(rows: usize, conflict: bool) -> ImportPreview {
        ImportPreview {
            workspace_name: "Roadmap".into(),
            document_count: rows,
            node_count: rows * 10,
            image_count: 3,
            warnings: Vec::new(),
            rows: (0..rows)
                .map(|i| ImportPreviewRow {
                    depth: i % 3,
                    title: format!("Document {i}"),
                    nodes: i + 1,
                    trashed: false,
                })
                .collect(),
            identity_conflict: conflict,
        }
    }

    #[test]
    fn long_tree_scrolls_between_fixed_header_and_footer() {
        let mut dialog = ImportDialog::new(preview(55, false));
        let panel = dialog.panel(VIEWPORT);
        let tree = dialog.preview_rect(VIEWPORT);
        assert!(tree.y > panel.y);
        assert!(tree.y + tree.h < panel.y + panel.h - button::H);
        assert!(dialog.list.max_scroll(dialog.item_count(), tree.h) > 0.0);
        dialog.scroll_by(MouseScrollDelta::LineDelta(0.0, -4.0), VIEWPORT);
        assert!(dialog.list.scroll > 0.0);
    }

    #[test]
    fn cancel_is_the_keyboard_default() {
        let mut dialog = ImportDialog::new(preview(2, false));
        assert!(matches!(
            dialog.key(&Key::Named(NamedKey::Enter), VIEWPORT),
            Outcome::Choose(ImportChoice::Cancel)
        ));
    }

    #[test]
    fn an_identity_conflict_removes_keep_identity() {
        let mut engine = TextEngine::new();
        let dialog = ImportDialog::new(preview(2, true));
        let hits: Vec<_> = dialog
            .targets(&mut engine, VIEWPORT)
            .into_iter()
            .map(|(hit, _)| hit)
            .collect();
        assert_eq!(hits, vec![Hit::Cancel, Hit::Copy]);
    }

    #[test]
    fn every_button_hit_tests_to_its_choice() {
        let mut engine = TextEngine::new();
        let mut dialog = ImportDialog::new(preview(2, false));
        for (hit, rect) in dialog.targets(&mut engine, VIEWPORT) {
            let outcome = dialog.mouse_down(
                &mut engine,
                VIEWPORT,
                rect.x + rect.w * 0.5,
                rect.y + rect.h * 0.5,
            );
            assert!(
                matches!(outcome, Outcome::Choose(choice) if choice == ImportDialog::choice(hit))
            );
        }
    }
}
