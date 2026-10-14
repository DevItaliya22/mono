// SPDX-License-Identifier: MPL-2.0

//! A confirmation surface: a title, what happened, and the actions that answer it.
//!
//! The prompt is the simplest floating surface that is still a component: one
//! measured flow (title, wrapped body sentences, a right-aligned action row)
//! whose panel, hit rectangles and paint all come from the same arithmetic, so
//! a button is never clickable somewhere it is not drawn. It is generic over
//! the host's action type — the same rule as `icon_button::IconButton<Action>`:
//! drawing stays independent from the application that gives a press its
//! meaning. A prompt never scrolls; its panel clamps to the viewport instead.

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{inspect, scene, theme};
use vello::Scene;
use vello::kurbo::Affine;

use crate::button::{ActionButton, ActionStyle};

/// One answer the prompt offers, with the host's meaning attached.
pub struct KoruPromptAction<A> {
    pub action: A,
    pub label: String,
    pub style: ActionStyle,
}

/// What it is, what happened, what to do next — measured and painted here,
/// answered by the host through its own `A`.
pub struct KoruPrompt<A> {
    pub title: String,
    pub body: Vec<String>,
    pub actions: Vec<KoruPromptAction<A>>,
}

impl<A: PartialEq> KoruPrompt<A> {
    /// The delegation modal's width; a prompt is narrow enough to read as an
    /// answer, not a workspace.
    pub const WIDTH: f32 = 480.0;
    pub const PAD: f32 = 16.0;
    pub const GAP: f32 = 10.0;
    pub const RADIUS: f32 = 10.0;

    pub const fn new(title: String, body: Vec<String>, actions: Vec<KoruPromptAction<A>>) -> Self {
        Self {
            title,
            body,
            actions,
        }
    }

    /// The panel, centred, sized to what it holds and clamped to the viewport.
    pub fn panel(&self, engine: &mut TextEngine, viewport: (f32, f32)) -> Rect {
        let w = Self::WIDTH.min((viewport.0 - 16.0).max(1.0));
        let h = self
            .content_height(engine, w)
            .min((viewport.1 - 16.0).max(1.0));
        Rect::new(
            ((viewport.0 - w) * 0.5).max(8.0),
            ((viewport.1 - h) * 0.5).max(8.0),
            w,
            h,
        )
    }

    /// The height the flow takes at a panel width, padding included.
    fn content_height(&self, engine: &mut TextEngine, width: f32) -> f32 {
        let inner = (width - Self::PAD * 2.0).max(1.0);
        let mut used = Self::PAD + theme::role::ui().metrics.line_height + Self::GAP;
        for body in &self.body {
            used += engine
                .simple_layout_wrapped(body, theme::role::hint(), inner)
                .height()
                + 4.0;
        }
        used + Self::GAP + ActionButton::H + Self::PAD
    }

    /// Every action's rectangle, in action order. The first action owns the
    /// right-hand corner and the rest walk left, so the answer that ends the
    /// dialog keeps the corner however wide the others turn out to be — the
    /// same recipe `button::row_from_right` states for dialogs.
    pub fn action_rects(&self, engine: &mut TextEngine, panel: Rect) -> Vec<Rect> {
        let inner = (panel.w - Self::PAD * 2.0).max(1.0);
        let mut y = panel.y + Self::PAD + theme::role::ui().metrics.line_height + Self::GAP;
        for body in &self.body {
            y += engine
                .simple_layout_wrapped(body, theme::role::hint(), inner)
                .height()
                + 4.0;
        }
        y += Self::GAP;
        let mut right = panel.x + panel.w - Self::PAD;
        let mut rects = Vec::with_capacity(self.actions.len());
        for action in &self.actions {
            let w = ActionButton::new(&action.label, action.style, None).width(engine);
            right -= w;
            rects.push(Rect::new(right, y, w, ActionButton::H));
            right -= ActionButton::GAP;
        }
        rects
    }

    /// Paint the panel, its text and one [`ActionButton`] per action. The
    /// panel is a popover-elevation floating panel, per the chrome rule that
    /// every floating surface goes through `scene::floating_panel`.
    #[track_caller]
    pub fn draw(
        &self,
        canvas: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        panel: Rect,
        hovered: Option<&A>,
        focused: Option<&A>,
    ) {
        let p = theme::p();
        let _prompt = inspect::scope(
            "KoruPrompt",
            "component",
            root,
            scene::rect(panel.x, panel.y, panel.w, panel.h),
            |r| {
                r.property("implementation", "koru_ui::prompt", "shared implementation");
                r.property(
                    "typography role",
                    "title theme::role::ui(); body theme::role::hint()",
                    "shared prompt recipe",
                );
                r.property("actions", self.actions.len(), "caller action list");
                r.property("docs", "docs/features/koruui/COMPONENTS.md", "catalog");
            },
        );
        scene::floating_panel(
            canvas,
            root,
            panel.x,
            panel.y,
            panel.w,
            panel.h,
            &scene::Panel::popover(Self::RADIUS),
        );
        let inner = (panel.w - Self::PAD * 2.0).max(1.0);
        let x = panel.x + Self::PAD;
        let mut y = panel.y + Self::PAD;
        let title = engine.simple_layout(&self.title, theme::role::ui());
        crate::typography::line(
            "Prompt title",
            canvas,
            root,
            &title,
            x,
            y,
            theme::role::ui(),
            "theme::role::ui()",
            p.text,
        );
        y += theme::role::ui().metrics.line_height + Self::GAP;
        for body in &self.body {
            let layout = engine.simple_layout_wrapped(body, theme::role::hint(), inner);
            scene::draw_layout(canvas, root, &layout, x, y, Some(p.text_dim));
            y += layout.height() + 4.0;
        }
        let _ = y;
        for (action, rect) in self.actions.iter().zip(self.action_rects(engine, panel)) {
            ActionButton::new(&action.label, action.style, None).draw_focused(
                canvas,
                root,
                engine,
                rect,
                hovered == Some(&action.action),
                focused == Some(&action.action),
            );
        }
    }
}

#[cfg(test)]
mod tests;
