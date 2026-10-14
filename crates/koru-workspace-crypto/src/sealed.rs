// SPDX-License-Identifier: MPL-2.0

//! **`ContentCipherSuiteV1`** — [c2sp.org/chunked-encryption](https://c2sp.org/chunked-encryption)
//! v1, instantiated as Cobblestone-256 (SHA-512 + AES-256-GCM).
//!
//! This module contains **no public-key cryptography and no reference to
//! layer 1**. It is the whole of how Koru encrypts customer data: blobs,
//! segments, snapshots, checkpoint bodies and small at-rest records. Changing
//! the grant suite does not reach it; changing this suite is the thing that
//! costs a re-encryption, which is why it is a published specification with
//! third-party test vectors rather than something written here.
//!
//! # The specification, and why each clause of D86 is already in it
//!
//! - *Random key per object.* For each message a `key`, `base nonce` and
//!   32-byte `commitment` are derived by HKDF-Expand from the input key, a
//!   24-byte random `salt` and an application `context`.
//! - *Deterministic, non-repeating per-chunk nonce.* The chunk nonce is the
//!   base nonce XOR the chunk index as a big-endian integer of the nonce's
//!   width. Within a message the index never repeats; across messages the salt
//!   makes the base nonce fresh.
//! - *Truncation and reordering protection.* The index is in the nonce, so a
//!   reordered or duplicated chunk decrypts under the wrong nonce and fails.
//!   The final chunk is **required to be shorter than 16 KiB**, so a truncation
//!   at a chunk boundary is detected as a missing final chunk rather than read
//!   as a shorter message.
//! - *Canonical chunking.* There is exactly one correct chunking of a message
//!   of a given length, and the empty message is one empty chunk.
//! - *Versioned framing.* The version is in the HKDF info string
//!   (`c2sp.org/chunked-encryption@v1+`), and Koru's own `format_version` and
//!   `suite_id` are in every header that reaches [`seal`] as `context`.
//!
//! # `context`, and why Koru's headers go there rather than into an AAD
//!
//! The specification says the AEAD's AAD **is not used and MUST be empty**;
//! application data binds through `context`, which enters the HKDF info. Koru
//! passes each record's complete fixed-width header as the `context`, so:
//!
//! - a modified header derives a different key, base nonce **and commitment**,
//!   and the reader fails at the commitment check *before any chunk is
//!   decrypted*;
//! - a body cannot be recombined with another record's header;
//! - the binding covers the whole header, in the exact bytes the parser read,
//!   with no second serialization that could disagree with the first.
//!
//! That is strictly stronger than binding the header as AEAD associated data
//! on the first chunk, which is the shape the approved bootstrap contract's
//! §9.1 describes. The contract's requirements — that H1–H5 in full and in
//! canonical order are what the ciphertext is bound to, and that the
//! serialization be unambiguous — are met; the mechanism differs and the format
//! document says so.
//!
//! # What this module deliberately does not do
//!
//! It does not choose a key, does not know what a workspace is, and does not
//! interpret `context`. Those belong to [`crate::content`],
//! [`crate::checkpoint`], [`crate::mapping`] and [`crate::account`], which is
//! the split PRINCIPLES.md asks for: the mechanism goes down, the knowledge of
//! what it is for stays with its subject.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use hmac::{Hmac, Mac};
use sha2::Sha512;
use zeroize::{Zeroize, Zeroizing};

use crate::codec::constant_time_eq;
use crate::error::{CryptoError, Result};

/// 16 KiB, from the specification. Not configurable: the chunk size is part of
/// the format, and a second value would be a second format.
pub const CHUNK_LEN: usize = 16 * 1024;
/// AES-256-GCM's tag, `Nt` = 16.
pub const TAG_LEN: usize = 16;
/// The salt, 24 bytes, prepended to the ciphertext.
pub const SALT_LEN: usize = 24;
/// The key commitment, 32 bytes, following the salt.
pub const COMMITMENT_LEN: usize = 32;
/// The fixed overhead the specification names: salt plus commitment.
pub const SEALED_HEADER_LEN: usize = SALT_LEN + COMMITMENT_LEN;
/// AES-256-GCM's `Nk`.
pub const KEY_LEN: usize = 32;
/// AES-256-GCM's `Nn`.
pub const NONCE_LEN: usize = 12;
/// One chunk on the wire.
pub const CHUNK_CIPHERTEXT_LEN: usize = CHUNK_LEN + TAG_LEN;
/// 2^38. The specification aborts when the chunk counter reaches this.
pub const MAX_CHUNKS: u64 = 1 << 38;

/// The IANA AEAD registry name, which the specification puts in the HKDF info
/// so that two instantiations cannot collide.
const AEAD_NAME: &[u8] = b"AEAD_AES_256_GCM";
const INFO_PREFIX: &[u8] = b"c2sp.org/chunked-encryption@v1+";

/// A 32-byte input key. Zeroized on drop.
///
/// The specification requires it to be **uniformly random** — from a CSPRNG or
/// a KDF output. Every construction in this crate satisfies that: a DEK is
/// drawn from the OS CSPRNG, a WEK likewise, and an account metadata key is
/// random by D59 and explicitly *not* `KDF(regional_key, account_id)`.
#[derive(Clone)]
pub struct InputKey([u8; KEY_LEN]);

impl InputKey {
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    pub fn generate(rng: &mut impl rand_core::CryptoRng) -> Self {
        let mut bytes = [0u8; KEY_LEN];
        rng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

impl Drop for InputKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl std::fmt::Debug for InputKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InputKey([REDACTED])")
    }
}

/// The per-message values the KDF produces. Zeroized on drop.
struct Derived {
    key: [u8; KEY_LEN],
    base_nonce: [u8; NONCE_LEN],
    commitment: [u8; COMMITMENT_LEN],
}

impl Drop for Derived {
    fn drop(&mut self) {
        self.key.zeroize();
        self.base_nonce.zeroize();
        // The commitment is public — it is on the wire — but zeroizing the
        // whole struct is cheaper to reason about than remembering which third
        // of it was not a secret.
        self.commitment.zeroize();
    }
}

/// `HKDF-Expand` over SHA-512, per RFC 5869 §2.3.
///
/// **Written out rather than taken from the `hkdf` crate**, and the reason is
/// the one PRINCIPLES.md asks to be recorded: `Hkdf::from_prk` rejects a
/// pseudorandom key shorter than the hash output, and this scheme expands a
/// 32-byte input key with SHA-512 on purpose. There is no way to express that
/// through the crate's API, and the alternative — a 64-byte input key — would
/// be a different format from the published one and from its test vectors.
/// What is left is fifteen lines over `hmac`, which is already in the tree,
/// and every byte of it is pinned by the Wycheproof vectors' `aeadKey` and
/// `baseNonce`.
fn hkdf_expand_sha512(prk: &[u8], info: &[u8], okm: &mut [u8]) {
    type HmacSha512 = Hmac<Sha512>;
    const HASH_LEN: usize = 64;

    let blocks = okm.len().div_ceil(HASH_LEN);
    debug_assert!(blocks <= 255, "L must not exceed 255 * HashLen");

    let mut previous = Zeroizing::new([0u8; HASH_LEN]);
    let mut have_previous = false;

    for block in 1..=blocks {
        let mut mac =
            <HmacSha512 as Mac>::new_from_slice(prk).expect("HMAC accepts a key of any length");
        if have_previous {
            mac.update(previous.as_ref());
        }
        mac.update(info);
        mac.update(&[block as u8]);
        previous.copy_from_slice(&mac.finalize().into_bytes());
        have_previous = true;

        let start = (block - 1) * HASH_LEN;
        let end = usize::min(start + HASH_LEN, okm.len());
        okm[start..end].copy_from_slice(&previous[..end - start]);
    }
}

fn derive(input_key: &InputKey, salt: &[u8; SALT_LEN], context: &[u8]) -> Derived {
    // info = "c2sp.org/chunked-encryption@v1+" || aead || 0x00 || salt || context
    let mut info =
        Vec::with_capacity(INFO_PREFIX.len() + AEAD_NAME.len() + 1 + SALT_LEN + context.len());
    info.extend_from_slice(INFO_PREFIX);
    info.extend_from_slice(AEAD_NAME);
    info.push(0x00);
    info.extend_from_slice(salt);
    info.extend_from_slice(context);

    let mut okm = Zeroizing::new([0u8; KEY_LEN + NONCE_LEN + COMMITMENT_LEN]);
    hkdf_expand_sha512(input_key.as_bytes(), &info, okm.as_mut());

    let mut derived = Derived {
        key: [0u8; KEY_LEN],
        base_nonce: [0u8; NONCE_LEN],
        commitment: [0u8; COMMITMENT_LEN],
    };
    derived.key.copy_from_slice(&okm[..KEY_LEN]);
    derived
        .base_nonce
        .copy_from_slice(&okm[KEY_LEN..KEY_LEN + NONCE_LEN]);
    derived
        .commitment
        .copy_from_slice(&okm[KEY_LEN + NONCE_LEN..]);
    derived
}

/// The chunk nonce: the base nonce XOR the index as a big-endian integer as
/// wide as the nonce.
fn chunk_nonce(base_nonce: &[u8; NONCE_LEN], index: u64) -> [u8; NONCE_LEN] {
    let mut nonce = *base_nonce;
    let counter = index.to_be_bytes();
    // `u64` is eight bytes and the nonce is twelve, so the counter lands in the
    // low eight and the high four are untouched — which is the same thing as
    // XORing a twelve-byte big-endian integer whose top four bytes are zero.
    // The index can never exceed 2^38, so those bytes are always zero.
    for (n, c) in nonce[NONCE_LEN - 8..].iter_mut().zip(counter.iter()) {
        *n ^= *c;
    }
    nonce
}

// ------------------------------------------------------------------ layout

/// What a well-formed ciphertext of a given length must look like.
///
/// **This is D40, and it is the whole of it.** Given a ciphertext length, the
/// exact plaintext length follows by arithmetic — no key, no decryption, no
/// trust in anything the writer claimed. What it proves and what it does not is
/// stated on [`SealedLayout::plaintext_len`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SealedLayout {
    ciphertext_len: u64,
    chunk_count: u64,
    plaintext_len: u64,
}

impl SealedLayout {
    /// Validates a ciphertext length and derives the plaintext length.
    ///
    /// Every rejection the specification names is here: a ciphertext shorter
    /// than the 56-byte header; a body with no final chunk; a body whose length
    /// is an exact multiple of `16 KiB + tag`, which means the final chunk was
    /// full length and the message is therefore truncated; and a final chunk
    /// shorter than a tag. The chunk count is bounded at 2^38 before it is
    /// used for anything.
    pub fn of_ciphertext_len(ciphertext_len: u64) -> Result<Self> {
        let body = ciphertext_len
            .checked_sub(SEALED_HEADER_LEN as u64)
            .ok_or(CryptoError::Malformed)?;
        if body == 0 {
            // Even an empty message has a final chunk, and a final chunk is at
            // least a tag.
            return Err(CryptoError::Malformed);
        }

        let stride = CHUNK_CIPHERTEXT_LEN as u64;
        let chunk_count = body.div_ceil(stride);
        if chunk_count > MAX_CHUNKS {
            return Err(CryptoError::TooLarge);
        }

        let final_chunk_len = body - (chunk_count - 1) * stride;
        // `< TAG_LEN` is the "shorter than a tag" case; `>= stride` cannot
        // happen given `div_ceil`, but the equality case can and means the
        // final chunk was a full 16 KiB, which the specification defines as
        // truncated.
        if final_chunk_len < TAG_LEN as u64 || final_chunk_len >= stride {
            return Err(CryptoError::Malformed);
        }

        let plaintext_len = body - chunk_count * TAG_LEN as u64;
        Ok(Self {
            ciphertext_len,
            chunk_count,
            plaintext_len,
        })
    }

    /// The layout a plaintext of this length will produce.
    pub fn of_plaintext_len(plaintext_len: u64) -> Result<Self> {
        let chunk_count = plaintext_len / CHUNK_LEN as u64 + 1;
        if chunk_count > MAX_CHUNKS {
            return Err(CryptoError::TooLarge);
        }
        let ciphertext_len =
            SEALED_HEADER_LEN as u64 + plaintext_len + chunk_count * TAG_LEN as u64;
        Ok(Self {
            ciphertext_len,
            chunk_count,
            plaintext_len,
        })
    }

    /// The exact plaintext length **of a valid encrypted object**.
    ///
    /// It is derived from framing the server can validate without a key. It is
    /// **not** a statement about the user's original local file: a client that
    /// encrypted something other than that file produces a valid object of the
    /// length it actually encrypted. What the server learns is the length of
    /// the plaintext inside *this* object, and length exposure is a deliberate
    /// property of the design rather than an accident — HPKE does not hide
    /// plaintext length either (RFC 9180 §9.7.6).
    pub fn plaintext_len(self) -> u64 {
        self.plaintext_len
    }

    pub fn ciphertext_len(self) -> u64 {
        self.ciphertext_len
    }

    pub fn chunk_count(self) -> u64 {
        self.chunk_count
    }

    /// The ciphertext offset of chunk `index`, and its length. The offset is
    /// `56 + k * (16384 + 16)`, which is what makes random access predictable.
    pub fn chunk_at(self, index: u64) -> Result<(u64, u64)> {
        if index >= self.chunk_count {
            return Err(CryptoError::Malformed);
        }
        let stride = CHUNK_CIPHERTEXT_LEN as u64;
        let offset = SEALED_HEADER_LEN as u64 + index * stride;
        let len = if index + 1 == self.chunk_count {
            self.ciphertext_len - offset
        } else {
            stride
        };
        Ok((offset, len))
    }
}

// -------------------------------------------------------------- one-shot

/// Seals `plaintext` under `input_key`, bound to `context`.
///
/// `salt` MUST NOT repeat for a given `input_key` and SHOULD be random; every
/// caller in this crate draws it from the CSPRNG it was handed. It is exposed
/// rather than drawn here so that the golden vectors are reproducible — the
/// same reason RFC 9180's own test vectors publish `ikmE`.
pub fn seal(
    input_key: &InputKey,
    salt: &[u8; SALT_LEN],
    context: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    let layout = SealedLayout::of_plaintext_len(plaintext.len() as u64)?;
    let derived = derive(input_key, salt, context);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&derived.key));

    let mut out = Vec::with_capacity(
        usize::try_from(layout.ciphertext_len).map_err(|_| CryptoError::TooLarge)?,
    );
    out.extend_from_slice(salt);
    out.extend_from_slice(&derived.commitment);

    for index in 0..layout.chunk_count {
        let start = usize::try_from(index * CHUNK_LEN as u64).map_err(|_| CryptoError::TooLarge)?;
        let end = usize::min(start + CHUNK_LEN, plaintext.len());
        let nonce = chunk_nonce(&derived.base_nonce, index);
        let sealed = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &plaintext[start..end],
                    aad: b"",
                },
            )
            .map_err(|_| CryptoError::Aead)?;
        out.extend_from_slice(&sealed);
    }

    debug_assert_eq!(out.len() as u64, layout.ciphertext_len);
    Ok(out)
}

/// Opens a complete ciphertext.
///
/// The commitment is checked before any chunk is decrypted, so a wrong key or a
/// modified header costs one HKDF and nothing else. Suitable for records; a
/// multi-gigabyte object is opened chunk by chunk through [`Opener`] instead.
pub fn open(input_key: &InputKey, context: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
    let opener = Opener::new(input_key, context, ciphertext.len() as u64, ciphertext)?;
    let mut out = Vec::with_capacity(
        usize::try_from(opener.layout().plaintext_len()).map_err(|_| CryptoError::TooLarge)?,
    );
    for index in 0..opener.layout().chunk_count() {
        let (offset, len) = opener.layout().chunk_at(index)?;
        let start = usize::try_from(offset).map_err(|_| CryptoError::Malformed)?;
        let end = start
            .checked_add(usize::try_from(len).map_err(|_| CryptoError::Malformed)?)
            .ok_or(CryptoError::Malformed)?;
        out.extend_from_slice(&opener.open_chunk(index, &ciphertext[start..end])?);
    }
    Ok(out)
}

// -------------------------------------------------- streaming / random access

/// A validated reader: the layout is fixed, the commitment has been checked,
/// and chunks can be opened in any order.
///
/// Random access is why this scheme was chosen over inventing one, and it is
/// what Plan 5.3's hydration and Plan 4.5's status surface will use. The
/// specification's own warning applies and is enforced by
/// [`SealedLayout::of_ciphertext_len`]: **do not assume the final encrypted
/// chunk is longer than a tag**, because assuming it is how a parser reaches a
/// negative length.
pub struct Opener {
    derived: Derived,
    layout: SealedLayout,
}

impl Opener {
    /// `header` must contain at least the first [`SEALED_HEADER_LEN`] bytes of
    /// the ciphertext; `total_len` is the length of the whole object, which a
    /// streaming caller knows from the object store before it has the bytes.
    pub fn new(
        input_key: &InputKey,
        context: &[u8],
        total_len: u64,
        header: &[u8],
    ) -> Result<Self> {
        let layout = SealedLayout::of_ciphertext_len(total_len)?;
        if header.len() < SEALED_HEADER_LEN {
            return Err(CryptoError::Malformed);
        }
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&header[..SALT_LEN]);
        let derived = derive(input_key, &salt, context);
        if !constant_time_eq(&derived.commitment, &header[SALT_LEN..SEALED_HEADER_LEN]) {
            return Err(CryptoError::Commitment);
        }
        Ok(Self { derived, layout })
    }

    pub fn layout(&self) -> SealedLayout {
        self.layout
    }

    /// Opens one chunk. `ciphertext` is exactly the bytes
    /// [`SealedLayout::chunk_at`] names for this index; anything else fails,
    /// because the length is part of what the tag authenticates.
    pub fn open_chunk(&self, index: u64, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let (_, expected) = self.layout.chunk_at(index)?;
        if ciphertext.len() as u64 != expected {
            return Err(CryptoError::Malformed);
        }
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.derived.key));
        let nonce = chunk_nonce(&self.derived.base_nonce, index);
        cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: ciphertext,
                    aad: b"",
                },
            )
            .map_err(|_| CryptoError::Aead)
    }
}

#[cfg(test)]
mod tests;
