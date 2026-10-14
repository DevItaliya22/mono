// SPDX-License-Identifier: MPL-2.0

//! What a result version is, decided without a gateway: the version is
//! described by hand and its bytes come from a closure that counts calls.

use std::cell::Cell;

use koru_artifact_wire::{Filename, MediaType};

use super::*;
use crate::tests::{family, version};

fn described(mime: &str, filename: Option<&str>) -> ArtifactVersion {
    let mut made = version(&family(1, "Findings", 500), 1, None, None);
    made.mime = MediaType::new(mime).unwrap();
    made.filename = filename.map(|name| Filename::new(name).unwrap());
    made
}

/// **Markdown is read as its text**, whether the store was told it was
/// Markdown or only the filename says so.
#[test]
fn markdown_is_read_as_text() {
    for (mime, filename) in [
        ("text/markdown", None),
        ("text/x-markdown", None),
        ("text/plain", Some("report.md")),
        ("application/octet-stream", Some("NOTES.MARKDOWN")),
    ] {
        let body = body_of(&described(mime, filename), || {
            Ok(b"# Findings\n\nAll good.".to_vec())
        })
        .unwrap();
        assert_eq!(
            body,
            ResultBody::Markdown("# Findings\n\nAll good.".into()),
            "{mime} {filename:?}"
        );
    }
}

/// **A picture is answered by the blob it names**, never downloaded: an
/// image node over that hash is all a host needs.
#[test]
fn an_image_is_its_blob_and_media_type_and_its_bytes_are_never_asked_for() {
    let asked = Cell::new(0);
    let version = described("image/png", Some("chart.png"));
    let body = body_of(&version, || {
        asked.set(asked.get() + 1);
        Ok(Vec::new())
    })
    .unwrap();
    assert_eq!(
        body,
        ResultBody::Image {
            blob: version.blob.clone(),
            mime: "image/png".into(),
        }
    );
    assert_eq!(asked.get(), 0);
}

/// Anything else is named and sized, and not read.
#[test]
fn anything_else_is_named_and_not_read() {
    let asked = Cell::new(0);
    let version = described("application/pdf", Some("report.pdf"));
    let body = body_of(&version, || {
        asked.set(asked.get() + 1);
        Ok(Vec::new())
    })
    .unwrap();
    assert_eq!(
        body,
        ResultBody::Other {
            mime: "application/pdf".into(),
            size_bytes: version.size_bytes,
        }
    );
    assert_eq!(asked.get(), 0);
    // A text file that does not say it is Markdown is not read as Markdown.
    assert!(matches!(
        body_of(&described("text/plain", Some("notes.txt")), || Ok(
            Vec::new()
        ))
        .unwrap(),
        ResultBody::Other { .. }
    ));
}

/// **Bytes this device cannot serve are the store's answer, not a guess.**
/// A result held on another device fails with the store's own code.
#[test]
fn markdown_whose_bytes_are_not_here_fails_with_the_stores_code() {
    let failure = body_of(&described("text/markdown", None), || {
        Err(Failure::new("blob_unavailable", "not on this device", true))
    })
    .unwrap_err();
    assert_eq!(failure.code, "blob_unavailable");

    let failure = body_of(&described("text/markdown", None), || Ok(vec![0xff, 0xfe])).unwrap_err();
    assert_eq!(failure.code, "unreadable");
}

/// **A result's words never reach a `Debug`** (standing requirement 7).
#[test]
fn a_bodys_debug_carries_no_words() {
    let printed = format!("{:?}", ResultBody::Markdown("The quarterly summary".into()));
    assert!(!printed.contains("quarterly"), "{printed}");
}
