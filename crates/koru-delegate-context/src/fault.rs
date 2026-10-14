// SPDX-License-Identifier: MPL-2.0

//! The closed vocabulary every failure here speaks.
//!
//! A store error carries a message, and some of those messages carry a
//! path — `get_blob` names where it quarantined a rotten blob. None of that
//! may reach a run's failure (standing requirement 7), so the store's
//! `Code` is mapped to one of these tokens **and its message is dropped**
//! at the boundary. A fault is a word, never a sentence.

use std::fmt;

use koru_store_core::errors::{Code, Error};

/// Why an operation on one context item did not complete. Tokens only.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fault {
    /// The item's workspace cannot be opened — unknown, or deleted.
    WorkspaceUnavailable,
    /// The document is not registered in its workspace.
    DocumentUnavailable,
    /// No `document_versions` row for the pinned version: never captured
    /// here, or gone with a physical purge.
    VersionUnavailable,
    /// The version's bytes no longer match their checksum.
    VersionCorrupt,
    /// The blob is unknown to the workspace, or its bytes are held only by
    /// a peer that has not sent them.
    BlobUnavailable,
    /// The blob's bytes no longer hash to their name.
    BlobCorrupt,
    /// The version's snapshot is not a document this build can read.
    SnapshotUnreadable,
    /// The pinned block is not in the pinned version.
    NodeUnavailable,
    /// The Markdown renderer refused the document.
    RenderFailed,
    /// The provider cannot receive attachments at all.
    DeliveryUnsupported,
    /// A filesystem operation failed; the kind is std's closed set.
    Io(std::io::ErrorKind),
    /// Any other store failure.
    StoreFailed,
}

impl Fault {
    /// Map a store error to a token. The message is dropped here, on
    /// purpose, and never read.
    ///
    /// Public because the boundary is not this crate's alone: a
    /// [`crate::ContextStore`] over the wire answers the same `Code`s and
    /// must drop the same messages, and a second mapping would be a second
    /// chance to let a quarantine path through.
    pub fn from_store(error: &Error) -> Self {
        match error.code {
            Code::WorkspaceNotFound => Self::WorkspaceUnavailable,
            Code::DocumentNotFound => Self::DocumentUnavailable,
            Code::HistoryEntryNotFound | Code::VersionUnavailable => Self::VersionUnavailable,
            Code::BlobNotFound | Code::BlobWaitingForPeer => Self::BlobUnavailable,
            Code::BlobChecksumMismatch => Self::BlobCorrupt,
            _ => Self::StoreFailed,
        }
    }

    pub(crate) fn from_io(error: &std::io::Error) -> Self {
        Self::Io(error.kind())
    }

    /// The stable spelling a surface or a log may carry.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceUnavailable => "workspace_unavailable",
            Self::DocumentUnavailable => "document_unavailable",
            Self::VersionUnavailable => "version_unavailable",
            Self::VersionCorrupt => "version_corrupt",
            Self::BlobUnavailable => "blob_unavailable",
            Self::BlobCorrupt => "blob_corrupt",
            Self::SnapshotUnreadable => "snapshot_unreadable",
            Self::NodeUnavailable => "node_unavailable",
            Self::RenderFailed => "render_failed",
            Self::DeliveryUnsupported => "delivery_unsupported",
            Self::Io(_) => "io",
            Self::StoreFailed => "store_failed",
        }
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(kind) => write!(f, "io ({kind:?})"),
            other => f.write_str(other.as_str()),
        }
    }
}
