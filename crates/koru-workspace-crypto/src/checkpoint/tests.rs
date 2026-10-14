// SPDX-License-Identifier: MPL-2.0

use koru_store_core::ids::{DocumentId, WorkspaceId};
use uuid::Uuid;

use super::*;
use crate::testkit::VectorRng;

pub(crate) fn sample_bootstrap() -> WorkspaceRecoveryBootstrapV1 {
    let mut documents = vec![
        DocumentEntry {
            document_id: DocumentId::from_uuid(Uuid::from_bytes([0x10; 16])),
            role: DocumentRole::Manifest,
            frontier: vec![1, 2, 3],
            version_vector: vec![4, 5],
            key_version: WekVersion(2),
            snapshot: ObjectReference {
                locator: b"seg/manifest/0".to_vec(),
                ciphertext_digest: [0xa0; 32],
            },
            updates: vec![ObjectReference {
                locator: b"seg/manifest/1".to_vec(),
                ciphertext_digest: [0xa1; 32],
            }],
        },
        DocumentEntry {
            document_id: DocumentId::from_uuid(Uuid::from_bytes([0x20; 16])),
            role: DocumentRole::Document,
            frontier: vec![6],
            version_vector: vec![7, 8, 9],
            key_version: WekVersion(1),
            snapshot: ObjectReference {
                locator: b"seg/doc/0".to_vec(),
                ciphertext_digest: [0xb0; 32],
            },
            updates: Vec::new(),
        },
    ];
    documents.sort_by_key(|d| *d.document_id.as_uuid().as_bytes());

    WorkspaceRecoveryBootstrapV1 {
        documents,
        objects: vec![
            ObjectEntry {
                cloud_object_id: crate::identity::CloudObjectId::from_bytes([0x01; 32]),
                key_version: WekVersion(1),
            },
            ObjectEntry {
                cloud_object_id: crate::identity::CloudObjectId::from_bytes([0x02; 32]),
                key_version: WekVersion(3),
            },
        ],
        blob_identity_key_version: BlobIdentityKeyVersion(4),
        // The union of every key_version above, together with H5 = 3.
        required_key_versions: vec![WekVersion(1), WekVersion(2), WekVersion(3)],
    }
}

pub(crate) fn sample_header() -> CheckpointHeader {
    CheckpointHeader::new(
        WorkspaceId::from_uuid(Uuid::from_bytes([0x11; 16])),
        Uuid::from_bytes([0x12; 16]),
        Uuid::from_bytes([0x13; 16]),
        WekVersion(3),
    )
}

#[test]
fn a_checkpoint_round_trips_with_exactly_the_approved_fields() {
    let mut rng = VectorRng::new(b"checkpoint");
    let wek = WorkspaceEncryptionKey::from_bytes([0x60; 32]);
    let bytes = seal_checkpoint(&sample_header(), &sample_bootstrap(), &wek, &mut rng).unwrap();

    assert_eq!(parse_header(&bytes).unwrap(), sample_header());
    let (header, bootstrap) = open_checkpoint(&bytes, &wek).unwrap();
    assert_eq!(header, sample_header());
    assert_eq!(bootstrap, sample_bootstrap());
}

#[test]
fn the_body_encoding_is_deterministic() {
    // The property Plan 4.4's sentinel test rests on (contract §10.4): the same
    // logical record encodes to the same bytes, so two publications from
    // Profiles that differ only in unrelated state can be compared exactly.
    let first = sample_bootstrap().encode(WekVersion(3)).unwrap();
    let second = sample_bootstrap().encode(WekVersion(3)).unwrap();
    assert_eq!(first, second);
}

#[test]
fn the_header_cannot_be_altered_or_recombined() {
    let mut rng = VectorRng::new(b"header");
    let wek = WorkspaceEncryptionKey::from_bytes([0x61; 32]);
    let bytes = seal_checkpoint(&sample_header(), &sample_bootstrap(), &wek, &mut rng).unwrap();

    for index in 0..CHECKPOINT_HEADER_LEN {
        let mut tampered = bytes.clone();
        tampered[index] ^= 0x01;
        assert!(
            open_checkpoint(&tampered, &wek).is_err(),
            "header byte {index} opened"
        );
    }

    // A body served under another checkpoint's header.
    let other = CheckpointHeader::new(
        WorkspaceId::from_uuid(Uuid::from_bytes([0x11; 16])),
        Uuid::from_bytes([0x12; 16]),
        Uuid::from_bytes([0x99; 16]),
        WekVersion(3),
    );
    let mut recombined = other.encode();
    recombined.extend_from_slice(&bytes[CHECKPOINT_HEADER_LEN..]);
    assert_eq!(
        open_checkpoint(&recombined, &wek).unwrap_err(),
        CryptoError::Commitment
    );
}

#[test]
fn an_unimplemented_bootstrap_version_stops_before_decryption() {
    let mut rng = VectorRng::new(b"version");
    let wek = WorkspaceEncryptionKey::from_bytes([0x62; 32]);
    let mut bytes = seal_checkpoint(&sample_header(), &sample_bootstrap(), &wek, &mut rng).unwrap();
    // H1 sits at offset 6, after the three envelope framing fields.
    bytes[6..8].copy_from_slice(&2u16.to_be_bytes());
    assert_eq!(
        parse_header(&bytes).unwrap_err(),
        CryptoError::UnsupportedFormatVersion
    );
    assert_eq!(
        open_checkpoint(&bytes, &wek).unwrap_err(),
        CryptoError::UnsupportedFormatVersion
    );
}

#[test]
fn the_completeness_assertions_are_checked_rather_than_trusted() {
    let h5 = WekVersion(3);

    // required_key_versions must equal the union, including H5.
    let mut wrong = sample_bootstrap();
    wrong.required_key_versions = vec![WekVersion(1), WekVersion(2)];
    assert_eq!(wrong.encode(h5).unwrap_err(), CryptoError::Malformed);

    // Exactly one MANIFEST.
    let mut two_manifests = sample_bootstrap();
    two_manifests.documents[1].role = DocumentRole::Manifest;
    assert_eq!(
        two_manifests.encode(h5).unwrap_err(),
        CryptoError::Malformed
    );

    let mut no_manifest = sample_bootstrap();
    no_manifest.documents[0].role = DocumentRole::Document;
    no_manifest.documents[1].role = DocumentRole::Document;
    assert_eq!(no_manifest.encode(h5).unwrap_err(), CryptoError::Malformed);

    // Non-empty documents.
    let mut empty = sample_bootstrap();
    empty.documents.clear();
    assert!(empty.encode(h5).is_err());

    // Canonical ordering, both arrays, and no duplicates.
    let mut unordered = sample_bootstrap();
    unordered.documents.reverse();
    assert_eq!(unordered.encode(h5).unwrap_err(), CryptoError::Malformed);

    let mut duplicate = sample_bootstrap();
    duplicate.objects[1] = duplicate.objects[0].clone();
    assert_eq!(duplicate.encode(h5).unwrap_err(), CryptoError::Malformed);
}

#[test]
fn a_truncated_body_is_refused_rather_than_restoring_less_than_the_workspace() {
    let h5 = WekVersion(3);
    let encoded = sample_bootstrap().encode(h5).unwrap();
    for cut in 0..encoded.len() {
        assert!(
            WorkspaceRecoveryBootstrapV1::decode(&encoded[..cut], h5).is_err(),
            "a {cut}-byte body decoded"
        );
    }
    let mut extended = encoded.clone();
    extended.push(0);
    assert_eq!(
        WorkspaceRecoveryBootstrapV1::decode(&extended, h5).unwrap_err(),
        CryptoError::Malformed
    );
}

#[test]
fn a_declared_array_length_never_reserves_memory() {
    // D91: malformed lengths must not overflow a parser. A body claiming four
    // billion documents in twelve bytes must be a parse error, immediately.
    let mut hostile = Vec::new();
    hostile.extend_from_slice(&u32::MAX.to_be_bytes());
    hostile.extend_from_slice(&[0u8; 8]);
    assert!(WorkspaceRecoveryBootstrapV1::decode(&hostile, WekVersion(1)).is_err());
}

#[test]
fn the_role_is_a_stable_ascii_token() {
    assert_eq!(DocumentRole::Manifest.token(), b"MANIFEST");
    assert_eq!(DocumentRole::Document.token(), b"DOCUMENT");
    assert_eq!(
        DocumentRole::from_token(b"manifest").unwrap_err(),
        CryptoError::Malformed
    );
    assert_eq!(
        DocumentRole::from_token(b"").unwrap_err(),
        CryptoError::Malformed
    );
}

#[test]
fn ordering_by_raw_bytes_agrees_with_ordering_by_storage_form() {
    // The contract orders `documents[]` by the `document_id` **storage form**;
    // the encoder compares raw bytes because the alternative allocates a String
    // per comparison inside a sort. The two agree because the hyphens sit at
    // fixed positions and lowercase hex sorts as its nibbles do — asserted here
    // rather than argued, over a spread that includes every nibble boundary.
    let mut ids: Vec<DocumentId> = Vec::new();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"document id ordering");
    let mut reader = hasher.finalize_xof();
    for _ in 0..256 {
        let mut bytes = [0u8; 16];
        reader.fill(&mut bytes);
        ids.push(DocumentId::from_uuid(Uuid::from_bytes(bytes)));
    }
    // Include the extremes, which is where a sign or a case mistake shows.
    ids.push(DocumentId::from_uuid(Uuid::from_bytes([0x00; 16])));
    ids.push(DocumentId::from_uuid(Uuid::from_bytes([0xff; 16])));
    ids.push(DocumentId::from_uuid(Uuid::from_bytes([0x7f; 16])));
    ids.push(DocumentId::from_uuid(Uuid::from_bytes([0x80; 16])));

    let mut by_bytes = ids.clone();
    by_bytes.sort_by_key(|id| *id.as_uuid().as_bytes());
    let mut by_storage = ids.clone();
    by_storage.sort_by_key(|id| id.to_storage());
    assert_eq!(
        by_bytes.iter().map(|i| i.to_storage()).collect::<Vec<_>>(),
        by_storage
            .iter()
            .map(|i| i.to_storage())
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_wrong_wek_version_makes_the_checkpoint_unopenable() {
    let mut rng = VectorRng::new(b"wek-version");
    let wek = WorkspaceEncryptionKey::from_bytes([0x63; 32]);
    let bytes = seal_checkpoint(&sample_header(), &sample_bootstrap(), &wek, &mut rng).unwrap();

    let other = WorkspaceEncryptionKey::from_bytes([0x64; 32]);
    assert_eq!(
        open_checkpoint(&bytes, &other).unwrap_err(),
        CryptoError::Commitment
    );
}
