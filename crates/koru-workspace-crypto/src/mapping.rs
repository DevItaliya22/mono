// SPDX-License-Identifier: MPL-2.0

//! **The encrypted `BlobHash` ↔ `CloudObjectId` mapping, and availability
//! metadata.**
//!
//! No plaintext `BlobHash` exists server-side. That is UC8's rule and the
//! architecture's encryption boundary (D1): logs, metrics, crash reports and
//! support tooling never contain workspace plaintext, raw keys, or a plaintext
//! `BlobHash`. The mapping row is the one place the correspondence is stored at
//! all, and it is stored sealed under the workspace's WEK.
//!
//! # Two records, on purpose
//!
//! [`BlobMappingRecordV1`] is the sealed half: `CloudObjectId` in the clear
//! because it is the row's key, `BlobHash` inside the ciphertext.
//!
//! [`ObjectAvailabilityV1`] is the cleartext half — what the object plane needs
//! in order to serve, account and purge. It holds **no** identifier of the
//! plaintext: the id, the stored length, the incarnation and a durability
//! state. The plaintext length is not stored; it is *derived* from the stored
//! length under D40, so there is one answer to a billing-adjacent question
//! rather than two.
//!
//! # Why the WEK directly, with no wrapped DEK
//!
//! A mapping row is a few dozen bytes. The wrapped DEK of [`crate::content`]
//! exists so that one large object can be made unreadable on its own; a row
//! this size is erased by deleting the row, and 104 bytes of envelope to reach
//! the same place would be pure overhead.

use koru_store_core::ids::WorkspaceId;
use koru_store_core::model::BlobHash;
use uuid::Uuid;

use crate::codec::{Reader, Writer};
use crate::envelope;
use crate::error::{CryptoError, Result};
use crate::identity::CloudObjectId;
use crate::keys::{BlobIdentityKeyVersion, WekVersion, WorkspaceEncryptionKey};
use crate::sealed::SALT_LEN;
use crate::suites::{ContentSuite, FORMAT_VERSION_V1, Purpose};

/// The fixed width of [`BlobMappingHeader`] on the wire.
pub const MAPPING_HEADER_LEN: usize = 2 + 2 + 2 + 16 + 4 + 4 + 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobMappingHeader {
    pub format_version: u16,
    pub suite: ContentSuite,
    pub workspace_id: WorkspaceId,
    pub wek_version: WekVersion,
    /// The identity-key version under which `cloud_object_id` was minted. Its
    /// own axis: routine WEK rotation does not change it (D14/D17).
    pub blob_identity_key_version: BlobIdentityKeyVersion,
    pub cloud_object_id: CloudObjectId,
}

impl BlobMappingHeader {
    pub const PURPOSE: Purpose = Purpose::BlobMappingRecord;

    pub fn new(
        workspace_id: WorkspaceId,
        wek_version: WekVersion,
        blob_identity_key_version: BlobIdentityKeyVersion,
        cloud_object_id: CloudObjectId,
    ) -> Self {
        Self {
            format_version: FORMAT_VERSION_V1,
            suite: ContentSuite::Cobblestone256,
            workspace_id,
            wek_version,
            blob_identity_key_version,
            cloud_object_id,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::with_capacity(MAPPING_HEADER_LEN);
        writer
            .u16(self.format_version)
            .u16(self.suite.id())
            .u16(Self::PURPOSE.id())
            .uuid(self.workspace_id.as_uuid())
            .u32(self.wek_version.0)
            .u32(self.blob_identity_key_version.0)
            .bytes(self.cloud_object_id.as_bytes());
        let bytes = writer.finish();
        debug_assert_eq!(bytes.len(), MAPPING_HEADER_LEN);
        bytes
    }

    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        let format_version = reader.u16()?;
        if format_version != FORMAT_VERSION_V1 {
            return Err(CryptoError::UnsupportedFormatVersion);
        }
        let suite = ContentSuite::from_id(reader.u16()?)?;
        if Purpose::from_id(reader.u16()?)? != Self::PURPOSE {
            return Err(CryptoError::PurposeMismatch);
        }
        Ok(Self {
            format_version,
            suite,
            workspace_id: WorkspaceId::from_uuid(reader.uuid()?),
            wek_version: WekVersion(reader.u32()?),
            blob_identity_key_version: BlobIdentityKeyVersion(reader.u32()?),
            cloud_object_id: CloudObjectId::from_bytes(reader.array::<32>()?),
        })
    }
}

/// `header || sealed(WEK, context = header, plaintext = BlobHash)`.
pub struct BlobMappingRecordV1;

impl BlobMappingRecordV1 {
    pub fn seal(
        header: &BlobMappingHeader,
        blob_hash: &BlobHash,
        wek: &WorkspaceEncryptionKey,
        rng: &mut impl rand_core::CryptoRng,
    ) -> Result<Vec<u8>> {
        let header_bytes = header.encode();
        let mut body = Writer::with_capacity(80);
        body.var(blob_hash.as_str().as_bytes())?;

        let mut salt = [0u8; SALT_LEN];
        rng.fill_bytes(&mut salt);
        let sealed =
            envelope::seal_body(&wek.as_input_key(), &header_bytes, &salt, body.as_slice())?;

        let mut out = Vec::with_capacity(header_bytes.len() + sealed.len());
        out.extend_from_slice(&header_bytes);
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    pub fn open(
        bytes: &[u8],
        wek: &WorkspaceEncryptionKey,
    ) -> Result<(BlobMappingHeader, BlobHash)> {
        let mut reader = Reader::new(bytes);
        let header = BlobMappingHeader::decode(&mut reader)?;
        let header_bytes = reader.consumed().to_vec();
        let plaintext = envelope::open_body(&wek.as_input_key(), &header_bytes, reader.rest())?;

        let mut body = Reader::new(&plaintext);
        let text = body.var()?;
        body.finish()?;
        let text = std::str::from_utf8(text).map_err(|_| CryptoError::Malformed)?;
        let blob_hash = BlobHash::parse(text).map_err(|_| CryptoError::Malformed)?;
        Ok((header, blob_hash))
    }
}

/// What the object plane keeps in the clear about an object.
///
/// Every field is something the object plane can already see or has to act on.
/// **Nothing here identifies the plaintext**: no `BlobHash`, no filename, no
/// document reference, no MIME type. `cloud_object_id` is a keyed hash whose
/// key never leaves the workspace ([`crate::identity`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectAvailabilityV1 {
    pub format_version: u16,
    pub cloud_object_id: CloudObjectId,
    /// The incarnation this object belongs to (D61/D64), so a fresh incarnation
    /// cannot inherit a previous lineage's objects.
    pub cloud_dataset_incarnation_id: Uuid,
    /// The length as stored, which the object plane knows without opening
    /// anything.
    pub stored_len: u64,
    pub state: ObjectDurabilityState,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ObjectDurabilityState {
    /// Admitted and being written; not yet part of a fenced durable commit.
    Staged,
    /// Durable under the fenced commit of Plan 4.2.
    Durable,
    /// Scheduled for purge by the lifecycle executor (Plan 5.4).
    PendingPurge,
}

impl ObjectDurabilityState {
    pub const fn tag(self) -> u8 {
        match self {
            Self::Staged => 1,
            Self::Durable => 2,
            Self::PendingPurge => 3,
        }
    }

    pub fn from_tag(tag: u8) -> Result<Self> {
        match tag {
            1 => Ok(Self::Staged),
            2 => Ok(Self::Durable),
            3 => Ok(Self::PendingPurge),
            _ => Err(CryptoError::Malformed),
        }
    }
}

impl ObjectAvailabilityV1 {
    pub fn new(
        cloud_object_id: CloudObjectId,
        cloud_dataset_incarnation_id: Uuid,
        stored_len: u64,
        state: ObjectDurabilityState,
    ) -> Self {
        Self {
            format_version: FORMAT_VERSION_V1,
            cloud_object_id,
            cloud_dataset_incarnation_id,
            stored_len,
            state,
        }
    }

    /// The plaintext length of the object this row describes, derived from
    /// `stored_len` rather than stored beside it (D40).
    pub fn plaintext_len(&self) -> Result<u64> {
        crate::content::object_plaintext_len(self.stored_len)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::with_capacity(2 + 32 + 16 + 8 + 1);
        writer
            .u16(self.format_version)
            .bytes(self.cloud_object_id.as_bytes())
            .uuid(self.cloud_dataset_incarnation_id)
            .u64(self.stored_len)
            .u8(self.state.tag());
        writer.finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(bytes);
        let format_version = reader.u16()?;
        if format_version != FORMAT_VERSION_V1 {
            return Err(CryptoError::UnsupportedFormatVersion);
        }
        let row = Self {
            format_version,
            cloud_object_id: CloudObjectId::from_bytes(reader.array::<32>()?),
            cloud_dataset_incarnation_id: reader.uuid()?,
            stored_len: reader.u64()?,
            state: ObjectDurabilityState::from_tag(reader.u8()?)?,
        };
        reader.finish()?;
        Ok(row)
    }
}

#[cfg(test)]
mod tests;
