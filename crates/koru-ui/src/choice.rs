// SPDX-License-Identifier: MPL-2.0

//! A question with up to three answers, stacked.
//!
//! [`Confirm`](crate::Confirm) has exactly two targets and lays them in a
//! row; a question with three answers — *End chat*, *Keep chat with a
//! floating icon*, *Close and tell me when support replies* — does not fit a
//! row in a narrow window and is not a confirmation. Same floating panel,
//! same buttons, same scrim; the answers stack full-width in the order given,
//! one of them declared the default (drawn primary, Enter presses it), and
//! Escape or a press outside the panel dismisses.
//!
//! Folding `Confirm` into a two-answer `Choice` is noted as tech debt rather
//! than done: `Confirm` carries the destructive-badge and default-answer
//! rules (ADR-265) that this widget deliberately does not.

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{scene, theme};
use vello::Scene;
use vello::kurbo::Affine;
use winit::keyboard::{Key, NamedKey};
use winit::window::CursorIcon;

use crate::button::{self, ButtonRole};

const WIDTH: f32 = 400.0;
const PAD: f32 = 20.0;
const RADIUS: f32 = 10.0;
const EDGE: f32 = 16.0;
const GAP: f32 = 8.0;

const TITLE: theme::TextStyle = theme::role::section_heading();
const BODY: theme::TextStyle = theme::role::row();

/// What a press or a key amounted to.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ChoiceOutcome {
    /// Still open: a press on the panel's blank part, or a key that moved focus.
    Keep,
    /// Escape, or a press outside the panel. Nothing was chosen.
    Dismiss,
    /// The answer at this index in the order the buttons were added.
    Chosen(usize),
}

pub struct Choice {
    title: String,
    body: String,
    labels: Vec<String>,
    default: Option<usize>,
    focus: Option<usize>,
    hover: Option<usize>,
}

impl Choice {
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            labels: Vec::new(),
            default: None,
            focus: None,
            hover: None,
        }
    }

    /// Add an answer. Order is presentation order and the index the outcome
    /// reports. At most three; a fourth is a design smell, not a feature.
    pub fn answer(mut self, label: impl Into<String>) -> Self {
        debug_assert!(
            self.labels.len() < 3,
            "a Choice holds at most three answers"
        );
        self.labels.push(label.into());
        self
    }

    /// Declare the answer Enter presses and the one drawn as primary.
    pub fn defaulting_to(mut self, index: usize) -> Self {
        self.default = Some(index);
        self.focus = Some(index);
        self
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    // ---------------------------------------------------------- geometry

    fn text_w(panel_w: f32) -> f32 {
        (panel_w - PAD * 2.0).max(80.0)
    }

    fn height(&self, engine: &mut TextEngine, panel_w: f32) -> f32 {
        let w = Self::text_w(panel_w);
        let title = engine.simple_layout_wrapped(&self.title, TITLE, w).height();
        let body = engine.simple_layout_wrapped(&self.body, BODY, w).height();
        let buttons = self.labels.len() as f32 * button::H
            + (self.labels.len().saturating_sub(1)) as f32 * GAP;
        PAD + title + 8.0 + body + 18.0 + buttons + PAD
    }

    pub fn panel(&self, engine: &mut TextEngine, viewport: (f32, f32)) -> Rect {
        let w = WIDTH.min(viewport.0 - EDGE * 2.0).max(0.0);
        let h = self.height(engine, w).min(viewport.1 - EDGE * 2.0);
        Rect::new((viewport.0 - w) * 0.5, (viewport.1 - h) * 0.5, w, h)
    }

    fn targets(&self, engine: &mut TextEngine, viewport: (f32, f32)) -> Vec<Rect> {
        let p = self.panel(engine, viewport);
        let w = Self::text_w(p.w);
        let title = engine.simple_layout_wrapped(&self.title, TITLE, w).height();
        let body = engine.simple_layout_wrapped(&self.body, BODY, w).height();
        let mut y = p.y + PAD + title + 8.0 + body + 18.0;
        self.labels
            .iter()
            .map(|_| {
                let r = Rect::new(p.x + PAD, y, w, button::H);
                y += button::H + GAP;
                r
            })
            .collect()
    }

    fn hit_at(
        &self,
        engine: &mut TextEngine,
        viewport: (f32, f32),
        x: f32,
        y: f32,
    ) -> Option<usize> {
        self.targets(engine, viewport)
            .into_iter()
            .position(|r| r.contains(x, y))
    }

    // ------------------------------------------------------------- input

    pub fn key(&mut self, key: &Key) -> ChoiceOutcome {
        match key {
            Key::Named(NamedKey::Escape) => ChoiceOutcome::Dismiss,
            Key::Named(NamedKey::Tab) | Key::Named(NamedKey::ArrowDown) => {
                self.focus = Some(self.focus.map_or(0, |i| (i + 1) % self.labels.len().max(1)));
                ChoiceOutcome::Keep
            }
            Key::Named(NamedKey::ArrowUp) => {
                let n = self.labels.len().max(1);
                self.focus = Some(self.focus.map_or(n - 1, |i| (i + n - 1) % n));
                ChoiceOutcome::Keep
            }
            Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space) => match self.focus {
                Some(i) if i < self.labels.len() => ChoiceOutcome::Chosen(i),
                _ => ChoiceOutcome::Keep,
            },
            _ => ChoiceOutcome::Keep,
        }
    }

    pub fn mouse_down(
        &mut self,
        engine: &mut TextEngine,
        viewport: (f32, f32),
        x: f32,
        y: f32,
    ) -> ChoiceOutcome {
        if let Some(i) = self.hit_at(engine, viewport, x, y) {
            return ChoiceOutcome::Chosen(i);
        }
        if self.panel(engine, viewport).contains(x, y) {
            ChoiceOutcome::Keep
        } else {
            ChoiceOutcome::Dismiss
        }
    }

    pub fn mouse_move(
        &mut self,
        engine: &mut TextEngine,
        viewport: (f32, f32),
        x: f32,
        y: f32,
    ) -> bool {
        let hover = self.hit_at(engine, viewport, x, y);
        let changed = hover != self.hover;
        self.hover = hover;
        changed
    }

    pub fn cursor(&self) -> CursorIcon {
        match self.hover {
            Some(_) => crate::cursor::PRESS,
            None => crate::cursor::IDLE,
        }
    }

    // ------------------------------------------------------------- paint

    pub fn draw(
        &self,
        scene: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        viewport: (f32, f32),
    ) {
        let p = theme::p();
        scene::fill(
            scene,
            root,
            p.scrim,
            &scene::rect(0.0, 0.0, viewport.0, viewport.1),
        );
        let panel = self.panel(engine, viewport);
        // Held to the end of `draw`: the title, the description and the
        // buttons are the panel's contents, and nest under it in the inspector.
        let _panel = scene::floating_panel_scope(
            scene,
            root,
            panel.x,
            panel.y,
            panel.w,
            panel.h,
            &scene::Panel::popover(RADIUS),
        );
        let tx = panel.x + PAD;
        let ty = panel.y + PAD;
        let tw = Self::text_w(panel.w);
        let title = engine.simple_layout_wrapped(&self.title, TITLE, tw);
        {
            let _part = crate::typography::part(
                "Dialog title",
                root,
                Rect::new(tx, ty, tw, title.height()),
                TITLE,
                "theme::role::section_heading()",
            );
            scene::draw_layout(scene, root, &title, tx, ty, Some(p.text));
        }
        let body = engine.simple_layout_wrapped(&self.body, BODY, tw);
        {
            let _part = crate::typography::part(
                "Dialog description",
                root,
                Rect::new(tx, ty + title.height() + 8.0, tw, body.height()),
                BODY,
                "theme::role::row()",
            );
            scene::draw_layout(
                scene,
                root,
                &body,
                tx,
                ty + title.height() + 8.0,
                Some(p.text_dim),
            );
        }
        for (i, r) in self.targets(engine, viewport).into_iter().enumerate() {
            let role = if self.default == Some(i) {
                ButtonRole::Primary
            } else {
                ButtonRole::Secondary
            };
            button::draw_button(
                scene,
                root,
                engine,
                r,
                &self.labels[i],
                role,
                self.hover == Some(i),
                self.focus == Some(i),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn three() -> Choice {
        Choice::new("Close support?", "What should happen to the chat?")
            .answer("End chat")
            .answer("Keep chat with a floating icon")
            .answer("Close and tell me when support replies")
            .defaulting_to(1)
    }

    #[test]
    fn enter_presses_the_default_and_escape_dismisses() {
        let mut c = three();
        assert_eq!(
            c.key(&Key::Named(NamedKey::Enter)),
            ChoiceOutcome::Chosen(1)
        );
        assert_eq!(c.key(&Key::Named(NamedKey::Escape)), ChoiceOutcome::Dismiss);
    }

    #[test]
    fn tab_moves_the_focus_and_wraps() {
        let mut c = three();
        assert_eq!(c.key(&Key::Named(NamedKey::Tab)), ChoiceOutcome::Keep);
        assert_eq!(
            c.key(&Key::Named(NamedKey::Enter)),
            ChoiceOutcome::Chosen(2)
        );
        c.key(&Key::Named(NamedKey::Tab));
        assert_eq!(
            c.key(&Key::Named(NamedKey::Enter)),
            ChoiceOutcome::Chosen(0)
        );
        c.key(&Key::Named(NamedKey::ArrowUp));
        assert_eq!(
            c.key(&Key::Named(NamedKey::Enter)),
            ChoiceOutcome::Chosen(2)
        );
    }

    #[test]
    fn the_answers_stack_full_width_in_order() {
        let mut engine = TextEngine::new();
        let c = three();
        let viewport = (440.0, 720.0);
        let panel = c.panel(&mut engine, viewport);
        let targets = c.targets(&mut engine, viewport);
        assert_eq!(targets.len(), 3);
        for (a, b) in targets.iter().zip(targets.iter().skip(1)) {
            assert!(b.y > a.y + a.h, "stacked, not in a row");
            assert_eq!(a.x, b.x);
            assert_eq!(a.w, b.w);
        }
        assert!(
            targets
                .iter()
                .all(|r| r.x >= panel.x && r.x + r.w <= panel.x + panel.w)
        );
        assert!(panel.w <= 440.0 - EDGE * 2.0);
    }

    #[test]
    fn a_press_outside_the_panel_dismisses_and_inside_keeps() {
        let mut engine = TextEngine::new();
        let mut c = three();
        let viewport = (440.0, 720.0);
        assert_eq!(
            c.mouse_down(&mut engine, viewport, 1.0, 1.0),
            ChoiceOutcome::Dismiss
        );
        let panel = c.panel(&mut engine, viewport);
        assert_eq!(
            c.mouse_down(&mut engine, viewport, panel.x + 2.0, panel.y + 2.0),
            ChoiceOutcome::Keep
        );
        let second = c.targets(&mut engine, viewport)[1];
        assert_eq!(
            c.mouse_down(&mut engine, viewport, second.x + 1.0, second.y + 1.0),
            ChoiceOutcome::Chosen(1)
        );
    }
}
