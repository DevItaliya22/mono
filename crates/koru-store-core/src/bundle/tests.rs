// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::lifecycle::checksum;
use crate::manifest::Manifest;

fn at() -> Timestamp {
    Timestamp::from_millis(1_770_000_000_000)
}

/// A bundle holding one manifest document, as a real export would.
fn bundle() -> Bundle {
    let workspace = WorkspaceId::new();
    let document = DocumentId::new();
    let snapshot = Manifest::create(workspace, "W", at())
        .unwrap()
        .export_snapshot()
        .unwrap();
    Bundle {
        manifest: BundleManifest {
            format_version: BUNDLE_FORMAT_VERSION,
            workspace,
            name: "W".into(),
            created_at: at(),
            exported_at: at(),
            lineage: Vec::new(),
            documents: vec![BundleDocument {
                id: document,
                kind: DocumentKind::Manifest,
                checksum: checksum(&snapshot),
            }],
            history_versions: Vec::new(),
            history_entries: Vec::new(),
            blobs: Vec::new(),
        },
        snapshots: vec![(document, snapshot)],
        history_snapshots: Vec::new(),
        blobs: Vec::new(),
    }
}

// ------------------------------------------------------------------ modes

#[test]
fn the_two_modes_are_stable_tokens_that_round_trip() {
    for mode in [ImportMode::Restore, ImportMode::Duplicate] {
        assert_eq!(ImportMode::parse(mode.as_str()), Some(mode));
        assert!(mode.as_str().chars().all(|c| c.is_ascii_lowercase()));
    }
}

#[test]
fn an_unknown_mode_is_none_rather_than_a_default() {
    // Defaulting here picks one of two operations that differ in whether the
    // result can ever converge with its origin. Never guess.
    assert_eq!(ImportMode::parse("copy"), None);
    assert_eq!(ImportMode::parse(""), None);
    assert_eq!(ImportMode::parse("restaurar"), None);
}

// ------------------------------------------------------------------ validation

#[test]
fn a_well_formed_bundle_validates() {
    bundle().validate().unwrap();
    assert!(bundle().manifest_document().is_some());
}

#[test]
fn a_bundle_from_a_newer_build_is_refused_by_version() {
    let mut b = bundle();
    b.manifest.format_version = BUNDLE_FORMAT_VERSION + 1;
    assert_eq!(
        b.validate().unwrap_err().code(),
        Code::BundleFormatUnsupported
    );
}

#[test]
fn a_bundle_with_no_manifest_document_is_refused() {
    // A workspace without a manifest is not a workspace; importing one would
    // produce something that cannot be listed or joined.
    let mut b = bundle();
    b.manifest.documents[0].kind = DocumentKind::Document;
    assert_eq!(b.validate().unwrap_err().code(), Code::InvalidRequest);
}

#[test]
fn a_bundle_listing_a_document_it_does_not_contain_is_refused() {
    let mut b = bundle();
    b.snapshots.clear();
    assert_eq!(b.validate().unwrap_err().code(), Code::InvalidRequest);
}

#[test]
fn a_document_that_does_not_match_its_checksum_is_refused() {
    // Detected before anything is written, rather than after — importing half
    // a workspace leaves something that looks present and is not.
    let mut b = bundle();
    b.snapshots[0].1 = b"rotted".to_vec();
    assert_eq!(b.validate().unwrap_err().code(), Code::CorruptState);
}

#[test]
fn a_bundle_listing_a_blob_it_does_not_contain_is_refused() {
    let mut b = bundle();
    b.manifest.blobs.push(crate::model::Blob {
        hash: crate::model::BlobHash::from_digest("b3", &"ab".repeat(32)).unwrap(),
        size: 2,
        mime: "text/plain".into(),
        filename: None,
        created_at: at(),
        attributes: Default::default(),
    });
    assert_eq!(b.validate().unwrap_err().code(), Code::InvalidRequest);
}

#[test]
fn a_bundle_from_an_older_build_is_accepted() {
    // Forward compatibility runs one way. An older format is one this build
    // understands; refusing it would strand every bundle already written.
    let mut b = bundle();
    b.manifest.format_version = 0;
    b.validate().unwrap();
}

#[test]
fn a_snapshot_is_reachable_by_document_id() {
    let b = bundle();
    let id = b.manifest_document().unwrap();
    assert!(b.snapshot_of(id).is_some());
    assert!(b.snapshot_of(DocumentId::new()).is_none());
}

#[test]
fn a_manifest_round_trips_through_json() {
    // The manifest is the one part of a bundle that is text, so it is the part
    // that has to survive being read by something that is not this build.
    let b = bundle();
    let json = serde_json::to_string(&b.manifest).unwrap();
    let back: BundleManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(back, b.manifest);
}
