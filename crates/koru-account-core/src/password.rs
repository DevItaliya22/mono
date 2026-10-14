// SPDX-License-Identifier: MPL-2.0

//! What Koru accepts as a password, and what a stored verifier means.
//!
//! **The policy lives here and the hashing does not.** Deciding whether a
//! password is allowed is domain: one rule, in one place, so the website, a
//! future client and the API cannot disagree about it. Turning an accepted
//! password into a stored verifier is cryptography, and follows the same
//! arrangement as every other primitive in this crate — the port is declared
//! in [`crate::ports`] and `koru-account-session` implements it over the
//! algorithm. That keeps `argon2` out of `koru-account-client`,
//! `koru-mobile-host` and the agent, none of which ever verify a password:
//! under the hosted-page decision a password is only ever typed into
//! `koru.do`, so only the API needs the code that checks one.
//!
//! ## The policy is deliberately not the recommended one
//!
//! Eight characters with three of four character classes is the maintainers'
//! decision, taken against a recommendation of twelve characters with no
//! composition rules (NIST 800-63B, NCSC). It is implemented as decided, and
//! the reasoning on both sides was settled with email sign-in, so it is
//! not silently re-decided here.
//!
//! Two consequences shape the code below. The requirement is reported as
//! *which* rules a password misses, so a surface can show the rule before
//! somebody submits rather than rejecting them afterwards. And nothing about a
//! stored verifier depends on the policy, so raising the minimum later is a
//! change to these constants alone — it takes effect on each person's next
//! password, with no migration and no forced reset.

use unicode_normalization::{IsNormalized, UnicodeNormalization, is_nfc_quick};

/// The maintainers' decision. See the module note before changing it.
pub const MIN_PASSWORD_CHARS: usize = 8;

/// Not a security bound — a bound on what we are willing to hash. Argon2id
/// costs the same for 8 characters and for 128, but an unbounded input is an
/// unbounded allocation on a public route.
pub const MAX_PASSWORD_CHARS: usize = 128;

/// Three of the four in [`CharacterClass`].
pub const REQUIRED_CHARACTER_CLASSES: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CharacterClass {
    Uppercase,
    Lowercase,
    Digit,
    /// Everything that is not a letter or a digit, **space included**.
    ///
    /// Defining the fourth class as a residue rather than a list of punctuation
    /// is what lets a passphrase with spaces, an em dash, or a character from a
    /// non-Latin script satisfy it. A list would quietly mean "ASCII
    /// punctuation only" and would fail a perfectly good password in Japanese.
    Symbol,
}

impl CharacterClass {
    pub const ALL: [Self; 4] = [Self::Uppercase, Self::Lowercase, Self::Digit, Self::Symbol];

    fn of(character: char) -> Self {
        if character.is_uppercase() {
            Self::Uppercase
        } else if character.is_lowercase() {
            Self::Lowercase
        } else if character.is_numeric() {
            Self::Digit
        } else {
            Self::Symbol
        }
    }
}

/// Why a password was refused. Every variant carries what a surface needs to
/// say, because the surface shows this *before* submission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PasswordRejection {
    TooShort {
        characters: usize,
        required: usize,
    },
    TooLong {
        characters: usize,
        allowed: usize,
    },
    TooFewClasses {
        present: Vec<CharacterClass>,
        required: usize,
    },
}

/// A password that satisfies the policy, normalized and ready to hash.
///
/// The only way to obtain one is [`check_password`], so a caller cannot hash an
/// unchecked or un-normalized string by accident. It is deliberately not
/// `Clone` or `Debug`.
pub struct AcceptedPassword(String);

impl AcceptedPassword {
    /// Borrow the normalized bytes at the boundary that hashes them.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl Drop for AcceptedPassword {
    fn drop(&mut self) {
        // `String` will not shrink in place, so zero the bytes it owns before
        // the allocation is returned. This is best-effort — a `String` may have
        // been reallocated on the way here — and it is worth doing anyway.
        use zeroize::Zeroize;
        self.0.zeroize();
    }
}

/// Check a password against the policy, returning it normalized on success.
///
/// **Normalization comes first, and counting comes after.** `é` typed as one
/// code point and `é` typed as `e` plus a combining accent are the same
/// password to the person who typed it, and they hash differently unless both
/// are put in the same form. Doing it before the length check also means the
/// composed form is what is counted, so the second spelling is not quietly
/// worth two characters.
///
/// Counting is in `char`s rather than bytes for the same reason: a password
/// that is eight characters in Japanese is twenty-four bytes, and a byte bound
/// would be a different policy per language.
pub fn check_password(raw: &str) -> Result<AcceptedPassword, PasswordRejection> {
    let normalized = normalize_password(raw);
    let characters = normalized.chars().count();
    if characters < MIN_PASSWORD_CHARS {
        return Err(PasswordRejection::TooShort {
            characters,
            required: MIN_PASSWORD_CHARS,
        });
    }
    if characters > MAX_PASSWORD_CHARS {
        return Err(PasswordRejection::TooLong {
            characters,
            allowed: MAX_PASSWORD_CHARS,
        });
    }
    let present = classes_present(&normalized);
    if present.len() < REQUIRED_CHARACTER_CLASSES {
        return Err(PasswordRejection::TooFewClasses {
            present,
            required: REQUIRED_CHARACTER_CLASSES,
        });
    }
    Ok(AcceptedPassword(normalized))
}

/// The classes a password already satisfies, for a surface that shows progress
/// rather than a verdict. Ordered and deduplicated.
pub fn classes_present(value: &str) -> Vec<CharacterClass> {
    let mut present: Vec<CharacterClass> = value.chars().map(CharacterClass::of).collect();
    present.sort_unstable();
    present.dedup();
    present
}

/// Put a password in NFC, cheaply when it is already there.
///
/// Sign-in calls this directly: the stored verifier was made from a normalized
/// password, so a sign-in that skipped it would reject the second spelling of
/// an accented password that sign-up accepted.
///
/// `is_nfc_quick` answers `Yes`/`No`/`Maybe` from a scan, and the overwhelming
/// majority of passwords answer `Yes` — so the allocation and the full
/// composition are skipped for them.
pub fn normalize_password(raw: &str) -> String {
    match is_nfc_quick(raw.chars()) {
        IsNormalized::Yes => raw.to_owned(),
        _ => raw.nfc().collect(),
    }
}

/// A stored password verifier — the PHC string, salt and cost parameters
/// included.
///
/// One opaque value rather than parsed columns, because the parameters
/// travelling with the hash are what make [`PasswordCheck::rehash`] possible
/// without a migration. `Debug` is redacted: a verifier is not a secret in the
/// way a password is, but it is the thing an offline attacker wants, and it has
/// no business in a log line.
#[derive(Clone, PartialEq, Eq)]
pub struct PasswordVerifier(pub String);

impl std::fmt::Debug for PasswordVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PasswordVerifier(<redacted>)")
    }
}

/// The outcome of checking a password against a stored verifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasswordCheck {
    Accepted {
        /// The stored verifier was made with parameters that are no longer
        /// current, so it should be replaced with a fresh hash of the password
        /// that just succeeded. This is the whole reason the cost parameters
        /// are stored beside the hash: raising them later costs one write on
        /// each person's next sign-in instead of a migration or a forced reset.
        rehash: bool,
    },
    Rejected,
}

#[cfg(test)]
mod tests;
