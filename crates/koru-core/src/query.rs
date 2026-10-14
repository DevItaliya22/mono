// SPDX-License-Identifier: MPL-2.0

//! The read API.
//!
//! This is the shape a third-party tool sees, whether it calls
//! [`WorkspaceDoc::query`] in-process or the eventual
//! `GET /tasks?assignee=…&due_lt=…` binding over HTTP. Both answer from the
//! same index, so the REST layer adds a transport and nothing else.
//!
//! Queries never read a node's text. A due date and an assignee are
//! *attributes* on the node; a pill is only how they are drawn. That is what
//! makes "every task assigned to Kim, due before August" a lookup rather than a
//! parse of every line in the workspace.

use chrono::NaiveDate;

use crate::ids::NodeId;
use crate::node::{Kind, Node, NodeMeta};
use crate::time::Temporal;

/// An inclusive-start, exclusive-end date window. `None` on either side means
/// unbounded, so `due_before(d)` is `DateRange { from: None, to: Some(d) }`.
#[derive(Copy, Clone, PartialEq, Eq, Default, Debug)]
pub struct DateRange {
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
}

impl DateRange {
    pub fn before(to: NaiveDate) -> Self {
        Self {
            from: None,
            to: Some(to),
        }
    }

    pub fn after(from: NaiveDate) -> Self {
        Self {
            from: Some(from),
            to: None,
        }
    }

    pub fn between(from: NaiveDate, to: NaiveDate) -> Self {
        Self {
            from: Some(from),
            to: Some(to),
        }
    }

    pub fn contains(&self, d: NaiveDate) -> bool {
        self.from.is_none_or(|f| d >= f) && self.to.is_none_or(|t| d < t)
    }

    /// Whether a **due value** falls in this window.
    ///
    /// A calendar window compares calendar days, so it answers for the two
    /// temporal types that *have* a day without being told where they are read
    /// (a date is one; a floating wall clock drops its time). A
    /// [`ZonedDateTime`] does not match: its day depends on the reader's zone,
    /// and this crate carries no timezone database — silently folding it
    /// through UTC would put a Zurich deadline on the wrong side of a midnight
    /// boundary for a third of the planet.
    ///
    /// That refusal is the point rather than a gap ([ADR-183], rule 5:
    /// queries differ by type and never coerce). A caller that means "every
    /// deadline this week, wherever it was set" compares **instants**, which
    /// is what the store's attribute query service does.
    ///
    /// [`ZonedDateTime`]: crate::time::ZonedDateTime
    /// [ADR-183]: ../../../docs/decisions/ADR-201-225.md#adr-206-timemodel--koru-has-three-temporal-types-not-one-with-a-nullable-timezone
    pub fn contains_due(&self, due: &Temporal) -> bool {
        match due.zone_free_date() {
            Some(day) => self.contains(day.date()),
            None => false,
        }
    }
}

/// Every filter is optional and they combine with AND. The default matches
/// every task in the workspace.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct TaskQuery {
    /// Restrict to descendants of this node — the general form of "in this
    /// project", now that grouping is just nesting.
    pub under: Option<NodeId>,
    /// The user's own name for a level: "Project", "Phase". Never enforced.
    pub level: Option<String>,
    /// A contact node referenced by an attribute with role `assignee`.
    pub assignee: Option<NodeId>,
    pub due: Option<DateRange>,
    pub done: Option<bool>,
    /// Only nodes with no children, or only nodes with children.
    pub leaf: Option<bool>,
    /// Case-insensitive substring of the node's line.
    pub text: Option<String>,
    /// Tasks holding a reference to this node, whatever the role — the
    /// backlinks of a document.
    ///
    /// Derived by walking, not read from a stored list: the forward reference
    /// is the truth, and a second copy could come to disagree with it. See
    /// `docs/deferred/`.
    pub links_to: Option<NodeId>,
}

impl TaskQuery {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn under(mut self, node: NodeId) -> Self {
        self.under = Some(node);
        self
    }

    pub fn level(mut self, level: impl Into<String>) -> Self {
        self.level = Some(level.into());
        self
    }

    pub fn assignee(mut self, contact: NodeId) -> Self {
        self.assignee = Some(contact);
        self
    }

    pub fn due(mut self, range: DateRange) -> Self {
        self.due = Some(range);
        self
    }

    pub fn done(mut self, done: bool) -> Self {
        self.done = Some(done);
        self
    }

    pub fn leaf(mut self, leaf: bool) -> Self {
        self.leaf = Some(leaf);
        self
    }

    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Everything referencing `node` — what a knowledge view asks to list the
    /// tasks related to a document.
    pub fn links_to(mut self, node: NodeId) -> Self {
        self.links_to = Some(node);
        self
    }
}

/// Enough to draw a kanban card or a calendar entry without a second lookup.
///
/// `assignees` holds resolved contacts rather than bare ids: a card needs the
/// person's *name*, and making every caller resolve that itself would be the
/// N+1 problem in a UI loop.
#[derive(Clone, PartialEq, Debug)]
pub struct TaskSummary {
    pub id: NodeId,
    pub text: String,
    pub done: bool,
    /// The due as stored — whichever of the three temporal types it is, never
    /// flattened to a day ([ADR-183]). A card that wants a day asks
    /// [`Temporal::zone_free_date`], or converts a zoned value with a zone in
    /// hand.
    ///
    /// [ADR-183]: ../../../docs/decisions/ADR-201-225.md#adr-206-timemodel--koru-has-three-temporal-types-not-one-with-a-nullable-timezone
    pub due: Option<Temporal>,
    pub assignees: Vec<ResolvedRef>,
    pub meta: NodeMeta,
    pub depth: usize,
    /// Children done / total children. `(0, 0)` for a leaf.
    pub progress: (u32, u32),
}

impl TaskSummary {
    pub fn is_group(&self) -> bool {
        self.progress.1 > 0
    }

    /// True when every child is ticked. A childless task is never "complete" by
    /// this measure — use `done` for that.
    pub fn children_complete(&self) -> bool {
        self.progress.1 > 0 && self.progress.0 == self.progress.1
    }
}

/// A reference that has been looked up. A target can legitimately be missing —
/// it may live in a profile that is not open, or one this user cannot read — so
/// [`ResolvedRef::Dangling`] is a normal result, not an error.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ResolvedRef {
    Found {
        id: NodeId,
        label: String,
        icon: Option<String>,
    },
    Dangling {
        id: NodeId,
    },
}

impl ResolvedRef {
    /// What to draw. A dangling reference still renders — as a placeholder —
    /// rather than vanishing or crashing the view.
    pub fn label(&self) -> &str {
        match self {
            Self::Found { label, .. } => label,
            Self::Dangling { .. } => "unavailable",
        }
    }

    pub fn id(&self) -> NodeId {
        match self {
            Self::Found { id, .. } | Self::Dangling { id } => *id,
        }
    }

    pub fn is_dangling(&self) -> bool {
        matches!(self, Self::Dangling { .. })
    }
}

/// Whether a node satisfies the parts of a query that depend only on the node
/// itself. Ancestry, depth and progress are supplied by the caller, which is
/// the only thing that knows the tree.
pub(crate) fn matches_node(q: &TaskQuery, node: &Node, has_children: bool) -> bool {
    if !node.kind.is_task_like() {
        return false;
    }
    if let Some(done) = q.done {
        if node.meta.done != done {
            return false;
        }
    }
    if let Some(leaf) = q.leaf {
        if has_children == leaf {
            return false;
        }
    }
    if let Some(level) = &q.level {
        if node.meta.level_name.as_deref() != Some(level.as_str()) {
            return false;
        }
    }
    if let Some(range) = &q.due {
        match node.due() {
            Some(due) if range.contains_due(&due) => {}
            _ => return false,
        }
    }
    if let Some(assignee) = q.assignee {
        if !node.assignees().any(|r| r.node == assignee) {
            return false;
        }
    }
    if let Some(target) = q.links_to {
        // Any role, not just `assignee`: a document pill, an attachment and a
        // reviewer are all "related to this node".
        let links = node
            .content
            .attrs()
            .iter()
            .filter_map(|a| a.as_ref_target())
            .any(|r| r.node == target);
        if !links {
            return false;
        }
    }
    if let Some(needle) = &q.text {
        let hay = node.content.as_text().unwrap_or_default().to_lowercase();
        if !hay.contains(&needle.to_lowercase()) {
            return false;
        }
    }
    true
}

/// The label a resolved reference should show: a contact's name is its line.
pub(crate) fn label_of(node: &Node) -> String {
    match node.kind {
        Kind::Image | Kind::Canvas => node
            .meta
            .icon
            .clone()
            .unwrap_or_else(|| "image".to_string()),
        Kind::Divider => "Divider".to_string(),
        _ => node.content.as_text().unwrap_or_default().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn date_range_is_half_open() {
        let r = DateRange::between(d(2026, 8, 1), d(2026, 9, 1));
        assert!(r.contains(d(2026, 8, 1)), "start is inclusive");
        assert!(r.contains(d(2026, 8, 31)));
        assert!(!r.contains(d(2026, 9, 1)), "end is exclusive");
        assert!(!r.contains(d(2026, 7, 31)));
    }

    #[test]
    fn unbounded_sides_mean_no_constraint() {
        assert!(DateRange::before(d(2026, 8, 1)).contains(d(1999, 1, 1)));
        assert!(DateRange::after(d(2026, 8, 1)).contains(d(2999, 1, 1)));
        assert!(DateRange::default().contains(d(2026, 8, 1)));
    }

    #[test]
    fn a_dangling_reference_still_has_something_to_draw() {
        let r = ResolvedRef::Dangling { id: NodeId::new() };
        assert_eq!(r.label(), "unavailable");
        assert!(r.is_dangling());
    }

    #[test]
    fn progress_distinguishes_a_group_from_a_leaf() {
        let leaf = TaskSummary {
            id: NodeId::new(),
            text: "one".into(),
            done: false,
            due: None,
            assignees: vec![],
            meta: NodeMeta::default(),
            depth: 0,
            progress: (0, 0),
        };
        assert!(!leaf.is_group());
        assert!(
            !leaf.children_complete(),
            "a leaf is never children-complete"
        );

        let group = TaskSummary {
            progress: (2, 2),
            ..leaf.clone()
        };
        assert!(group.is_group());
        assert!(group.children_complete());
    }
}
