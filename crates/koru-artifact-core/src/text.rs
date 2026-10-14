// SPDX-License-Identifier: MPL-2.0

//! The two strings a version carries beside its title, and what each hides.
//!
//! A **filename** is content: it is what the person's or the agent's file was
//! called, and standing requirement 7 puts it beside a title and a path —
//! never in a log, an event attribute or a `Debug`. [`Filename`] redacts the
//! way `ArtifactTitle` does. It is **metadata only**, never identity and never
//! a path component: P1.11 resolves paths under `.artifacts/<RunId>/` before a
//! filename reaches this type, and the blob store files bytes by hash.
//!
//! A **media type** is not content. `text/markdown` says nothing about a
//! person, and a reader of a failure needs it to know what was refused, so
//! [`MediaType`] prints in full.
//!
//! Both are bounded and refused at construction, and — unlike the run
//! vocabulary's texts — **also refused on deserialization**, because the one
//! place they are deserialized is a request body at the gateway, and
//! `artifacts.db` is device-local behind refuse-newer rather than a synced
//! document an older build must still open.

use std::fmt;

use koru_delegate_core::TextError;
use serde::{Deserialize, Serialize};

/// The longest filename a version may carry, in scalar values. Wider than
/// one filesystem component because it is metadata, not a path.
pub const MAX_FILENAME_CHARS: usize = 1_024;
/// The longest media type accepted — RFC 6838 bounds each of the two names
/// at 127, and a parameter list is not carried here.
pub const MAX_MEDIA_TYPE_CHARS: usize = 255;

fn check(value: &str, max_chars: usize) -> Result<(), TextError> {
    if value.trim().is_empty() {
        return Err(TextError::Empty);
    }
    if value.chars().count() > max_chars {
        return Err(TextError::TooLong { max_chars });
    }
    if value.chars().any(char::is_control) {
        return Err(TextError::ControlCharacter);
    }
    Ok(())
}

/// The name the bytes had where the agent wrote them (D6). Redacted in
/// `Debug`; shown only by the surface that lists the version.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(try_from = "String", into = "String")]
pub struct Filename(String);

impl Filename {
    /// Accept a filename, or say why not — never echoing it in the error.
    pub fn new(value: impl Into<String>) -> Result<Self, TextError> {
        let value = value.into();
        check(&value, MAX_FILENAME_CHARS)?;
        Ok(Self(value))
    }

    /// The name, for the one place that shows it to a person.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Length in scalar values — a bounded count is safe to report.
    pub fn char_count(&self) -> usize {
        self.0.chars().count()
    }
}

impl TryFrom<String> for Filename {
    type Error = TextError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Filename> for String {
    fn from(value: Filename) -> Self {
        value.0
    }
}

/// Redacted. The marker is the type's name so a reader knows what was
/// withheld.
impl fmt::Debug for Filename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Filename(<redacted>)")
    }
}

/// Why a media type was refused: a text fault, or a shape that is not
/// `type/subtype`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaTypeError {
    Text(TextError),
    Malformed,
}

impl From<TextError> for MediaTypeError {
    fn from(error: TextError) -> Self {
        Self::Text(error)
    }
}

impl fmt::Display for MediaTypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(error) => error.fmt(f),
            Self::Malformed => f.write_str("media type is not type/subtype"),
        }
    }
}

impl std::error::Error for MediaTypeError {}

/// A `type/subtype` media type, ASCII and lowercased, as the blob store
/// records it. Not content: it prints in full.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(try_from = "String", into = "String")]
pub struct MediaType(String);

impl MediaType {
    /// The type the blob store assigns when the uploader named none.
    pub const OCTET_STREAM: &'static str = "application/octet-stream";

    /// Accept a media type. It must be ASCII with exactly one `/` between
    /// two non-empty names; parameters (`; charset=`) are refused, because
    /// what the store records is the type and a parameter is not part of an
    /// address.
    pub fn new(value: impl Into<String>) -> Result<Self, MediaTypeError> {
        let value = value.into().to_ascii_lowercase();
        check(&value, MAX_MEDIA_TYPE_CHARS)?;
        let Some((kind, subtype)) = value.split_once('/') else {
            return Err(MediaTypeError::Malformed);
        };
        let token = |part: &str| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "!#$&-^_.+".contains(c))
        };
        if !token(kind) || !token(subtype) {
            return Err(MediaTypeError::Malformed);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for MediaType {
    type Error = MediaTypeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<MediaType> for String {
    fn from(value: MediaType) -> Self {
        value.0
    }
}

impl fmt::Debug for MediaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MediaType({})", self.0)
    }
}

impl fmt::Display for MediaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The longest relative path a run folder listing may carry, in scalar
/// values. A path is deeper than a name, so it is wider than
/// [`MAX_FILENAME_CHARS`] — and still bounded, because a listing crosses a
/// route with a body limit.
pub const MAX_RELATIVE_PATH_CHARS: usize = 4_096;

/// Why a relative path was refused: a text fault, or a shape that is not a
/// path *inside* a folder. Mirrors [`MediaTypeError`], because the question
/// is the same one — is this text, and is it the right kind of text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelativePathError {
    Text(TextError),
    /// Absolute, rooted, drive-qualified (`C:…`), backslashed, or carrying
    /// an empty, `.` or `..` segment. **Not** merely containing a colon: a
    /// POSIX filename may, and refusing one would leave a real result
    /// invisible on a Mac.
    Malformed,
}

impl From<TextError> for RelativePathError {
    fn from(error: TextError) -> Self {
        Self::Text(error)
    }
}

impl fmt::Display for RelativePathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(error) => error.fmt(f),
            Self::Malformed => f.write_str("path is not a relative path inside a folder"),
        }
    }
}

impl std::error::Error for RelativePathError {}

/// A file's place **inside** a run's `.artifacts/<RunId>/` folder:
/// `/`-separated, never absolute, never carrying a parent-directory step.
///
/// Content, like a filename, so its `Debug` redacts. It is a *statement of
/// where to look*, never an authority to look there: the register path rule
/// (`staging::stage`) still resolves it, walks it for links and proves
/// containment. Refusing the obvious faults here as well means a listing
/// this type could not have produced can never be echoed back as a
/// registration.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(try_from = "String", into = "String")]
pub struct RelativePath(String);

impl RelativePath {
    /// Accept a relative path, or say why not — never echoing it back.
    pub fn new(value: impl Into<String>) -> Result<Self, RelativePathError> {
        let value = value.into();
        check(&value, MAX_RELATIVE_PATH_CHARS)?;
        // A drive qualifier is the first two characters and nothing else:
        // `C:` is one, `notes:draft.md` is a perfectly ordinary POSIX
        // filename and this type has no business refusing it.
        let drive_qualified = value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
            && value.as_bytes().get(1) == Some(&b':');
        let shaped = !value.starts_with('/')
            && !value.contains('\\')
            && !drive_qualified
            && value
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != "..");
        if !shaped {
            return Err(RelativePathError::Malformed);
        }
        Ok(Self(value))
    }

    /// The path, for the one surface that shows it and the one rule that
    /// resolves it.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The last segment — what a row is labelled with.
    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// How many segments deep it is. A bounded count is safe to report.
    pub fn depth(&self) -> usize {
        self.0.split('/').count()
    }
}

impl TryFrom<String> for RelativePath {
    type Error = RelativePathError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<RelativePath> for String {
    fn from(value: RelativePath) -> Self {
        value.0
    }
}

impl fmt::Debug for RelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RelativePath(<redacted>)")
    }
}
