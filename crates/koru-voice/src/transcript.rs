// SPDX-License-Identifier: MPL-2.0

//! **The words somebody said, wrapped so that printing them is impossible by
//! accident.**
//!
//! A transcript is the most sensitive value in this product. It is what a
//! person dictated into a private document, it exists for a second or two, and
//! it passes through a state machine, a socket, a client and a view — every one
//! of which is a place somebody will one day add a `{:?}` while chasing
//! something else. `docs/contributing/operations/LOGGING.md` says never a transcript; this
//! type is what makes that rule hold without anybody remembering it.
//!
//! The idiom is `koru_agent_wire::LinkCode`'s, and it was chosen there for the
//! same reason: **request tracing and test failures routinely format whole wire
//! messages.** A `Debug` on the enclosing type walks into its fields, so the
//! only durable defence is at the leaf.
//!
//! ## What redaction does and does not cover
//!
//! `Debug` is redacted. **`Serialize` is not**, and must not be — the text
//! crossing the socket is the entire point of the socket. So the two rules that
//! go with this type are:
//!
//! - never log a transcript, in any form, at any level. The `Debug` makes the
//!   easy mistake harmless; serializing a message to JSON and logging *that*
//!   defeats it, and no type can stop you.
//! - there is deliberately **no `Display`**. A `Display` is what somebody
//!   reaches for when they want the text in a message, which is the thing that
//!   must be a deliberate [`Transcript::as_str`] rather than a formatting
//!   convenience.

use serde::{Deserialize, Serialize};

/// A partial or final transcript.
///
/// Transparent to serde and opaque to `Debug`. The field is private, so `.0` is
/// not a way around it either.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Transcript(String);

impl Transcript {
    /// Wrap text an engine produced.
    pub fn new(text: impl Into<String>) -> Transcript {
        Transcript(text.into())
    }

    /// The text, for the one caller that is actually going to insert it.
    ///
    /// Deliberately a named method rather than a `Deref` or a `Display`: every
    /// place this is called is a place a reviewer should look at, and there are
    /// meant to be very few of them.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The text, consuming the wrapper.
    pub fn into_string(self) -> String {
        self.0
    }

    /// Whether the engine has produced anything yet.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How long it is, **in characters rather than bytes**.
    ///
    /// The same rule as `validate_name`: a length counted in bytes is one
    /// number in English and a third of it in Japanese, which reads as one
    /// rule and behaves as eight. This is the only measurement of a transcript
    /// that may appear in a log or a diagnostic.
    pub fn chars(&self) -> usize {
        self.0.chars().count()
    }
}

impl std::fmt::Debug for Transcript {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Transcript([REDACTED] chars={})", self.chars())
    }
}

impl From<&str> for Transcript {
    fn from(text: &str) -> Transcript {
        Transcript::new(text)
    }
}

impl From<String> for Transcript {
    fn from(text: String) -> Transcript {
        Transcript(text)
    }
}

#[cfg(test)]
mod tests;
