// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_store_core::bundle::{Bundle, BundleDocument, BundleManifest};
use koru_store_core::ids::DocumentId;
use koru_store_core::lifecycle::checksum;
use koru_store_core::manifest::Manifest;
use koru_store_core::model::{Blob, BlobHash, DocumentKind};

fn at() -> Timestamp {
    Timestamp::from_millis(1_770_000_000_000)
}

fn a_bundle(with_blob: bool) -> Bundle {
    let workspace = WorkspaceId::new();
    let document = DocumentId::new();
    let snapshot = Manifest::create(workspace, "W", at())
        .unwrap()
        .export_snapshot()
        .unwrap();
    let hash = BlobHash::from_digest(BlobHash::BLAKE3, &checksum(b"bytes")).unwrap();
    Bundle {
        manifest: BundleManifest {
            format_version: koru_store_core::version::BUNDLE_FORMAT_VERSION,
            workspace,
            name: "四半期計画".into(),
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
            blobs: if with_blob {
                vec![Blob {
                    hash: hash.clone(),
                    size: 5,
                    mime: "text/plain".into(),
                    filename: Some(koru_store_core::fixtures::UNICODE_FILENAME.into()),
                    created_at: at(),
                    attributes: Default::default(),
                }]
            } else {
                Vec::new()
            },
        },
        snapshots: vec![(document, snapshot)],
        history_snapshots: Vec::new(),
        blobs: if with_blob {
            vec![(hash, b"bytes".to_vec())]
        } else {
            Vec::new()
        },
    }
}

#[test]
fn a_bundle_round_trips_through_the_frame() {
    let bundle = a_bundle(true);
    let back = decode(&encode(&bundle).unwrap()).unwrap();
    assert_eq!(back.manifest, bundle.manifest);
    assert_eq!(back.snapshots, bundle.snapshots);
    assert_eq!(back.blobs, bundle.blobs);
}

#[test]
fn a_workspace_name_in_any_script_survives_the_frame() {
    // The manifest is JSON inside a binary frame, which is exactly the sort of
    // boundary where an encoding gets guessed.
    for name in [
        "四半期計画",
        koru_store_core::fixtures::BIDI_LINE,
        koru_store_core::fixtures::COMPOSED,
    ] {
        let mut bundle = a_bundle(false);
        bundle.manifest.name = name.to_string();
        let back = decode(&encode(&bundle).unwrap()).unwrap();
        assert_eq!(back.manifest.name, name);
    }
}

#[test]
fn something_that_is_not_a_bundle_is_refused() {
    for bad in [&b""[..], b"nope", b"KORUnot-a-length"] {
        assert!(decode(bad).is_err(), "{bad:?} was accepted");
    }
}

#[test]
fn a_truncated_bundle_is_refused_rather_than_read_short() {
    // Reading short would silently drop a document, which is the failure this
    // whole frame exists to make impossible.
    let whole = encode(&a_bundle(true)).unwrap();
    for cut in [5, 12, whole.len() / 2, whole.len() - 1] {
        let err = decode(&whole[..cut]).unwrap_err();
        assert_eq!(err.code(), Code::InvalidRequest, "cut at {cut}");
    }
}

#[test]
fn a_length_that_cannot_be_addressed_is_refused() {
    // A hostile or corrupt length must not be able to index past the buffer.
    let mut framed = encode(&a_bundle(false)).unwrap();
    let manifest_len = u32::from_le_bytes(framed[4..8].try_into().unwrap()) as usize;
    let at = 8 + manifest_len;
    framed[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode(&framed).is_err());
}

#[test]
fn a_bundle_whose_snapshot_was_tampered_with_is_refused_on_decode() {
    // `decode` validates, so a caller cannot act on a damaged bundle at all.
    let bundle = a_bundle(false);
    let mut framed = encode(&bundle).unwrap();
    let last = framed.len() - 1;
    framed[last] ^= 0xFF;
    assert_eq!(decode(&framed).unwrap_err().code(), Code::CorruptState);
}

#[test]
fn the_frame_is_self_identifying() {
    let framed = encode(&a_bundle(false)).unwrap();
    assert_eq!(&framed[..4], MAGIC);
}
