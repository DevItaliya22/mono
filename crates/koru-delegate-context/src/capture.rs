// SPDX-License-Identifier: MPL-2.0

//! Capture — the bytes a run was given become a pin (§7.2 steps 1–2).
//!
//! Two entry points, because a run's context is pinned *before* the request
//! that carries it is durable (§7.1 keys everything on the request, and the
//! request's `context` is "already pinned"):
//!
//! - [`StoreContext::pin`] takes what a person chose — a document, a block,
//!   a file — commits one `capture_history` version for each document the
//!   items name (shared by all of that document's items), asserts each blob
//!   is retained, and returns the pinned [`RunContextItem`]s a
//!   `DelegationRequest` carries.
//! - [`StoreContext::capture`] takes those pinned items back — at the first
//!   attempt, and at every resume, which keeps the original pins (D10) —
//!   and verifies that each still names bytes this store holds. It commits
//!   nothing.
//!
//! A failure names the item by **index** and the stage it failed at, never
//! by title or filename.

use std::collections::HashMap;
use std::fmt;

use koru_delegate_core::{
    BlobHash, BlobKind, BlockNodeId, DocumentId, DocumentVersionId, RunContextItem, RunId,
    WorkspaceId,
};
use koru_store_core::model::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{ContextStore, Fault, SourceRefusal};

/// The versions one [`StoreContext::pin`] call has already captured, by
/// document.
type Captured = HashMap<(WorkspaceId, DocumentId), DocumentVersionId>;

/// What a person chose, before it is pinned. The version is the store's to
/// mint; a caller never names one here.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum ContextSource {
    Document {
        workspace: WorkspaceId,
        document: DocumentId,
    },
    Block {
        workspace: WorkspaceId,
        document: DocumentId,
        node: BlockNodeId,
    },
    Blob {
        workspace: WorkspaceId,
        blob: BlobHash,
        blob_kind: BlobKind,
    },
}

/// Where in §7.2 an item failed.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureStage {
    /// Step 1: the item's workspace opens and the document or blob is one
    /// it knows.
    Resolve,
    /// Step 2, documents and blocks: the `capture_history` commit.
    Capture,
    /// Step 2, everything: the pinned bytes are present and verify.
    Retention,
}

impl CaptureStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resolve => "resolve",
            Self::Capture => "capture",
            Self::Retention => "retention",
        }
    }
}

/// One item could not be pinned or verified. The index is into the slice
/// the caller passed; the stage and fault are tokens.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CaptureFailure {
    pub item: usize,
    pub stage: CaptureStage,
    pub fault: Fault,
}

impl fmt::Display for CaptureFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "context item {} failed at {}: {}",
            self.item,
            self.stage.as_str(),
            self.fault
        )
    }
}

impl std::error::Error for CaptureFailure {}

/// One pinned item, with what its bytes must verify against: a version's
/// snapshot checksum, or the blob's own hash. Ids and digests only.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ContextPin {
    pub item: RunContextItem,
    pub checksum: String,
}

/// Every pin of one run, in the order the items were given.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CapturedContext {
    pub run: RunId,
    /// The run's own workspace, as the caller named it. Items resolve in
    /// the workspace each one carries; this is recorded, not consulted.
    pub workspace: WorkspaceId,
    pub pins: Vec<ContextPin>,
}

/// Why a fresh scheduled run could not turn its live pointers into pins.
///
/// This keeps capture failures and current-state refusals distinct inside
/// the context boundary. A host may present both as one schedule-context
/// failure, but it must not lose the admission check that distinguishes a
/// fresh start from an attempt already under way (D48).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PinLiveFailure {
    Capture(CaptureFailure),
    Refused(SourceRefusal),
}

impl PinLiveFailure {
    pub fn item(self) -> usize {
        match self {
            Self::Capture(failure) => failure.item,
            Self::Refused(refusal) => refusal.item(),
        }
    }
}

/// §7.2 over one store. Holds the store only for as long as one call.
pub struct StoreContext<'a, S: ContextStore> {
    pub(crate) store: &'a mut S,
}

impl<'a, S: ContextStore> StoreContext<'a, S> {
    pub fn new(store: &'a mut S) -> Self {
        Self { store }
    }

    /// Pin what a person chose: commit a version for every document the
    /// sources name, assert every blob is retained, and return the items a
    /// request carries. All or nothing — the first failure is returned and
    /// nothing is undone, because a spare history marker is harmless and a
    /// half-pinned request is not.
    ///
    /// **One version per document per call.** Every Document and Block item
    /// from the same document shares the version its first item captured.
    /// Twenty cards handed over from one canvas are one moment of that
    /// canvas: one `capture_history` commit and one *manual* marker in its
    /// History, not twenty — and every card is read from the same bytes, so
    /// no two of them can disagree about what the canvas said.
    pub fn pin(
        &mut self,
        sources: &[ContextSource],
    ) -> Result<Vec<RunContextItem>, CaptureFailure> {
        let at = Timestamp::now();
        let mut captured = HashMap::new();
        sources
            .iter()
            .enumerate()
            .map(|(item, source)| self.pin_one(item, source, at, &mut captured))
            .collect()
    }

    /// Resolve fresh schedule pointers completely before their run is
    /// claimed. Besides minting immutable pins this verifies the exact
    /// captured bytes (including a block's presence) and requires every
    /// source to be live now. A failure returns no pins; history markers
    /// already committed by [`Self::pin`] are harmless and reusable.
    pub fn pin_live(
        &mut self,
        run: RunId,
        sources: &[ContextSource],
        workspace: WorkspaceId,
    ) -> Result<Vec<RunContextItem>, PinLiveFailure> {
        let items = self.pin(sources).map_err(PinLiveFailure::Capture)?;
        let captured = self
            .capture(run, &items, workspace)
            .map_err(PinLiveFailure::Capture)?;
        self.admit(&captured.pins)
            .map_err(PinLiveFailure::Refused)?;
        Ok(items)
    }

    fn pin_one(
        &mut self,
        item: usize,
        source: &ContextSource,
        at: Timestamp,
        captured: &mut Captured,
    ) -> Result<RunContextItem, CaptureFailure> {
        let fail = |(stage, fault)| CaptureFailure { item, stage, fault };
        match source {
            ContextSource::Document {
                workspace,
                document,
            } => {
                let version = self
                    .version_once(*workspace, *document, at, captured)
                    .map_err(fail)?;
                Ok(RunContextItem::Document {
                    workspace: *workspace,
                    document: *document,
                    version,
                })
            }
            ContextSource::Block {
                workspace,
                document,
                node,
            } => {
                let version = self
                    .version_once(*workspace, *document, at, captured)
                    .map_err(fail)?;
                Ok(RunContextItem::Block {
                    workspace: *workspace,
                    document: *document,
                    version,
                    node: *node,
                })
            }
            ContextSource::Blob {
                workspace,
                blob,
                blob_kind,
            } => {
                self.resolve_blob(*workspace, blob)
                    .map_err(|f| fail((CaptureStage::Resolve, f)))?;
                self.retain_blob(*workspace, blob)
                    .map_err(|f| fail((CaptureStage::Retention, f)))?;
                Ok(RunContextItem::Blob {
                    workspace: *workspace,
                    blob: blob.clone(),
                    blob_kind: *blob_kind,
                })
            }
        }
    }

    /// Resolve and capture `document` the first time this call names it;
    /// every later item from the same document gets the same version.
    fn version_once(
        &mut self,
        workspace: WorkspaceId,
        document: DocumentId,
        at: Timestamp,
        captured: &mut Captured,
    ) -> Result<DocumentVersionId, (CaptureStage, Fault)> {
        if let Some(version) = captured.get(&(workspace, document)) {
            return Ok(*version);
        }
        self.resolve_document(workspace, document)
            .map_err(|f| (CaptureStage::Resolve, f))?;
        let version = self
            .store
            .capture_version(workspace, document, at)
            .map_err(|f| (CaptureStage::Capture, f))?;
        captured.insert((workspace, document), version);
        Ok(version)
    }

    /// Verify every pinned item still names bytes this store holds, and
    /// return the pins with their checksums. Commits nothing: the version
    /// an item carries is the version the run keeps (D10).
    pub fn capture(
        &mut self,
        run: RunId,
        items: &[RunContextItem],
        workspace: WorkspaceId,
    ) -> Result<CapturedContext, CaptureFailure> {
        let pins = items
            .iter()
            .enumerate()
            .map(|(item, pinned)| self.capture_one(item, pinned))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CapturedContext {
            run,
            workspace,
            pins,
        })
    }

    fn capture_one(
        &mut self,
        item: usize,
        pinned: &RunContextItem,
    ) -> Result<ContextPin, CaptureFailure> {
        let fail = |stage, fault| CaptureFailure { item, stage, fault };
        let checksum = match pinned {
            RunContextItem::Document {
                workspace,
                document,
                version,
            } => {
                self.resolve_document(*workspace, *document)
                    .map_err(|f| fail(CaptureStage::Resolve, f))?;
                let stored = self
                    .store
                    .version(*workspace, *version)
                    .map_err(|f| fail(CaptureStage::Retention, f))?
                    .ok_or(fail(CaptureStage::Retention, Fault::VersionUnavailable))?;
                if stored.document != *document {
                    return Err(fail(CaptureStage::Retention, Fault::VersionUnavailable));
                }
                stored.checksum
            }
            RunContextItem::Block {
                workspace,
                document,
                version,
                node,
            } => {
                self.resolve_document(*workspace, *document)
                    .map_err(|f| fail(CaptureStage::Resolve, f))?;
                let stored = self
                    .store
                    .version(*workspace, *version)
                    .map_err(|f| fail(CaptureStage::Retention, f))?
                    .ok_or(fail(CaptureStage::Retention, Fault::VersionUnavailable))?;
                if stored.document != *document {
                    // A version id that names another document's bytes is a
                    // pin that was assembled wrong, not one to deliver.
                    return Err(fail(CaptureStage::Retention, Fault::VersionUnavailable));
                }
                let document = crate::render::open(&stored.snapshot)
                    .map_err(|f| fail(CaptureStage::Retention, f))?;
                if document
                    .read(koru_core::NodeId::from_uuid(node.as_uuid()))
                    .is_err()
                {
                    return Err(fail(CaptureStage::Resolve, Fault::NodeUnavailable));
                }
                stored.checksum
            }
            RunContextItem::Blob {
                workspace, blob, ..
            } => {
                self.resolve_blob(*workspace, blob)
                    .map_err(|f| fail(CaptureStage::Resolve, f))?;
                self.retain_blob(*workspace, blob)
                    .map_err(|f| fail(CaptureStage::Retention, f))?;
                blob.as_str().to_owned()
            }
        };
        Ok(ContextPin {
            item: pinned.clone(),
            checksum,
        })
    }

    /// Step 1 for a document: its workspace opens and lists it — in any
    /// state. Whether it is in the trash is admission's question (D48),
    /// not capture's: an attempt already under way keeps going.
    fn resolve_document(
        &mut self,
        workspace: WorkspaceId,
        document: DocumentId,
    ) -> Result<(), Fault> {
        let entries = self.store.entries(workspace)?;
        if entries.iter().any(|entry| entry.id == document) {
            Ok(())
        } else {
            Err(Fault::DocumentUnavailable)
        }
    }

    /// Step 1 for a blob: its workspace opens and has a record of it.
    fn resolve_blob(&mut self, workspace: WorkspaceId, blob: &BlobHash) -> Result<(), Fault> {
        match self.store.blob(workspace, blob)? {
            Some(_) => Ok(()),
            None => Err(Fault::BlobUnavailable),
        }
    }

    /// Step 2 for a blob: the bytes are here and hash to their name.
    fn retain_blob(&mut self, workspace: WorkspaceId, blob: &BlobHash) -> Result<(), Fault> {
        self.store.blob_bytes(workspace, blob).map(drop)
    }
}
