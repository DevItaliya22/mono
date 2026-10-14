// SPDX-License-Identifier: MPL-2.0

use koru_store_core::ids::WorkspaceId;
use uuid::Uuid;

use super::*;
use crate::sealed::CHUNK_LEN;
use crate::testkit::VectorRng;

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(Uuid::from_bytes([0x31; 16]))
}

fn blob_header() -> ContentObjectHeader {
    ContentObjectHeader::for_blob(
        workspace(),
        WekVersion(2),
        CloudObjectId::from_bytes([0x41; 32]),
    )
}

#[test]
fn a_blob_object_round_trips() {
    let mut rng = VectorRng::new(b"content");
    let wek = WorkspaceEncryptionKey::from_bytes([0x51; 32]);
    let plaintext: Vec<u8> = (0..CHUNK_LEN + 9).map(|i| (i % 97) as u8).collect();

    let bytes = seal_object(&blob_header(), &wek, &plaintext, &mut rng).unwrap();
    let object = ContentObject::parse(&bytes).unwrap();
    assert_eq!(object.header, blob_header());
    assert_eq!(object.open(&wek).unwrap(), plaintext);
}

#[test]
fn the_stored_length_gives_the_exact_plaintext_length_without_a_key() {
    // D40 end to end: empty, single byte, either side of a chunk boundary,
    // multi-chunk. The server holds no key and still gets the exact answer.
    let mut rng = VectorRng::new(b"lengths");
    let wek = WorkspaceEncryptionKey::from_bytes([0x52; 32]);
    for len in [
        0usize,
        1,
        CHUNK_LEN - 1,
        CHUNK_LEN,
        CHUNK_LEN + 1,
        3 * CHUNK_LEN + 5,
    ] {
        let plaintext = vec![0x7eu8; len];
        let bytes = seal_object(&blob_header(), &wek, &plaintext, &mut rng).unwrap();
        assert_eq!(
            object_plaintext_len(bytes.len() as u64).unwrap(),
            len as u64
        );
        assert_eq!(
            ContentObject::parse(&bytes)
                .unwrap()
                .plaintext_len()
                .unwrap(),
            len as u64
        );
    }
}

#[test]
fn the_ten_gibibyte_ceiling_is_arithmetic_rather_than_encrypted() {
    // D14's blob ceiling, and one byte either side of it. Computed from the
    // format's constants; encrypting 10 GiB in a unit test would assert the
    // same thing and take minutes.
    let ten_gib = 10u64 * 1024 * 1024 * 1024;
    for plaintext_len in [ten_gib - 1, ten_gib, ten_gib + 1] {
        let body = crate::sealed::SealedLayout::of_plaintext_len(plaintext_len).unwrap();
        let stored = CONTENT_HEADER_LEN as u64 + WRAPPED_KEY_LEN as u64 + body.ciphertext_len();
        assert_eq!(object_plaintext_len(stored).unwrap(), plaintext_len);
    }
}

#[test]
fn an_object_cannot_be_served_under_another_objects_name() {
    let mut rng = VectorRng::new(b"transplant");
    let wek = WorkspaceEncryptionKey::from_bytes([0x53; 32]);
    let bytes = seal_object(&blob_header(), &wek, b"payload", &mut rng).unwrap();

    // Re-label the object as a different CloudObjectId and it stops opening —
    // the header is the derivation context of both the wrap and the body.
    let other = ContentObjectHeader::for_blob(
        workspace(),
        WekVersion(2),
        CloudObjectId::from_bytes([0x42; 32]),
    );
    let mut relabelled = other.encode();
    relabelled.extend_from_slice(&bytes[CONTENT_HEADER_LEN..]);
    let object = ContentObject::parse(&relabelled).unwrap();
    assert_eq!(object.open(&wek).unwrap_err(), CryptoError::Commitment);
}

#[test]
fn an_object_from_another_workspace_or_wek_version_does_not_open() {
    let mut rng = VectorRng::new(b"context");
    let wek = WorkspaceEncryptionKey::from_bytes([0x54; 32]);
    let bytes = seal_object(&blob_header(), &wek, b"payload", &mut rng).unwrap();

    for header in [
        ContentObjectHeader::for_blob(
            WorkspaceId::from_uuid(Uuid::from_bytes([0x32; 16])),
            WekVersion(2),
            CloudObjectId::from_bytes([0x41; 32]),
        ),
        ContentObjectHeader::for_blob(
            workspace(),
            WekVersion(3),
            CloudObjectId::from_bytes([0x41; 32]),
        ),
    ] {
        let mut relabelled = header.encode();
        relabelled.extend_from_slice(&bytes[CONTENT_HEADER_LEN..]);
        assert_eq!(
            ContentObject::parse(&relabelled)
                .unwrap()
                .open(&wek)
                .unwrap_err(),
            CryptoError::Commitment
        );
    }
}

#[test]
fn a_segment_and_a_snapshot_with_the_same_locator_are_different_ciphertexts() {
    // The purpose is bound, so a segment cannot be replayed as a snapshot even
    // though both are Cloud-written objects under the same key and locator.
    let mut rng = VectorRng::new(b"purpose");
    let wek = WorkspaceEncryptionKey::from_bytes([0x55; 32]);
    let segment = ContentObjectHeader::for_cloud_object(
        Purpose::ContentSegment,
        workspace(),
        WekVersion(1),
        b"loc",
    )
    .unwrap();
    let snapshot = ContentObjectHeader::for_cloud_object(
        Purpose::ContentSnapshot,
        workspace(),
        WekVersion(1),
        b"loc",
    )
    .unwrap();
    assert_ne!(segment.encode(), snapshot.encode());

    let bytes = seal_object(&segment, &wek, b"update", &mut rng).unwrap();
    let mut relabelled = snapshot.encode();
    relabelled.extend_from_slice(&bytes[CONTENT_HEADER_LEN..]);
    assert_eq!(
        ContentObject::parse(&relabelled)
            .unwrap()
            .open(&wek)
            .unwrap_err(),
        CryptoError::Commitment
    );
}

#[test]
fn a_purpose_that_is_not_a_content_object_is_refused() {
    assert_eq!(
        ContentObjectHeader::for_cloud_object(
            Purpose::WekGrant,
            workspace(),
            WekVersion(1),
            b"loc"
        )
        .unwrap_err(),
        CryptoError::PurposeMismatch
    );
    let mut header = blob_header().encode();
    header[4..6].copy_from_slice(&Purpose::WekGrant.id().to_be_bytes());
    let mut reader = crate::codec::Reader::new(&header);
    assert_eq!(
        ContentObjectHeader::decode(&mut reader).unwrap_err(),
        CryptoError::PurposeMismatch
    );
}

#[test]
fn truncated_extended_and_reordered_objects_are_refused() {
    let mut rng = VectorRng::new(b"hostile");
    let wek = WorkspaceEncryptionKey::from_bytes([0x56; 32]);
    let plaintext: Vec<u8> = (0..2 * CHUNK_LEN + 3).map(|i| (i % 251) as u8).collect();
    let bytes = seal_object(&blob_header(), &wek, &plaintext, &mut rng).unwrap();

    // Truncated at every byte: either the parse refuses, or the open does.
    for cut in 0..bytes.len() {
        let outcome = ContentObject::parse(&bytes[..cut]).and_then(|o| o.open(&wek));
        assert!(outcome.is_err(), "a {cut}-byte prefix opened");
    }
    // Extended by one byte, which cannot be a valid chunk boundary.
    let mut extended = bytes.clone();
    extended.push(0);
    assert!(
        ContentObject::parse(&extended)
            .and_then(|o| o.open(&wek))
            .is_err()
    );

    // Chunks 0 and 1 swapped.
    let body_at = CONTENT_HEADER_LEN + WRAPPED_KEY_LEN;
    let stride = CHUNK_LEN + 16;
    let mut reordered = bytes.clone();
    let first = body_at + crate::sealed::SEALED_HEADER_LEN;
    let (a, b) = (first, first + stride);
    let chunk_a = reordered[a..a + stride].to_vec();
    let chunk_b = reordered[b..b + stride].to_vec();
    reordered[a..a + stride].copy_from_slice(&chunk_b);
    reordered[b..b + stride].copy_from_slice(&chunk_a);
    assert_eq!(
        ContentObject::parse(&reordered)
            .unwrap()
            .open(&wek)
            .unwrap_err(),
        CryptoError::Aead
    );
}

#[test]
fn a_locator_binding_hides_the_locator_layout() {
    // The binding is a hash, so Plan 4.2 can change its object-key layout
    // without changing this format — and two locators never collide.
    assert_ne!(locator_binding(b"a"), locator_binding(b"b"));
    assert_ne!(locator_binding(b"ab"), locator_binding(b"a\0b"));
    assert_eq!(locator_binding(b"a"), locator_binding(b"a"));
}

#[test]
fn a_dek_is_fresh_for_every_object() {
    let mut rng = VectorRng::new(b"dek");
    let wek = WorkspaceEncryptionKey::from_bytes([0x57; 32]);
    let first = seal_object(&blob_header(), &wek, b"same", &mut rng).unwrap();
    let second = seal_object(&blob_header(), &wek, b"same", &mut rng).unwrap();
    assert_ne!(
        first, second,
        "identical plaintext must not seal identically"
    );

    let a = ContentObject::parse(&first)
        .unwrap()
        .unwrap_dek(&wek)
        .unwrap();
    let b = ContentObject::parse(&second)
        .unwrap()
        .unwrap_dek(&wek)
        .unwrap();
    assert_ne!(a.as_bytes(), b.as_bytes());
}

#[test]
fn destroying_one_object_wrap_erases_that_object_and_no_other() {
    // What the per-object DEK actually buys (D86): the purge of Plan 5.4 can
    // make one object unreadable **even to a holder of the WEK**, without
    // touching any other object in the workspace.
    let mut rng = VectorRng::new(b"erasure");
    let wek = WorkspaceEncryptionKey::from_bytes([0x58; 32]);
    let first = seal_object(&blob_header(), &wek, b"the doomed object", &mut rng).unwrap();
    let neighbour = ContentObjectHeader::for_blob(
        workspace(),
        WekVersion(2),
        CloudObjectId::from_bytes([0x43; 32]),
    );
    let second = seal_object(&neighbour, &wek, b"the surviving object", &mut rng).unwrap();

    // Destroy the first object's 104-byte wrap by overwriting it. The header and
    // body are untouched and the WEK is intact.
    let mut erased = first.clone();
    for byte in &mut erased[CONTENT_HEADER_LEN..CONTENT_HEADER_LEN + WRAPPED_KEY_LEN] {
        *byte = 0;
    }
    assert_eq!(
        ContentObject::parse(&erased)
            .unwrap()
            .open(&wek)
            .unwrap_err(),
        CryptoError::Commitment
    );
    // The neighbour is unaffected, which is the half that makes it erasure of
    // one object rather than of the workspace.
    assert_eq!(
        ContentObject::parse(&second).unwrap().open(&wek).unwrap(),
        b"the surviving object"
    );
}

#[test]
fn a_rotation_leaves_older_objects_readable_under_the_version_they_name() {
    // Rotation compatibility, which is what §33 and D14/D17 actually ask for:
    // historical objects are not rewritten, each names its own `wek_version`,
    // and the grant for that version keeps existing. Neither key opens the
    // other's object.
    let mut rng = VectorRng::new(b"rotation");
    let wek_v2 = WorkspaceEncryptionKey::from_bytes([0x59; 32]);
    let wek_v3 = WorkspaceEncryptionKey::from_bytes([0x5a; 32]);

    let old = seal_object(&blob_header(), &wek_v2, b"written before", &mut rng).unwrap();
    let new_header = ContentObjectHeader::for_blob(
        workspace(),
        WekVersion(3),
        CloudObjectId::from_bytes([0x44; 32]),
    );
    let new = seal_object(&new_header, &wek_v3, b"written after", &mut rng).unwrap();

    assert_eq!(
        ContentObject::parse(&old).unwrap().header.wek_version,
        WekVersion(2)
    );
    assert_eq!(
        ContentObject::parse(&old).unwrap().open(&wek_v2).unwrap(),
        b"written before"
    );
    assert_eq!(
        ContentObject::parse(&new).unwrap().open(&wek_v3).unwrap(),
        b"written after"
    );
    assert!(ContentObject::parse(&old).unwrap().open(&wek_v3).is_err());
    assert!(ContentObject::parse(&new).unwrap().open(&wek_v2).is_err());
}
