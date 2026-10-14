// SPDX-License-Identifier: MPL-2.0

//! What a run was given — captured bytes, not a remembered number (§7.2).
//!
//! **The pin is the identity.** A document is pinned at the
//! `DocumentVersionId` of a `capture_history` version committed *before* the
//! provider starts; a blob is pinned at its content hash. Editing the source
//! afterwards changes nothing here (UC2), and deleting it changes nothing
//! either (D47): the run keeps naming exactly what it received, and a surface
//! renders the target's *current* state — live, in trash, gone — beside the
//! pin (D46). Nothing here is a pointer to live bytes.

use serde::{Deserialize, Serialize};

use crate::{BlobHash, BlockNodeId, DocumentId, DocumentVersionId, WorkspaceId};

/// What kind of bytes a blob item carries. The kind decides how the item is
/// materialized for the provider (P1.7); the hash decides what it is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum BlobKind {
    File,
    Image,
    Pdf,
}

/// One item of context, pinned.
///
/// The workspace travels with every item because content lives in exactly
/// one workspace and a document id alone cannot say which room to open
/// (DATAMODEL, *content lives in exactly one workspace*).
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunContextItem {
    /// A whole document at one captured version.
    Document {
        workspace: WorkspaceId,
        document: DocumentId,
        version: DocumentVersionId,
    },
    /// One block of a document, at one captured version of that document.
    /// The version is the document's — a block has no history of its own.
    Block {
        workspace: WorkspaceId,
        document: DocumentId,
        version: DocumentVersionId,
        node: BlockNodeId,
    },
    /// A file, image or PDF from the content-addressed blob store, pinned at
    /// its hash. Retention is asserted at capture (§7.2 step 2).
    Blob {
        workspace: WorkspaceId,
        blob: BlobHash,
        blob_kind: BlobKind,
    },
}

impl RunContextItem {
    pub fn workspace(&self) -> WorkspaceId {
        match self {
            Self::Document { workspace, .. }
            | Self::Block { workspace, .. }
            | Self::Blob { workspace, .. } => *workspace,
        }
    }

    /// The document version this item pins, when it pins one.
    pub fn document_version(&self) -> Option<(DocumentId, DocumentVersionId)> {
        match self {
            Self::Document {
                document, version, ..
            }
            | Self::Block {
                document, version, ..
            } => Some((*document, *version)),
            Self::Blob { .. } => None,
        }
    }
}
