// SPDX-License-Identifier: MPL-2.0

use koru_store_core::ids::WorkspaceId;
use uuid::Uuid;

use super::*;
use crate::testkit::VectorRng;

fn header() -> BlobMappingHeader {
    BlobMappingHeader::new(
        WorkspaceId::from_uuid(Uuid::from_bytes([0x71; 16])),
        WekVersion(5),
        BlobIdentityKeyVersion(2),
        CloudObjectId::from_bytes([0x81; 32]),
    )
}

fn blob_hash() -> BlobHash {
    BlobHash::from_digest("b3", &"ab".repeat(32)).unwrap()
}

#[test]
fn a_mapping_row_round_trips_and_carries_no_plaintext_blob_hash() {
    let mut rng = VectorRng::new(b"mapping");
    let wek = WorkspaceEncryptionKey::from_bytes([0x91; 32]);
    let bytes = BlobMappingRecordV1::seal(&header(), &blob_hash(), &wek, &mut rng).unwrap();

    // The digest must not appear anywhere in the stored row.
    let needle = blob_hash().as_str().as_bytes().to_vec();
    assert!(
        !bytes.windows(needle.len()).any(|w| w == needle),
        "the plaintext BlobHash is present in the stored row"
    );

    let (parsed, hash) = BlobMappingRecordV1::open(&bytes, &wek).unwrap();
    assert_eq!(parsed, header());
    assert_eq!(hash, blob_hash());
}

#[test]
fn a_row_from_another_workspace_does_not_open() {
    let mut rng = VectorRng::new(b"mapping-context");
    let wek = WorkspaceEncryptionKey::from_bytes([0x92; 32]);
    let bytes = BlobMappingRecordV1::seal(&header(), &blob_hash(), &wek, &mut rng).unwrap();

    let other = BlobMappingHeader::new(
        WorkspaceId::from_uuid(Uuid::from_bytes([0x72; 16])),
        WekVersion(5),
        BlobIdentityKeyVersion(2),
        CloudObjectId::from_bytes([0x81; 32]),
    );
    let mut relabelled = other.encode();
    relabelled.extend_from_slice(&bytes[MAPPING_HEADER_LEN..]);
    assert_eq!(
        BlobMappingRecordV1::open(&relabelled, &wek).unwrap_err(),
        CryptoError::Commitment
    );
}

#[test]
fn every_header_byte_is_bound() {
    let mut rng = VectorRng::new(b"mapping-bound");
    let wek = WorkspaceEncryptionKey::from_bytes([0x93; 32]);
    let bytes = BlobMappingRecordV1::seal(&header(), &blob_hash(), &wek, &mut rng).unwrap();
    for index in 0..MAPPING_HEADER_LEN {
        let mut tampered = bytes.clone();
        tampered[index] ^= 0x01;
        assert!(
            BlobMappingRecordV1::open(&tampered, &wek).is_err(),
            "header byte {index} opened"
        );
    }
}

#[test]
fn availability_metadata_holds_nothing_that_identifies_a_plaintext() {
    let row = ObjectAvailabilityV1::new(
        CloudObjectId::from_bytes([0x81; 32]),
        Uuid::from_bytes([0x82; 16]),
        // 162 fixed bytes plus a 56-byte sealed header plus one chunk of 1 KiB.
        (crate::content::CONTENT_HEADER_LEN + crate::envelope::WRAPPED_KEY_LEN) as u64
            + 56
            + 1024
            + 16,
        ObjectDurabilityState::Durable,
    );
    assert_eq!(row.plaintext_len().unwrap(), 1024);

    let encoded = row.encode();
    assert_eq!(ObjectAvailabilityV1::decode(&encoded).unwrap(), row);

    // No field can hold a hash, a name or a MIME type: the struct has five
    // fields and this is the whole of them.
    let needle = blob_hash().as_str().as_bytes().to_vec();
    assert!(!encoded.windows(needle.len()).any(|w| w == needle));
}

#[test]
fn an_availability_row_with_an_impossible_stored_length_is_refused() {
    let row = ObjectAvailabilityV1::new(
        CloudObjectId::from_bytes([0x81; 32]),
        Uuid::from_bytes([0x82; 16]),
        7,
        ObjectDurabilityState::Staged,
    );
    assert!(row.plaintext_len().is_err());
}

#[test]
fn a_truncated_or_extended_availability_row_is_malformed() {
    let row = ObjectAvailabilityV1::new(
        CloudObjectId::from_bytes([0x81; 32]),
        Uuid::from_bytes([0x82; 16]),
        1000,
        ObjectDurabilityState::PendingPurge,
    );
    let encoded = row.encode();
    for cut in 0..encoded.len() {
        assert!(ObjectAvailabilityV1::decode(&encoded[..cut]).is_err());
    }
    let mut extended = encoded.clone();
    extended.push(0);
    assert_eq!(
        ObjectAvailabilityV1::decode(&extended).unwrap_err(),
        CryptoError::Malformed
    );
}
