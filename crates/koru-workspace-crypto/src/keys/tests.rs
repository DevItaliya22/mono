// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::testkit::VectorRng;

#[test]
fn a_secret_never_prints_itself() {
    let key = WorkspaceEncryptionKey::from_bytes([0x11; 32]);
    let printed = format!("{key:?}");
    assert_eq!(printed, "WorkspaceEncryptionKey([REDACTED])");
    assert!(!printed.contains("11"));

    let dek = ContentEncryptionKey::from_bytes([0x22; 32]);
    assert_eq!(format!("{dek:?}"), "ContentEncryptionKey([REDACTED])");
    let account = AccountMetadataKey::from_bytes([0x33; 32]);
    assert_eq!(format!("{account:?}"), "AccountMetadataKey([REDACTED])");
    let kek = RegionalMetadataKek::from_bytes([0x44; 32]);
    assert_eq!(format!("{kek:?}"), "RegionalMetadataKek([REDACTED])");
    let identity = WorkspaceBlobIdentityKey::from_bytes([0x55; 32]);
    assert_eq!(
        format!("{identity:?}"),
        "WorkspaceBlobIdentityKey([REDACTED])"
    );
}

#[test]
fn generated_keys_are_not_constant() {
    let mut rng = VectorRng::new(b"keys");
    let first = WorkspaceEncryptionKey::generate(&mut rng);
    let second = WorkspaceEncryptionKey::generate(&mut rng);
    assert_ne!(first.as_bytes(), second.as_bytes());
    assert_ne!(first.as_bytes(), &[0u8; 32]);
}

#[test]
fn a_key_id_prints_as_a_prefix_and_none_prints_as_none() {
    assert_eq!(format!("{:?}", KeyId::NONE), "KeyId(NONE)");
    assert!(KeyId::NONE.is_none());
    let id = KeyId::from_bytes([0xab; 16]);
    assert!(!id.is_none());
    assert_eq!(format!("{id:?}"), "KeyId(abababab…)");
}

/// **The compile-time half of D90.** [`WorkspaceBlobIdentityKey`] has no
/// `as_input_key`, so it cannot reach [`crate::sealed`] and cannot be used as
/// an encryption key. This test cannot assert the absence of a method — that is
/// what the compiler does — so it asserts the presence of the four that do have
/// it, which is the list somebody would have to edit to break the rule.
#[test]
fn exactly_four_key_types_may_seal() {
    let mut rng = VectorRng::new(b"sealing");
    let _ = WorkspaceEncryptionKey::generate(&mut rng).as_input_key();
    let _ = ContentEncryptionKey::generate(&mut rng).as_input_key();
    let _ = AccountMetadataKey::generate(&mut rng).as_input_key();
    let _ = RegionalMetadataKek::generate(&mut rng).as_input_key();
    // WorkspaceBlobIdentityKey deliberately has no such method. Adding one
    // would make the line below compile, and would be the change this comment
    // exists to stop.
    let identity = WorkspaceBlobIdentityKey::generate(&mut rng);
    assert_eq!(identity.as_bytes().len(), 32);
}
