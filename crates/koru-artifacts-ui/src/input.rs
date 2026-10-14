// SPDX-License-Identifier: MPL-2.0

//! What a press and a key do.
//!
//! **The column performs almost everything it offers.** Expanding a family,
//! selecting a version, revealing more history and registering a file are
//! this crate's own to do through its own gateway handle, so a press answers
//! `true` (something changed, repaint) rather than an outcome the container
//! has to route. The one exception is *Open*: a document in Koru opens as a
//! tab, and only the window can make one — so that press leaves a request
//! behind for the host to drain (ADR-369), the way the tasks rail's does.
//!
//! **Only the arrows, Enter, Space and Escape are claimed.** Everything
//! else falls through, so a focused column never makes the window feel
//! stuck — the explore tree's own rule, kept here because it is the same
//! column.

use koru_draw::geometry::Rect;

use crate::model::Registering;
use crate::rows::Row;
use crate::{ArtifactsSurface, Press};

impl ArtifactsSurface {
    /// A press. `true` when something changed and a repaint is owed.
    ///
    /// A press on the column's background is still ours: it must not fall
    /// through to the view beside the column.
    pub fn mouse_down(&mut self, area: Rect, top: f32, x: f32, y: f32) -> bool {
        let Some((row, rect)) = self.row_at(area, top, x, y) else {
            return self.take_focus(None);
        };
        if !row.pressable() {
            return self.take_focus(None);
        }
        let parts = self.row_parts(row, rect);
        self.focus = Some(row);
        match row {
            // The family row is the latest artifact. Its small caret alone
            // reveals history; the rest opens the artifact.
            Row::Family(index) if parts.caret.contains(x, y) && parts.caret.w > 0.0 => {
                if let Some(id) = self.family_id(index) {
                    self.toggle_family(id);
                }
            }
            // The trailing lane is *Register*; the rest of the row is not.
            // **The word is the button.** *Register* sits in the trailing
            // lane — the fact at the left, the thing to do about it at the
            // right (`DESIGN.md`) — and pressing the rest of the row puts
            // the keyboard on it without adopting anything by accident.
            Row::File { .. } if !parts.trailing.contains(x, y) => {}
            _ => self.activate(row),
        }
        true
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

    /// The pointer has left the column.
    pub fn clear_hover(&mut self) -> bool {
        self.hover.take().is_some()
    }

    pub fn hover(&self) -> Option<Row> {
        self.hover
    }

    pub fn focus(&self) -> Option<Row> {
        self.focus
    }

    /// Whether the column is holding the keyboard.
    pub fn has_keyboard(&self) -> bool {
        self.focus.is_some()
    }

    /// Put the keyboard on the first row that answers to it — what the
    /// container calls when the column is shown by its chord.
    pub fn focus_first(&mut self, area: Rect, top: f32) -> bool {
        if self.focus.is_some() {
            return false;
        }
        let first = self.pressable_rows(area, top).first().copied();
        self.focus = first;
        first.is_some()
    }

    fn take_focus(&mut self, row: Option<Row>) -> bool {
        let changed = self.focus != row;
        self.focus = row;
        changed
    }

    /// A key. `true` when the column answered it; `false` means it falls
    /// through to whatever is behind.
    pub fn key(&mut self, area: Rect, top: f32, press: Press) -> bool {
        if self.focus.is_none() && press != Press::Down && press != Press::Up {
            return false;
        }
        let rows = self.pressable_rows(area, top);
        if rows.is_empty() {
            if press == Press::Escape {
                return self.take_focus(None);
            }
            return false;
        }
        let at = self
            .focus
            .and_then(|focus| rows.iter().position(|row| *row == focus));
        match press {
            Press::Escape => {
                // The keyboard goes back where it came from. A list that
                // keeps the arrows after you have left it is a window that
                // has quietly stopped answering the keys you are pressing.
                self.take_focus(None)
            }
            Press::Down => {
                let next = at.map(|at| (at + 1).min(rows.len() - 1)).unwrap_or(0);
                self.take_focus(Some(rows[next]))
            }
            Press::Up => {
                let next = at.map(|at| at.saturating_sub(1)).unwrap_or(0);
                self.take_focus(Some(rows[next]))
            }
            Press::Right => match self.focus {
                // Right opens a shut family, and steps into an open one.
                Some(Row::Family(index)) => {
                    let Some(id) = self.family_id(index) else {
                        return false;
                    };
                    if self
                        .catalog
                        .families
                        .get(index)
                        .is_none_or(|entry| entry.versions.len() <= 1)
                    {
                        return false;
                    }
                    if self.is_expanded(id) {
                        let step = at.map(|at| (at + 1).min(rows.len() - 1)).unwrap_or(0);
                        self.take_focus(Some(rows[step]))
                    } else {
                        self.toggle_family(id);
                        true
                    }
                }
                _ => false,
            },
            Press::Left => match self.focus {
                Some(Row::Family(index)) => {
                    let Some(id) = self.family_id(index) else {
                        return false;
                    };
                    if self.is_expanded(id) {
                        self.toggle_family(id);
                        true
                    } else {
                        false
                    }
                }
                // From inside a family, fold it and land on it.
                Some(Row::Version { family, .. }) | Some(Row::Earlier(family)) => {
                    let Some(id) = self.family_id(family) else {
                        return false;
                    };
                    if self.is_expanded(id) {
                        self.toggle_family(id);
                    }
                    self.focus = Some(Row::Family(family));
                    true
                }
                _ => false,
            },
            Press::Enter | Press::Space => match self.focus {
                Some(row) => {
                    self.activate(row);
                    true
                }
                None => false,
            },
        }
    }

    /// What a row does when it is chosen, however it was chosen — so the
    /// keyboard and the pointer can never mean two different things.
    fn activate(&mut self, row: Row) {
        match row {
            Row::Family(index) => {
                self.ask_to_open(index, 0);
            }
            Row::Earlier(index) => {
                if let Some(id) = self.family_id(index) {
                    self.reveal_more(id);
                }
            }
            Row::Version { family, version } => {
                if let Some(id) = self
                    .catalog
                    .families
                    .get(family)
                    .and_then(|entry| entry.version(version))
                    .map(|version| version.id)
                {
                    self.select_version(id);
                }
            }
            Row::Open { family, version } => self.ask_to_open(family, version),
            Row::File { run, file } => {
                let Some(entry) = self.catalog.runs.get(run) else {
                    return;
                };
                let Some(found) = entry.files.get(file) else {
                    return;
                };
                let (run, path) = (entry.run, found.path.clone());
                // A refusal is cleared by pressing again: the person read
                // it, fixed what it named, and is asking a second time.
                if matches!(
                    self.registering.get(&(run, path.clone())),
                    Some(Registering::Refused(_))
                ) {
                    self.registering.remove(&(run, path.clone()));
                }
                self.register(run, path);
            }
            _ => {}
        }
    }

    pub(crate) fn family_id(&self, index: usize) -> Option<koru_artifact_wire::ArtifactFamilyId> {
        self.catalog
            .families
            .get(index)
            .map(|entry| entry.family.id)
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
