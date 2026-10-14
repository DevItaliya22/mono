// SPDX-License-Identifier: MPL-2.0

//! Standing requirement 7, structurally: nothing a person wrote — a path,
//! a filename, a title — survives into a `Debug` or a `Display` here.
//! Every text-bearing type is rendered with the store's eight-language
//! fixtures planted in it, so a byte-exact leak of any script is caught.

use std::path::PathBuf;

use koru_delegate_core::{BlobHash, BlobKind, DocumentId, DocumentVersionId, RunId, WorkspaceId};
use koru_store_core::fixtures::{SAMPLES, UNICODE_FILENAME};

use super::*;

fn sentinels() -> Vec<&'static str> {
    let mut all: Vec<&str> = SAMPLES.iter().flat_map(|s| [s.title, s.body]).collect();
    all.push(UNICODE_FILENAME);
    all
}

fn assert_clean(rendered: &str, what: &str) {
    for sentinel in sentinels() {
        assert!(
            !rendered.contains(sentinel),
            "{what} leaked {sentinel:?}: {rendered}"
        );
    }
}

#[test]
fn materialized_paths_and_manifest_filenames_never_render() {
    let workspace_path = PathBuf::from(format!(
        "C:/people/{}/{}",
        SAMPLES[6].title, SAMPLES[5].body
    ));
    let file = MaterializedFile {
        item: 0,
        path: workspace_path.join(UNICODE_FILENAME),
        relative: UNICODE_FILENAME.to_owned(),
        media_type: "application/pdf".into(),
        delivery: DeliveryMode::QuotedPath,
    };
    let context = MaterializedContext {
        run: RunId::mint(),
        root: workspace_path,
        files: vec![file.clone()],
    };
    let manifest = Manifest {
        version: MANIFEST_VERSION,
        run: RunId::mint(),
        delivery: DeliveryMode::NativeStructured,
        files: vec![ManifestFile::Blob {
            item: 0,
            workspace: WorkspaceId::new(),
            hash: BlobHash::from_digest("b3", &"cd".repeat(32)).unwrap(),
            blob_kind: BlobKind::Pdf,
            path: "blobs/x.pdf".into(),
            size: 1,
            media_type: "application/pdf".into(),
            filename: Some(UNICODE_FILENAME.to_owned()),
        }],
    };
    for (what, rendered) in [
        ("file", format!("{file:?}")),
        ("file (pretty)", format!("{file:#?}")),
        ("context", format!("{context:?}")),
        ("context (pretty)", format!("{context:#?}")),
        ("manifest", format!("{manifest:?}")),
        ("manifest (pretty)", format!("{manifest:#?}")),
        ("manifest file", format!("{:?}", manifest.files[0])),
    ] {
        assert_clean(&rendered, what);
    }
    // The manifest *serializes* the filename: it is metadata for the
    // provider, and only its Debug is what a log could carry.
    assert!(
        serde_json::to_string(&manifest)
            .unwrap()
            .contains(UNICODE_FILENAME)
    );
}

#[test]
fn every_failure_is_tokens_and_indices() {
    let capture = CaptureFailure {
        item: 3,
        stage: CaptureStage::Retention,
        fault: Fault::BlobCorrupt,
    };
    let materialize = MaterializeFailure {
        item: Some(1),
        stage: MaterializeStage::Write,
        fault: Fault::Io(std::io::ErrorKind::PermissionDenied),
    };
    let refusal = SourceRefusal::InTrash { item: 2 };
    let unanswered = SourceRefusal::Unanswered {
        item: 0,
        fault: Fault::StoreFailed,
    };
    assert_eq!(
        capture.to_string(),
        "context item 3 failed at retention: blob_corrupt"
    );
    assert_eq!(
        materialize.to_string(),
        "context item 1 could not be materialized at write: io (PermissionDenied)"
    );
    assert_eq!(refusal.to_string(), "source_in_trash (item 2)");
    assert_eq!(
        unanswered.to_string(),
        "source_unanswered (item 0): store_failed"
    );
    for rendered in [
        format!("{capture:?}"),
        format!("{materialize:?}"),
        format!("{refusal:?}"),
        format!("{unanswered:?}"),
    ] {
        assert!(rendered.is_ascii(), "{rendered}");
    }
}

#[test]
fn a_pin_and_a_captured_context_carry_ids_and_digests_only() {
    let pin = ContextPin {
        item: RunContextItem::Document {
            workspace: WorkspaceId::new(),
            document: DocumentId::new(),
            version: DocumentVersionId::new(),
        },
        checksum: "b3:00".into(),
    };
    let captured = CapturedContext {
        run: RunId::mint(),
        workspace: WorkspaceId::new(),
        pins: vec![pin.clone()],
    };
    let json = serde_json::to_string(&captured).unwrap();
    assert_clean(&json, "captured json");
    assert_clean(&format!("{captured:#?}"), "captured debug");
    let back: CapturedContext = serde_json::from_str(&json).unwrap();
    assert_eq!(back, captured);
}
