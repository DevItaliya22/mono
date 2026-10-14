// SPDX-License-Identifier: MPL-2.0

//! Text rules the store applies, and the much larger set it deliberately does not.
//!
//! Two operations live here and nothing else:
//!
//! - [`normalize_key`] — NFC, for text used as a **key** or compared for
//!   equality. `"é"` can be one scalar or two, and two spellings of one name
//!   that compare unequal are two rows where there should be one.
//! - [`Locale`] — a BCP 47 tag, validated structurally.
//!
//! **What is not here is the point.** No case folding, no locale-aware
//! collation, no tokenisation, no transliteration, no width folding. Canonical
//! CRDT state must not depend on a tokenizer or a collation, because both are
//! language-specific and both change between library versions — and a stored
//! order that shifts when a dependency is upgraded is a data-loss bug wearing a
//! sorting costume. Ordering inside a document is Loro's; ordering for display
//! is the client's, in the reader's locale.
//!
//! Content is **never** normalised. Only keys. Rewriting what someone typed is
//! not the store's business, and NFC is lossy for text that deliberately
//! carries a decomposed form.

use std::fmt;

use unicode_normalization::UnicodeNormalization;

/// NFC-normalise text that will be used as a key or compared for equality.
///
/// Cheap in the common case: `is_nfc_quick` answers `Yes` for ASCII and for
/// most already-composed text, so nothing is allocated unless it must be.
pub fn normalize_key(s: &str) -> String {
    use unicode_normalization::IsNormalized;
    match unicode_normalization::is_nfc_quick(s.chars()) {
        IsNormalized::Yes => s.to_string(),
        _ => s.nfc().collect(),
    }
}

/// Whether two strings name the same thing, once composition is accounted for.
pub fn keys_equal(a: &str, b: &str) -> bool {
    // Fast path first: identical bytes are equal under any normalisation.
    a == b || normalize_key(a) == normalize_key(b)
}

/// A BCP 47 language tag — `en`, `pt-BR`, `zh-Hans`, `ar`.
///
/// Validated **structurally**, not against a registry. A registry goes stale,
/// and rejecting a valid tag because this build shipped before it was
/// registered would lock a person out of their own language. The shape is
/// checked so a tag cannot be an arbitrary string; whether it names a language
/// anyone speaks is not ours to rule on.
///
/// Stored canonically: language lowercase, script title case, region uppercase.
/// That is the conventional spelling, and it makes the column comparable
/// without a locale-aware comparison — which this module refuses to do.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Locale(String);

impl Locale {
    /// The tag used when a profile has said nothing. Not a claim that the
    /// person speaks English — it is the absence of an answer, and the client
    /// is free to ask the operating system instead.
    pub const UNDETERMINED: &'static str = "und";

    pub fn parse(tag: &str) -> Option<Self> {
        let tag = tag.trim();
        if tag.is_empty() || tag.len() > 35 {
            return None;
        }
        let mut parts = tag.split('-');

        // Primary language: 2-3 letters (or 4-8 for the reserved/registered
        // forms). Anything else is not a language subtag.
        let language = parts.next()?;
        if !(2..=8).contains(&language.len()) || !language.chars().all(|c| c.is_ascii_alphabetic())
        {
            return None;
        }
        let mut out = language.to_ascii_lowercase();

        for part in parts {
            let len = part.len();
            if len == 0 || len > 8 || !part.chars().all(|c| c.is_ascii_alphanumeric()) {
                return None;
            }
            out.push('-');
            if len == 4 && part.chars().all(|c| c.is_ascii_alphabetic()) {
                // Script: Hans, Latn, Arab.
                let mut cs = part.chars();
                out.push(cs.next()?.to_ascii_uppercase());
                out.extend(cs.map(|c| c.to_ascii_lowercase()));
            } else if len == 2 && part.chars().all(|c| c.is_ascii_alphabetic()) {
                // Region: BR, DE, EG.
                out.push_str(&part.to_ascii_uppercase());
            } else {
                // Region as digits (419), or a variant. Lowercase is the
                // conventional spelling for variants and harmless for digits.
                out.push_str(&part.to_ascii_lowercase());
            }
        }
        Some(Self(out))
    }

    pub fn undetermined() -> Self {
        Self(Self::UNDETERMINED.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The primary language subtag — `pt` for `pt-BR`.
    pub fn language(&self) -> &str {
        self.0.split('-').next().unwrap_or(&self.0)
    }
}

impl Default for Locale {
    fn default() -> Self {
        Self::undetermined()
    }
}

impl fmt::Display for Locale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl serde::Serialize for Locale {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for Locale {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).ok_or_else(|| serde::de::Error::custom("not a BCP 47 language tag"))
    }
}

/// Hand-written for the same reason the ids' is: this serializes as a string
/// through an impl of its own, so a derive would describe the `String` field
/// rather than the tag.
///
/// **The description says what absent means**, because that is the mistake an
/// integrator makes here — `und` is the absence of an answer and never a claim
/// that somebody speaks English.
#[cfg(feature = "schema")]
impl schemars::JsonSchema for Locale {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("Locale")
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "examples": ["en", "ko-KR", "zh-Hant", "pt-BR"],
            "description":
                "A BCP 47 language tag, checked structurally rather than against a \
                 registry. Stored canonically: language lowercase, script title case, \
                 region uppercase. `und` means undetermined — the absence of an answer, \
                 which is not the same as English.",
        })
    }
}

#[cfg(test)]
mod tests;
