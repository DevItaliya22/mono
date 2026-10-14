// SPDX-License-Identifier: MPL-2.0

//! **What one version's bytes are**, read the one way this crate reads them.
//!
//! The Result tab asks this and draws the answer; a host that wants to *use*
//! a result somewhere else — a canvas placing a run's written result as a
//! card, its picture as an image — asks exactly the same question through
//! [`read_result`], so the two can never disagree about whether a version is
//! Markdown. The metadata comes first, because what a version is decides
//! whether its bytes are worth asking for at all: a picture is answered by
//! the blob it names and never downloaded here, and a PDF is only named.
//!
//! **Blocking, and never on a window thread.** [`read_result`] dials the
//! gateway twice at most; a host calls it from a worker of its own, as the
//! tab's reader does from `koru-artifact-tab`. The decision itself is
//! [`body_of`], which takes the described version and a way to fetch the
//! bytes, so it is testable with neither.

use std::fmt;

use koru_artifact_wire::{ArtifactVersion, ArtifactVersionId, BlobHash, Failure};

/// The media type Koru's own Markdown is registered under.
pub(crate) const MARKDOWN: &str = "text/markdown";
/// What a blob store calls bytes nobody described.
const OCTET_STREAM: &str = "application/octet-stream";

/// One result version, as something a host can use.
#[derive(Clone, PartialEq, Eq)]
pub enum ResultBody {
    /// Markdown — registered as such, or a plain-text file the agent named
    /// `.md` — as the text it is.
    Markdown(String),
    /// A picture. The blob the version names in its workspace's store and
    /// its media type: everything an image node needs, with no copy of the
    /// bytes. Whether those bytes are on *this* device is the run header's
    /// `held_by` to say, not this read's.
    Image { blob: BlobHash, mime: String },
    /// Anything else — a PDF, an archive. Named by what it is, never read.
    Other { mime: String, size_bytes: u64 },
}

impl fmt::Debug for ResultBody {
    /// **A result's text is content** (standing requirement 7): its length
    /// is what a reader of a log needs, never its words.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Markdown(text) => f
                .debug_struct("Markdown")
                .field("bytes", &text.len())
                .finish(),
            Self::Image { blob, mime } => f
                .debug_struct("Image")
                .field("blob", blob)
                .field("mime", mime)
                .finish(),
            Self::Other { mime, size_bytes } => f
                .debug_struct("Other")
                .field("mime", mime)
                .field("size_bytes", size_bytes)
                .finish(),
        }
    }
}

/// **Read one result version.** Blocking: call it from a worker.
///
/// Asks the Artifact Store to describe the version, then — only for
/// Markdown — for its bytes. A version this store will not describe, or
/// whose Markdown bytes it cannot serve (they are held on another device, or
/// are not UTF-8), answers with the store's `Failure`.
pub fn read_result(
    client: &koru_api_client::Client,
    version: ArtifactVersionId,
) -> Result<ResultBody, Failure> {
    let described = client.artifacts().version(version)?;
    body_of(&described, || client.artifacts().content(version))
}

/// The decision [`read_result`] makes, over a version already described
/// and a way to fetch its bytes that is called only for Markdown.
pub(crate) fn body_of(
    described: &ArtifactVersion,
    content: impl FnOnce() -> Result<Vec<u8>, Failure>,
) -> Result<ResultBody, Failure> {
    let mime = described.mime.as_str();
    let filename = described.filename.as_ref().map(|name| name.as_str());
    if is_markdown(mime, filename) {
        let bytes = content()?;
        return String::from_utf8(bytes)
            .map(ResultBody::Markdown)
            .map_err(|_| Failure::new("unreadable", "The result is not text.", false));
    }
    if is_image(mime) {
        return Ok(ResultBody::Image {
            blob: described.blob.clone(),
            mime: mime.to_owned(),
        });
    }
    Ok(ResultBody::Other {
        mime: mime.to_owned(),
        size_bytes: described.size_bytes,
    })
}

/// Whether a version's bytes are worth handing to the Markdown surface.
///
/// The media type first, because that is what the store recorded; the
/// filename second, because an agent that wrote `report.md` and registered
/// it without saying what it was still wrote Markdown. **Nothing sniffs the
/// bytes.**
pub(crate) fn is_markdown(mime: &str, filename: Option<&str>) -> bool {
    if matches!(mime, MARKDOWN | "text/x-markdown") {
        return true;
    }
    if mime != "text/plain" && mime != OCTET_STREAM {
        return false;
    }
    filename.is_some_and(|name| {
        let lower = name.to_ascii_lowercase();
        lower.ends_with(".md") || lower.ends_with(".markdown")
    })
}

/// A picture, by the media type the store recorded. The same rule: nothing
/// sniffs the bytes, and an image the store did not call one is not placed
/// as one.
fn is_image(mime: &str) -> bool {
    mime.trim().to_ascii_lowercase().starts_with("image/")
}

#[cfg(test)]
mod tests;
