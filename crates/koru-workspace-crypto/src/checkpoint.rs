// SPDX-License-Identifier: MPL-2.0

//! **The encrypted `WorkspaceRecoveryCheckpoint`**, carrying exactly the fields
//! of `WorkspaceRecoveryBootstrapV1`.
//!
//! Built against the contract at
//! `docs/cloud-platform/contracts/workspace-recovery-bootstrap-v1.md`, approved
//! by the maintainers on 2026-09-05 at commit **`49c2e397`** — the row in
//! the contract approval record that Plan 2.1 cites and could
//! not begin without.
//!
//! # Shape
//!
//! ```text
//! header  = envelope framing (3 fields) || H1 H2 H3 H4 H5
//! record  = header || wrapped_dek || sealed_body(B1…B6)
//! ```
//!
//! # The three envelope fields in front of H1–H5
//!
//! The contract fixes H1–H5 as the cleartext header and leaves "serialization,
//! AEAD suite, key derivation, envelope layout" to this plan (§1, §11). D87
//! requires every persisted envelope to carry `format_version`, `suite_id`, a
//! key id with a version, and a `purpose`. Those are not duplicates of H1–H5:
//!
//! - the envelope's `format_version` versions **the encryption framing**;
//!   H1's `bootstrap_format_version` versions **the field table inside**. A
//!   reader needs both and they move independently.
//! - `suite_id` and `purpose` have no counterpart in the contract at all.
//! - the key **id** and version are H5 — the WEK version. There is deliberately
//!   no separate key id: a workspace has one WEK lineage and H5 names the
//!   version within it.
//!
//! All eight fields are bound. The contract requires that "H1–H5, in full and
//! in canonical order, are the AAD"; they are, contiguously and canonically,
//! with three additional fields in front of them. More binding, not less.
//!
//! # It is bound through the key derivation, not through an AEAD AAD
//!
//! `ContentCipherSuiteV1` is c2sp.org/chunked-encryption, whose AEAD AAD **must
//! be empty**; application data binds through the `context` that enters the
//! HKDF info. So the complete header is the context, and altering any byte of
//! it changes the derived key, base nonce **and commitment** — the reader fails
//! at the commitment check before decrypting a single chunk. The contract's
//! §9.1 properties hold exactly: the header cannot be altered without
//! decryption failing, and a body cannot be served under another workspace's,
//! incarnation's or checkpoint's header. The mechanism differs from the one
//! §9.1 describes and this is where that is written down.
//!
//! # Reader obligations
//!
//! Steps 2, 5 and 6 of the contract's §13 live here: refuse an unimplemented
//! major version before decryption, decrypt with the key named by H5 with
//! H1–H5 bound, and check the completeness assertions of §9.3. Steps 1, 3, 4
//! and 7–10 need the network, custody and the store, and belong to Plan 8.1.

use koru_store_core::ids::{DocumentId, WorkspaceId};
use uuid::Uuid;

use crate::codec::{Reader, Writer};
use crate::envelope::{self, WRAPPED_KEY_LEN};
use crate::error::{CryptoError, Result};
use crate::identity::CloudObjectId;
use crate::keys::{
    BlobIdentityKeyVersion, ContentEncryptionKey, WekVersion, WorkspaceEncryptionKey,
};
use crate::sealed::SALT_LEN;
use crate::suites::{ContentSuite, FORMAT_VERSION_V1, Purpose};

/// H1. The major version of `WorkspaceRecoveryBootstrapV1`.
pub const BOOTSTRAP_FORMAT_VERSION_V1: u16 = 1;

/// The fixed width of [`CheckpointHeader`] on the wire.
pub const CHECKPOINT_HEADER_LEN: usize = 2 + 2 + 2 // envelope framing
    + 2  // H1
    + 16 // H2
    + 16 // H3
    + 16 // H4
    + 4; // H5

/// The cleartext header: the envelope framing, then H1–H5 in the contract's
/// order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointHeader {
    pub format_version: u16,
    pub suite: ContentSuite,
    /// H1 — the bootstrap contract's major version.
    pub bootstrap_format_version: u16,
    /// H2.
    pub workspace_id: WorkspaceId,
    /// H3.
    pub cloud_dataset_incarnation_id: Uuid,
    /// H4 — minted per publication by Plan 4.4.
    pub checkpoint_id: Uuid,
    /// H5 — the WEK version whose grant opens this checkpoint's DEK envelope.
    pub wek_version: WekVersion,
}

impl CheckpointHeader {
    pub const PURPOSE: Purpose = Purpose::RecoveryCheckpoint;

    pub fn new(
        workspace_id: WorkspaceId,
        cloud_dataset_incarnation_id: Uuid,
        checkpoint_id: Uuid,
        wek_version: WekVersion,
    ) -> Self {
        Self {
            format_version: FORMAT_VERSION_V1,
            suite: ContentSuite::Cobblestone256,
            bootstrap_format_version: BOOTSTRAP_FORMAT_VERSION_V1,
            workspace_id,
            cloud_dataset_incarnation_id,
            checkpoint_id,
            wek_version,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::with_capacity(CHECKPOINT_HEADER_LEN);
        writer
            .u16(self.format_version)
            .u16(self.suite.id())
            .u16(Self::PURPOSE.id())
            .u16(self.bootstrap_format_version)
            .uuid(self.workspace_id.as_uuid())
            .uuid(self.cloud_dataset_incarnation_id)
            .uuid(self.checkpoint_id)
            .u32(self.wek_version.0);
        let bytes = writer.finish();
        debug_assert_eq!(bytes.len(), CHECKPOINT_HEADER_LEN);
        bytes
    }

    /// Parses the header. **A `bootstrap_format_version` this build does not
    /// implement stops here**, before decryption, and is reported as a
    /// different fact from a corrupt record — the contract's §5 rule, and the
    /// caller maps it to `CLOUD_SCHEMA_UPDATE_REQUIRED`.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        let format_version = reader.u16()?;
        if format_version != FORMAT_VERSION_V1 {
            return Err(CryptoError::UnsupportedFormatVersion);
        }
        let suite = ContentSuite::from_id(reader.u16()?)?;
        if Purpose::from_id(reader.u16()?)? != Self::PURPOSE {
            return Err(CryptoError::PurposeMismatch);
        }
        let bootstrap_format_version = reader.u16()?;
        if bootstrap_format_version != BOOTSTRAP_FORMAT_VERSION_V1 {
            return Err(CryptoError::UnsupportedFormatVersion);
        }
        Ok(Self {
            format_version,
            suite,
            bootstrap_format_version,
            workspace_id: WorkspaceId::from_uuid(reader.uuid()?),
            cloud_dataset_incarnation_id: reader.uuid()?,
            checkpoint_id: reader.uuid()?,
            wek_version: WekVersion(reader.u32()?),
        })
    }
}

// ------------------------------------------------------------ the body, B1–B6

/// `MANIFEST` or `DOCUMENT`. A **stable ASCII token**, never a translated
/// label, per the contract's §11.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DocumentRole {
    Manifest,
    Document,
}

impl DocumentRole {
    pub const fn token(self) -> &'static [u8] {
        match self {
            Self::Manifest => b"MANIFEST",
            Self::Document => b"DOCUMENT",
        }
    }

    pub fn from_token(token: &[u8]) -> Result<Self> {
        match token {
            b"MANIFEST" => Ok(Self::Manifest),
            b"DOCUMENT" => Ok(Self::Document),
            _ => Err(CryptoError::Malformed),
        }
    }
}

/// §4d. Names an object the checkpoint depends on; never contains it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectReference {
    /// Plan 4.2's deterministic object key. **Opaque here**, so a change in
    /// object-key layout is not a change to an approved contract.
    pub locator: Vec<u8>,
    /// BLAKE3-256 over the **stored ciphertext bytes**, exactly as the object
    /// plane serves them.
    pub ciphertext_digest: [u8; 32],
}

/// §4b. One document's entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentEntry {
    pub document_id: DocumentId,
    pub role: DocumentRole,
    /// Loro `Frontiers::encode()`, carried verbatim. This record never
    /// re-encodes a Loro value and never compares one by its encoding.
    pub frontier: Vec<u8>,
    /// Loro `VersionVector::encode()`, carried verbatim. Compared with
    /// `includes_vv`, never by equality of the encoding and never by timestamp.
    pub version_vector: Vec<u8>,
    /// May be **older** than H5: historical objects are not rewritten on
    /// rotation.
    pub key_version: WekVersion,
    pub snapshot: ObjectReference,
    /// **The order is the required application order.** Loro history is causal;
    /// a replay that skips a broken update and applies what follows builds a
    /// document from a history that never happened.
    pub updates: Vec<ObjectReference>,
}

/// §4c. One object's entry. Carries no `BlobHash`, no plaintext length and no
/// `ciphertext_digest` — all three deliberately, and all three argued in the
/// contract's §7.2.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectEntry {
    pub cloud_object_id: CloudObjectId,
    pub key_version: WekVersion,
}

/// B1–B6, the whole of `WorkspaceRecoveryBootstrapV1`.
///
/// There are **no optional fields**. An optional field in a recovery record is
/// a field whose absence must have a defined meaning, and each one doubles the
/// reader's paths at the moment a person has already lost every device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceRecoveryBootstrapV1 {
    /// B1, canonically ordered by `document_id` ascending. The manifest entry
    /// sorts with the rest and holds no special position.
    pub documents: Vec<DocumentEntry>,
    /// B3, canonically ordered by `cloud_object_id` byte order ascending.
    pub objects: Vec<ObjectEntry>,
    /// B5. Its axis is independent of the WEK's.
    pub blob_identity_key_version: BlobIdentityKeyVersion,
    /// B6, sorted ascending, no duplicates. **MUST** equal the union of every
    /// `key_version` in `documents` and `objects` together with H5.
    pub required_key_versions: Vec<WekVersion>,
}

impl WorkspaceRecoveryBootstrapV1 {
    /// Encodes the body. `document_count` (B2) and `object_count` (B4) precede
    /// their arrays: a count immediately before the array it counts is the same
    /// truncation detector the contract asks for, and it removes the
    /// possibility of the two disagreeing rather than merely detecting it.
    pub fn encode(&self, h5: WekVersion) -> Result<Vec<u8>> {
        self.check(h5)?;
        let mut writer = Writer::with_capacity(1024);

        writer.u32(u32::try_from(self.documents.len()).map_err(|_| CryptoError::TooLarge)?);
        for entry in &self.documents {
            writer.uuid(entry.document_id.as_uuid());
            writer.var(entry.role.token())?;
            writer.var(&entry.frontier)?;
            writer.var(&entry.version_vector)?;
            writer.u32(entry.key_version.0);
            encode_reference(&mut writer, &entry.snapshot)?;
            writer.u32(u32::try_from(entry.updates.len()).map_err(|_| CryptoError::TooLarge)?);
            for update in &entry.updates {
                encode_reference(&mut writer, update)?;
            }
        }

        writer.u32(u32::try_from(self.objects.len()).map_err(|_| CryptoError::TooLarge)?);
        for entry in &self.objects {
            writer.bytes(entry.cloud_object_id.as_bytes());
            writer.u32(entry.key_version.0);
        }

        writer.u32(self.blob_identity_key_version.0);
        writer.u32(
            u32::try_from(self.required_key_versions.len()).map_err(|_| CryptoError::TooLarge)?,
        );
        for version in &self.required_key_versions {
            writer.u32(version.0);
        }

        Ok(writer.finish())
    }

    pub fn decode(bytes: &[u8], h5: WekVersion) -> Result<Self> {
        let mut reader = Reader::new(bytes);

        let document_count = reader.u32()?;
        let mut documents = Vec::new();
        for _ in 0..document_count {
            let document_id = DocumentId::from_uuid(reader.uuid()?);
            let role = DocumentRole::from_token(reader.var()?)?;
            let frontier = reader.var()?.to_vec();
            let version_vector = reader.var()?.to_vec();
            let key_version = WekVersion(reader.u32()?);
            let snapshot = decode_reference(&mut reader)?;
            let update_count = reader.u32()?;
            let mut updates = Vec::new();
            for _ in 0..update_count {
                updates.push(decode_reference(&mut reader)?);
            }
            documents.push(DocumentEntry {
                document_id,
                role,
                frontier,
                version_vector,
                key_version,
                snapshot,
                updates,
            });
        }

        let object_count = reader.u32()?;
        let mut objects = Vec::new();
        for _ in 0..object_count {
            objects.push(ObjectEntry {
                cloud_object_id: CloudObjectId::from_bytes(reader.array::<32>()?),
                key_version: WekVersion(reader.u32()?),
            });
        }

        let blob_identity_key_version = BlobIdentityKeyVersion(reader.u32()?);
        let required_count = reader.u32()?;
        let mut required_key_versions = Vec::new();
        for _ in 0..required_count {
            required_key_versions.push(WekVersion(reader.u32()?));
        }
        reader.finish()?;

        let bootstrap = Self {
            documents,
            objects,
            blob_identity_key_version,
            required_key_versions,
        };
        bootstrap.check(h5)?;
        Ok(bootstrap)
    }

    /// The contract's §9.3 completeness and structure checks, plus §4a's
    /// canonical ordering. Every one is a value the reader can recompute, which
    /// is the point: they are assertions to be checked, not copies to be
    /// trusted.
    pub fn check(&self, h5: WekVersion) -> Result<()> {
        if self.documents.is_empty() {
            return Err(CryptoError::Malformed);
        }
        if self
            .documents
            .iter()
            .filter(|d| d.role == DocumentRole::Manifest)
            .count()
            != 1
        {
            return Err(CryptoError::Malformed);
        }
        // Canonical ordering, strictly ascending, which also rules out
        // duplicates in one pass.
        for pair in self.documents.windows(2) {
            if document_order(&pair[0].document_id) >= document_order(&pair[1].document_id) {
                return Err(CryptoError::Malformed);
            }
        }
        for pair in self.objects.windows(2) {
            if pair[0].cloud_object_id.as_bytes() >= pair[1].cloud_object_id.as_bytes() {
                return Err(CryptoError::Malformed);
            }
        }

        let mut expected: Vec<u32> = self
            .documents
            .iter()
            .map(|d| d.key_version.0)
            .chain(self.objects.iter().map(|o| o.key_version.0))
            .chain(std::iter::once(h5.0))
            .collect();
        expected.sort_unstable();
        expected.dedup();
        let declared: Vec<u32> = self.required_key_versions.iter().map(|v| v.0).collect();
        if declared != expected {
            return Err(CryptoError::Malformed);
        }
        Ok(())
    }
}

/// The canonical order of `documents[]`.
///
/// The contract says "by `document_id` storage form, ascending" — the
/// hyphenated lowercase UUID. Comparing the 16 raw bytes gives the identical
/// order, because the hyphens sit at fixed positions and lowercase hex digits
/// sort the same way as the nibbles they spell; `tests.rs` proves the two agree
/// rather than asserting it. Raw bytes are used because the alternative
/// allocates a `String` per comparison inside a sort.
fn document_order(id: &DocumentId) -> [u8; 16] {
    *id.as_uuid().as_bytes()
}

fn encode_reference(writer: &mut Writer, reference: &ObjectReference) -> Result<()> {
    writer.var(&reference.locator)?;
    writer.bytes(&reference.ciphertext_digest);
    Ok(())
}

fn decode_reference(reader: &mut Reader<'_>) -> Result<ObjectReference> {
    let locator = reader.var()?.to_vec();
    let ciphertext_digest = reader.array::<32>()?;
    Ok(ObjectReference {
        locator,
        ciphertext_digest,
    })
}

// -------------------------------------------------------------- the envelope

/// Seals a checkpoint.
pub fn seal_checkpoint(
    header: &CheckpointHeader,
    bootstrap: &WorkspaceRecoveryBootstrapV1,
    wek: &WorkspaceEncryptionKey,
    rng: &mut impl rand_core::CryptoRng,
) -> Result<Vec<u8>> {
    let header_bytes = header.encode();
    let body = bootstrap.encode(header.wek_version)?;

    let dek = ContentEncryptionKey::generate(rng);
    let mut wrap_salt = [0u8; SALT_LEN];
    let mut body_salt = [0u8; SALT_LEN];
    rng.fill_bytes(&mut wrap_salt);
    rng.fill_bytes(&mut body_salt);

    let wrapped = envelope::wrap_key(&wek.as_input_key(), &header_bytes, &dek, &wrap_salt)?;
    let sealed_body = envelope::seal_body(&dek.as_input_key(), &header_bytes, &body_salt, &body)?;

    let mut out = Vec::with_capacity(header_bytes.len() + wrapped.len() + sealed_body.len());
    out.extend_from_slice(&header_bytes);
    out.extend_from_slice(&wrapped);
    out.extend_from_slice(&sealed_body);
    Ok(out)
}

/// Parses a checkpoint's cleartext header without opening it — the contract's
/// §13 steps 2 and 3, which happen before a key is asked for.
pub fn parse_header(bytes: &[u8]) -> Result<CheckpointHeader> {
    let mut reader = Reader::new(bytes);
    CheckpointHeader::decode(&mut reader)
}

/// Opens a checkpoint.
pub fn open_checkpoint(
    bytes: &[u8],
    wek: &WorkspaceEncryptionKey,
) -> Result<(CheckpointHeader, WorkspaceRecoveryBootstrapV1)> {
    let mut reader = Reader::new(bytes);
    let header = CheckpointHeader::decode(&mut reader)?;
    let header_bytes = reader.consumed().to_vec();
    let wrapped = reader.take(WRAPPED_KEY_LEN)?;
    let body = reader.rest();

    let dek = envelope::unwrap_key(&wek.as_input_key(), &header_bytes, wrapped)?;
    let plaintext = envelope::open_body(&dek.as_input_key(), &header_bytes, body)?;
    let bootstrap = WorkspaceRecoveryBootstrapV1::decode(&plaintext, header.wek_version)?;
    Ok((header, bootstrap))
}

#[cfg(test)]
mod tests;
