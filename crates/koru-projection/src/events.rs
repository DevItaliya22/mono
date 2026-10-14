// SPDX-License-Identifier: MPL-2.0

//! Semantic attribute events — what changed, in the product's vocabulary.
//!
//! **Derived at the projection step, never recorded by the editor** (roadmap
//! D10). Every attribute lives on a node and every node change reaches this
//! layer as a re-projection — the *all attribute and track state flows through
//! documents* invariant — so a diff of the projection's own attr rows sees
//! every change there is, including one made offline on another peer and one
//! made by a build that never heard of events.
//!
//! **Events are triggers, never truth** (the crate's standing rule), and these
//! are no exception. A consumer acts on what an event *names* by reading state,
//! not by folding event payloads into a mirror: a projection rebuilt from
//! scratch (schema bump, deleted `projection.db`) re-emits `Set` for every
//! attribute it meets again, which is the mechanism working — a consumer that
//! kept its own record deduplicates against it, and one that did not needed
//! the event anyway. Nothing here is persisted; the vector lives for one
//! consumer call and is gone.
//!
//! The vocabulary is D10's: the semantic names — `tag.added`, `tag.removed`,
//! `due.changed`, `priority.changed`, `color.changed` — for the system keys a
//! consumer subscribes to by meaning, and the generic `attribute.changed` for
//! every other key. Values travel in the projection's own typed terms
//! ([`ProjectedAttr`]'s `kind` + canonical string), so a consumer of events
//! links no node model, exactly like a consumer of rows.

use std::collections::{BTreeMap, BTreeSet};

use koru_core::attributes::{KEY_COLOR, KEY_DUE, KEY_PRIORITY, KEY_TAG};
use koru_store_core::ids::{DocumentId, WorkspaceId};

use crate::model::{ProjectedAttr, ProjectedNode};

/// One semantic attribute change on one entity.
///
/// The entity is the triple, together — a `node` id means nothing outside its
/// document, and a document means nothing without its workspace once several
/// are in scope. Never an offset, a line number or a tree position.
#[derive(Clone, Debug, PartialEq)]
pub struct AttributeEvent {
    pub workspace: WorkspaceId,
    pub document: DocumentId,
    /// The node's stable id, as text — the projection's own coordinate.
    pub node: String,
    pub kind: AttributeEventKind,
}

/// Which named event this is — D10's vocabulary as a type, not a string.
///
/// The system keys with stable semantics get their own arms so a consumer
/// matches on meaning; everything else — custom attributes included — is the
/// generic [`AttributeChanged`](Self::AttributeChanged), which carries the
/// key. [`name`](Self::name) renders the wire token when one is needed for a
/// log or a subscription filter.
#[derive(Clone, Debug, PartialEq)]
pub enum AttributeEventKind {
    /// `tag.added` — the [`TagId`] in storage form, which is what the node
    /// stores and the attrs table carries. The tag's *name* lives in the
    /// workspace system document and deliberately does not travel here: a
    /// name is user content, and resolving it is a reader's business.
    ///
    /// [`TagId`]: koru_core::ids::TagId
    TagAdded { tag: String },
    /// `tag.removed` — the [`TagId`](koru_core::ids::TagId) in storage form.
    TagRemoved { tag: String },
    /// `due.changed` — `koru.due` was set, moved or cleared.
    DueChanged { change: AttributeChange },
    /// `priority.changed` — `koru.priority` was set, moved or cleared.
    PriorityChanged { change: AttributeChange },
    /// `color.changed` — `koru.color` was set, moved or cleared.
    ColorChanged { change: AttributeChange },
    /// `attribute.changed` — any other key, custom attributes included.
    AttributeChanged {
        key: String,
        change: AttributeChange,
    },
}

impl AttributeEventKind {
    /// The D10 vocabulary token, for logs and subscription filters. The type
    /// is the API; this is its spelling.
    pub fn name(&self) -> &'static str {
        match self {
            Self::TagAdded { .. } => "tag.added",
            Self::TagRemoved { .. } => "tag.removed",
            Self::DueChanged { .. } => "due.changed",
            Self::PriorityChanged { .. } => "priority.changed",
            Self::ColorChanged { .. } => "color.changed",
            Self::AttributeChanged { .. } => "attribute.changed",
        }
    }

    /// The attribute key the event is about — `koru.due`, `koru.tag`, or the
    /// generic arm's own.
    pub fn key(&self) -> &str {
        match self {
            Self::TagAdded { .. } | Self::TagRemoved { .. } => KEY_TAG,
            Self::DueChanged { .. } => KEY_DUE,
            Self::PriorityChanged { .. } => KEY_PRIORITY,
            Self::ColorChanged { .. } => KEY_COLOR,
            Self::AttributeChanged { key, .. } => key,
        }
    }
}

/// What happened to a single-valued key.
///
/// `Set` includes **first sight**: a document this projection has never held
/// rows for emits `Set` for every attribute it arrives with, deliberately —
/// a consumer that subscribed after the attribute was written still needs the
/// trigger, and the reconcile path re-projects exactly such documents. The
/// cost is that a rebuilt read model re-emits them, which the module note says
/// a consumer must tolerate anyway.
#[derive(Clone, Debug, PartialEq)]
pub enum AttributeChange {
    /// The key had no value and has one now.
    Set { value: AttributeEventValue },
    /// The key had a value and has a different one now.
    Changed { value: AttributeEventValue },
    /// The key had a value and no longer does — including the whole document
    /// being dropped.
    Cleared,
}

/// A value in the projection's typed terms — the attrs table's `kind` token
/// (`date`, `datetime`, `number`, `bool`, `ref`, `url`, `text`) and the
/// canonical string it stores under it. The tag and the value apart, the same
/// reason [`ProjectedAttr`] keeps them apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttributeEventValue {
    pub kind: String,
    pub value: String,
}

impl AttributeEventValue {
    fn of(attr: &ProjectedAttr) -> Self {
        Self {
            kind: attr.kind.clone(),
            value: attr.value.clone(),
        }
    }
}

/// Diff a document's previous attr rows against the nodes just projected.
///
/// `old` is the projection's own rows before [`replace`] swapped them — one
/// query's worth, `(node id, attr)` pairs. `new` is the projection about to
/// stand. O(this document's attrs) and nothing else: no other document is
/// read, no table is scanned.
///
/// Two shapes of key, two rules:
///
/// - **`koru.tag` is a set.** One attr per tag is what makes concurrent adds
///   merge to the union (ADR-176), so the diff is set difference — each new
///   value is a [`TagAdded`], each vanished one a [`TagRemoved`], and a tag
///   present on both sides says nothing.
/// - **Everything else is single-valued**, and the effective value is the
///   first row under the key — the projection writes attrs in `read_attrs`'s
///   deterministic `(role, id)` order, so between convergence-duplicate folds
///   the first row is the same fold winner every reader sees. Only the
///   `(kind, value)` pair is compared: a label is display text, and a
///   label-only edit is not a semantic change.
///
/// Events come out ordered by `(node id, key)` — deterministic, not
/// document-ordered, because `old` may name nodes the new projection no
/// longer holds.
///
/// [`replace`]: crate::store::Projections::replace
/// [`TagAdded`]: AttributeEventKind::TagAdded
/// [`TagRemoved`]: AttributeEventKind::TagRemoved
pub fn diff(
    workspace: WorkspaceId,
    document: DocumentId,
    old: &[(String, ProjectedAttr)],
    new: &[ProjectedNode],
) -> Vec<AttributeEvent> {
    let mut before: BTreeMap<(&str, &str), Vec<&ProjectedAttr>> = BTreeMap::new();
    for (node, attr) in old {
        before
            .entry((node.as_str(), attr.role.as_str()))
            .or_default()
            .push(attr);
    }
    let mut after: BTreeMap<(&str, &str), Vec<&ProjectedAttr>> = BTreeMap::new();
    // **A binned node holds no live attribute.** `old` comes from
    // `document_attrs`, which is live-only, so the new side has to be too or
    // every projection of a document with a bin in it would announce the same
    // due dates again. Binning therefore reads as `Cleared` and restoring as
    // `Set`, which is what a consumer tracking "everything due this week"
    // needs told — and is what it was told when binning deleted the rows.
    for node in new.iter().filter(|node| !node.in_trash) {
        for attr in &node.attrs {
            after
                .entry((node.id.as_str(), attr.role.as_str()))
                .or_default()
                .push(attr);
        }
    }

    let keys: BTreeSet<(&str, &str)> = before.keys().chain(after.keys()).copied().collect();
    let mut out = Vec::new();
    for (node, role) in keys {
        let was = before.get(&(node, role)).map(Vec::as_slice).unwrap_or(&[]);
        let now = after.get(&(node, role)).map(Vec::as_slice).unwrap_or(&[]);
        let mut emit = |kind: AttributeEventKind| {
            out.push(AttributeEvent {
                workspace,
                document,
                node: node.to_owned(),
                kind,
            });
        };
        if role == KEY_TAG {
            for attr in now {
                if !was.iter().any(|b| b.value == attr.value) {
                    emit(AttributeEventKind::TagAdded {
                        tag: attr.value.clone(),
                    });
                }
            }
            for attr in was {
                if !now.iter().any(|a| a.value == attr.value) {
                    emit(AttributeEventKind::TagRemoved {
                        tag: attr.value.clone(),
                    });
                }
            }
            continue;
        }
        let was = was.first().map(|a| AttributeEventValue::of(a));
        let now = now.first().map(|a| AttributeEventValue::of(a));
        let change = match (was, now) {
            (None, Some(value)) => AttributeChange::Set { value },
            (Some(b), Some(value)) if b != value => AttributeChange::Changed { value },
            (Some(_), None) => AttributeChange::Cleared,
            _ => continue,
        };
        emit(match role {
            KEY_DUE => AttributeEventKind::DueChanged { change },
            KEY_PRIORITY => AttributeEventKind::PriorityChanged { change },
            KEY_COLOR => AttributeEventKind::ColorChanged { change },
            _ => AttributeEventKind::AttributeChanged {
                key: role.to_owned(),
                change,
            },
        });
    }
    out
}

/// The events for a document that was **dropped** — every known attr, cleared.
///
/// A drop is a removal, not a bin: the rows are about to go, so a consumer
/// tracking "everything due this week" must be told each due is gone, or it
/// keeps a trigger for an entity nothing can resolve. Tags come out as
/// [`TagRemoved`](AttributeEventKind::TagRemoved), everything else as
/// [`Cleared`](AttributeChange::Cleared) — the same shapes an edit produces,
/// so a consumer has one vocabulary rather than a removal special case.
pub fn cleared(
    workspace: WorkspaceId,
    document: DocumentId,
    old: &[(String, ProjectedAttr)],
) -> Vec<AttributeEvent> {
    diff(workspace, document, old, &[])
}

#[cfg(test)]
mod tests;
