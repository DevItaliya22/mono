// SPDX-License-Identifier: MPL-2.0

//! One error type, and it never says which byte was wrong.
//!
//! A cryptographic failure is reported as *what the reader was doing* and never
//! as *what the attacker got right*. `Commitment` and `Aead` are separate
//! variants because they mean different things to an operator — a wrong key
//! against a well-formed object, versus a modified object — and neither carries
//! an offset, an expected value or a length.

use std::fmt;

pub type Result<T> = std::result::Result<T, CryptoError>;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CryptoError {
    /// The buffer is shorter than its own fixed-width header requires, or a
    /// declared length does not fit what is present. **Never an arithmetic
    /// result** — every length in this crate is computed in `u64` with checked
    /// operations, and this is what a failed check reports.
    Malformed,
    /// A `format_version` this build does not implement. Distinct from
    /// [`CryptoError::Malformed`] because only one of the two is fixed by
    /// updating; the caller maps it to `CLOUD_SCHEMA_UPDATE_REQUIRED`.
    UnsupportedFormatVersion,
    /// A `suite_id` this build does not implement, or one it implements but
    /// refuses in this position. Unknown suites fail closed (D91).
    UnsupportedSuite,
    /// A suite this build can still *read* but may not use for a new write.
    /// Raised by the writer, never by the reader.
    DeprecatedSuiteForWrite,
    /// A `purpose` that is not the one this format carries, or a key offered
    /// for a purpose it does not belong to (D30).
    PurposeMismatch,
    /// The envelope is well-formed and describes a different context than the
    /// caller declared — another workspace, another recipient, another key
    /// version. Raised *before* any decryption is attempted.
    ContextMismatch,
    /// The chunked scheme's key commitment did not match. In practice: the
    /// wrong input key, or a modified header, salt or commitment.
    Commitment,
    /// AEAD authentication failed, or HPKE decapsulation failed.
    Aead,
    /// The Ed25519 signature over a grant envelope did not verify.
    Signature,
    /// The message is longer than the format admits, or the chunk counter would
    /// reach the 2^38 limit of c2sp.org/chunked-encryption.
    TooLarge,
    /// A public key, private key or encapsulation that is not a valid element
    /// of its group. Includes RFC 9180 §7.1.4 validation failures.
    InvalidKey,
    /// The caller asked for something this crate refuses to build — the case
    /// that matters is a Koru recovery grant for a customer-managed workspace
    /// (D89), which has no constructor at all and so cannot reach here; this
    /// variant covers the runtime half of the same rule.
    Forbidden,
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl CryptoError {
    /// The stable `UPPER_SNAKE_CASE` code, per the roadmap's standing
    /// requirement that a code has one form and every surface serializes it
    /// unchanged. These are crate-internal codes; the product-visible ones
    /// (`CLOUD_SCHEMA_UPDATE_REQUIRED` and friends) are mapped by the callers
    /// that have a DTO.
    pub fn code(self) -> &'static str {
        match self {
            Self::Malformed => "CRYPTO_MALFORMED",
            Self::UnsupportedFormatVersion => "CRYPTO_UNSUPPORTED_FORMAT_VERSION",
            Self::UnsupportedSuite => "CRYPTO_UNSUPPORTED_SUITE",
            Self::DeprecatedSuiteForWrite => "CRYPTO_DEPRECATED_SUITE_FOR_WRITE",
            Self::PurposeMismatch => "CRYPTO_PURPOSE_MISMATCH",
            Self::ContextMismatch => "CRYPTO_CONTEXT_MISMATCH",
            Self::Commitment => "CRYPTO_COMMITMENT_MISMATCH",
            Self::Aead => "CRYPTO_AUTHENTICATION_FAILED",
            Self::Signature => "CRYPTO_SIGNATURE_INVALID",
            Self::TooLarge => "CRYPTO_TOO_LARGE",
            Self::InvalidKey => "CRYPTO_INVALID_KEY",
            Self::Forbidden => "CRYPTO_FORBIDDEN",
        }
    }
}

impl std::error::Error for CryptoError {}
