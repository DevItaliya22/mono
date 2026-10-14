// SPDX-License-Identifier: MPL-2.0

//! Shared peer/Cloud change detection, derived from durable document positions.
//!
//! A body edit does NOT mutate either parent manifest. Never use a manifest's
//! version as evidence that all of its documents are unchanged. Compare these
//! per-document revision digests first; reconcile differing versions with Loro.
//! Digests are disposable hints, never authorization or a durable write receipt.
use sha2::{Digest, Sha256};

pub type Revision = [u8; 32];

/// Hash the exact stored version bytes. This is also the Cloud object catalog's
/// `current_version_digest` encoding. Do not decode/re-encode before hashing:
/// unequal encodings may cause an extra reconciliation, never a false match.
pub fn revision(version: &[u8]) -> Revision {
    Sha256::digest(version).into()
}

/// The peer hierarchy and Cloud batch comparison share this leaf definition.
pub fn document(
    workspace: crate::WorkspaceId,
    document: crate::DocumentId,
    revision: Revision,
) -> Revision {
    let mut hash = blake3::Hasher::new();
    hash.update(b"koru-sync-document-v2\0");
    hash.update(workspace.as_uuid().as_bytes());
    hash.update(document.as_uuid().as_bytes());
    hash.update(&revision);
    *hash.finalize().as_bytes()
}

#[cfg(test)]
mod tests;
