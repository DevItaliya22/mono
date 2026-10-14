// SPDX-License-Identifier: MPL-2.0

//! A [`DocumentDiff`] rearranged into reading order.
//!
//! [`compare`] answers *what changed*: destination preorder, removals appended
//! at the end. A person reads a diff top to bottom, with each removal at the
//! place it used to be and long unchanged stretches folded away. This module is
//! that projection — still renderer-independent, and serializable so the same
//! rows can cross IPC or feed a future side-by-side view (each modified row
//! keeps separate old and new segment lists).

use std::collections::{HashMap, HashSet};

use koru_core::NodeId;
use serde::{Deserialize, Serialize};

use crate::{DiffTag, DocumentDiff, NodeChange, SemanticDocument, SemanticNode, TextSpan};

/// Display data for one block — plain values, not a [`SemanticNode`], so the
/// model serializes without dragging the document schema along.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct BlockView {
    /// The kind's stable code ([`koru_core::Kind::as_str`]).
    pub kind: String,
    pub depth: usize,
    /// The block's text; a blob-content block carries its display name, which
    /// may be empty.
    pub text: String,
    /// Task state, for the kinds that have one.
    pub done: Option<bool>,
    /// Heading level 1–6, or `None` for body text.
    pub heading: Option<u8>,
}

impl BlockView {
    pub(crate) fn of(node: &SemanticNode) -> Self {
        Self {
            kind: node.node.kind.as_str().to_string(),
            depth: node.depth,
            text: node
                .node
                .content
                .as_text()
                .map(str::to_string)
                .or_else(|| node.node.meta.name.clone())
                .unwrap_or_default()
                .replace("\r\n", "\n")
                .replace('\r', "\n"),
            done: node.node.kind.is_task_like().then_some(node.node.meta.done),
            heading: node.node.meta.heading.map(|h| h.level()),
        }
    }
}

/// One row of the diff, in reading order.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum DiffRow {
    /// An unchanged block kept as context around a change.
    Context(BlockView),
    /// A folded run of unchanged blocks.
    Skipped {
        blocks: usize,
    },
    Added(BlockView),
    Removed(BlockView),
    Modified {
        /// The block's "after" identity.
        block: BlockView,
        /// The interleaved word spans, in the order the diff produced them —
        /// what an inline rendering draws directly. Either side alone is a
        /// filter away: [`old_side`] and [`new_side`].
        spans: Vec<TextSpan>,
        kind_changed: bool,
        metadata_changed: bool,
        marks_changed: bool,
    },
    /// Same content, different parent. A marker row, not a text diff.
    Moved {
        block: BlockView,
    },
    /// Same content and parent, different position among its siblings.
    Reordered {
        block: BlockView,
    },
}

/// Block counts for the one-line summary. Every changed block lands in exactly
/// one field.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct DiffStats {
    pub added: usize,
    pub removed: usize,
    pub modified: usize,
    pub moved: usize,
    pub reordered: usize,
    pub unchanged: usize,
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct DiffOptions {
    /// Unchanged blocks kept on each side of a change before the rest of the
    /// run folds into [`DiffRow::Skipped`].
    pub context: usize,
    /// Minimum token-set overlap (0.0–1.0) for pairing a removed block with an
    /// added block as [`DiffRow::Modified`]. Conservative: a higher threshold
    /// prefers an unpaired remove/add over a false modification.
    pub similarity_threshold: f64,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            context: 3,
            similarity_threshold: 0.6,
        }
    }
}

#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct UnifiedDiff {
    pub rows: Vec<DiffRow>,
    pub stats: DiffStats,
}

/// Project a [`DocumentDiff`] into reading order.
///
/// Walks the *after* document in preorder. Each removal appears immediately
/// after its nearest surviving predecessor in the *before* order (at the top,
/// for removals with none), and each run of unchanged blocks keeps `context`
/// blocks against each adjacent change while the middle folds into one
/// [`DiffRow::Skipped`].
///
/// This convenience wrapper uses identity correspondence: a before block
/// anchors to an after block only when they share the same [`NodeId`]. Use
/// [`unified_with_correspondence`] when the diff pairs blocks by content
/// rather than identity.
pub fn unified(
    before: &SemanticDocument,
    after: &SemanticDocument,
    diff: &DocumentDiff,
    options: DiffOptions,
) -> UnifiedDiff {
    let after_ids: HashSet<NodeId> = after.nodes.iter().map(|n| n.node.id).collect();
    let identity: HashMap<NodeId, NodeId> = before
        .nodes
        .iter()
        .filter(|n| after_ids.contains(&n.node.id))
        .map(|n| (n.node.id, n.node.id))
        .collect();
    unified_with_correspondence(before, after, diff, &identity, options)
}

/// [`unified`] with an explicit before→after correspondence map.
///
/// `correspondence` must contain an entry for every before block that has a
/// visual counterpart in `after` — both unchanged blocks and modified pairs.
/// Removals anchor to the nearest preceding before block that has a
/// correspondence, mapped through to its after id. This is what makes removals
/// render in the right place when content matching pairs blocks with different
/// ids (e.g. paste-a-whole-page).
pub fn unified_with_correspondence(
    before: &SemanticDocument,
    after: &SemanticDocument,
    diff: &DocumentDiff,
    correspondence: &HashMap<NodeId, NodeId>,
    options: DiffOptions,
) -> UnifiedDiff {
    // Where a reader's eye expects each removal: right after the nearest before
    // block that has a visual counterpart, mapped to its after id. One pass over
    // the old document, so a wholesale deletion stays O(document).
    let mut anchor_of: HashMap<NodeId, Option<NodeId>> = HashMap::new();
    let mut last_survivor = None;
    for node in &before.nodes {
        let id = node.node.id;
        if let Some(&after_id) = correspondence.get(&id) {
            last_survivor = Some(after_id);
        } else {
            anchor_of.insert(id, last_survivor);
        }
    }

    let mut changes: HashMap<NodeId, &NodeChange> = HashMap::new();
    let mut removals_by_anchor: HashMap<Option<NodeId>, Vec<&SemanticNode>> = HashMap::new();
    for change in &diff.changes {
        match change {
            NodeChange::Removed(node) => {
                let anchor = anchor_of.get(&node.node.id).copied().flatten();
                removals_by_anchor.entry(anchor).or_default().push(node);
            }
            NodeChange::Added(node) => {
                changes.insert(node.node.id, change);
            }
            NodeChange::Moved { after: node, .. } | NodeChange::Reordered { after: node, .. } => {
                changes.insert(node.node.id, change);
            }
            NodeChange::Modified(modification) => {
                changes.insert(modification.after.node.id, change);
            }
        }
    }

    let mut out = UnifiedDiff::default();
    let mut run: Vec<BlockView> = Vec::new();
    let mut any_change = false;

    for removed in removals_by_anchor.remove(&None).unwrap_or_default() {
        push_removed(
            &mut out,
            &mut run,
            &mut any_change,
            options.context,
            removed,
        );
    }

    for node in &after.nodes {
        let id = node.node.id;
        match changes.get(&id) {
            None => {
                out.stats.unchanged += 1;
                run.push(BlockView::of(node));
            }
            Some(change) => {
                flush_before_change(&mut out, &mut run, &mut any_change, options.context);
                match change {
                    NodeChange::Added(added) => {
                        out.stats.added += 1;
                        out.rows.push(DiffRow::Added(BlockView::of(added)));
                    }
                    NodeChange::Moved { after: moved, .. } => {
                        out.stats.moved += 1;
                        out.rows.push(DiffRow::Moved {
                            block: BlockView::of(moved),
                        });
                    }
                    NodeChange::Reordered {
                        after: reordered, ..
                    } => {
                        out.stats.reordered += 1;
                        out.rows.push(DiffRow::Reordered {
                            block: BlockView::of(reordered),
                        });
                    }
                    NodeChange::Modified(m) => {
                        out.stats.modified += 1;
                        out.rows.push(DiffRow::Modified {
                            block: BlockView::of(&m.after),
                            spans: m.text.clone(),
                            kind_changed: m.kind_changed,
                            metadata_changed: m.metadata_changed,
                            marks_changed: m.marks_changed,
                        });
                    }
                    NodeChange::Removed(_) => unreachable!("removals are keyed by anchor"),
                }
            }
        }
        for removed in removals_by_anchor.remove(&Some(id)).unwrap_or_default() {
            push_removed(
                &mut out,
                &mut run,
                &mut any_change,
                options.context,
                removed,
            );
        }
    }

    // The final run has no change after it: keep only its head as context.
    // With no change in the whole document, everything folds.
    let head = if any_change { options.context } else { 0 };
    fold(&mut out, &mut run, head, 0);
    out
}

fn push_removed(
    out: &mut UnifiedDiff,
    run: &mut Vec<BlockView>,
    any_change: &mut bool,
    context: usize,
    removed: &SemanticNode,
) {
    flush_before_change(out, run, any_change, context);
    out.stats.removed += 1;
    out.rows.push(DiffRow::Removed(BlockView::of(removed)));
}

/// Flush the unchanged run sitting before a change: keep its head as context
/// for the previous change (none, at the top of the document) and its tail as
/// context for this one.
fn flush_before_change(
    out: &mut UnifiedDiff,
    run: &mut Vec<BlockView>,
    any_change: &mut bool,
    context: usize,
) {
    let head = if *any_change { context } else { 0 };
    fold(out, run, head, context);
    *any_change = true;
}

/// The old text of a modification: every span the new side did not insert.
pub fn old_side(spans: &[TextSpan]) -> impl Iterator<Item = &TextSpan> {
    spans.iter().filter(|s| s.tag != DiffTag::Insert)
}

/// The new text of a modification: every span the old side did not lose.
pub fn new_side(spans: &[TextSpan]) -> impl Iterator<Item = &TextSpan> {
    spans.iter().filter(|s| s.tag != DiffTag::Delete)
}

/// Emit `head` context blocks, a [`DiffRow::Skipped`] for the middle, and
/// `tail` context blocks — folding only when the middle is more than one
/// block, because "1 unchanged block" reads worse than the block itself.
fn fold(out: &mut UnifiedDiff, run: &mut Vec<BlockView>, head: usize, tail: usize) {
    let run: Vec<BlockView> = std::mem::take(run);
    if run.len() <= head + tail + 1 {
        out.rows.extend(run.into_iter().map(DiffRow::Context));
        return;
    }
    let middle = run.len() - head - tail;
    for (index, block) in run.into_iter().enumerate() {
        if index == head {
            out.rows.push(DiffRow::Skipped { blocks: middle });
        }
        if index < head || index >= head + middle {
            out.rows.push(DiffRow::Context(block));
        }
    }
}

#[cfg(test)]
mod tests;
