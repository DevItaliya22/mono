// SPDX-License-Identifier: MPL-2.0

//! The search window's `…` menu: what its rows are and what they mean.
//!
//! The geometry and drawing are the shared [`koru_ui::Menu`] widget — the
//! same one the container's context menus use, so this menu looks and steps
//! like every other in the app. What lives here is only the semantics: which
//! rows exist, what each is called, and the state each reports.

use std::borrow::Cow;

use koru_ui::MenuItem;

use crate::state::SearchView;

/// What a row in the `…` menu does.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum AppMenuAction {
    /// Toggles the shortcut column across the surface. The menu stays open,
    /// so the check is seen to move.
    KeyboardHints,
    HelpPreview,
    FeedbackPreview,
}

/// One row. `checked` is the toggle state drawn in the check column.
#[derive(Clone)]
pub struct AppMenuRow {
    pub action: AppMenuAction,
    pub checked: bool,
}

impl AppMenuRow {
    /// The rows, built from the surface's state as the menu opens.
    pub fn list(show_shortcuts: bool, view: SearchView) -> Vec<AppMenuRow> {
        vec![
            AppMenuRow {
                action: AppMenuAction::KeyboardHints,
                checked: show_shortcuts,
            },
            AppMenuRow {
                action: AppMenuAction::HelpPreview,
                checked: view == SearchView::HelpPreview,
            },
            AppMenuRow {
                action: AppMenuAction::FeedbackPreview,
                checked: view == SearchView::FeedbackPreview,
            },
        ]
    }
}

impl MenuItem for AppMenuRow {
    fn label(&self) -> Cow<'_, str> {
        match self.action {
            AppMenuAction::KeyboardHints => "Keyboard hints",
            AppMenuAction::HelpPreview => "Help preview",
            AppMenuAction::FeedbackPreview => "Feedback preview",
        }
        .into()
    }

    /// **This surface's own bindings, written out by hand on purpose** — the
    /// registry has no entry for them because this surface is a window beside
    /// the container rather than under it; see the note on the header chips
    /// in `lib.rs` and the TECHDEBT entry it names.
    fn shortcut(&self) -> Option<String> {
        Some(
            match self.action {
                AppMenuAction::KeyboardHints => "Ctrl+/",
                AppMenuAction::HelpPreview => "Ctrl+H",
                AppMenuAction::FeedbackPreview => "Ctrl+Shift+F",
            }
            .into(),
        )
    }

    fn checked(&self) -> Option<bool> {
        Some(self.checked)
    }
}
