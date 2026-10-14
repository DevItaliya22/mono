// SPDX-License-Identifier: MPL-2.0

//! Resolving a `task:` reference, and the one answer it may give when the
//! task is gone.

use crate::ids::{DocumentId, EntityRef, NodeId};
use crate::schema::WorkspaceDoc;

use super::is_task_node;

/// **The document segment of every reference minted in this document.**
///
/// One function, because the segment is a fact about the *document* and not
/// about the node being addressed: two tasks on one page carry the same middle
/// segment, and that is the property the whole convention rests on. It used to
/// be `WorkspaceDoc::document_id(node)`, which derives an id per node for
/// anything that is not a document root — so the segment looked like a
/// containing document and was not, and anything that ever grouped by it would
/// have answered per node (TECHDEBT 72, closed by this).
///
/// [`DocumentId::UNIDENTIFIED`] is the answer for a document with no id of its
/// own — a document nothing registered, which in practice means one a test or a
/// headless caller built. Every document that came from the store is stamped
/// with the store's own id when it is opened, so the nil segment is not a shape
/// a shipped reference is minted in.
pub fn document_segment(ws: &WorkspaceDoc) -> DocumentId {
    ws.own_document_id().unwrap_or(DocumentId::UNIDENTIFIED)
}

/// **Mint a `task:` reference for one node of this document.**
///
/// The single spelling: `EntityRef::task(ws.id()?, document_segment(ws), node)`
/// written once, so no caller can mint a reference the other callers cannot
/// match. A burst that never closes is what a second, drifting copy produces.
///
/// `None` when the document carries no workspace identity — the same read every
/// command's own preflight makes first, since a reference missing its first
/// segment is not one. It does **not** ask whether the node is a task: a
/// command mints its subject before it makes one (`ensure_task`), and the
/// callers that do need the question ask [`is_task_node`] themselves.
pub fn task_ref(ws: &WorkspaceDoc, node: NodeId) -> Option<EntityRef> {
    Some(EntityRef::task(ws.id()?, document_segment(ws), node))
}

/// What a `task:` reference resolved to.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum TaskRefResolution {
    Found(NodeId),
    /// The node does not resolve, or resolves and is not a task.
    NotFound,
}

/// Resolve a `task:` reference **in this document**.
///
/// It checks [`is_task_node`]: a node that stopped being a task answers
/// `NotFound`, exactly as a node that never existed does. **No "removed"
/// state, no existence leak, and no fallback navigation to the underlying
/// content** — a reference that named a task and now names a paragraph must not
/// quietly open the paragraph, because the thing the person asked for is gone
/// and saying so is the honest answer. A UI may phrase it as "this task doesn't
/// exist or was removed"; the domain says one word.
///
/// A reference of another kind — `block:`, `document:` — is `NotFound` too:
/// the kind is preserved, so a block reference is not a spelling of the task.
/// So is a reference into another workspace, which this document cannot answer
/// for. The *document* segment is deliberately not compared: whoever opened
/// this document did so because the reference named it, and a `NodeId` is
/// document-scoped anyway.
pub fn resolve_task_ref(ws: &WorkspaceDoc, r: &EntityRef) -> TaskRefResolution {
    let EntityRef::Task {
        workspace, node, ..
    } = r
    else {
        return TaskRefResolution::NotFound;
    };
    if ws.id().is_some_and(|here| here != *workspace) {
        return TaskRefResolution::NotFound;
    }
    match is_task_node(ws, *node) {
        Ok(true) => TaskRefResolution::Found(*node),
        _ => TaskRefResolution::NotFound,
    }
}
