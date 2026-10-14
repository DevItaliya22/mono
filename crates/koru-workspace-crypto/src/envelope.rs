// SPDX-License-Identifier: MPL-2.0

//! The two shapes every at-rest record in Koru takes, and the rule for which.
//!
//! Both are built from [`crate::sealed`] alone — no public-key cryptography
//! reaches this module or anything below it.
//!
//! # Shape one: a sealed record
//!
//! `header || sealed(key, context = header ‖ BODY_TAG, plaintext)`
//!
//! Used where the plaintext is small and re-encrypting it is free: the blob
//! mapping row, the account-metadata envelope, the wrapped account metadata
//! key.
//!
//! # Shape two: a sealed object
//!
//! `header || wrapped_dek || sealed(dek, context = header ‖ BODY_TAG, plaintext)`
//!
//! Used where the plaintext is large: blob objects, segments, snapshots, and
//! the recovery checkpoint. D86 requires a random DEK per object; what the
//! extra 104 bytes buy here is **per-object crypto-erasure** — destroying one
//! object's wrap makes that object unrecoverable even to a holder of the WEK,
//! which is what Plan 5.4's purge needs — and the property that one object's
//! key is not the workspace's.
//!
//! **It deliberately does not buy a cheap re-key.** The whole header, including
//! `wek_version`, is the derivation context of the body as well as of the wrap,
//! so rewrapping under a new WEK version means re-encrypting the body. Buying
//! the cheap re-key would mean leaving `wek_version` out of the body's binding,
//! and there is nothing to buy: §33 and D14/D17 say historical objects are not
//! rewritten on rotation at all — each keeps the WEK version it names, and the
//! grant for that version keeps existing.
//!
//! # Layer tags
//!
//! The wrap and the body derive from the same header bytes under different
//! keys, which is already enough to keep them apart. They are separated
//! anyway — one trailing byte — because "different keys" is an argument and
//! "different context" is a fact, and the byte costs nothing.

use crate::error::{CryptoError, Result};
use crate::keys::ContentEncryptionKey;
use crate::sealed::{self, InputKey, KEY_LEN, SALT_LEN, SEALED_HEADER_LEN, SealedLayout, TAG_LEN};

/// A wrapped 32-byte key: salt, commitment, one chunk, one tag.
pub const WRAPPED_KEY_LEN: usize = SEALED_HEADER_LEN + KEY_LEN + TAG_LEN;

const WRAP_TAG: u8 = 0x01;
const BODY_TAG: u8 = 0x02;

fn context(header: &[u8], tag: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(header.len() + 1);
    out.extend_from_slice(header);
    out.push(tag);
    out
}

/// Wraps `dek` under `kek`, bound to `header`.
pub fn wrap_key(
    kek: &InputKey,
    header: &[u8],
    dek: &ContentEncryptionKey,
    salt: &[u8; SALT_LEN],
) -> Result<Vec<u8>> {
    let wrapped = sealed::seal(kek, salt, &context(header, WRAP_TAG), dek.as_bytes())?;
    debug_assert_eq!(wrapped.len(), WRAPPED_KEY_LEN);
    Ok(wrapped)
}

/// Unwraps a DEK. A wrong KEK fails at the commitment, not at a tag, so the
/// failure says "wrong key" rather than "modified object".
pub fn unwrap_key(kek: &InputKey, header: &[u8], wrapped: &[u8]) -> Result<ContentEncryptionKey> {
    if wrapped.len() != WRAPPED_KEY_LEN {
        return Err(CryptoError::Malformed);
    }
    let plaintext = sealed::open(kek, &context(header, WRAP_TAG), wrapped)?;
    let bytes: [u8; KEY_LEN] = plaintext.as_slice().try_into().map_err(|_| {
        // A 104-byte wrap can only contain 32 bytes, so this is unreachable
        // through the format; it is here because "unreachable" is a claim about
        // today's constants.
        CryptoError::Malformed
    })?;
    Ok(ContentEncryptionKey::from_bytes(bytes))
}

/// Seals a body under `key`, bound to `header`.
pub fn seal_body(
    key: &InputKey,
    header: &[u8],
    salt: &[u8; SALT_LEN],
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    sealed::seal(key, salt, &context(header, BODY_TAG), plaintext)
}

/// Opens a complete body.
pub fn open_body(key: &InputKey, header: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
    sealed::open(key, &context(header, BODY_TAG), ciphertext)
}

/// An [`sealed::Opener`] over a body, for a payload too large to hold in
/// memory. `total_len` is the body's length, not the whole record's.
pub fn body_opener(
    key: &InputKey,
    header: &[u8],
    total_len: u64,
    first_bytes: &[u8],
) -> Result<sealed::Opener> {
    sealed::Opener::new(key, &context(header, BODY_TAG), total_len, first_bytes)
}

/// The plaintext length of a **sealed object** of this stored length, given the
/// fixed width of its header.
///
/// This is D40 as a server applies it: subtract the two fixed overheads it can
/// see and check, then let [`SealedLayout`] validate the rest. Every step is
/// arithmetic over lengths the object plane already has, and every failure is a
/// rejection rather than a guess.
pub fn object_plaintext_len(header_len: usize, stored_len: u64) -> Result<u64> {
    let fixed = (header_len as u64)
        .checked_add(WRAPPED_KEY_LEN as u64)
        .ok_or(CryptoError::Malformed)?;
    let body = stored_len
        .checked_sub(fixed)
        .ok_or(CryptoError::Malformed)?;
    Ok(SealedLayout::of_ciphertext_len(body)?.plaintext_len())
}

#[cfg(test)]
mod tests;
