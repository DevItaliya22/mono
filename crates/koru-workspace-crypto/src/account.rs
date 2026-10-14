// SPDX-License-Identifier: MPL-2.0

//! **The generic account-metadata envelope (D56/D57/D59).**
//!
//! This plan owns the **envelope** and deliberately does not know what goes
//! inside it. Which entries a purged account record carries — the tombstone's
//! display name, its causal manifest, its field table — is regional
//! account-domain behaviour and belongs with its domain (Plan 3.2). No client
//! implements it, no client holds the key, and this module's plaintext is a
//! byte string with a `record_type` it does not interpret.
//!
//! # The hierarchy (D59)
//!
//! ```text
//! regional metadata KEK  (KMS, one per region, symmetric)
//!   └── wraps ──►  per-account metadata key  (random, one per account)
//!                    └── encrypts ──►  every account-metadata record
//! ```
//!
//! The per-account key is **random**. It is not `KDF(regional_key, account_id)`
//! and must never become that: a derived key can be recomputed from public
//! inputs, so destroying it would destroy nothing and individual crypto-erasure
//! would be a fiction.
//!
//! # The wrapped key is stored once, and that is load-bearing
//!
//! [`WrappedAccountMetadataKeyV1`] is **one record per account**, and
//! [`AccountMetadataEnvelopeV1`] carries only a *reference* to it — a key id
//! and a version. If the wrapped key were copied into each envelope, destroying
//! "the" wrapped key would erase nothing, because every envelope would still
//! carry a copy. Crypto-erasure only means anything if there is exactly one
//! thing to destroy.
//!
//! # What crypto-erasure actually covers, stated honestly (D90)
//!
//! Destroying the wrapped per-account metadata key renders unrecoverable
//! **ciphertext whose only remaining decryption path is that key.** That is a
//! real and useful property and it is not the same sentence as "deletion erases
//! their data". It is defeated by:
//!
//! - plaintext in logs, traces or crash reports;
//! - a search index built over the plaintext;
//! - caches, in the service or in front of it;
//! - analytics events carrying the values;
//! - **backups of the unwrapped key**, or of anything holding it.
//!
//! Those are operational controls, not cryptographic ones, and no format
//! decision here can supply them. What this format supplies is that there is
//! exactly one key, that it is random, and that it is stored in exactly one
//! place.
//!
//! # Region
//!
//! This envelope carries a `region`, unlike the client-held formats. The
//! standing requirement is that every schema, token and config carries one;
//! the client formats are the exception the approved bootstrap contract argued
//! and the maintainers settled (§10.5 — the regional key hierarchies already make
//! cross-region use impossible, and a checkpoint fetched from a regional
//! endpoint already knows its region). A server-side record has no such
//! argument available to it, so it carries the field.

use uuid::Uuid;

use crate::codec::{Reader, Writer};
use crate::envelope;
use crate::error::{CryptoError, Result};
use crate::keys::{AccountMetadataKey, KeyId, RegionalMetadataKek};
use crate::regional::MetadataKekId;
use crate::sealed::{KEY_LEN, SALT_LEN};
use crate::suites::{ContentSuite, FORMAT_VERSION_V1, Purpose};

/// A region code, fixed at 8 ASCII bytes, zero-padded. Fixed width because
/// every header in this crate is.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Region([u8; 8]);

impl Region {
    pub const US: Self = Self(*b"us\0\0\0\0\0\0");
    pub const EU: Self = Self(*b"eu\0\0\0\0\0\0");

    /// Refuses anything that is not lowercase ASCII letters or digits followed
    /// by zero padding — a region is a stable code, never a translated label.
    pub fn parse(code: &str) -> Result<Self> {
        if code.is_empty() || code.len() > 8 {
            return Err(CryptoError::Malformed);
        }
        if !code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(CryptoError::Malformed);
        }
        let mut bytes = [0u8; 8];
        bytes[..code.len()].copy_from_slice(code.as_bytes());
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8; 8] {
        &self.0
    }

    pub fn from_bytes(bytes: [u8; 8]) -> Result<Self> {
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        if bytes[end..].iter().any(|b| *b != 0) {
            return Err(CryptoError::Malformed);
        }
        let code = std::str::from_utf8(&bytes[..end]).map_err(|_| CryptoError::Malformed)?;
        Self::parse(code)
    }
}

impl std::fmt::Debug for Region {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let end = self.0.iter().position(|b| *b == 0).unwrap_or(self.0.len());
        write!(f, "Region({})", String::from_utf8_lossy(&self.0[..end]))
    }
}

/// Names one account metadata key: which account, which key, which version, in
/// which region.
///
/// **It does not name the KEK.** Which KEK wraps this key is a fact about the
/// *wrapped-key record*, and it changes on a KEK rotation. If it lived here it
/// would be copied into every envelope and every copy would go stale the moment
/// the KEK rotated — a field nothing could act on, and, per PRINCIPLES, a field
/// that exists is a field somebody will compare. The service finds the wrapped
/// key by `(account_id, key_id, key_version)`; **that** record says which KEK
/// opens it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AccountMetadataKeyRef {
    pub account_id: Uuid,
    pub key_id: KeyId,
    pub key_version: u32,
    pub region: Region,
}

impl AccountMetadataKeyRef {
    fn write(&self, writer: &mut Writer) {
        writer
            .uuid(self.account_id)
            .bytes(self.key_id.as_bytes())
            .u32(self.key_version)
            .bytes(self.region.as_bytes());
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            account_id: reader.uuid()?,
            key_id: KeyId::from_bytes(reader.array::<16>()?),
            key_version: reader.u32()?,
            region: Region::from_bytes(reader.array::<8>()?)?,
        })
    }

    const WIDTH: usize = 16 + 16 + 4 + 8;
}

/// The wrapped-key record's own header: which key, and which KEK wraps it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WrappedKeyRef {
    pub key_ref: AccountMetadataKeyRef,
    pub kek_id: MetadataKekId,
    pub kek_version: u32,
}

/// `Wrap(regional_metadata_KEK, account_metadata_key)`.
///
/// **One per account.** Destroying it is the crypto-erasure.
pub const WRAPPED_KEY_HEADER_LEN: usize = 2 + 2 + 2 + AccountMetadataKeyRef::WIDTH + 16 + 4;

pub struct WrappedAccountMetadataKeyV1;

impl WrappedAccountMetadataKeyV1 {
    pub const PURPOSE: Purpose = Purpose::AccountMetadataKeyWrap;

    fn header(reference: &WrappedKeyRef) -> Vec<u8> {
        let mut writer = Writer::with_capacity(WRAPPED_KEY_HEADER_LEN);
        writer
            .u16(FORMAT_VERSION_V1)
            .u16(ContentSuite::Cobblestone256.id())
            .u16(Self::PURPOSE.id());
        reference.key_ref.write(&mut writer);
        writer
            .bytes(reference.kek_id.key_id().as_bytes())
            .u32(reference.kek_version);
        let bytes = writer.finish();
        debug_assert_eq!(bytes.len(), WRAPPED_KEY_HEADER_LEN);
        bytes
    }

    fn parse_header(reader: &mut Reader<'_>) -> Result<WrappedKeyRef> {
        let format_version = reader.u16()?;
        if format_version != FORMAT_VERSION_V1 {
            return Err(CryptoError::UnsupportedFormatVersion);
        }
        ContentSuite::from_id(reader.u16()?)?;
        if Purpose::from_id(reader.u16()?)? != Self::PURPOSE {
            return Err(CryptoError::PurposeMismatch);
        }
        let key_ref = AccountMetadataKeyRef::read(reader)?;
        let kek_id = MetadataKekId::new(KeyId::from_bytes(reader.array::<16>()?));
        let kek_version = reader.u32()?;
        Ok(WrappedKeyRef {
            key_ref,
            kek_id,
            kek_version,
        })
    }

    pub fn seal(
        reference: &WrappedKeyRef,
        account_key: &AccountMetadataKey,
        kek: &RegionalMetadataKek,
        rng: &mut impl rand_core::CryptoRng,
    ) -> Result<Vec<u8>> {
        let header = Self::header(reference);
        let mut salt = [0u8; SALT_LEN];
        rng.fill_bytes(&mut salt);
        let sealed =
            envelope::seal_body(&kek.as_input_key(), &header, &salt, account_key.as_bytes())?;
        let mut out = Vec::with_capacity(header.len() + sealed.len());
        out.extend_from_slice(&header);
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    /// Reads the record's cleartext header without the KEK — which key it holds
    /// and which KEK version opens it. This is how a rotation job finds the
    /// records it must rewrap.
    pub fn parse(bytes: &[u8]) -> Result<WrappedKeyRef> {
        let mut reader = Reader::new(bytes);
        Self::parse_header(&mut reader)
    }

    pub fn open(
        bytes: &[u8],
        kek: &RegionalMetadataKek,
    ) -> Result<(WrappedKeyRef, AccountMetadataKey)> {
        let mut reader = Reader::new(bytes);
        let reference = Self::parse_header(&mut reader)?;
        let header = reader.consumed().to_vec();
        let plaintext = envelope::open_body(&kek.as_input_key(), &header, reader.rest())?;
        let key: [u8; KEY_LEN] = plaintext
            .as_slice()
            .try_into()
            .map_err(|_| CryptoError::Malformed)?;
        Ok((reference, AccountMetadataKey::from_bytes(key)))
    }
}

/// One account-metadata record.
///
/// `record_type` is **opaque to this crate**. Plan 3.2 owns the values and the
/// field tables behind them; putting that knowledge here would give a
/// cryptography crate an opinion about account-domain behaviour, and would put
/// the tombstone's shape under a plan that never reads it.
pub const ENVELOPE_HEADER_LEN: usize = 2 + 2 + 2 + 2 + AccountMetadataKeyRef::WIDTH;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountMetadataEnvelopeV1 {
    pub key_ref: AccountMetadataKeyRef,
    pub record_type: u16,
}

impl AccountMetadataEnvelopeV1 {
    pub const PURPOSE: Purpose = Purpose::AccountMetadataRecord;

    pub fn new(key_ref: AccountMetadataKeyRef, record_type: u16) -> Self {
        Self {
            key_ref,
            record_type,
        }
    }

    fn header(&self) -> Vec<u8> {
        let mut writer = Writer::with_capacity(ENVELOPE_HEADER_LEN);
        writer
            .u16(FORMAT_VERSION_V1)
            .u16(ContentSuite::Cobblestone256.id())
            .u16(Self::PURPOSE.id())
            .u16(self.record_type);
        self.key_ref.write(&mut writer);
        let bytes = writer.finish();
        debug_assert_eq!(bytes.len(), ENVELOPE_HEADER_LEN);
        bytes
    }

    fn parse_header(reader: &mut Reader<'_>) -> Result<Self> {
        let format_version = reader.u16()?;
        if format_version != FORMAT_VERSION_V1 {
            return Err(CryptoError::UnsupportedFormatVersion);
        }
        ContentSuite::from_id(reader.u16()?)?;
        if Purpose::from_id(reader.u16()?)? != Self::PURPOSE {
            return Err(CryptoError::PurposeMismatch);
        }
        let record_type = reader.u16()?;
        let key_ref = AccountMetadataKeyRef::read(reader)?;
        Ok(Self {
            key_ref,
            record_type,
        })
    }

    /// Seals an opaque plaintext under the account metadata key.
    pub fn seal(
        &self,
        account_key: &AccountMetadataKey,
        plaintext: &[u8],
        rng: &mut impl rand_core::CryptoRng,
    ) -> Result<Vec<u8>> {
        let header = self.header();
        let mut salt = [0u8; SALT_LEN];
        rng.fill_bytes(&mut salt);
        let sealed = envelope::seal_body(&account_key.as_input_key(), &header, &salt, plaintext)?;
        let mut out = Vec::with_capacity(header.len() + sealed.len());
        out.extend_from_slice(&header);
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    /// Reads the envelope's cleartext header — which account, which key, which
    /// record type — without the key. This is how the rotation job finds every
    /// envelope that must be re-encrypted.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(bytes);
        Self::parse_header(&mut reader)
    }

    pub fn open(bytes: &[u8], account_key: &AccountMetadataKey) -> Result<(Self, Vec<u8>)> {
        let mut reader = Reader::new(bytes);
        let envelope_header = Self::parse_header(&mut reader)?;
        let header = reader.consumed().to_vec();
        let plaintext = envelope::open_body(&account_key.as_input_key(), &header, reader.rest())?;
        Ok((envelope_header, plaintext))
    }
}

/// Re-encrypts an envelope under a new account metadata key.
///
/// **Rotation of the KEK does not reach here, and does not leave a stale field
/// behind either.** A new KEK version rewraps the account key — one record per
/// account — and every envelope stays byte-identical, because an envelope is
/// encrypted under the *account key* and does not name the KEK at all.
///
/// Rotating the *account key* is the expensive one and is what this function is
/// for. It is also, deliberately, the same operation as crypto-erasure with a
/// replacement, which is why it is spelled out rather than left implicit.
pub fn rotate_envelope(
    bytes: &[u8],
    old_key: &AccountMetadataKey,
    new_ref: &AccountMetadataKeyRef,
    new_key: &AccountMetadataKey,
    rng: &mut impl rand_core::CryptoRng,
) -> Result<Vec<u8>> {
    let (envelope, plaintext) = AccountMetadataEnvelopeV1::open(bytes, old_key)?;
    if new_ref.account_id != envelope.key_ref.account_id {
        return Err(CryptoError::ContextMismatch);
    }
    AccountMetadataEnvelopeV1::new(*new_ref, envelope.record_type).seal(new_key, &plaintext, rng)
}

#[cfg(test)]
mod tests;
