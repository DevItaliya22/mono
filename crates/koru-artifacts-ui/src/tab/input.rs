// SPDX-License-Identifier: MPL-2.0

//! What a press and a key do on the result tab.
//!
//! **Every control is reachable from the keyboard** — one ordered list, the
//! same one the pointer hits, walked by Tab and the arrows, activated by
//! Enter or Space, with Escape letting go.
//!
//! **The page comes first while it holds the keyboard**, because a person
//! reading or typing a document expects the arrow keys to move the caret
//! rather than step between buttons. The page is the host's, so that
//! ordering is the host's to keep; what this file keeps is that a control
//! with the focus ring answers the keyboard before anything else, and that
//! Escape lets go.

use winit::keyboard::{Key as WinitKey, ModifiersState, NamedKey};

use super::{ArtifactTab, TabRow};

impl ArtifactTab {
    /// A press. `true` when the tab took it.
    pub fn mouse_down(&mut self, x: f32, y: f32) -> bool {
        let Some((row, _)) = self.row_at(x, y) else {
            return self.take_focus(None);
        };
        if !row.pressable() {
            return self.take_focus(None);
        }
        self.focus = Some(row);
        self.activate(row);
        true
    }

    /// The pointer let go. Nothing on this tab follows a press out of it —
    /// the chrome is a list of one-press rows, and the only thing that ever
    /// tracked a drag was the composer, which is gone (ADR-378).
    pub fn mouse_up(&mut self) -> bool {
        false
    }

    pub fn mouse_move(&mut self, x: f32, y: f32) -> bool {
        let hover = self
            .row_at(x, y)
            .map(|(row, _)| row)
            .filter(|row| row.pressable());
        let moved = self.hover != hover;
        self.hover = hover;
        moved
    }

    pub fn cursor_icon(&self) -> winit::window::CursorIcon {
        match self.hover {
            Some(_) => koru_ui::cursor::PRESS,
            None => koru_ui::cursor::IDLE,
        }
    }

    pub fn hover(&self) -> Option<TabRow> {
        self.hover
    }

    pub fn focus(&self) -> Option<TabRow> {
        self.focus
    }

    /// **Put the ring on the first control**, for a host whose page has the
    /// keyboard and wants to hand it back. `false` when there is no control
    /// to put it on.
    pub fn focus_first_control(&mut self) -> bool {
        let Some(first) = self.pressable_rows().first().copied() else {
            return false;
        };
        self.take_focus(Some(first));
        true
    }

    /// A key. `true` when the tab answered it; `false` means it falls through
    /// to whatever is behind.
    ///
    /// **No `text`**, unlike every other `key` in the app: this surface is a
    /// list of controls and there is nowhere on it to type. What was typed
    /// belongs to the page, which is the host's (ADR-378).
    pub fn key(&mut self, key: &WinitKey, mods: ModifiersState) -> bool {
        self.relayout();
        match key {
            WinitKey::Named(NamedKey::Tab) => {
                self.step_focus(!mods.shift_key());
                true
            }
            WinitKey::Named(NamedKey::ArrowRight) | WinitKey::Named(NamedKey::ArrowDown) => {
                self.step_focus(true);
                true
            }
            WinitKey::Named(NamedKey::ArrowLeft) | WinitKey::Named(NamedKey::ArrowUp) => {
                self.step_focus(false);
                true
            }
            WinitKey::Named(NamedKey::Enter) | WinitKey::Named(NamedKey::Space) => {
                let Some(row) = self.focus else {
                    return false;
                };
                self.activate(row);
                true
            }
            WinitKey::Named(NamedKey::Escape) if self.focus.is_some() => self.take_focus(None),
            _ => false,
        }
    }

    /// Let the chrome go, because the keyboard is somewhere else now — the
    /// host's document view, which a press put the caret into. Without it
    /// the focus ring would sit on *Edit* while the arrow keys moved a caret
    /// in the page below.
    pub fn clear_focus(&mut self) -> bool {
        self.take_focus(None)
    }

    fn take_focus(&mut self, row: Option<TabRow>) -> bool {
        let changed = self.focus != row;
        self.focus = row;
        if changed {
            self.stale = true;
        }
        changed
    }

    fn step_focus(&mut self, forward: bool) {
        let rows = self.pressable_rows();
        if rows.is_empty() {
            return;
        }
        let at = self
            .focus
            .and_then(|focus| rows.iter().position(|row| *row == focus));
        let next = match (at, forward) {
            (Some(at), true) => (at + 1) % rows.len(),
            (Some(at), false) => (at + rows.len() - 1) % rows.len(),
            (None, true) => 0,
            (None, false) => rows.len() - 1,
        };
        self.take_focus(Some(rows[next]));
    }

    fn pressable_rows(&mut self) -> Vec<TabRow> {
        self.relayout();
        self.laid
            .iter()
            .map(|(row, _)| *row)
            .filter(|row| row.pressable())
            .collect()
    }

    /// What a row does when it is chosen, however it was chosen — so the
    /// keyboard and the pointer can never mean two different things.
    fn activate(&mut self, row: TabRow) {
        match row {
            TabRow::Run => {
                self.wants_run = true;
                self.stale = true;
            }
            TabRow::Edit => self.begin_editing(),
            TabRow::StopEditing => self.stop_editing(),
            // **Not the save itself.** The bytes are the host's document and
            // turning one into Markdown is work proportional to it; doing
            // that inside a press would put it in the frame that answered
            // the click. The ask is recorded and the host settles it.
            TabRow::Save => {
                self.save_requested = true;
                self.stale = true;
            }
            _ => {}
        }
    }
}
