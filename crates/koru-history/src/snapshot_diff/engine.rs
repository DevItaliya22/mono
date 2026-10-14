// SPDX-License-Identifier: MPL-2.0

//! Content-based diff over projected blocks.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use koru_core::NodeId;
use similar::{Algorithm, TextDiff};

use crate::unified::{BlockView, DiffOptions};
use crate::{DocumentDiff, NodeChange, NodeModification};

use super::project::ProjectedBlock;
use super::refine::refine_text;

/// Per-stage timing for one snapshot compare, measured on the worker.
#[derive(Clone, Copy, Debug, Default)]
pub struct EngineTiming {
    pub outer_us: u64,
    pub refine_us: u64,
}

/// Compare two projected snapshots and return a [`DocumentDiff`], the
/// before→after correspondence map, and per-stage timing.
pub(crate) fn compare_blocks(
    before: &[ProjectedBlock],
    after: &[ProjectedBlock],
    options: DiffOptions,
) -> (DocumentDiff, HashMap<NodeId, NodeId>, EngineTiming) {
    let outer_start = Instant::now();
    let before_texts: Vec<&str> = before.iter().map(|b| b.view.text.as_str()).collect();
    let after_texts: Vec<&str> = after.iter().map(|b| b.view.text.as_str()).collect();

    let diff = TextDiff::configure()
        .algorithm(Algorithm::Patience)
        .diff_slices(&before_texts, &after_texts);

    let mut out = DocumentDiff::default();
    let mut correspondence: HashMap<NodeId, NodeId> = HashMap::new();
    let mut refine_us: u64 = 0;

    // Walk the diff output once, accumulating adjacent Delete/Insert operations
    // into change groups. Equal operations flush the current group.
    let mut group = ChangeGroup::default();
    for change in diff.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Equal => {
                flush_group(
                    &mut group,
                    before,
                    after,
                    options,
                    &mut out,
                    &mut correspondence,
                    &mut refine_us,
                );
                if let (Some(old_index), Some(new_index)) = (change.old_index(), change.new_index())
                {
                    let before_block = &before[old_index];
                    let after_block = &after[new_index];
                    correspondence.insert(before_block.node.node.id, after_block.node.node.id);
                    if views_match_except_text(&before_block.view, &after_block.view) {
                        out.unchanged += 1;
                    } else {
                        let refine_start = Instant::now();
                        let m = modification(before_block, after_block);
                        refine_us += refine_start.elapsed().as_micros() as u64;
                        out.changes.push(NodeChange::Modified(m));
                    }
                }
            }
            similar::ChangeTag::Delete => {
                if let Some(index) = change.old_index() {
                    group.deleted.push(index);
                }
            }
            similar::ChangeTag::Insert => {
                if let Some(index) = change.new_index() {
                    group.inserted.push(index);
                }
            }
        }
    }
    flush_group(
        &mut group,
        before,
        after,
        options,
        &mut out,
        &mut correspondence,
        &mut refine_us,
    );
    let outer_us = outer_start.elapsed().as_micros() as u64;

    (
        out,
        correspondence,
        EngineTiming {
            outer_us,
            refine_us,
        },
    )
}

#[derive(Default)]
struct ChangeGroup {
    deleted: Vec<usize>,
    inserted: Vec<usize>,
}

fn flush_group(
    group: &mut ChangeGroup,
    before: &[ProjectedBlock],
    after: &[ProjectedBlock],
    options: DiffOptions,
    out: &mut DocumentDiff,
    correspondence: &mut HashMap<NodeId, NodeId>,
    refine_us: &mut u64,
) {
    if group.deleted.is_empty() && group.inserted.is_empty() {
        return;
    }

    // Pair deletions and insertions within this run using token-set similarity.
    // Greedy: consider all pairs, pick the highest-scoring pair above the
    // threshold, repeat until no pair qualifies. Runs are local, so this stays
    // small in practice.
    let threshold = options.similarity_threshold;
    let mut paired: HashSet<usize> = HashSet::new();
    let mut paired_after: HashSet<usize> = HashSet::new();

    let mut pairs: Vec<(usize, usize, f64)> = Vec::new();
    for (di, &before_idx) in group.deleted.iter().enumerate() {
        for (ii, &after_idx) in group.inserted.iter().enumerate() {
            let score = similarity(&before[before_idx].view.text, &after[after_idx].view.text);
            if score >= threshold {
                pairs.push((di, ii, score));
            }
        }
    }
    pairs.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

    for (di, ii, _) in pairs {
        if paired.contains(&di) || paired_after.contains(&ii) {
            continue;
        }
        let before_idx = group.deleted[di];
        let after_idx = group.inserted[ii];
        let before_block = &before[before_idx];
        let after_block = &after[after_idx];
        let refine_start = Instant::now();
        let m = modification(before_block, after_block);
        *refine_us += refine_start.elapsed().as_micros() as u64;
        out.changes.push(NodeChange::Modified(m));
        correspondence.insert(before_block.node.node.id, after_block.node.node.id);
        paired.insert(di);
        paired_after.insert(ii);
    }

    for (di, &before_idx) in group.deleted.iter().enumerate() {
        if paired.contains(&di) {
            continue;
        }
        out.changes
            .push(NodeChange::Removed(before[before_idx].node.clone()));
    }

    for (ii, &after_idx) in group.inserted.iter().enumerate() {
        if paired_after.contains(&ii) {
            continue;
        }
        out.changes
            .push(NodeChange::Added(after[after_idx].node.clone()));
    }

    group.deleted.clear();
    group.inserted.clear();
}

fn modification(before: &ProjectedBlock, after: &ProjectedBlock) -> NodeModification {
    let before_text = before.view.text.as_str();
    let after_text = after.view.text.as_str();
    NodeModification {
        kind_changed: before.view.kind != after.view.kind,
        content_changed: before_text != after_text,
        metadata_changed: before.node.node.meta != after.node.node.meta
            || before.view.depth != after.view.depth,
        marks_changed: before.node.runs != after.node.runs,
        text: refine_text(before_text, after_text),
        before: before.node.clone(),
        after: after.node.clone(),
    }
}

/// True when two [`BlockView`]s are identical aside from their text.
///
/// Patience already matched them by text, so this checks whether the paired
/// blocks differ only in kind, heading level, task state or depth — the
/// property changes that render as a `Modified` row with the "properties
/// changed" badge.
fn views_match_except_text(a: &BlockView, b: &BlockView) -> bool {
    a.kind == b.kind && a.depth == b.depth && a.done == b.done && a.heading == b.heading
}

/// Token-set Jaccard similarity, bounded [0.0, 1.0].
fn similarity(a: &str, b: &str) -> f64 {
    let tokens_a: HashSet<&str> = a.split_whitespace().collect();
    let tokens_b: HashSet<&str> = b.split_whitespace().collect();
    if tokens_a.is_empty() && tokens_b.is_empty() {
        return 1.0;
    }
    let intersection: HashSet<_> = tokens_a.intersection(&tokens_b).copied().collect();
    let union: HashSet<_> = tokens_a.union(&tokens_b).copied().collect();
    if union.is_empty() {
        return 1.0;
    }
    intersection.len() as f64 / union.len() as f64
}
