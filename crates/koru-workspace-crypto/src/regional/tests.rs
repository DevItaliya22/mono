// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn there_are_exactly_four_purposes_and_one_is_symmetric() {
    assert_eq!(RegionalPurpose::ALL.len(), 4);
    let symmetric: Vec<_> = RegionalPurpose::ALL
        .iter()
        .filter(|p| p.is_symmetric())
        .collect();
    assert_eq!(symmetric, vec![&RegionalPurpose::AccountMetadataEncryption]);
}

#[test]
fn no_two_purposes_share_a_label() {
    let mut labels: Vec<&str> = RegionalPurpose::ALL.iter().map(|p| p.label()).collect();
    labels.sort_unstable();
    let count = labels.len();
    labels.dedup();
    assert_eq!(labels.len(), count);
    // Stable codes, so they can appear in an audit record unchanged.
    for label in labels {
        assert!(
            label.bytes().all(|b| b.is_ascii_uppercase() || b == b'_'),
            "{label} is not an UPPER_SNAKE_CASE code"
        );
    }
}

#[test]
fn each_key_id_type_declares_the_one_purpose_it_belongs_to() {
    assert_eq!(
        TokenSigningKeyId::PURPOSE,
        RegionalPurpose::TokenCapabilitySigning
    );
    assert_eq!(
        TrustResetKeyId::PURPOSE,
        RegionalPurpose::TrustResetAttestation
    );
    assert_eq!(
        RecoveryUnwrapKeyId::PURPOSE,
        RegionalPurpose::WorkspaceRecoveryUnwrap
    );
    assert_eq!(
        MetadataKekId::PURPOSE,
        RegionalPurpose::AccountMetadataEncryption
    );

    // The types are distinct even when the bytes are identical, which is the
    // whole of the compile-time separation: `MetadataKekId::new(id)` cannot be
    // passed where a `RecoveryUnwrapKeyId` is expected, and there is no
    // conversion between them to reach for.
    let id = KeyId::from_bytes([7u8; 16]);
    assert_eq!(
        MetadataKekId::new(id).key_id(),
        RecoveryUnwrapKeyId::new(id).key_id()
    );
}

#[test]
fn a_recovery_key_id_is_a_shard_and_not_a_key() {
    // D88: sharding recovery keys by account or workspace bucket must not need
    // a format change, so the shard is its own field beside the key id.
    let shard = RecoveryKeyId::new(KeyId::from_bytes([1u8; 16]));
    let key = RecoveryUnwrapKeyId::new(KeyId::from_bytes([2u8; 16]));
    assert_ne!(shard.key_id(), key.key_id());
    assert!(!shard.key_id().is_none());
}
