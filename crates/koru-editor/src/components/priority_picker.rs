// SPDX-License-Identifier: MPL-2.0

//! The `koru.priority` picker: four fixed levels, the current one checked,
//! and a Clear row while one is set.
//!
//! On the [`LanguagePicker`] template like the [`ColorPicker`], and for the
//! same reason: choosing one of four named levels is that interaction. The
//! rows are in **semantic order** — Urgent, High, Medium, Low (D6) — not
//! alphabetical: the list *is* the scale, and reading it teaches it. It
//! commits the level's storage token through [`Outcome::SetAttr`] /
//! [`Outcome::ClearAttr`]; unset is the normal state, never a fifth token.
//!
//! [`LanguagePicker`]: super::language_picker::LanguagePicker
//! [`ColorPicker`]: super::color_picker::ColorPicker

use winit::keyboard::NamedKey;

use super::Outcome;
use crate::trigger::Trigger;
use koru_core::NodeId;
use koru_core::attributes::{KEY_PRIORITY, Priority};
use koru_core::node::AttrValue;
use koru_draw::text::TextEngine;
use koru_draw::theme;

/// The rows, top to bottom: most important first (D6's semantic order).
pub const LEVELS: [Priority; 4] = [
    Priority::Urgent,
    Priority::High,
    Priority::Medium,
    Priority::Low,
];

/// Display labels, in [`LEVELS`] order.
pub const LABELS: [&str; 4] = ["Urgent", "High", "Medium", "Low"];

pub struct PriorityPicker {
    pub trigger: Trigger,
    /// The nodes the choice applies to — one for `/priority`, several for a
    /// multi-select apply from the context menu.
    pub nodes: Vec<NodeId>,
    /// What the (first) node carries now, checked in the list. Also decides
    /// whether the Clear row exists: clearing nothing is not an offer.
    pub current: Option<Priority>,
    pub sel: usize,
    /// Measured from the labels; floored by `panel_size`.
    pub width: f32,
}

impl PriorityPicker {
    /// Open for `nodes`, preselecting the current level the way the color
    /// picker preselects its token.
    pub fn open_for(trigger: Trigger, nodes: Vec<NodeId>, current: Option<Priority>) -> Self {
        let sel = match current {
            // Past the Clear row, which exists exactly when `current` does.
            Some(p) => 1 + LEVELS.iter().position(|l| *l == p).unwrap_or(0),
            None => 0,
        };
        Self {
            trigger,
            nodes,
            current,
            sel,
            width: 0.0,
        }
    }

    /// Whether row 0 is Clear.
    pub fn offers_clear(&self) -> bool {
        self.current.is_some()
    }

    pub fn row_count(&self) -> usize {
        LEVELS.len() + usize::from(self.offers_clear())
    }

    /// What sits on row `i`: `None` is the Clear row, `Some` a level.
    pub fn row(&self, i: usize) -> Option<Option<Priority>> {
        if self.offers_clear() {
            if i == 0 {
                return Some(None);
            }
            return LEVELS.get(i - 1).copied().map(Some);
        }
        LEVELS.get(i).copied().map(Some)
    }

    pub fn measure_width(engine: &mut TextEngine) -> f32 {
        let widest = LABELS
            .iter()
            .chain(std::iter::once(&"Clear"))
            .map(|label| engine.simple_layout(label, theme::role::ui()).width())
            .fold(0.0_f32, f32::max);
        // Icon column plus the trailing check column — the color picker's.
        widest + 72.0
    }

    pub fn height(&self) -> f32 {
        theme::PANEL_PAD * 2.0 + self.row_count() as f32 * theme::ROW_H
    }

    pub fn key(&mut self, named: Option<NamedKey>) -> Outcome {
        let n = self.row_count();
        match named {
            Some(NamedKey::ArrowDown) => {
                self.sel = (self.sel + 1) % n;
                Outcome::Consumed
            }
            Some(NamedKey::ArrowUp) => {
                self.sel = (self.sel + n - 1) % n;
                Outcome::Consumed
            }
            Some(NamedKey::Enter) | Some(NamedKey::Tab) => self.choose(self.sel),
            // Fieldless: swallow, so characters do not land in the document.
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
        match self.row(i) {
            Some(None) => Outcome::ClearAttr(self.nodes.clone(), KEY_PRIORITY.to_owned()),
            Some(Some(level)) => Outcome::SetAttr(
                self.nodes.clone(),
                KEY_PRIORITY.to_owned(),
                AttrValue::Text(level.as_str().to_owned()),
            ),
            None => Outcome::Close,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nodes() -> Vec<NodeId> {
        vec![NodeId::new()]
    }

    /// The list is the scale: Urgent first, Low last — semantic order, never
    /// alphabetical (D6).
    #[test]
    fn the_rows_read_most_important_first() {
        assert_eq!(
            LEVELS,
            [
                Priority::Urgent,
                Priority::High,
                Priority::Medium,
                Priority::Low
            ]
        );
        for (label, level) in LABELS.iter().zip(LEVELS) {
            assert_eq!(label.to_lowercase(), level.as_str());
        }
    }

    #[test]
    fn a_level_commits_its_storage_token() {
        let p = PriorityPicker::open_for(Trigger::at_start(), nodes(), None);
        assert!(!p.offers_clear(), "nothing set, nothing to clear");
        assert_eq!(p.row_count(), 4);
        let chosen = p.choose(1);
        assert!(
            matches!(&chosen, Outcome::SetAttr(_, key, AttrValue::Text(t))
                if key == KEY_PRIORITY && t == "high"),
            "row 1 with no Clear row is High, got {chosen:?}"
        );
    }

    #[test]
    fn a_set_priority_is_preselected_and_clearable() {
        let p = PriorityPicker::open_for(Trigger::at_start(), nodes(), Some(Priority::Medium));
        assert!(p.offers_clear());
        assert_eq!(p.row_count(), 5);
        assert_eq!(p.sel, 3, "Clear, Urgent, High, then Medium checked");
        assert!(matches!(
            p.choose(0),
            Outcome::ClearAttr(_, key) if key == KEY_PRIORITY
        ));
        assert!(matches!(
            p.choose(1),
            Outcome::SetAttr(_, _, AttrValue::Text(t)) if t == "urgent"
        ));
    }

    #[test]
    fn arrows_wrap_and_enter_chooses_and_typing_is_swallowed() {
        let mut p = PriorityPicker::open_for(Trigger::at_start(), nodes(), None);
        p.sel = p.row_count() - 1;
        p.key(Some(NamedKey::ArrowDown));
        assert_eq!(p.sel, 0);
        p.key(Some(NamedKey::ArrowUp));
        assert_eq!(p.sel, p.row_count() - 1);
        assert!(matches!(
            p.key(Some(NamedKey::Enter)),
            Outcome::SetAttr(_, _, _)
        ));
        // A printable key must not fall through into the document.
        assert!(matches!(p.key(None), Outcome::Consumed));
    }

    #[test]
    fn a_click_past_the_rows_chooses_nothing() {
        let p = PriorityPicker::open_for(Trigger::at_start(), nodes(), None);
        assert!(matches!(p.click(-3.0), Outcome::Consumed));
        assert!(matches!(p.click(9999.0), Outcome::Close));
    }
}
