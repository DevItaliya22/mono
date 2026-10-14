// SPDX-License-Identifier: MPL-2.0

//! The five questions this crate asks a store, and the store's own answers.
//!
//! **Nothing here is a new seam.** A capture is `capture_history` — the
//! same immutable, checksummed version the History panel, the AI chat and
//! the MCP tools read — a read-back is `document_version`, the listing is
//! `documents`, and a blob is the workspace's content-addressed store
//! verified against its name. The port exists so the worker can be handed
//! any `Store` and the tests can open a real one under a temporary root;
//! it is not a place to put behaviour.
//!
//! Every method drops the store's error *message* at the boundary
//! ([`Fault::from_store`]) — a quarantine path in a blob error must never
//! become part of a run's failure.

use koru_delegate_core::{BlobHash, DocumentId, DocumentVersionId, WorkspaceId};
use koru_store_core::model::{Blob, DocumentEntry, Timestamp};
use koru_store_core::ports::{Catalog, DocumentVersion, HistoryReason};
use koru_store_core::service::Backend;
use koru_store_core::service::Store;

use crate::Fault;

/// What §7.2 needs from a store, in the store's own vocabulary.
pub trait ContextStore {
    /// Commit an immutable version of `document` at its live frontier and
    /// return its id. Deduplicated by the store: capturing an unchanged
    /// document twice pins the same version.
    fn capture_version(
        &mut self,
        workspace: WorkspaceId,
        document: DocumentId,
        at: Timestamp,
    ) -> Result<DocumentVersionId, Fault>;

    /// The version's bytes, checksum-verified. `None` when the workspace
    /// holds no such version — never captured here, or purged.
    fn version(
        &mut self,
        workspace: WorkspaceId,
        version: DocumentVersionId,
    ) -> Result<Option<DocumentVersion>, Fault>;

    /// Every entry the workspace lists, trashed ones included, as the
    /// store's derived index reports them.
    fn entries(&mut self, workspace: WorkspaceId) -> Result<Vec<DocumentEntry>, Fault>;

    /// The blob's record — size, media type, the person's filename as
    /// metadata — or `None` when the workspace has never held it.
    fn blob(&mut self, workspace: WorkspaceId, hash: &BlobHash) -> Result<Option<Blob>, Fault>;

    /// The blob's bytes, verified against the name they are filed under.
    fn blob_bytes(&mut self, workspace: WorkspaceId, hash: &BlobHash) -> Result<Vec<u8>, Fault>;
}

impl<C: Catalog, B: Backend> ContextStore for Store<C, B> {
    fn capture_version(
        &mut self,
        workspace: WorkspaceId,
        document: DocumentId,
        at: Timestamp,
    ) -> Result<DocumentVersionId, Fault> {
        // `Manual` because a person delegating *is* asking for this moment
        // to be kept, and it is the one reason the history panel shows as
        // pinned. No label and no message: both are free text a person
        // would read, and a fixed English string is not localisation. A
        // `Delegation` reason of its own is deferred (docs/deferred/).
        let entry = Store::capture_history(
            self,
            workspace,
            document,
            None,
            HistoryReason::Manual,
            at,
            None,
            None,
            None,
        )
        .map_err(|e| Fault::from_store(&e))?;
        Ok(entry.version)
    }

    fn version(
        &mut self,
        workspace: WorkspaceId,
        version: DocumentVersionId,
    ) -> Result<Option<DocumentVersion>, Fault> {
        Store::document_version(self, workspace, version).map_err(|e| {
            // A version that fails its checksum is reported by the store as
            // corrupt state; here that is a fact about *this* version.
            if e.code == koru_store_core::errors::Code::CorruptState {
                Fault::VersionCorrupt
            } else {
                Fault::from_store(&e)
            }
        })
    }

    fn entries(&mut self, workspace: WorkspaceId) -> Result<Vec<DocumentEntry>, Fault> {
        Store::documents(self, workspace).map_err(|e| Fault::from_store(&e))
    }

    fn blob(&mut self, workspace: WorkspaceId, hash: &BlobHash) -> Result<Option<Blob>, Fault> {
        match Store::blob(self, workspace, hash) {
            Ok(blob) => Ok(Some(blob)),
            Err(e) if e.code == koru_store_core::errors::Code::BlobNotFound => Ok(None),
            Err(e) => Err(Fault::from_store(&e)),
        }
    }

    fn blob_bytes(&mut self, workspace: WorkspaceId, hash: &BlobHash) -> Result<Vec<u8>, Fault> {
        Store::get_blob(self, workspace, hash).map_err(|e| Fault::from_store(&e))
    }
}
