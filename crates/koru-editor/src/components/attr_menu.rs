// SPDX-License-Identifier: MPL-2.0

//! The `/attr` chooser: which attribute to set on this block.
//!
//! D7's engine surface — `/tag` and `/color` are sugar that jump straight to
//! the pickers this menu's rows become. Two rows in R0 (Tags, Color); A3/A4
//! add Due and Priority rows here, which is the whole reason the browsing
//! step exists as its own panel rather than as more slash commands.
//!
//! Flat and fieldless on the [`LanguagePicker`] template: arrow keys, Enter,
//! click. Choosing a row `Become`s the specific picker, exactly the way the
//! command menu's own rows do.
//!
//! [`LanguagePicker`]: super::language_picker::LanguagePicker

use winit::keyboard::NamedKey;

use super::Outcome;
use super::command_menu::Command;
use crate::trigger::Trigger;
use koru_draw::icons;
use koru_draw::text::TextEngine;
use koru_draw::theme;

/// The rows, in menu order: label, the command the row becomes, its glyph.
/// A2's two first, then A3/A4's — appended rather than re-sorted, so the
/// muscle memory of the first two rows survived the menu growing.
pub const ROWS: [(&str, Command, char); 4] = [
    ("Tags", Command::Tag, icons::TAG),
    ("Color", Command::Color, icons::PALETTE),
    ("Due", Command::Due, icons::DATE),
    ("Priority", Command::Priority, icons::FLAG),
];

/// The rows this build shows: [`ROWS`] minus any whose command belongs to a
/// surface the product hides ([`super::command_menu::product_shows`]) —
/// Priority, when tasks are off. Due stays: it creates a task through the
/// same door either way.
pub fn rows() -> Vec<(&'static str, Command, char)> {
    ROWS.iter()
        .copied()
        .filter(|(_, command, _)| super::command_menu::product_shows(*command))
        .collect()
}

pub struct AttrMenu {
    pub trigger: Trigger,
    pub sel: usize,
    /// Measured from the labels, like the language dropdown; floored by
    /// `panel_size`.
    pub width: f32,
}

impl AttrMenu {
    pub fn open(trigger: Trigger) -> Self {
        Self {
            trigger,
            sel: 0,
            width: 0.0,
        }
    }

    /// Wide enough for the widest row plus its icon column and padding.
    pub fn measure_width(engine: &mut TextEngine) -> f32 {
        let widest = rows()
            .iter()
            .map(|(label, _, _)| engine.simple_layout(label, theme::role::ui()).width())
            .fold(0.0_f32, f32::max);
        widest + 52.0
    }

    pub fn height(&self) -> f32 {
        theme::PANEL_PAD * 2.0 + rows().len() as f32 * theme::ROW_H
    }

    pub fn key(&mut self, named: Option<NamedKey>) -> Outcome {
        match named {
            Some(NamedKey::ArrowDown) => {
                self.sel = (self.sel + 1) % rows().len();
                Outcome::Consumed
            }
            Some(NamedKey::ArrowUp) => {
                let len = rows().len();
                self.sel = (self.sel + len - 1) % len;
                Outcome::Consumed
            }
            Some(NamedKey::Enter) | Some(NamedKey::Tab) => self.choose(self.sel),
            // Swallow anything else: the chooser has no field, and a character
            // that fell through would land in the document behind the panel.
            _ => Outcome::Consumed,
        }
    }

    pub fn click(&self, local_y: f32) -> Outcome {
        let row = (local_y / theme::ROW_H).floor();
        if row < 0.0 {
            return Outcome::Consumed;
        }
        self.choose(row as usize)
    }

    fn choose(&self, i: usize) -> Outcome {
        match rows().get(i) {
            Some((_, cmd, _)) => Outcome::Become(*cmd),
            None => Outcome::Close,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_become_their_pickers() {
        let m = AttrMenu::open(Trigger::at_start());
        assert!(matches!(m.choose(0), Outcome::Become(Command::Tag)));
        assert!(matches!(m.choose(1), Outcome::Become(Command::Color)));
        assert!(matches!(m.choose(2), Outcome::Become(Command::Due)));
        assert!(matches!(m.choose(3), Outcome::Become(Command::Priority)));
        assert!(matches!(m.choose(9), Outcome::Close));
    }

    #[test]
    fn arrows_wrap_and_characters_are_swallowed() {
        let mut m = AttrMenu::open(Trigger::at_start());
        m.key(Some(NamedKey::ArrowUp));
        assert_eq!(m.sel, rows().len() - 1, "wraps up to the last row");
        m.key(Some(NamedKey::ArrowDown));
        assert_eq!(m.sel, 0);
        // A printable key must not fall through into the document.
        assert!(matches!(m.key(None), Outcome::Consumed));
    }

    /// With tasks hidden, Priority is not a row and Due still is.
    #[test]
    fn priority_is_hidden_with_tasks_and_due_stays() {
        koru_features::testing::with(koru_features::Features::ALL_OFF, || {
            let labels: Vec<&str> = rows().iter().map(|(label, _, _)| *label).collect();
            assert_eq!(labels, ["Tags", "Color", "Due"]);
            let m = AttrMenu::open(Trigger::at_start());
            assert!(matches!(m.choose(2), Outcome::Become(Command::Due)));
            assert!(matches!(m.choose(3), Outcome::Close));
        });
    }
}
