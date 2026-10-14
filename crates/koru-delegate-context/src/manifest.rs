// SPDX-License-Identifier: MPL-2.0

//! `manifest.json` — what is in a run's context directory, by pin.
//!
//! The one file materialization writes that is not a copy. It names each
//! file by the ids and hashes that identify it, so a provider (and a person
//! reading the directory afterwards) can say which pinned version a file
//! *is*. The person's original filename appears here, as metadata, and
//! nowhere in a path.
//!
//! Its `Debug` prints counts only, because the filename is the one piece
//! of person-written text in it.

use std::fmt;

use koru_delegate_core::{
    BlobHash, BlobKind, BlockNodeId, DocumentId, DocumentVersionId, RunId, WorkspaceId,
};
use koru_session_protocol::DeliveryMode;
use serde::{Deserialize, Serialize};

/// The manifest's own shape version, for a reader that meets an older one.
pub const MANIFEST_VERSION: u32 = 1;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub run: RunId,
    pub delivery: DeliveryMode,
    pub files: Vec<ManifestFile>,
}

impl fmt::Debug for Manifest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Manifest")
            .field("run", &self.run)
            .field("delivery", &self.delivery)
            .field("files", &self.files.len())
            .finish()
    }
}

/// One file, keyed by the pin it materializes. `path` is relative to the
/// run's context directory and is always an id-derived name.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ManifestFile {
    Document {
        item: usize,
        workspace: WorkspaceId,
        document: DocumentId,
        version: DocumentVersionId,
        path: String,
        checksum: String,
        media_type: String,
    },
    Block {
        item: usize,
        workspace: WorkspaceId,
        document: DocumentId,
        version: DocumentVersionId,
        node: BlockNodeId,
        path: String,
        checksum: String,
        media_type: String,
    },
    Blob {
        item: usize,
        workspace: WorkspaceId,
        hash: BlobHash,
        blob_kind: BlobKind,
        path: String,
        size: u64,
        media_type: String,
        /// The name the person's file had. Metadata: never a path.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
    },
}

impl ManifestFile {
    pub fn item(&self) -> usize {
        match self {
            Self::Document { item, .. } | Self::Block { item, .. } | Self::Blob { item, .. } => {
                *item
            }
        }
    }

    pub fn path(&self) -> &str {
        match self {
            Self::Document { path, .. } | Self::Block { path, .. } | Self::Blob { path, .. } => {
                path
            }
        }
    }
}

impl fmt::Debug for ManifestFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Document { .. } => "document",
            Self::Block { .. } => "block",
            Self::Blob { .. } => "blob",
        };
        f.debug_struct("ManifestFile")
            .field("kind", &kind)
            .field("item", &self.item())
            .finish_non_exhaustive()
    }
}
