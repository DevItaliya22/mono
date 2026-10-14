// SPDX-License-Identifier: MPL-2.0

//! A captured version's bytes, rendered to Markdown through the exporter.
//!
//! A version is a Loro snapshot of a `WorkspaceDoc`; the renderer is the
//! one Koru already has (`koru-export-markdown`), fed the **captured**
//! bytes rather than the live document — which is the whole point of UC2.
//! Inline images are fetched from the store *before* rendering, so the
//! exporter's blob source is a map and the store borrow is not held across
//! the render. An image the store cannot serve fails the item: a document
//! delivered without one of its pictures is not the document the person
//! attached. **Only the item's own images are fetched** — a whole document's
//! for a Document item, one block's subtree for a Block item — so a picture
//! that cannot be served fails the items that contain it and no other.

use std::collections::HashMap;

use koru_core::NodeId;
use koru_core::node::{Content, Kind};
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_delegate_core::{BlobHash, BlockNodeId, WorkspaceId};
use koru_export::{BlobSource, ExportError};
use koru_export_markdown::Rendered;

use crate::{ContextStore, Fault};

/// The blobs a document references, read once.
struct Prefetched(HashMap<String, Vec<u8>>);

impl BlobSource for Prefetched {
    fn get(&self, hash: &koru_core::node::BlobHash) -> Result<Vec<u8>, ExportError> {
        self.0
            .get(&hash.0)
            .cloned()
            .ok_or_else(|| ExportError::new("blob was not prefetched"))
    }
}

/// Open a version's bytes as a document.
pub(crate) fn open(snapshot: &[u8]) -> Result<WorkspaceDoc, Fault> {
    WorkspaceDoc::from_snapshot(snapshot).map_err(|_| Fault::SnapshotUnreadable)
}

/// Every blob the whole document references — what a Document item renders.
pub(crate) fn document_blobs(doc: &WorkspaceDoc) -> Result<Vec<String>, Fault> {
    doc.blob_hashes().map_err(|_| Fault::SnapshotUnreadable)
}

/// The blobs one block's own subtree references — what a Block item renders,
/// and nothing beside it.
///
/// **Not the whole document's.** A canvas hands an agent twenty cards as
/// twenty Block items of one document; fetching every picture in that
/// document for each of them was twenty times the reads, and one picture the
/// store could not serve failed every card — including the nineteen that do
/// not contain it. The walk follows [`koru_export_markdown::render_node`]'s
/// own rule: a nested page is a separate document and is not rendered, so its
/// pictures are not fetched either.
pub(crate) fn block_blobs(doc: &WorkspaceDoc, node: BlockNodeId) -> Result<Vec<String>, Fault> {
    let id = NodeId::from_uuid(node.as_uuid());
    let top = doc.read(id).map_err(|_| Fault::NodeUnavailable)?;
    let mut hashes = Vec::new();
    let mut stack = vec![(id, top)];
    while let Some((id, node)) = stack.pop() {
        if let Content::Blob { hash, .. } = &node.content {
            hashes.push(hash.0.clone());
        }
        let children = doc
            .children(Parent::Node(id))
            .map_err(|_| Fault::SnapshotUnreadable)?;
        for child in children {
            let node = doc.read(child).map_err(|_| Fault::SnapshotUnreadable)?;
            if node.kind == Kind::Document {
                continue;
            }
            stack.push((child, node));
        }
    }
    Ok(hashes)
}

/// Fetch the named blobs, each once. A hash the store cannot name or serve
/// is the item's failure.
pub(crate) fn prefetch<S: ContextStore>(
    store: &mut S,
    workspace: WorkspaceId,
    hashes: Vec<String>,
) -> Result<impl BlobSource, Fault> {
    let mut bytes = HashMap::new();
    for hash in hashes {
        if bytes.contains_key(&hash) {
            continue;
        }
        let parsed = BlobHash::parse(&hash).map_err(|_| Fault::BlobUnavailable)?;
        let content = store.blob_bytes(workspace, &parsed)?;
        bytes.insert(hash, content);
    }
    Ok(Prefetched(bytes))
}

/// The whole document, title line included.
pub(crate) fn document(
    doc: &WorkspaceDoc,
    blobs: &dyn BlobSource,
    assets_rel: &str,
) -> Result<Rendered, Fault> {
    koru_export_markdown::render_document(doc, blobs, assets_rel).map_err(|_| Fault::RenderFailed)
}

/// One block and its subtree.
pub(crate) fn block(
    doc: &WorkspaceDoc,
    node: BlockNodeId,
    blobs: &dyn BlobSource,
    assets_rel: &str,
) -> Result<Rendered, Fault> {
    let id = NodeId::from_uuid(node.as_uuid());
    if doc.read(id).is_err() {
        return Err(Fault::NodeUnavailable);
    }
    koru_export_markdown::render_node(doc, id, blobs, assets_rel).map_err(|_| Fault::RenderFailed)
}
