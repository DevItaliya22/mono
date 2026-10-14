// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn unknown_ids_fail_closed() {
    assert_eq!(
        GrantSuite::from_id(0).unwrap_err(),
        CryptoError::UnsupportedSuite
    );
    assert_eq!(
        GrantSuite::from_id(0xffff).unwrap_err(),
        CryptoError::UnsupportedSuite
    );
    assert_eq!(
        ContentSuite::from_id(0x0002).unwrap_err(),
        CryptoError::UnsupportedSuite
    );
    assert_eq!(
        Purpose::from_id(0x0002).unwrap_err(),
        CryptoError::PurposeMismatch
    );
    assert_eq!(
        RecipientType::from_tag(0).unwrap_err(),
        CryptoError::UnsupportedSuite
    );
    assert_eq!(
        AuthorType::from_tag(9).unwrap_err(),
        CryptoError::UnsupportedSuite
    );
}

#[test]
fn every_id_round_trips() {
    for suite in [
        GrantSuite::X25519HkdfSha256ChaCha20Poly1305,
        GrantSuite::P256HkdfSha256ChaCha20Poly1305,
    ] {
        assert_eq!(GrantSuite::from_id(suite.id()).unwrap(), suite);
    }
    assert_eq!(
        ContentSuite::from_id(ContentSuite::Cobblestone256.id()).unwrap(),
        ContentSuite::Cobblestone256
    );
    for purpose in [
        Purpose::WekGrant,
        Purpose::ContentBlobObject,
        Purpose::ContentSegment,
        Purpose::ContentSnapshot,
        Purpose::RecoveryCheckpoint,
        Purpose::BlobMappingRecord,
        Purpose::AccountMetadataRecord,
        Purpose::AccountMetadataKeyWrap,
    ] {
        assert_eq!(Purpose::from_id(purpose.id()).unwrap(), purpose);
    }
}

#[test]
fn no_two_purposes_share_a_value() {
    let all = [
        Purpose::WekGrant,
        Purpose::ContentBlobObject,
        Purpose::ContentSegment,
        Purpose::ContentSnapshot,
        Purpose::RecoveryCheckpoint,
        Purpose::BlobMappingRecord,
        Purpose::AccountMetadataRecord,
        Purpose::AccountMetadataKeyWrap,
    ];
    let mut ids: Vec<u16> = all.iter().map(|p| p.id()).collect();
    ids.sort_unstable();
    let count = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), count, "purpose ids must be unique");
}

#[test]
fn a_deprecated_suite_is_readable_and_unwritable_for_a_device() {
    // P-256 is not a deprecated suite; it is the shape the check has. A device
    // recipient may only be written under the one approved device suite, and
    // the recovery recipient may use either, because its KEM is chosen by the
    // recovery key's own material (D88).
    assert!(GrantSuite::X25519HkdfSha256ChaCha20Poly1305.writable_for(RecipientType::Device));
    assert!(!GrantSuite::P256HkdfSha256ChaCha20Poly1305.writable_for(RecipientType::Device));
    assert!(!GrantSuite::P256HkdfSha256ChaCha20Poly1305.writable_for(RecipientType::CloudReplica));
    assert!(
        GrantSuite::P256HkdfSha256ChaCha20Poly1305.writable_for(RecipientType::RegionalRecovery)
    );
    assert!(
        GrantSuite::X25519HkdfSha256ChaCha20Poly1305.writable_for(RecipientType::RegionalRecovery)
    );
}

#[test]
fn the_rfc_9180_code_points_are_the_ones_the_documents_claim() {
    assert_eq!(
        GrantSuite::X25519HkdfSha256ChaCha20Poly1305.rfc9180_ids(),
        (0x0020, 0x0001, 0x0003)
    );
    assert_eq!(
        GrantSuite::P256HkdfSha256ChaCha20Poly1305.rfc9180_ids(),
        (0x0010, 0x0001, 0x0003)
    );
    assert_eq!(
        GrantSuite::X25519HkdfSha256ChaCha20Poly1305.encapsulation_len(),
        32
    );
    assert_eq!(
        GrantSuite::P256HkdfSha256ChaCha20Poly1305.encapsulation_len(),
        65
    );
}
