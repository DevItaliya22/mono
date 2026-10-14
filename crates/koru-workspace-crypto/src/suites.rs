// SPDX-License-Identifier: MPL-2.0

//! Suite ids, format versions and purposes — Koru's registry, not IANA's.
//!
//! **A suite id names a whole construction**, because that is the thing a
//! reader has to implement. RFC 9180's three code points name a KEM, a KDF and
//! an AEAD separately and say nothing about the mode, the `info` layout or the
//! envelope; a reader that saw only those three would still have to guess.
//! One 16-bit id per construction, defined here, is the smaller surface.
//!
//! **Unknown ids fail closed** (D91). There is no "try it and see", no
//! negotiation and no fall-back: a reader that meets an id it does not know
//! refuses the object.
//!
//! **The writer is opinionated** (D87). [`WRITER_SUITES`] is the allow-list for
//! *new writes* and holds exactly one suite per layer. Readers accept every id
//! in the tables below. Deprecating a suite means moving it out of
//! [`WRITER_SUITES`] and leaving it readable, which is a one-line change with a
//! golden vector that already proves the two halves are distinct.

use crate::error::{CryptoError, Result};

// ---------------------------------------------------------------- versions

/// The major version of every format in this crate's v1 family. A reader that
/// does not implement the version it finds refuses before decryption and never
/// falls back — the same rule the approved bootstrap contract states in §5.
pub const FORMAT_VERSION_V1: u16 = 1;

// ------------------------------------------------------------------ layer 1

/// A grant suite: HPKE mode, KEM, KDF and AEAD, plus this crate's envelope and
/// `info` layout. **Layer 1 only.** Nothing in this enum has any bearing on how
/// a blob, a segment or a snapshot is encrypted (D86).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GrantSuite {
    /// `0x0001` — HPKE Base mode (RFC 9180 §5.1.1) over
    /// DHKEM(X25519, HKDF-SHA256) `0x0020` + HKDF-SHA256 `0x0001` +
    /// ChaCha20Poly1305 `0x0003`. RFC 9180 appendix A.2.1 is its interop
    /// vector. **The one suite the writer emits for device and replica
    /// recipients.**
    X25519HkdfSha256ChaCha20Poly1305,
    /// `0x0002` — the same mode, KDF and AEAD over DHKEM(P-256, HKDF-SHA256)
    /// `0x0010`. RFC 9180 appendix A.5.1 is its interop vector.
    ///
    /// **This exists for one recipient and one reason (D88).** The regional
    /// recovery private key must never leave KMS/HSM protection, and a provider
    /// that cannot do X25519 in protected hardware can do P-256. Only the KEM
    /// changes: keeping HKDF-SHA256 and ChaCha20Poly1305 fixed means the second
    /// suite is one substitution away from the first rather than a second
    /// construction to review, and RFC 9180 publishes vectors for exactly this
    /// combination.
    ///
    /// It is **not** selectable for a device recipient. A per-grant KEM is the
    /// point: recipients need not share one.
    P256HkdfSha256ChaCha20Poly1305,
}

impl GrantSuite {
    pub const fn id(self) -> u16 {
        match self {
            Self::X25519HkdfSha256ChaCha20Poly1305 => 0x0001,
            Self::P256HkdfSha256ChaCha20Poly1305 => 0x0002,
        }
    }

    pub fn from_id(id: u16) -> Result<Self> {
        match id {
            0x0001 => Ok(Self::X25519HkdfSha256ChaCha20Poly1305),
            0x0002 => Ok(Self::P256HkdfSha256ChaCha20Poly1305),
            _ => Err(CryptoError::UnsupportedSuite),
        }
    }

    /// `Nenc` — the length of the HPKE encapsulated key, from RFC 9180 table 2.
    pub const fn encapsulation_len(self) -> usize {
        match self {
            Self::X25519HkdfSha256ChaCha20Poly1305 => 32,
            Self::P256HkdfSha256ChaCha20Poly1305 => 65,
        }
    }

    /// `Npk`. Equal to `Nenc` for both DHKEMs here.
    pub const fn public_key_len(self) -> usize {
        self.encapsulation_len()
    }

    /// The RFC 9180 code points, recorded so the format document and the ADR
    /// can be checked against the implementation rather than against memory.
    pub const fn rfc9180_ids(self) -> (u16, u16, u16) {
        match self {
            Self::X25519HkdfSha256ChaCha20Poly1305 => (0x0020, 0x0001, 0x0003),
            Self::P256HkdfSha256ChaCha20Poly1305 => (0x0010, 0x0001, 0x0003),
        }
    }
}

// ------------------------------------------------------------------ layer 2

/// A bulk-content suite. **`ContentCipherSuiteV1` is this enum**, and it is
/// reached without touching layer 1.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ContentSuite {
    /// `0x0001` — [c2sp.org/chunked-encryption](https://c2sp.org/chunked-encryption)
    /// v1, instantiated as **Cobblestone-256**: HKDF-Expand over SHA-512,
    /// AES-256-GCM, 16 KiB chunks, a 24-byte salt and a 32-byte key commitment.
    ///
    /// Chosen over inventing a framing because it already answers every clause
    /// of D86 — a per-message key and base nonce, a non-repeating XOR-counter
    /// nonce, truncation and reordering protection from the counter and the
    /// short-final-chunk rule, exactly one correct chunking per length, and a
    /// fixed 56-byte header that makes the plaintext-to-ciphertext offset
    /// relationship arithmetic rather than a lookup.
    Cobblestone256,
}

impl ContentSuite {
    pub const fn id(self) -> u16 {
        match self {
            Self::Cobblestone256 => 0x0001,
        }
    }

    pub fn from_id(id: u16) -> Result<Self> {
        match id {
            0x0001 => Ok(Self::Cobblestone256),
            _ => Err(CryptoError::UnsupportedSuite),
        }
    }
}

// ----------------------------------------------------------- writer policy

/// The suites a **new write** may use. Everything else in the tables above is
/// read-only.
///
/// This is the whole of Koru's downgrade prevention, and it is deliberately a
/// constant rather than anything a peer can influence. RFC 9180 §9.7.2 is
/// explicit that HPKE assumes the parties already agree and offers nothing
/// against an intermediary that forces a weaker choice; §9.7.1 and §9.7.3 are
/// equally explicit that ordering and replay are the application's job. Koru
/// never negotiates: a client writes this suite or it does not write.
pub struct WriterSuites {
    pub grant_device: GrantSuite,
    pub content: ContentSuite,
}

pub const WRITER_SUITES: WriterSuites = WriterSuites {
    grant_device: GrantSuite::X25519HkdfSha256ChaCha20Poly1305,
    content: ContentSuite::Cobblestone256,
};

impl GrantSuite {
    /// Whether a **new** grant may be written with this suite for this
    /// recipient. The recovery KEM is chosen by the recovery key's own material
    /// (D88), so both suites are writable there and only one is writable for a
    /// device.
    pub fn writable_for(self, recipient: RecipientType) -> bool {
        match recipient {
            RecipientType::RegionalRecovery => true,
            RecipientType::Device | RecipientType::CloudReplica => {
                self == WRITER_SUITES.grant_device
            }
        }
    }
}

impl ContentSuite {
    pub fn writable(self) -> bool {
        self == WRITER_SUITES.content
    }
}

// -------------------------------------------------------------- recipients

/// Who a grant is for. The value is bound into the HPKE `info`, so a grant
/// minted for a device cannot be presented as a recovery grant even if the key
/// material somehow matched.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RecipientType {
    /// A person's device, holding an X25519 encryption key distinct from its
    /// Ed25519 identity key (Plan 2.3).
    Device,
    /// A Cloud Peer replica, under managed encryption (D1).
    CloudReplica,
    /// The regional workspace-recovery/unwrap authority. **Absent entirely for
    /// a customer-managed workspace** (D89).
    RegionalRecovery,
}

impl RecipientType {
    pub const fn tag(self) -> u8 {
        match self {
            Self::Device => 1,
            Self::CloudReplica => 2,
            Self::RegionalRecovery => 3,
        }
    }

    pub fn from_tag(tag: u8) -> Result<Self> {
        match tag {
            1 => Ok(Self::Device),
            2 => Ok(Self::CloudReplica),
            3 => Ok(Self::RegionalRecovery),
            _ => Err(CryptoError::UnsupportedSuite),
        }
    }
}

/// Who signed a grant envelope.
///
/// **Only [`AuthorType::Device`] is exercised by this plan.** The second value
/// is defined now because adding one later would break every reader that fails
/// closed on an unknown tag — which is every reader here — and because custody
/// (Plan 2.2) will issue grants from a regional authority. It is recorded as a
/// question for the maintainers in the format document rather than assumed.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AuthorType {
    /// A device that already holds the WEK, granting it to another recipient
    /// during enrollment (Plan 2.4). Its Ed25519 key is the one `koru-device`
    /// already defines; no second identity is invented (D87).
    Device,
    /// A regional authority. Reserved; nothing writes it in this plan.
    RegionalAuthority,
}

impl AuthorType {
    pub const fn tag(self) -> u8 {
        match self {
            Self::Device => 1,
            Self::RegionalAuthority => 2,
        }
    }

    pub fn from_tag(tag: u8) -> Result<Self> {
        match tag {
            1 => Ok(Self::Device),
            2 => Ok(Self::RegionalAuthority),
            _ => Err(CryptoError::UnsupportedSuite),
        }
    }
}

// ---------------------------------------------------------------- purposes

/// What a ciphertext is *for*. One namespace, no value reused, and every value
/// bound into the key derivation of the record that carries it — so a
/// ciphertext sealed for one purpose cannot be opened as another even under the
/// same key. This is the mechanical half of D30's "no key reuse across the four
/// regional purposes"; [`crate::regional`] is the typed half.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum Purpose {
    /// Layer 1. The only purpose HPKE is ever used for in Koru.
    WekGrant = 0x0001,

    /// Layer 2 — a client-encrypted blob object.
    ContentBlobObject = 0x0010,
    /// Layer 2 — a Cloud-written update segment (Plan 4.2).
    ContentSegment = 0x0011,
    /// Layer 2 — a Cloud-written document snapshot (Plan 4.2).
    ContentSnapshot = 0x0012,
    /// Layer 2 — the `WorkspaceRecoveryCheckpoint` body (Plan 4.4).
    RecoveryCheckpoint = 0x0013,

    /// The encrypted `BlobHash` ↔ `CloudObjectId` mapping row (Plan 5.0).
    BlobMappingRecord = 0x0020,

    /// A regional account-metadata record (D56/D57). Its *contents* are Plan
    /// 3.2's; this crate owns only the envelope.
    AccountMetadataRecord = 0x0030,
    /// `Wrap(regional_metadata_KEK, account_metadata_key)` (D59).
    AccountMetadataKeyWrap = 0x0031,
}

impl Purpose {
    pub const fn id(self) -> u16 {
        self as u16
    }

    pub fn from_id(id: u16) -> Result<Self> {
        Ok(match id {
            0x0001 => Self::WekGrant,
            0x0010 => Self::ContentBlobObject,
            0x0011 => Self::ContentSegment,
            0x0012 => Self::ContentSnapshot,
            0x0013 => Self::RecoveryCheckpoint,
            0x0020 => Self::BlobMappingRecord,
            0x0030 => Self::AccountMetadataRecord,
            0x0031 => Self::AccountMetadataKeyWrap,
            _ => return Err(CryptoError::PurposeMismatch),
        })
    }
}

#[cfg(test)]
mod tests;
