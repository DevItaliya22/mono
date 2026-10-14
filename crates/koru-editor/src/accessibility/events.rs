// SPDX-License-Identifier: MPL-2.0

//! **What one publish will make the platform say, and the rule it has to
//! satisfy before it may say it.**
//!
//! One sentence, and what it excludes: *this module reads a [`BodyUpdate`] and
//! answers two questions — is this update internally coherent, and which
//! paragraphs will its arrival raise a native text or selection notification
//! about.* It excludes publishing (nothing here builds a `TreeUpdate`), it
//! excludes speaking (nothing here sets `live`, and see below), and it excludes
//! the platform (the events are `accesskit_windows`' to raise; this only says
//! which ones it is about to be handed the reasons for).
//!
//! ## The two events are a consequence of publishing, not a call
//!
//! Established by experiment in Plan 1b.1 and recorded in
//! `guide/contributing/deps/ACCESSKIT.md`: `accesskit_windows` raises
//! `UIA_Text_TextChangedEventId` when a `Role::TextRun` node's `value` changes
//! or a run is added or removed, and `UIA_Text_TextSelectionChangedEventId`
//! when a text container's own `text_selection` differs from the previous
//! update's. Both come out of the adapter's diff of one update against the
//! last. **There is no API to raise either one**, and a run republished with
//! identical properties raises nothing.
//!
//! So a provider does not *send* these events. What it does is publish an
//! update that will cause the right ones — and that is why the thing worth
//! owning here is the coherence rule, not a send.
//!
//! ## The coherence rule, in the roadmap's own words
//!
//! > A screen reader must not receive a selection pointing at text-run IDs
//! > removed in the same update.
//!
//! [`coherent`] is that sentence as a checkable property. It fails an update
//! whose published selection names a run belonging to a paragraph this same
//! update removes, a run belonging to a *different* paragraph, or a run this
//! update neither publishes nor re-states. The consequence of getting it wrong
//! is not a wrong answer: `accesskit_consumer` resolves a text position with an
//! `unwrap()` inside a UI Automation callback, which aborts the process with
//! `STATUS_STACK_BUFFER_OVERRUN` and no backtrace.
//!
//! ## Nothing here speaks
//!
//! The approved peer-edit policy is that Koru adds **no** authored spoken
//! announcement for a remote edit, while the native text and selection
//! notifications are preserved. Both halves live here, and they are the same
//! shape: [`Effects`] counts the notifications an update is about to cause —
//! which must not be zero when a peer changed the text — and this module has no
//! way to produce an announcement at all, because an announcement is
//! `SemanticNode::live`, which belongs to whoever assembles the tree and which
//! nothing in this crate sets. A test that asserts silence is asserting about
//! the publisher; a test that asserts the notifications survived is asserting
//! about this.

use std::collections::HashMap;

use accesskit::NodeId;
use koru_core::NodeId as KoruNodeId;

use super::projection::{BodyUpdate, RowText, RowUpdate};

/// Why an update must not be published.
///
/// Every arm is a crash or a lie, not a cosmetic problem. See the module note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Incoherent {
    /// A published selection names a run of a paragraph this update removes.
    SelectionNamesRemovedRow { row: KoruNodeId, run: NodeId },
    /// A published selection names a run that is not among the runs this update
    /// publishes or re-states for that paragraph.
    SelectionNamesUnpublishedRun { row: KoruNodeId, run: NodeId },
    /// Two paragraphs claim the same run id. A run is also a reference held by
    /// its neighbours' `next_on_line`, so one id in two places is one line
    /// walking into another.
    RunPublishedTwice {
        run: NodeId,
        first: KoruNodeId,
        second: KoruNodeId,
    },
    /// A paragraph is both published and removed by one update.
    RowPublishedAndRemoved(KoruNodeId),
}

/// The native notifications this update is about to be the reason for.
///
/// **"About to be the reason for", not "will raise".** The adapter diffs, so a
/// paragraph whose runs are republished byte for byte raises nothing. What this
/// counts is the paragraphs whose runs *this update carries* — the upper bound,
/// and the only number a provider controls. It is the right number for the
/// question the roadmap asks, which is whether the correctness events were
/// suppressed: zero here means the client was told nothing, whatever the
/// adapter would have done with more.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Effects {
    /// Paragraphs whose run nodes this update carries — a text change, an
    /// insertion or a removal inside the paragraph.
    pub text: Vec<KoruNodeId>,
    /// Paragraphs whose `text_selection` this update states.
    pub selection: Vec<KoruNodeId>,
    /// Paragraphs this update takes out of the document. The adapter raises a
    /// text change on the container for these too.
    pub removed: usize,
    /// Whether the container's ordered child list moved — an insert, a delete
    /// or a reorder rather than an edit inside one paragraph.
    pub reordered: bool,
}

impl Effects {
    /// Whether this update tells a client anything at all about the text.
    ///
    /// The half of the peer-edit policy that is easy to satisfy by breaking
    /// it: "no custom announcement" must not become "no notification".
    pub fn says_anything(&self) -> bool {
        !self.text.is_empty() || !self.selection.is_empty() || self.removed > 0 || self.reordered
    }

    /// What one update carries.
    pub fn of(update: &BodyUpdate) -> Self {
        let mut effects = Self {
            removed: update.removed.len(),
            reordered: update.order.is_some(),
            ..Self::default()
        };
        for row in &update.rows {
            match &row.text {
                RowText::Absent => {}
                RowText::Runs { runs, selection } => {
                    if !runs.is_empty() {
                        effects.text.push(row.id);
                    }
                    if selection.is_some() {
                        effects.selection.push(row.id);
                    }
                }
                RowText::Retained { selection, .. } => {
                    if selection.is_some() {
                        effects.selection.push(row.id);
                    }
                }
            }
        }
        effects
    }
}

/// Whether this update may be published.
///
/// Called by the publisher before it hands anything over, and by the tests that
/// stand in for the ones a person cannot run. Cheap: one pass over the rows the
/// update actually carries, which in a delta is one row.
pub fn coherent(update: &BodyUpdate) -> Result<(), Incoherent> {
    let removed: std::collections::HashSet<KoruNodeId> = update.removed.iter().copied().collect();
    let mut owner: HashMap<NodeId, KoruNodeId> = HashMap::new();
    for row in &update.rows {
        if removed.contains(&row.id) {
            return Err(Incoherent::RowPublishedAndRemoved(row.id));
        }
        for run in run_ids(row) {
            if let Some(first) = owner.insert(run, row.id)
                && first != row.id
            {
                return Err(Incoherent::RunPublishedTwice {
                    run,
                    first,
                    second: row.id,
                });
            }
        }
    }
    let gone: std::collections::HashSet<NodeId> = update.removed_runs.iter().copied().collect();
    for row in &update.rows {
        let Some(selection) = selection_of(row) else {
            continue;
        };
        for run in [selection.anchor.node, selection.focus.node] {
            // **§5's sentence, first and by name.** A run belonging to a
            // paragraph this same update takes away is the failure the roadmap
            // calls out, so it is told apart from an id that is merely absent.
            if gone.contains(&run) {
                return Err(Incoherent::SelectionNamesRemovedRow { row: row.id, run });
            }
            // Everything else: the selection has to name a run of the paragraph
            // that is carrying it, published or re-stated by this same update.
            // A run of a *different* paragraph would be one caret spanning two
            // text patterns, which is not a thing Koru has; a run in no
            // paragraph is a range the client cannot resolve, and
            // `accesskit_consumer` resolves one with an `unwrap()`.
            if owner.get(&run) != Some(&row.id) {
                return Err(Incoherent::SelectionNamesUnpublishedRun { row: row.id, run });
            }
        }
    }
    Ok(())
}

/// Every run id this row's update names, whether the nodes are carried or only
/// re-stated.
fn run_ids(row: &RowUpdate) -> Vec<NodeId> {
    match &row.text {
        RowText::Absent => Vec::new(),
        RowText::Runs { runs, .. } => runs.iter().map(|(id, _)| *id).collect(),
        RowText::Retained { runs, .. } => runs.clone(),
    }
}

fn selection_of(row: &RowUpdate) -> Option<&accesskit::TextSelection> {
    match &row.text {
        RowText::Absent => None,
        RowText::Runs { selection, .. } | RowText::Retained { selection, .. } => selection.as_ref(),
    }
}

#[cfg(test)]
mod tests;
