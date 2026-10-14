// SPDX-License-Identifier: MPL-2.0

//! A row of icon-only buttons composed as one control cluster.
//!
//! The cluster is the component the window caption's three direct controls
//! are: each button keeps its own glyph, slot, tone and typed action; the
//! group owns their row geometry and draws them under one boundary, so the
//! inspector shows the cluster and its buttons rather than three loose
//! glyphs. Drawing stays independent of what a click means — the group's
//! `Action` is the host's vocabulary, exactly as in [`IconButton`].

use koru_draw::geometry::Rect;
use koru_draw::text::TextEngine;
use koru_draw::{inspect, scene};
use vello::Scene;
use vello::kurbo::Affine;

use crate::icon_button::IconButton;

/// Equal-width slots for a right-aligned row of `count` buttons, leftmost
/// first — the caption's `at(n)` pattern as a shared answer, so a host cannot
/// re-derive it with the buttons in the wrong corners.
pub fn slots_right(area: Rect, width: f32, count: usize) -> Vec<Rect> {
    let right = area.x + area.w;
    (1..=count)
        .rev()
        .map(|n| Rect::new(right - width * n as f32, area.y, width, area.h))
        .collect()
}

/// A row of [`IconButton`]s with one typed click result.
///
/// Construct it from fully-formed buttons — the group does not re-theme or
/// re-place them, it composes them:
///
/// ```ignore
/// let group = ButtonGroup::new(vec![
///     IconButton::new(icons::WINDOW_MINIMISE, slots[0]).on_click(Action::Minimise),
///     IconButton::new(icons::SQUARE, slots[1]).on_click(Action::ToggleMaximise),
///     IconButton::new(icons::WINDOW_CLOSE, slots[2])
///         .on_click(Action::Close)
///         .tone(Tone::Danger),
/// ]);
/// ```
pub struct ButtonGroup<Action = ()> {
    buttons: Vec<IconButton<Action>>,
}

impl<Action> Default for ButtonGroup<Action> {
    fn default() -> Self {
        Self {
            buttons: Vec::new(),
        }
    }
}

impl<Action> ButtonGroup<Action> {
    pub fn new(buttons: impl Into<Vec<IconButton<Action>>>) -> Self {
        Self {
            buttons: buttons.into(),
        }
    }

    pub fn buttons(&self) -> &[IconButton<Action>] {
        &self.buttons
    }

    /// The smallest rectangle covering every slot — the group's boundary for
    /// hit answers and inspection. `None` for an empty group.
    pub fn bounds(&self) -> Option<Rect> {
        let mut iter = self.buttons.iter().map(IconButton::rect);
        let first = iter.next()?;
        Some(iter.fold(first, |acc, rect| acc.union(rect)))
    }

    /// The action of the enabled button under (`x`, `y`), if any.
    pub fn action_at(&self, x: f32, y: f32) -> Option<Action>
    where
        Action: Copy,
    {
        self.buttons
            .iter()
            .find_map(|button| button.action_at(x, y))
    }

    #[track_caller]
    pub fn draw(&self, canvas: &mut Scene, root: Affine, engine: &mut TextEngine) {
        let Some(bounds) = self.bounds() else {
            return;
        };
        let _group = inspect::scope(
            "KoruButtonGroup",
            "component",
            root,
            scene::rect(bounds.x, bounds.y, bounds.w, bounds.h),
            |r| {
                r.property(
                    "implementation",
                    "koru_ui::button_group",
                    "shared implementation",
                );
                r.property("buttons", self.buttons.len(), "composed IconButtons");
                r.property("docs", "docs/features/koruui/COMPONENTS.md", "catalog");
            },
        );
        for button in &self.buttons {
            button.draw(canvas, root, engine);
        }
    }
}

#[cfg(test)]
mod tests;
