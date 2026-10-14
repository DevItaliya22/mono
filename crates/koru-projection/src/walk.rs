// SPDX-License-Identifier: MPL-2.0

//! Turning one document into its read model.
//!
//! **The only place above the store that knows what a Koru node is.** Everything
//! it produces is strings and primitives ([`crate::model`]), so this file is
//! where the node model stops and every consumer's world begins.
//!
//! Two calls per node and both are O(that node):
//!
//! - `read` gives the kind, the meta and the typed attributes,
//! - `runs` gives the text **and** its formatting and pill anchors.
//!
//! Do not take text from `read` and marks from `runs`. One call is
//! authoritative for both, and using two invites them to disagree about a
//! document that changed underneath — which cannot happen here, because the
//! caller hands us a fork, but would be a latent trap for anyone who copied the
//! shape.

use koru_core::node::{AttrValue, Content, Kind};
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_core::{NodeId, TextRun};

use koru_store_core::ids::{DocumentId, WorkspaceId};

use crate::model::{DocumentProjection, ProjectedAttr, ProjectedNode, ProjectedRun};

/// Walk a document into its projection.
///
/// `revision` is the store revision this document was read at — the caller's to
/// supply, because only the caller knows which commit it forked for.
pub fn project(
    ws: &WorkspaceDoc,
    workspace: WorkspaceId,
    document: DocumentId,
    revision: Vec<u8>,
) -> Result<DocumentProjection, String> {
    // **`walk_with_trash`, and this is the one walk in the tree that takes
    // it.** `walk`/`walk_all` filter the hidden trash root out through
    // `children(Parent::Root)`, which is right for anything that *draws* a
    // document and wrong here: a binned node keeps its text, its marks and its
    // pills, and the bin is a place somebody searches and restores from. A
    // projection that omitted it would make restore hand back content no
    // consumer had ever heard of.
    //
    // **Flagged, never hidden** — every row this builds from the bin carries
    // `in_trash`, and it is the *queries* over those rows that decide who sees
    // them (`store.rs`, "two notions of trash").
    //
    // This comment used to say `walk_all` did all of that. It did not:
    // `walk_all` is an alias for the filtered walk, so the bin was never
    // projected at all and the claim had been false since it was written.
    //
    // **Every id here is distinct**, which is what lets these rows be written
    // under a `(document_id, node_id)` key with no check of their own: a repeat
    // failed the insert and cost the document its projection for good, because
    // the walk reads state and every retry reproduced it exactly.
    //
    // The walk keeps that promise itself. The *checked* form is taken because
    // this is the one reader of a document that has a logger and its id, and a
    // tree handing back the same node twice is worth a sentence somebody can
    // act on rather than a silence that only shows up as missing content.
    let (order, repeats) = ws.walk_with_trash_checked().map_err(|e| e.to_string())?;
    for node in &repeats {
        tracing::warn!(%document, %node, "the walk returned this node more than once");
    }

    let mut nodes = Vec::with_capacity(order.len());
    // **The bin is carried down the walk, not asked per node.**
    // `WorkspaceDoc::in_trash` is an `ancestors` climb, so asking it once per
    // node would make a projection O(nodes × depth) to answer a question the
    // depth-first order already contains. `walk_with_trash` yields parents
    // before children, so the depth a trash root was met at is the whole of
    // the state: everything deeper is inside it, and the first node at that
    // depth or shallower has left it.
    let mut trash_from: Option<usize> = None;
    for (id, depth) in order {
        if trash_from.is_some_and(|start| depth <= start) {
            trash_from = None;
        }
        match one(ws, id, depth) {
            Ok(mut node) => {
                if trash_from.is_none() && node.kind == Kind::Trash.as_str() {
                    trash_from = Some(depth);
                }
                node.in_trash = trash_from.is_some();
                nodes.push(node);
            }
            // **A node that will not read is skipped, loudly, not fatal.** One
            // unreadable node must not cost the other nine hundred their
            // projection — and a document that refuses to project at all is a
            // document that silently disappears from search.
            Err(why) => tracing::warn!(%document, node = %id, "skipping a node: {why}"),
        }
    }

    Ok(DocumentProjection {
        workspace,
        document,
        source_revision: revision,
        nodes,
    })
}

fn one(ws: &WorkspaceDoc, id: NodeId, depth: usize) -> Result<ProjectedNode, String> {
    let node = ws.read(id).map_err(|e| e.to_string())?;
    let meta = &node.meta;

    let (parent, ordinal) = position(ws, id);
    // **Empty for anything that is not text**, and that is not a failure: an
    // image has no text handle, so asking for one errors, and the node is still
    // a node worth projecting. Its label is `name`.
    let runs = ws.runs(id).unwrap_or_default();
    let (blob, mime) = match &node.content {
        Content::Blob { hash, mime, .. } => (Some(hash.0.clone()), Some(mime.clone())),
        _ => (None, None),
    };
    let embed = match &node.content {
        Content::Embed { target } => Some(target.node.to_storage()),
        _ => None,
    };

    Ok(ProjectedNode {
        id: id.to_storage(),
        parent: parent.map(|p| p.to_storage()),
        ordinal,
        depth,
        kind: node.kind.as_str().to_owned(),
        // From the runs, which carry it — one traversal rather than two.
        text: runs.iter().map(|r| r.text.as_str()).collect(),
        heading: meta.heading.map(|h| h.level()),
        list_style: meta.list_style.map(|s| s.as_str().to_owned()),
        done: meta.done,
        level_name: meta.level_name.clone(),
        name: meta.name.clone(),
        icon: meta.icon.clone(),
        colour: meta.colour.clone(),
        language: meta.language.clone(),
        blob,
        mime,
        embed,
        // Set by the caller, which is the only place that knows: the answer is
        // a fact about where this node sits in the walk, not about the node.
        in_trash: false,
        attrs: attrs(&node.content),
        runs: runs.iter().map(run).collect(),
    })
}

/// Where a node sits among its siblings.
///
/// Asked of the parent rather than remembered while walking, because
/// `walk_all` yields depth and not ordinal — and a consumer shaping a tree needs
/// the order a person sees.
fn position(ws: &WorkspaceDoc, id: NodeId) -> (Option<NodeId>, usize) {
    let parent = ws.parent(id).unwrap_or(Parent::Root);
    let siblings = ws.children(parent).unwrap_or_default();
    let ordinal = siblings.iter().position(|&s| s == id).unwrap_or(0);
    let parent = match parent {
        Parent::Node(p) => Some(p),
        Parent::Root => None,
    };
    (parent, ordinal)
}

fn run(r: &TextRun) -> ProjectedRun {
    ProjectedRun {
        start: r.start.0,
        end: r.end.0,
        bold: r.bold,
        italic: r.italic,
        underline: r.underline,
        strike: r.strike,
        code: r.code,
        highlight: r.style.highlight.clone(),
        script: r.style.script.map(|s| s.as_str().to_owned()),
        pill: r.pill.map(|a| a.to_storage()),
    }
}

fn attrs(content: &Content) -> Vec<ProjectedAttr> {
    content
        .attrs()
        .iter()
        .map(|a| {
            // **The tag and the value, apart.** A consumer asking "every due
            // date this month" must not have to parse every string and hope; it
            // filters on `kind` and reads `value` knowing what it is.
            //
            // **Each temporal type projects under its own kind** ([ADR-183]).
            // There is no `datetime` here that might carry a zone or might
            // not: a calendar day, a floating wall clock and a zoned instant
            // are three tokens, compared three ways, and the zone travels in
            // its own column beside the instant it belongs to rather than
            // being dropped as presentation.
            //
            // [ADR-183]: ../../../docs/decisions/ADR-201-225.md#adr-206-timemodel--koru-has-three-temporal-types-not-one-with-a-nullable-timezone
            let (value, zone) = match &a.value {
                AttrValue::Date(d) => (d.to_storage(), None),
                AttrValue::FloatingDateTime(f) => (f.to_storage(), None),
                // The instant as decimal epoch-ms — what an instant window
                // compares — and the zone it was meant in beside it.
                AttrValue::ZonedDateTime(z) => (
                    z.instant_ms().to_string(),
                    Some(z.zone().as_str().to_owned()),
                ),
                AttrValue::Number(n) => (n.to_string(), None),
                AttrValue::Bool(b) => (b.to_string(), None),
                AttrValue::Document(reference) => (reference.document.to_storage(), None),
                AttrValue::Ref(r) => (r.node.to_storage(), None),
                AttrValue::Url(u) => (u.clone(), None),
                AttrValue::Text(t) => (t.clone(), None),
            };
            ProjectedAttr {
                id: a.id.to_storage(),
                role: a.role.clone(),
                // The storage token itself, so the projection's `kind` and the
                // document's type token can never drift apart.
                kind: a.value.key().to_owned(),
                value,
                zone,
                label: a.label.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
