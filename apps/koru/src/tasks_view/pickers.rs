// SPDX-License-Identifier: MPL-2.0

//! The three pickers the Tasks page hosts over a row, and the one adapter
//! from what a widget answered to what the page asks for.
//!
//! **One field, three panels.** The page holds `Option<Picker>` rather than
//! three `Option`s, so *two pickers open at once* is not a state the code can
//! be in and no site that reads the field has to decide what it would mean.
//! Every one of them is anchored on the row's `…`, closes on a choice, and
//! leaves as a [`Request`] — the page writes nothing itself; the container
//! turns the request into the ordinary task command, which is the one door a
//! Tasks surface has to the store.
//!
//! **The calendar is the shared widget.** [`DateGrid`] is the same month,
//! phrase box and Clear row the editor's `/due` shows and the search surface
//! hosts; what this module adds is the four-arm adapter from its [`Pick`] to a
//! request, and the seeds — which day it opens on, whether Clear is offered —
//! read off the row, by the editor's own rule for its picker. The priority
//! rows are the editor's tables, imported: the semantic order is a decision
//! (D6) and a second list of four strings would be a second place to get it
//! wrong.

use std::borrow::Cow;

use chrono::NaiveDate;
use koru_core::attributes::Priority;
use koru_core::time::{DateOnly, Temporal};
use koru_core::{DocumentId, NodeId};
use koru_draw::geometry::Rect;
use koru_draw::theme;
use koru_editor::components::priority_picker::{LABELS, LEVELS};
use koru_editor::utils::when;
use koru_ui::date_grid::{DateGrid, Pick};
use koru_ui::menu::{Menu, MenuItem};

use super::menu::ListPick;
use super::view::Request;

/// The panel that is up over a row, if any. Exactly one.
pub(super) enum Picker {
    List(ListPicker),
    Priority(PriorityPick),
    Due(DuePick),
}

/// The membership picker, and the row it belongs to.
///
/// The ids ride with the panel rather than being looked up again when a row is
/// chosen: the page underneath refreshes every half minute, and a picker that
/// resolved its task at *choice* time could write into whichever row had slid
/// into that position. The same rule holds for the other two.
pub(super) struct ListPicker {
    pub target: Target,
    pub menu: Menu<ListPick>,
}

/// What a picker is deciding for: a row on the page, whose change leaves as
/// a request, or the capture row, whose pick rides on the words until Enter.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum Target {
    Row { document: DocumentId, node: NodeId },
    Capture,
}

impl Target {
    pub fn row(document: DocumentId, node: NodeId) -> Self {
        Self::Row { document, node }
    }
}

/// A pick made for the capture row, handed back to the page to wear.
#[derive(Clone, PartialEq, Debug)]
pub(super) enum Picked {
    Due(Option<Temporal>),
    Priority(Option<Priority>),
}

/// The priority picker: four levels in the scale's own order, the current one
/// checked, and Clear while one is set.
pub(super) struct PriorityPick {
    pub target: Target,
    pub menu: Menu<PriorityRow>,
}

/// One row of the priority picker.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum PriorityRow {
    Level {
        level: Priority,
        checked: bool,
    },
    Divider,
    /// Take the priority off. Offered only while one is set — clearing nothing
    /// is not an offer, the rule the editor's picker and the Clear row on the
    /// calendar both follow.
    Clear,
}

/// The rows, from the editor's tables: `LEVELS` in the semantic order the
/// scale is read in, then a divider and Clear when there is something to clear.
pub(super) fn priority_rows(current: Option<Priority>) -> Vec<PriorityRow> {
    let mut rows: Vec<PriorityRow> = LEVELS
        .iter()
        .map(|level| PriorityRow::Level {
            level: *level,
            checked: current == Some(*level),
        })
        .collect();
    if current.is_some() {
        rows.push(PriorityRow::Divider);
        rows.push(PriorityRow::Clear);
    }
    rows
}

impl MenuItem for PriorityRow {
    fn label(&self) -> Cow<'_, str> {
        match self {
            Self::Level { level, .. } => Cow::Borrowed(
                LEVELS
                    .iter()
                    .position(|l| l == level)
                    .map(|i| LABELS[i])
                    .unwrap_or(""),
            ),
            Self::Divider => Cow::Borrowed(""),
            Self::Clear => Cow::Borrowed("Clear"),
        }
    }

    fn checked(&self) -> Option<bool> {
        match self {
            Self::Level { checked, .. } => Some(*checked),
            _ => None,
        }
    }

    fn divider(&self) -> bool {
        matches!(self, Self::Divider)
    }

    fn selectable(&self) -> bool {
        !matches!(self, Self::Divider)
    }

    fn height(&self) -> f32 {
        match self {
            Self::Divider => 8.0,
            _ => theme::ROW_H,
        }
    }
}

/// What a press or a key on a picker decided.
#[derive(Clone, PartialEq, Debug)]
pub(super) enum Choice {
    /// Something to ask the container for; the panel closes.
    Request(Request),
    /// Something for the capture row to wear; the panel closes.
    Picked(Picked),
    /// Nothing to ask; the panel closes.
    Close,
    /// Nothing to ask; the panel stays — a refine landed, a phrase was left
    /// in the box, a key was swallowed.
    Stay,
}

impl PriorityPick {
    /// Choose row `row`. Choosing the level it already has is a no-op, not a
    /// second write — the guarded set would answer the same thing, and not
    /// asking is cheaper than being told.
    pub fn choose(&self, row: usize) -> Choice {
        let priority = match self.menu.items().get(row) {
            Some(PriorityRow::Level { checked: true, .. }) => return Choice::Close,
            Some(PriorityRow::Level { level, .. }) => Some(*level),
            Some(PriorityRow::Clear) => None,
            Some(PriorityRow::Divider) | None => return Choice::Stay,
        };
        match self.target {
            Target::Row { document, node } => Choice::Request(Request::SetPriority {
                document,
                node,
                priority,
            }),
            Target::Capture => Choice::Picked(Picked::Priority(priority)),
        }
    }
}

/// The calendar over a row: the shared grid, and where the page put it.
pub(super) struct DuePick {
    pub target: Target,
    pub grid: DateGrid,
    /// The panel's rectangle in the view's coordinates — what the press and
    /// the drawing both read, so a cell is clickable exactly where it is
    /// drawn.
    pub panel: Rect,
    /// Whether the pointer is over the Clear row, for the row's hover fill.
    pub clear_hovered: bool,
}

/// What the calendar opens on, by the editor's own rule for its picker
/// (`open_due_chooser_seeded`): the row's due day when it has one — which is
/// also what puts the Clear row on — else today. A task due in November opens
/// on November, and `+2w` in the phrase box counts from the day the panel
/// shows.
pub(super) fn due_seeds(due_day: Option<NaiveDate>, today: NaiveDate) -> (NaiveDate, bool) {
    match due_day {
        Some(day) => (day, true),
        None => (today, false),
    }
}

impl DuePick {
    /// Where the panel goes: hanging under the row's `…`, right edge aligned
    /// to it, the way the two menus are — and kept inside the viewport, above
    /// the row when there is no room below it.
    pub fn place(dots: Rect, (w, h): (f32, f32), viewport: (f32, f32)) -> Rect {
        const GAP: f32 = 4.0;
        let pad = theme::PANEL_PAD;
        let mut x = dots.x + dots.w - w;
        if x + w > viewport.0 - pad {
            x = viewport.0 - pad - w;
        }
        if x < pad {
            x = pad;
        }
        let below = dots.y + dots.h + GAP;
        let y = if below + h <= viewport.1 - pad {
            below
        } else {
            let above = dots.y - GAP - h;
            if above >= pad {
                above
            } else {
                (viewport.1 - pad - h).max(pad)
            }
        };
        Rect::new(x, y, w, h)
    }

    /// The grid's answer, in the page's vocabulary.
    pub fn resolve(&self, pick: Pick) -> Choice {
        resolve_pick(self.target, &self.grid, pick)
    }
}

/// **The whole adapter: four arms.** A day stores a calendar date, as the
/// document's own picker stores one — a task due "Thursday 17:00" picked to
/// Friday is due Friday with no time (D8). A timed phrase is parsed against
/// **this grid's** reference, selected day included, and converted by the one
/// function every surface uses, so the same words in the same panel store
/// what the editor stores. Clear is the same request with no due — one
/// dimension, both directions. Nothing is nothing, and the panel stays.
pub(super) fn resolve_pick(target: Target, grid: &DateGrid, pick: Pick) -> Choice {
    let due = match pick {
        Pick::None => return Choice::Stay,
        Pick::Day(day) => Some(Temporal::Date(DateOnly::new(day))),
        Pick::Timed(phrase) => {
            match when::parse(&phrase, grid.reference(), when::DEFAULT_DATE_ORDER) {
                Some(parsed) => Some(when::to_temporal(parsed)),
                // The grid only hands up a phrase it has parsed against this
                // same reference, so this arm is unreachable in practice; if
                // it were reached, losing the typed words would be the wrong
                // answer, and the panel keeps them.
                None => return Choice::Stay,
            }
        }
        Pick::Clear => None,
    };
    match target {
        Target::Row { document, node } => Choice::Request(Request::SetDue {
            document,
            node,
            due,
        }),
        Target::Capture => Choice::Picked(Picked::Due(due)),
    }
}
