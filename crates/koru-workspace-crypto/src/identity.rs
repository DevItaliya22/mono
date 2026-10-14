// SPDX-License-Identifier: MPL-2.0

//! **`CloudObjectId`** — keyed BLAKE3 over a `BlobHash`, and the exact claim
//! that construction supports (D90).
//!
//! # What it does
//!
//! ```text
//! subkey        = BLAKE3::derive_key(IDENTITY_SUBKEY_CONTEXT, root)
//! CloudObjectId = BLAKE3::keyed_hash(subkey,
//!                     "koru:cloud-object-id:v1\0"
//!                  || workspace_id (16)
//!                  || blob_identity_key_version (u32 be)
//!                  || u32be(len(blob_hash)) || blob_hash)
//! ```
//!
//! # What it prevents, stated precisely
//!
//! It prevents **cross-workspace equality correlation** — an observer holding
//! the object plane cannot tell that workspace A and workspace B store the same
//! blob — **assuming every workspace's [`WorkspaceBlobIdentityKey`] is
//! independently random.** That assumption is the whole claim: two workspaces
//! sharing a root would produce identical ids, and a root derived from a
//! workspace id would be no key at all.
//!
//! # What it deliberately does not prevent
//!
//! - **Equality within one workspace is still visible, and is meant to be.**
//!   That is what deduplication is. Two documents in one workspace that
//!   reference the same blob reference the same `CloudObjectId`, and the object
//!   plane can see it.
//! - **Length is still visible.** The stored ciphertext's length is public and
//!   D40 turns it into an exact plaintext length. Nothing here hides that.
//! - **A guess can be confirmed by somebody who holds the key.** Keyed BLAKE3
//!   is a PRF, not a commitment scheme: an attacker who has the workspace's
//!   identity key *and* a candidate blob can check whether the workspace holds
//!   it. The key is the secret; the construction assumes it stays one.
//! - **It is not an integrity check on the object.** A substituted object is
//!   caught by re-deriving the plaintext hash after decryption (the approved
//!   bootstrap contract §7.2), not by the id.
//!
//! # Why a derived subkey rather than the root
//!
//! D90: this key never doubles as an encryption key, and if the root is ever
//! used for more than `CloudObjectId`, purpose-specific subkeys are derived
//! from it. Deriving *now* — when there is exactly one use — is what makes that
//! sentence free later. `BLAKE3::derive_key` takes a hardcoded globally-unique
//! context string, which is the mechanism the hash function already provides
//! for exactly this.

use koru_store_core::ids::WorkspaceId;
use koru_store_core::model::BlobHash;
use zeroize::Zeroizing;

use crate::codec::Writer;
use crate::keys::{BlobIdentityKeyVersion, WorkspaceBlobIdentityKey};

/// The BLAKE3 `derive_key` context. Hardcoded, globally unique, and never
/// generated: that is the contract of `derive_key`.
const IDENTITY_SUBKEY_CONTEXT: &str =
    "koru.do/workspace-crypto/v1 blob-identity-root -> cloud-object-id subkey";

/// The domain label inside the keyed hash, so that a second future use of the
/// same subkey could not collide with this one.
const CLOUD_OBJECT_ID_DOMAIN: &[u8] = b"koru:cloud-object-id:v1\0";

/// The opaque 32-byte name an object is stored under. Carries no plaintext
/// content hash, and the bootstrap contract keeps it opaque on purpose.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CloudObjectId([u8; 32]);

impl CloudObjectId {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase hex, the one spelling wherever it is rendered as text.
    pub fn to_storage(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
        out
    }
}

impl std::fmt::Debug for CloudObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CloudObjectId({:02x}{:02x}{:02x}{:02x}…)",
            self.0[0], self.0[1], self.0[2], self.0[3]
        )
    }
}

/// Mints the `CloudObjectId` for a blob in a workspace.
///
/// `blob_hash` is carried as its canonical `<alg>:<hex>` text, length-prefixed,
/// so that the legacy `fnv1a` spelling and `b3:` cannot produce the same input
/// and no algorithm prefix can be confused with a digest.
pub fn cloud_object_id(
    root: &WorkspaceBlobIdentityKey,
    version: BlobIdentityKeyVersion,
    workspace_id: WorkspaceId,
    blob_hash: &BlobHash,
) -> CloudObjectId {
    let subkey = Zeroizing::new(blake3::derive_key(IDENTITY_SUBKEY_CONTEXT, root.as_bytes()));

    let mut input = Writer::with_capacity(64 + blob_hash.as_str().len());
    input.bytes(CLOUD_OBJECT_ID_DOMAIN);
    input.uuid(workspace_id.as_uuid());
    input.u32(version.0);
    input
        .var(blob_hash.as_str().as_bytes())
        .expect("a BlobHash is far shorter than u32::MAX");

    CloudObjectId(blake3::keyed_hash(&subkey, input.as_slice()).into())
}

#[cfg(test)]
mod tests;
