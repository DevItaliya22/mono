// SPDX-License-Identifier: MPL-2.0

//! Name a document — reached from `/document`, and from double-clicking a
//! document's title to rename it.
//!
//! One field. What happens on Enter differs between those two uses, but that
//! difference lives on [`Anchor`](crate::trigger::Anchor), not here: this
//! prompt's whole job is to hand back a name.

use winit::keyboard::{Key, ModifiersState, NamedKey};

use super::text_field::TextField;
use super::{Outcome, text_field};
use crate::trigger::{Anchor, PillSpec, Trigger};
use koru_draw::geometry::Rect;
use koru_draw::pill::PillKind;
use koru_draw::text::TextEngine;
use koru_draw::theme;

pub struct NamePrompt {
    pub trigger: Trigger,
    pub input: TextField,
    /// Current replicated page icon. Only `Rename` prompts expose its button;
    /// other name prompts leave this unused.
    pub page_icon: Option<String>,
}

/// Where the field's box sits inside the panel, in panel-local coordinates.
///
/// One function, shared by drawing and hit-testing, so the field is clickable
/// exactly where it is drawn.
pub fn field_rect(pw: f32) -> Rect {
    Rect::new(
        theme::PANEL_PAD + 2.0,
        theme::PANEL_PAD + 20.0,
        pw - theme::PANEL_PAD * 2.0 - 4.0,
        26.0,
    )
}

const ICON_GAP: f32 = 6.0;
const ICON_W: f32 = 30.0;

pub fn icon_rect() -> Rect {
    Rect::new(
        theme::PANEL_PAD + 2.0,
        theme::PANEL_PAD + 20.0,
        ICON_W,
        26.0,
    )
}

impl NamePrompt {
    pub fn size() -> (f32, f32) {
        (theme::PANEL_W, theme::PANEL_PAD * 2.0 + 20.0 + 30.0)
    }

    pub fn can_set_page_icon(&self) -> bool {
        matches!(self.trigger.anchor, Anchor::Rename(_))
    }

    pub fn field_rect(&self, pw: f32) -> Rect {
        let mut rect = field_rect(pw);
        if self.can_set_page_icon() {
            let shift = ICON_W + ICON_GAP;
            rect.x += shift;
            rect.w -= shift;
        }
        rect
    }

    pub fn page_icon_at(&self, local_x: f32, local_y: f32) -> Option<koru_core::NodeId> {
        match self.trigger.anchor {
            Anchor::Rename(node) if icon_rect().contains(local_x, local_y) => Some(node),
            _ => None,
        }
    }

    /// A panel-local x as an x in the field's own text.
    fn tx(&self, local_x: f32, pw: f32) -> f32 {
        local_x - self.field_rect(pw).x - text_field::PAD + self.input.scroll()
    }

    /// A click in the field puts the caret where it landed. The panel has only
    /// this one field, so there is no focus to move — just a position to take.
    pub fn click(&mut self, engine: &mut TextEngine, local_x: f32, local_y: f32, pw: f32) -> bool {
        if !self.field_rect(pw).contains(local_x, local_y) {
            return false;
        }
        let tx = self.tx(local_x, pw);
        self.input.click(engine, tx);
        true
    }

    /// Carry the selection to a panel-local point. No hit test: a drag that has
    /// left the box is still this field's.
    pub fn drag(&mut self, engine: &mut TextEngine, local_x: f32, pw: f32) {
        let tx = self.tx(local_x, pw);
        self.input.drag_to(engine, tx);
    }

    pub fn double_click(&mut self, engine: &mut TextEngine, local_x: f32, local_y: f32, pw: f32) {
        if !self.field_rect(pw).contains(local_x, local_y) {
            return;
        }
        let tx = self.tx(local_x, pw);
        self.input.select_word_at(engine, tx);
    }

    pub fn key(
        &mut self,
        engine: &mut TextEngine,
        key: &Key,
        text: Option<&str>,
        mods: ModifiersState,
    ) -> Outcome {
        if matches!(key, Key::Named(NamedKey::Enter)) {
            let label = self.input.value();
            // An empty name is not a refusal, it is an unfinished one: keep the
            // prompt open rather than closing on a keystroke the user meant as
            // "done".
            return if label.is_empty() {
                Outcome::Consumed
            } else {
                Outcome::Commit(PillSpec::new(PillKind::Document, label))
            };
        }
        self.input.key(engine, key, text, mods);
        Outcome::Consumed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt(text: &str) -> (NamePrompt, TextEngine) {
        let mut engine = TextEngine::new();
        let p = NamePrompt {
            trigger: Trigger::at_start(),
            // `with_text` leaves the caret at the end, which is where typing
            // would actually happen.
            input: TextField::with_text(&mut engine, theme::role::body(), text),
            page_icon: None,
        };
        (p, engine)
    }

    #[test]
    fn enter_commits_the_trimmed_name() {
        let (mut p, mut e) = prompt("  spec.md ");
        match p.key(
            &mut e,
            &Key::Named(NamedKey::Enter),
            None,
            ModifiersState::empty(),
        ) {
            Outcome::Commit(spec) => {
                assert_eq!(spec.label, "spec.md");
                assert_eq!(spec.kind, PillKind::Document);
            }
            other => panic!("expected a commit, got {other:?}"),
        }
    }

    /// Enter on an empty field keeps the prompt up. Closing would throw away
    /// what the user was about to type.
    #[test]
    fn enter_on_an_empty_name_keeps_the_prompt_open() {
        let (mut p, mut e) = prompt("   ");
        assert!(matches!(
            p.key(
                &mut e,
                &Key::Named(NamedKey::Enter),
                None,
                ModifiersState::empty()
            ),
            Outcome::Consumed
        ));
    }

    #[test]
    fn typing_reaches_the_field() {
        let (mut p, mut e) = prompt("spec");
        p.key(
            &mut e,
            &Key::Character("s".into()),
            Some("s"),
            ModifiersState::empty(),
        );
        assert_eq!(p.input.value(), "specs");
    }

    #[test]
    fn document_rename_exposes_a_separate_page_icon_button() {
        let (mut p, _) = prompt("spec");
        let node = koru_core::NodeId::new();
        p.trigger.node = node;
        p.trigger.anchor = Anchor::Rename(node);
        let icon = icon_rect();
        let field = p.field_rect(theme::PANEL_W);
        assert!(p.can_set_page_icon());
        assert_eq!(p.page_icon_at(icon.centre().0, icon.centre().1), Some(node));
        assert!(
            icon.x + icon.w < field.x,
            "the icon does not cover the name"
        );
    }
}
