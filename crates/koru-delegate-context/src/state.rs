// SPDX-License-Identifier: MPL-2.0

//! What a pin points at *now* — live, in trash, or no longer available
//! (D46, D47).
//!
//! The run keeps naming exactly what it received; this reads the target's
//! current state beside that pin, from what the store reports at read
//! time, and writes nothing. Trash is inherited from an ancestor here the
//! way the manifest's own `is_trashed` inherits it, over the listing the
//! store already returns — a pure function, so the state of a pin can be
//! asked as often as a surface repaints without a store round trip per
//! ancestor.

use koru_delegate_core::{ContextState, DocumentId, RunContextItem};
use koru_store_core::model::DocumentEntry;

use crate::{ContextPin, ContextStore, Fault, StoreContext};

/// The state of one document given the workspace's listing and whether
/// its pinned version can still be read. Pure: no store, no clock.
///
/// Trash inherits: a document under a trashed ancestor is in the trash
/// even with no stamp of its own. The walk is bounded by the listing's
/// length, so a parent chain that somehow cycles cannot spin.
pub fn document_state(
    entries: &[DocumentEntry],
    document: DocumentId,
    version_retained: bool,
) -> ContextState {
    if !version_retained {
        return ContextState::Unavailable;
    }
    let mut at = Some(document);
    for _ in 0..entries.len().saturating_add(1) {
        let Some(current) = at.and_then(|id| entries.iter().find(|entry| entry.id == id)) else {
            // The pinned document itself is unlisted: purged, or never
            // here. An unlisted *ancestor* ends the walk as not trashed —
            // the document is listed, and that is what a listing shows.
            return if at == Some(document) {
                ContextState::Unavailable
            } else {
                ContextState::Live
            };
        };
        if current.trashed_at.is_some() {
            return ContextState::InTrash;
        }
        at = current.parent;
    }
    ContextState::Live
}

/// The state of a blob: blobs have no trash, so they are retained or not.
pub fn blob_state(retained: bool) -> ContextState {
    if retained {
        ContextState::Live
    } else {
        ContextState::Unavailable
    }
}

impl<S: ContextStore> StoreContext<'_, S> {
    /// The current state of every pin, in order. A workspace that will not
    /// open makes each of its pins `Unavailable` rather than an error: the
    /// question was "what does this point at", and the answer is "nothing
    /// reachable".
    pub fn states(&mut self, pins: &[ContextPin]) -> Result<Vec<ContextState>, Fault> {
        pins.iter().map(|pin| self.state(pin)).collect()
    }

    pub fn state(&mut self, pin: &ContextPin) -> Result<ContextState, Fault> {
        match &pin.item {
            RunContextItem::Document {
                workspace,
                document,
                version,
            }
            | RunContextItem::Block {
                workspace,
                document,
                version,
                ..
            } => {
                let entries = match self.store.entries(*workspace) {
                    Ok(entries) => entries,
                    Err(Fault::WorkspaceUnavailable) => return Ok(ContextState::Unavailable),
                    Err(other) => return Err(other),
                };
                let retained = match self.store.version(*workspace, *version) {
                    Ok(Some(stored)) => stored.document == *document,
                    Ok(None) | Err(Fault::VersionCorrupt) | Err(Fault::VersionUnavailable) => false,
                    Err(other) => return Err(other),
                };
                Ok(document_state(&entries, *document, retained))
            }
            RunContextItem::Blob {
                workspace, blob, ..
            } => {
                let retained = match self.store.blob(*workspace, blob) {
                    Ok(Some(_)) => match self.store.blob_bytes(*workspace, blob) {
                        Ok(_) => true,
                        Err(Fault::BlobUnavailable) | Err(Fault::BlobCorrupt) => false,
                        Err(other) => return Err(other),
                    },
                    Ok(None) | Err(Fault::WorkspaceUnavailable) => false,
                    Err(other) => return Err(other),
                };
                Ok(blob_state(retained))
            }
        }
    }
}
