// SPDX-License-Identifier: MPL-2.0

//! Read the command owner's visible controls; activation stays with `choose`.

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) fn next_instance() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Panel-local row geometry shared by painting and semantic publication.
pub fn row_rect(panel_width: f32, search_top: f32, visible_index: usize) -> Rect {
    Rect::new(
        0.0,
        theme::PANEL_PAD + search_top + visible_index as f32 * theme::ROW_H,
        panel_width,
        theme::ROW_H,
    )
}

impl CommandMenu {
    pub fn visible_controls(&self, panel_width: f32) -> impl Iterator<Item = (Command, Rect)> + '_ {
        let top = if self.has_search() { rows_top() } else { 0.0 };
        self.hits
            .iter()
            .copied()
            .skip(self.first_visible())
            .take(visible_rows(self.hits.len()))
            .enumerate()
            .map(move |(i, command)| (command, row_rect(panel_width, top, i)))
    }

    /// Resolve the current command identity, never a remembered result index.
    /// A filtered, scrolled-away or disabled row cannot receive an old press.
    pub fn invoke_visible(&self, name: &str) -> Option<Outcome> {
        let first = self.first_visible();
        let (offset, command) = self
            .hits
            .iter()
            .skip(first)
            .take(visible_rows(self.hits.len()))
            .enumerate()
            .find(|(_, command)| command.name() == name)?;
        if disabled(*command, &self.allows).is_some() {
            return None;
        }
        Some(self.choose(first + offset))
    }
}

#[cfg(test)]
mod tests;
