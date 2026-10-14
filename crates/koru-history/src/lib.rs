// SPDX-License-Identifier: MPL-2.0

//! Semantic history over Koru's typed node model.
//!
//! This crate intentionally does not turn a document into Markdown. A diff is
//! a comparison of stable node ids, tree positions, typed content, attributes,
//! block metadata and rich-text runs. That is the state a restore acts on and
//! the state every future view projects; a text export would discard it.

use std::collections::{HashMap, HashSet};

use koru_core::{Content, Node, NodeId, Parent, TextRun, WorkspaceDoc};
use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug)]
pub enum Error {
    Model(koru_core::StoreError),
    UnsupportedSchema { found: i64, understood: i64 },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Model(e) => write!(f, "{e}"),
            Self::UnsupportedSchema { found, understood } => write!(
                f,
                "document schema {found} is newer than the understood schema {understood}"
            ),
        }
    }
}

impl std::error::Error for Error {}

impl From<koru_core::StoreError> for Error {
    fn from(value: koru_core::StoreError) -> Self {
        Self::Model(value)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub mod snapshot_diff;
pub mod unified;

/// Everything about one block which is meaningful outside a renderer.
#[derive(Clone, PartialEq, Debug)]
pub struct SemanticNode {
    pub node: Node,
    pub parent: Parent,
    pub ordinal: usize,
    pub depth: usize,
    pub runs: Vec<TextRun>,
}

/// A read-only, model-validated state suitable for diffing on a worker.
#[derive(Clone, Debug)]
pub struct SemanticDocument {
    pub schema_version: Option<i64>,
    pub nodes: Vec<SemanticNode>,
}

impl SemanticDocument {
    pub fn from_snapshot(snapshot: &[u8]) -> Result<Self> {
        let doc = WorkspaceDoc::from_snapshot(snapshot)?;
        Self::read(&doc)
    }

    pub fn read(doc: &WorkspaceDoc) -> Result<Self> {
        if let koru_core::schema::Compatibility::TooNew { found, understood } = doc.compatibility()
        {
            return Err(Error::UnsupportedSchema { found, understood });
        }
        let mut nodes = Vec::new();
        for (id, depth) in doc.walk_all()? {
            let node = doc.read(id)?;
            let parent = doc.parent(id)?;
            let ordinal = doc.sibling_index(id)?;
            let runs = match node.content {
                Content::Text { .. } => doc.runs(id)?,
                _ => Vec::new(),
            };
            nodes.push(SemanticNode {
                node,
                parent,
                ordinal,
                depth,
                runs,
            });
        }
        Ok(Self {
            schema_version: doc.schema_version(),
            nodes,
        })
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum DiffTag {
    Equal,
    Insert,
    Delete,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct TextSpan {
    pub tag: DiffTag,
    pub text: String,
}

#[derive(Clone, PartialEq, Debug)]
pub struct NodeModification {
    pub before: SemanticNode,
    pub after: SemanticNode,
    pub kind_changed: bool,
    pub content_changed: bool,
    pub metadata_changed: bool,
    pub marks_changed: bool,
    pub text: Vec<TextSpan>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum NodeChange {
    Added(SemanticNode),
    Removed(SemanticNode),
    Moved {
        before: SemanticNode,
        after: SemanticNode,
    },
    Reordered {
        before: SemanticNode,
        after: SemanticNode,
    },
    Modified(NodeModification),
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct DocumentDiff {
    /// Changes in the destination document's preorder, followed by removals in
    /// the source order. Rendering state is deliberately absent.
    pub changes: Vec<NodeChange>,
    pub unchanged: usize,
}

pub fn compare(before: &SemanticDocument, after: &SemanticDocument) -> DocumentDiff {
    let old: HashMap<NodeId, &SemanticNode> = before.nodes.iter().map(|n| (n.node.id, n)).collect();
    let new: HashMap<NodeId, &SemanticNode> = after.nodes.iter().map(|n| (n.node.id, n)).collect();
    let reordered = truly_reordered(after, &old);
    let mut seen = HashSet::new();
    let mut out = DocumentDiff::default();

    for after_node in &after.nodes {
        let id = after_node.node.id;
        seen.insert(id);
        let Some(before_node) = old.get(&id).copied() else {
            out.changes.push(NodeChange::Added(after_node.clone()));
            continue;
        };
        if before_node.parent != after_node.parent {
            out.changes.push(NodeChange::Moved {
                before: before_node.clone(),
                after: after_node.clone(),
            });
            continue;
        }
        // An edit outranks a shuffle: a block that was both edited and
        // reordered reads as Modified at its new position, because the words
        // (or a task's checkbox) are what the person changed — a Reordered
        // marker would hide them. Reordered is only ever a pure reorder.
        if before_node.node == after_node.node && before_node.runs == after_node.runs {
            if reordered.contains(&id) {
                out.changes.push(NodeChange::Reordered {
                    before: before_node.clone(),
                    after: after_node.clone(),
                });
            } else {
                out.unchanged += 1;
            }
            continue;
        }
        let before_text = before_node.node.content.as_text().unwrap_or("");
        let after_text = after_node.node.content.as_text().unwrap_or("");
        out.changes.push(NodeChange::Modified(NodeModification {
            kind_changed: before_node.node.kind != after_node.node.kind,
            content_changed: before_node.node.content != after_node.node.content,
            metadata_changed: before_node.node.meta != after_node.node.meta,
            marks_changed: before_node.runs != after_node.runs,
            text: text_diff(before_text, after_text),
            before: before_node.clone(),
            after: after_node.clone(),
        }));
    }
    for before_node in &before.nodes {
        if !seen.contains(&before_node.node.id) && !new.contains_key(&before_node.node.id) {
            out.changes.push(NodeChange::Removed(before_node.clone()));
        }
    }
    out
}

/// The nodes a person actually reordered, as opposed to the ones whose
/// *position* shifted because a sibling appeared or disappeared above them.
///
/// An ordinal is a sibling index, so comparing ordinals directly would call
/// every block below one deletion "reordered". Instead, per parent, the common
/// children are read in the new sibling order and the longest increasing
/// subsequence of their old positions is kept as "still in order"; only the
/// nodes off it are reorders — the minimal set, which is also the set a person
/// would name ("I moved this one up").
fn truly_reordered(
    after: &SemanticDocument,
    old: &HashMap<NodeId, &SemanticNode>,
) -> HashSet<NodeId> {
    // Key by the parent node (None for the root); grouping only nodes whose
    // parent survived unchanged — a changed parent is a Move, not a reorder.
    let mut groups: HashMap<Option<NodeId>, Vec<(usize, usize, NodeId)>> = HashMap::new();
    for after_node in &after.nodes {
        let Some(before_node) = old.get(&after_node.node.id) else {
            continue;
        };
        if before_node.parent != after_node.parent {
            continue;
        }
        let key = match after_node.parent {
            Parent::Root => None,
            Parent::Node(parent) => Some(parent),
        };
        groups.entry(key).or_default().push((
            after_node.ordinal,
            before_node.ordinal,
            after_node.node.id,
        ));
    }
    let mut out = HashSet::new();
    for mut members in groups.into_values() {
        members.sort_unstable_by_key(|&(after_ordinal, _, _)| after_ordinal);
        let old_positions: Vec<usize> = members.iter().map(|&(_, o, _)| o).collect();
        for (index, kept) in longest_increasing(&old_positions).into_iter().enumerate() {
            if !kept {
                out.insert(members[index].2);
            }
        }
    }
    out
}

/// Marks the members of one longest strictly-increasing subsequence.
/// Patience: `tails[k]` is the index of the smallest tail of any increasing
/// subsequence of length `k + 1`; back-links reconstruct the winning chain.
fn longest_increasing(seq: &[usize]) -> Vec<bool> {
    let mut kept = vec![false; seq.len()];
    let mut tails: Vec<usize> = Vec::new();
    let mut prev: Vec<Option<usize>> = vec![None; seq.len()];
    for i in 0..seq.len() {
        let pos = tails.partition_point(|&t| seq[t] < seq[i]);
        if pos > 0 {
            prev[i] = Some(tails[pos - 1]);
        }
        if pos == tails.len() {
            tails.push(i);
        } else {
            tails[pos] = i;
        }
    }
    let mut cursor = tails.last().copied();
    while let Some(i) = cursor {
        kept[i] = true;
        cursor = prev[i];
    }
    kept
}

/// Unicode word-boundary diff. Grapheme boundaries are preserved because each
/// token is built from `split_word_bounds`, never byte or scalar slicing.
pub fn text_diff(before: &str, after: &str) -> Vec<TextSpan> {
    let old: Vec<&str> = UnicodeSegmentation::split_word_bounds(before).collect();
    let new: Vec<&str> = UnicodeSegmentation::split_word_bounds(after).collect();
    TextDiff::from_slices(&old, &new)
        .iter_all_changes()
        .map(|change| TextSpan {
            tag: match change.tag() {
                ChangeTag::Equal => DiffTag::Equal,
                ChangeTag::Insert => DiffTag::Insert,
                ChangeTag::Delete => DiffTag::Delete,
            },
            text: change.value().to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_text_is_never_split_inside_a_grapheme() {
        let diff = text_diff("안녕하세요 👩🏽‍💻", "안녕하세요 반가워요 👩🏽‍💻");
        assert_eq!(
            diff.iter().map(|p| p.text.as_str()).collect::<String>(),
            "안녕하세요 반가워요 👩🏽‍💻"
        );
        assert!(diff.iter().any(|p| p.tag == DiffTag::Insert));
    }

    #[test]
    fn rtl_text_gets_semantic_insertions() {
        let diff = text_diff("مرحبا بالعالم", "مرحبا بك بالعالم");
        assert!(
            diff.iter()
                .any(|p| p.tag == DiffTag::Insert && p.text.contains("بك"))
        );
    }
}
