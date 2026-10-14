// SPDX-License-Identifier: MPL-2.0

//! **Layer 2's records** — a blob object, a segment, a snapshot.
//!
//! One format, three purposes. Each is
//! `header || wrapped_dek || sealed_body`, and the header is bound into both
//! derivations so that neither half can be transplanted.
//!
//! # The header, and what `object_binding` is
//!
//! | Field | Width | |
//! | --- | --- | --- |
//! | `format_version` | 2 | 1 |
//! | `suite_id` | 2 | [`ContentSuite`] |
//! | `purpose` | 2 | blob object, segment or snapshot |
//! | `workspace_id` | 16 | |
//! | `wek_version` | 4 | the WEK version whose grant opens the wrapped DEK |
//! | `object_binding` | 32 | see below |
//!
//! `object_binding` is what stops object A's bytes being served under object
//! B's name. Its meaning is fixed by `purpose`:
//!
//! - **blob object** — the [`CloudObjectId`], which the client knows before it
//!   uploads because the id is minted from the plaintext's `BlobHash`.
//! - **segment / snapshot** — [`locator_binding`], a plain BLAKE3 hash of the
//!   opaque locator Plan 4.2's durability state machine mints. Hashing rather
//!   than embedding keeps the locator's *layout* out of this format, which is
//!   what the approved bootstrap contract's §7.1 asks for: a change in object-
//!   key layout must not be a change to an approved format.
//!
//! # There is no plaintext length field, and there is no plaintext hash
//!
//! The length is derived from the framing ([`object_plaintext_len`]); a second
//! copy would be a second, unverified answer to a billing-adjacent question.
//! The hash is re-derived by the restoring device from the decrypted bytes,
//! which is strictly stronger than being told and keeps a plaintext `BlobHash`
//! out of one more artifact.

use koru_store_core::ids::WorkspaceId;

use crate::codec::{Reader, Writer};
use crate::envelope::{self, WRAPPED_KEY_LEN};
use crate::error::{CryptoError, Result};
use crate::identity::CloudObjectId;
use crate::keys::{ContentEncryptionKey, WekVersion, WorkspaceEncryptionKey};
use crate::sealed::{self, SALT_LEN};
use crate::suites::{ContentSuite, FORMAT_VERSION_V1, Purpose};

/// The fixed width of [`ContentObjectHeader`] on the wire.
pub const CONTENT_HEADER_LEN: usize = 2 + 2 + 2 + 16 + 4 + 32;

/// Everything before the wrapped DEK, all of it bound into both derivations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentObjectHeader {
    pub format_version: u16,
    pub suite: ContentSuite,
    pub purpose: Purpose,
    pub workspace_id: WorkspaceId,
    pub wek_version: WekVersion,
    pub object_binding: [u8; 32],
}

impl ContentObjectHeader {
    /// A client-encrypted blob.
    pub fn for_blob(
        workspace_id: WorkspaceId,
        wek_version: WekVersion,
        cloud_object_id: CloudObjectId,
    ) -> Self {
        Self {
            format_version: FORMAT_VERSION_V1,
            suite: ContentSuite::Cobblestone256,
            purpose: Purpose::ContentBlobObject,
            workspace_id,
            wek_version,
            object_binding: *cloud_object_id.as_bytes(),
        }
    }

    /// A Cloud-written update segment or document snapshot (Plan 4.2).
    pub fn for_cloud_object(
        purpose: Purpose,
        workspace_id: WorkspaceId,
        wek_version: WekVersion,
        locator: &[u8],
    ) -> Result<Self> {
        if !matches!(purpose, Purpose::ContentSegment | Purpose::ContentSnapshot) {
            return Err(CryptoError::PurposeMismatch);
        }
        Ok(Self {
            format_version: FORMAT_VERSION_V1,
            suite: ContentSuite::Cobblestone256,
            purpose,
            workspace_id,
            wek_version,
            object_binding: locator_binding(locator),
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::with_capacity(CONTENT_HEADER_LEN);
        writer
            .u16(self.format_version)
            .u16(self.suite.id())
            .u16(self.purpose.id())
            .uuid(self.workspace_id.as_uuid())
            .u32(self.wek_version.0)
            .bytes(&self.object_binding);
        let bytes = writer.finish();
        debug_assert_eq!(bytes.len(), CONTENT_HEADER_LEN);
        bytes
    }

    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        let format_version = reader.u16()?;
        if format_version != FORMAT_VERSION_V1 {
            return Err(CryptoError::UnsupportedFormatVersion);
        }
        let suite = ContentSuite::from_id(reader.u16()?)?;
        let purpose = Purpose::from_id(reader.u16()?)?;
        if !matches!(
            purpose,
            Purpose::ContentBlobObject | Purpose::ContentSegment | Purpose::ContentSnapshot
        ) {
            return Err(CryptoError::PurposeMismatch);
        }
        Ok(Self {
            format_version,
            suite,
            purpose,
            workspace_id: WorkspaceId::from_uuid(reader.uuid()?),
            wek_version: WekVersion(reader.u32()?),
            object_binding: reader.array::<32>()?,
        })
    }
}

/// The 32 bytes a segment or snapshot binds to: BLAKE3 over a domain label and
/// Plan 4.2's opaque locator.
pub fn locator_binding(locator: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"koru:content-object-locator:v1\0");
    hasher.update(&(locator.len() as u64).to_be_bytes());
    hasher.update(locator);
    hasher.finalize().into()
}

/// Seals `plaintext` as a content object.
///
/// The DEK is fresh, random and per-object (D86). `salts` are drawn from the
/// caller's CSPRNG rather than inside, so a golden vector is reproducible.
pub fn seal_object(
    header: &ContentObjectHeader,
    wek: &WorkspaceEncryptionKey,
    plaintext: &[u8],
    rng: &mut impl rand_core::CryptoRng,
) -> Result<Vec<u8>> {
    let header_bytes = header.encode();
    let dek = ContentEncryptionKey::generate(rng);
    let mut wrap_salt = [0u8; SALT_LEN];
    let mut body_salt = [0u8; SALT_LEN];
    rng.fill_bytes(&mut wrap_salt);
    rng.fill_bytes(&mut body_salt);

    let wrapped = envelope::wrap_key(&wek.as_input_key(), &header_bytes, &dek, &wrap_salt)?;
    let body = envelope::seal_body(&dek.as_input_key(), &header_bytes, &body_salt, plaintext)?;

    let mut out = Vec::with_capacity(header_bytes.len() + wrapped.len() + body.len());
    out.extend_from_slice(&header_bytes);
    out.extend_from_slice(&wrapped);
    out.extend_from_slice(&body);
    Ok(out)
}

/// A parsed object: its header, its wrapped DEK and where its body starts.
///
/// Parsing is separate from opening because the object plane parses objects it
/// holds no key for — that is how it validates lengths and derives the
/// plaintext length under D40.
#[derive(Clone, Debug)]
pub struct ContentObject<'a> {
    pub header: ContentObjectHeader,
    wrapped_dek: &'a [u8],
    body: &'a [u8],
}

impl<'a> ContentObject<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        let mut reader = Reader::new(bytes);
        let header = ContentObjectHeader::decode(&mut reader)?;
        let wrapped_dek = reader.take(WRAPPED_KEY_LEN)?;
        let body = reader.rest();
        // Validates the body's framing without a key: this is where a
        // truncated, extended or malformed object is refused.
        sealed::SealedLayout::of_ciphertext_len(body.len() as u64)?;
        Ok(Self {
            header,
            wrapped_dek,
            body,
        })
    }

    /// The exact plaintext length of this valid object, without any key.
    pub fn plaintext_len(&self) -> Result<u64> {
        Ok(sealed::SealedLayout::of_ciphertext_len(self.body.len() as u64)?.plaintext_len())
    }

    pub fn body(&self) -> &'a [u8] {
        self.body
    }

    pub fn unwrap_dek(&self, wek: &WorkspaceEncryptionKey) -> Result<ContentEncryptionKey> {
        envelope::unwrap_key(&wek.as_input_key(), &self.header.encode(), self.wrapped_dek)
    }

    /// Opens the whole plaintext. For a multi-gigabyte object use
    /// [`ContentObject::opener`] instead and read chunk by chunk.
    pub fn open(&self, wek: &WorkspaceEncryptionKey) -> Result<Vec<u8>> {
        let dek = self.unwrap_dek(wek)?;
        envelope::open_body(&dek.as_input_key(), &self.header.encode(), self.body)
    }

    /// A random-access reader over the body.
    pub fn opener(&self, wek: &WorkspaceEncryptionKey) -> Result<sealed::Opener> {
        let dek = self.unwrap_dek(wek)?;
        envelope::body_opener(
            &dek.as_input_key(),
            &self.header.encode(),
            self.body.len() as u64,
            self.body,
        )
    }
}

/// The plaintext length of a stored content object of this length.
///
/// Narrowly stated, because the narrow statement is the true one: this is the
/// exact plaintext length **of a valid encrypted object**, derived from
/// canonical framing plus fixed versioned overhead. It is not a proof of what
/// the user's original local file contained — a client that encrypted something
/// else produces a valid object of the length it actually encrypted. Length
/// exposure is deliberate; HPKE does not hide plaintext length either
/// (RFC 9180 §9.7.6).
pub fn object_plaintext_len(stored_len: u64) -> Result<u64> {
    envelope::object_plaintext_len(CONTENT_HEADER_LEN, stored_len)
}

#[cfg(test)]
mod tests;
