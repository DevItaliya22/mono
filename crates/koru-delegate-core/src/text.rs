// SPDX-License-Identifier: MPL-2.0

//! Text a person wrote, and how it never leaks.
//!
//! Four things in a run are free text: the objective, a rating's note, an
//! artifact's title and a folder inside a working scope. Standing requirement
//! 7 says none of them may reach a log, an event attribute, a metric, a
//! `Debug` rendering or an error message. Making that structural means the
//! text lives in a newtype whose `Debug` prints a marker and never the value,
//! so every derived `Debug` above it — a request, a header, a whole run — is
//! safe without anyone remembering to write one by hand.
//!
//! Each is **bounded in Unicode scalar values**, not bytes, so the limit is
//! the same rule in Japanese as in English (PRINCIPLES, *a length limit that
//! was a different product per language*). A limit is enforced when a value
//! is *constructed*; on deserialization the value is accepted as it arrives,
//! because a header written by a build with a wider limit must still open on
//! this one (§8: an older reader preserves what it does not know).

use std::fmt;

use serde::{Deserialize, Serialize};

/// The longest objective a person may delegate, in scalar values.
pub const MAX_OBJECTIVE_CHARS: usize = 8_192;
/// The longest note a rating may carry, in scalar values (D44).
pub const MAX_RATING_NOTE_CHARS: usize = 2_048;
/// The longest artifact title a result reference carries, in scalar values.
pub const MAX_ARTIFACT_TITLE_CHARS: usize = 512;
/// The longest folder path inside a working scope, in scalar values — the
/// session protocol's project-root bound, because the host resolves it
/// against the same filesystem.
pub const MAX_RELATIVE_FOLDER_CHARS: usize = 4_096;

/// Why a text value was refused at construction. Codes, not prose, and never
/// the text itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextError {
    Empty,
    TooLong { max_chars: usize },
    ControlCharacter,
}

impl fmt::Display for TextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("text is empty"),
            Self::TooLong { max_chars } => write!(f, "text exceeds {max_chars} characters"),
            Self::ControlCharacter => f.write_str("text contains a control character"),
        }
    }
}

impl std::error::Error for TextError {}

fn check(value: &str, max_chars: usize, allow_line_breaks: bool) -> Result<(), TextError> {
    if value.trim().is_empty() {
        return Err(TextError::Empty);
    }
    if value.chars().count() > max_chars {
        return Err(TextError::TooLong { max_chars });
    }
    let forbidden =
        |c: char| c.is_control() && !(allow_line_breaks && matches!(c, '\n' | '\r' | '\t'));
    if value.chars().any(forbidden) {
        return Err(TextError::ControlCharacter);
    }
    Ok(())
}

macro_rules! redacted_text {
    ($(#[$doc:meta])* $name:ident, $max:expr, line_breaks = $line_breaks:expr) => {
        $(#[$doc])*
        #[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Accept the text a person wrote, or say why not — never echoing
            /// the text in the error.
            pub fn new(value: impl Into<String>) -> Result<Self, TextError> {
                let value = value.into();
                check(&value, $max, $line_breaks)?;
                Ok(Self(value))
            }

            /// The text, for the one place that shows it to the person who
            /// wrote it. Nothing else should call this.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Length in scalar values — a bounded count is safe to report.
            pub fn char_count(&self) -> usize {
                self.0.chars().count()
            }
        }

        /// Redacted. The value never appears; the marker is the type's name so
        /// a reader knows what was withheld.
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}(<redacted>)", stringify!($name))
            }
        }
    };
}

redacted_text!(
    /// What a person asked the agent to do (D9). The one field of a run that
    /// is a prompt, and therefore the one field that must never be reported.
    Objective,
    MAX_OBJECTIVE_CHARS,
    line_breaks = true
);
redacted_text!(
    /// The optional note beside a rating (D44). It lives in the run's own
    /// content and never in an event attribute or a metric, so a private
    /// remark cannot become a free-text analytics field.
    RatingNote,
    MAX_RATING_NOTE_CHARS,
    line_breaks = true
);
redacted_text!(
    /// The title a result is named by on a device that cannot open its bytes
    /// (D27). A title is content: it is redacted like the objective.
    ArtifactTitle,
    MAX_ARTIFACT_TITLE_CHARS,
    line_breaks = false
);
redacted_text!(
    /// A folder inside a working scope's root, as a person typed or chose
    /// it. A path is private (standing requirement 7); the host canonicalizes
    /// it at admission and this type only bounds and hides it.
    RelativeFolder,
    MAX_RELATIVE_FOLDER_CHARS,
    line_breaks = false
);
