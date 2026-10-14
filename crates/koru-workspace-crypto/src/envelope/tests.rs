// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::keys::WorkspaceEncryptionKey;
use crate::sealed::SEALED_HEADER_LEN;
use crate::testkit::VectorRng;

fn key(seed: u8) -> WorkspaceEncryptionKey {
    WorkspaceEncryptionKey::from_bytes([seed; 32])
}

#[test]
fn a_wrapped_key_is_exactly_the_advertised_width() {
    let mut rng = VectorRng::new(b"envelope");
    let dek = ContentEncryptionKey::generate(&mut rng);
    let wrapped = wrap_key(&key(1).as_input_key(), b"header", &dek, &[2u8; SALT_LEN]).unwrap();
    assert_eq!(wrapped.len(), WRAPPED_KEY_LEN);
    assert_eq!(WRAPPED_KEY_LEN, 104);

    let opened = unwrap_key(&key(1).as_input_key(), b"header", &wrapped).unwrap();
    assert_eq!(opened.as_bytes(), dek.as_bytes());
}

#[test]
fn a_wrap_and_a_body_do_not_share_a_derivation() {
    // Same key, same header, same salt, different layer. The layer tag means
    // the two ciphertexts have nothing in common — including their
    // commitments — so neither can be presented as the other.
    let mut rng = VectorRng::new(b"layers");
    let dek = ContentEncryptionKey::generate(&mut rng);
    let wrapped = wrap_key(&key(3).as_input_key(), b"h", &dek, &[4u8; SALT_LEN]).unwrap();
    let body = seal_body(
        &key(3).as_input_key(),
        b"h",
        &[4u8; SALT_LEN],
        dek.as_bytes(),
    )
    .unwrap();
    assert_ne!(wrapped, body);
    assert_eq!(
        open_body(&key(3).as_input_key(), b"h", &wrapped).unwrap_err(),
        CryptoError::Commitment
    );
    assert_eq!(
        unwrap_key(&key(3).as_input_key(), b"h", &body).unwrap_err(),
        CryptoError::Commitment
    );
}

#[test]
fn a_wrong_kek_fails_at_the_commitment() {
    let mut rng = VectorRng::new(b"kek");
    let dek = ContentEncryptionKey::generate(&mut rng);
    let wrapped = wrap_key(&key(5).as_input_key(), b"h", &dek, &[6u8; SALT_LEN]).unwrap();
    assert_eq!(
        unwrap_key(&key(6).as_input_key(), b"h", &wrapped).unwrap_err(),
        CryptoError::Commitment
    );
}

#[test]
fn a_wrapped_key_of_the_wrong_length_is_malformed_before_any_derivation() {
    let mut rng = VectorRng::new(b"len");
    let dek = ContentEncryptionKey::generate(&mut rng);
    let wrapped = wrap_key(&key(7).as_input_key(), b"h", &dek, &[8u8; SALT_LEN]).unwrap();
    for cut in [0usize, 1, WRAPPED_KEY_LEN - 1] {
        assert_eq!(
            unwrap_key(&key(7).as_input_key(), b"h", &wrapped[..cut]).unwrap_err(),
            CryptoError::Malformed
        );
    }
    let mut extended = wrapped.clone();
    extended.push(0);
    assert_eq!(
        unwrap_key(&key(7).as_input_key(), b"h", &extended).unwrap_err(),
        CryptoError::Malformed
    );
}

#[test]
fn object_plaintext_len_refuses_a_length_that_cannot_hold_the_fixed_parts() {
    let floor = 64 + WRAPPED_KEY_LEN as u64 + SEALED_HEADER_LEN as u64;
    for stored in 0u64..floor {
        assert!(object_plaintext_len(64, stored).is_err(), "{stored}");
    }
    // And does not overflow on the largest values a length field can carry.
    assert!(object_plaintext_len(64, u64::MAX).is_err());
    assert!(object_plaintext_len(usize::MAX, 1).is_err());
}
