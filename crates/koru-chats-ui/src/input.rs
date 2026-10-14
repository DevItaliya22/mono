// SPDX-License-Identifier: MPL-2.0

//! What a press and a key do — **the interaction contract both hosts share**
//! (`docs/features/koruui/CONTRIBUTING.md`, *Chats*):
//!
//! - focus enters on open ([`focus_first`](ChatsSurface::focus_first)) and
//!   returns on close — Escape gives the keyboard back;
//! - Up/Down move the selection a row, Home/End to either end;
//! - Enter (or Space) opens; Delete asks;
//! - a row chosen by the pointer and one chosen by the keyboard mean the same
//!   thing, because both reach [`activate`](ChatsSurface::activate).
//!
//! **Only those keys are claimed.** Everything else falls through, so a
//! focused list never makes the window feel stuck.

use koru_draw::geometry::Rect;

use crate::rows::Row;
use crate::{ChatsSurface, Focus, Press};

impl ChatsSurface {
    /// A press. `true` when something changed and a repaint is owed.
    ///
    /// A press on the list's background is still ours: it must not fall
    /// through to whatever is beside or behind the list.
    pub fn mouse_down(&mut self, area: Rect, top: f32, x: f32, y: f32) -> bool {
        match self.row_at(area, top, x, y) {
            Some((row, _)) if row.pressable() => {
                self.activate(row);
                true
            }
            _ => true,
        }
    }

    /// A right-click. Puts the keyboard on the chat under the pointer and
    /// names it, so the host can raise its menu about that chat. `None` off a
    /// chat row.
    pub fn context_menu(&mut self, area: Rect, top: f32, x: f32, y: f32) -> Option<crate::ChatId> {
        let Some((Row::Chat(index), _)) = self.row_at(area, top, x, y) else {
            return None;
        };
        let chat = self.chats.get(index)?.id;
        self.focus = Some(Focus::Chat(chat));
        Some(chat)
    }

    /// A pointer move. `true` when the hover changed.
    pub fn mouse_move(&mut self, area: Rect, top: f32, x: f32, y: f32) -> bool {
        let hover = self
            .row_at(area, top, x, y)
            .map(|(row, _)| row)
            .filter(Row::pressable);
        let moved = self.hover != hover;
        self.hover = hover;
        moved
    }

    /// The pointer has left the list.
    pub fn clear_hover(&mut self) -> bool {
        self.hover.take().is_some()
    }

    pub fn hover(&self) -> Option<Row> {
        self.hover
    }

    pub fn focus(&self) -> Option<Focus> {
        self.focus
    }

    /// The chat the keyboard is on, if it is on one.
    pub fn focused_chat(&self) -> Option<crate::ChatId> {
        match self.focus? {
            Focus::Chat(chat) => Some(chat),
            Focus::New => None,
        }
    }

    /// Whether the surface holds the keyboard.
    pub fn has_keyboard(&self) -> bool {
        self.focus.is_some()
    }

    /// Give the keyboard back. `true` when the surface had it.
    pub fn clear_focus(&mut self) -> bool {
        self.focus.take().is_some()
    }

    /// Put the keyboard on one row — an accessibility client's *Focus*.
    pub fn set_focus(&mut self, focus: Focus, area: Rect, top: f32) -> bool {
        self.focus = Some(focus);
        if let Some(row) = self.focus_row() {
            self.scroll_into_view(row, area, top);
            return true;
        }
        false
    }

    /// **Focus enters on open.** The chat the host has open when the list
    /// holds it — so opening the list from a chat starts you on that chat —
    /// else the newest chat, else *New chat*. Nothing changes when the
    /// keyboard is already here.
    pub fn focus_first(&mut self, area: Rect, top: f32) -> bool {
        if self.focus.is_some() {
            return false;
        }
        // Shown before the list has answered: land on the open chat when it
        // arrives rather than leave the keyboard on *New chat* for good.
        if let Some(current) = self.current
            && !self.chats.iter().any(|chat| chat.id == current)
            && self.load != crate::Load::Ready
        {
            self.pending_reveal = Some(current);
        }
        let landing = self
            .current
            .filter(|current| self.chats.iter().any(|chat| chat.id == *current))
            .or_else(|| self.chats.first().map(|chat| chat.id))
            .map_or(Focus::New, Focus::Chat);
        self.set_focus(landing, area, top);
        true
    }

    /// A key. `true` when the surface answered it; `false` falls through.
    pub fn key(&mut self, area: Rect, top: f32, press: Press) -> bool {
        let moving = matches!(press, Press::Up | Press::Down | Press::Home | Press::End);
        if self.focus.is_none() && !moving {
            return false;
        }
        if press == Press::Escape {
            // The keyboard goes back where it came from. A list that keeps
            // the arrows after you have left it is a window that has quietly
            // stopped answering the keys you are pressing.
            return self.clear_focus();
        }
        let rows = self.pressable_rows(area, top);
        if rows.is_empty() {
            return false;
        }
        let at = self
            .focus_row()
            .and_then(|focus| rows.iter().position(|row| *row == focus));
        let last = rows.len() - 1;
        let next = match press {
            Press::Down => at.map_or(0, |at| (at + 1).min(last)),
            Press::Up => at.map_or(0, |at| at.saturating_sub(1)),
            Press::Home => 0,
            Press::End => last,
            Press::Enter | Press::Space => {
                return match self.focus_row() {
                    Some(row) => {
                        self.activate(row);
                        true
                    }
                    None => false,
                };
            }
            Press::Delete => {
                return match self.focus {
                    Some(Focus::Chat(chat)) => {
                        self.ask_delete(chat);
                        true
                    }
                    // *New chat* has nothing to delete; the key is not ours.
                    _ => false,
                };
            }
            Press::Escape => unreachable!("answered above"),
        };
        let row = rows[next];
        self.focus = self.focus_of(row);
        self.scroll_into_view(row, area, top);
        true
    }

    /// What a row does when it is chosen, however it was chosen.
    fn activate(&mut self, row: Row) {
        match row {
            Row::New => self.ask_new(),
            Row::Chat(index) => {
                if let Some(chat) = self.chats.get(index).map(|chat| chat.id) {
                    self.ask_open(chat);
                }
            }
            Row::Notice(_) | Row::Note(_) => {}
        }
    }

    fn focus_of(&self, row: Row) -> Option<Focus> {
        match row {
            Row::New => Some(Focus::New),
            Row::Chat(index) => self.chats.get(index).map(|chat| Focus::Chat(chat.id)),
            _ => self.focus,
        }
    }

    /// The rows the keyboard steps through, in order.
    fn pressable_rows(&mut self, area: Rect, top: f32) -> Vec<Row> {
        self.laid_out(area, top)
            .into_iter()
            .map(|(row, _)| row)
            .filter(Row::pressable)
            .collect()
    }
}
