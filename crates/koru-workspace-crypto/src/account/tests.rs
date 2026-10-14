// SPDX-License-Identifier: MPL-2.0

use uuid::Uuid;

use super::*;
use crate::testkit::VectorRng;

pub(crate) fn key_ref() -> AccountMetadataKeyRef {
    AccountMetadataKeyRef {
        account_id: Uuid::from_bytes([0xa1; 16]),
        key_id: KeyId::from_bytes([0xa2; 16]),
        key_version: 1,
        region: Region::US,
    }
}

pub(crate) fn wrapped_ref(kek_version: u32) -> WrappedKeyRef {
    WrappedKeyRef {
        key_ref: key_ref(),
        kek_id: MetadataKekId::new(KeyId::from_bytes([0xa3; 16])),
        kek_version,
    }
}

#[test]
fn an_envelope_round_trips_over_an_opaque_plaintext() {
    // The plaintext is opaque on purpose: which entries a purged account record
    // carries is Plan 3.2's, and this crate must not know.
    let mut rng = VectorRng::new(b"account");
    let account_key = AccountMetadataKey::generate(&mut rng);
    let envelope = AccountMetadataEnvelopeV1::new(key_ref(), 0x0007);
    let plaintext = b"whatever plan 3.2 decides this is";

    let bytes = envelope.seal(&account_key, plaintext, &mut rng).unwrap();
    assert_eq!(AccountMetadataEnvelopeV1::parse(&bytes).unwrap(), envelope);
    let (parsed, opened) = AccountMetadataEnvelopeV1::open(&bytes, &account_key).unwrap();
    assert_eq!(parsed, envelope);
    assert_eq!(opened, plaintext);
}

#[test]
fn the_wrapped_key_is_never_inside_the_envelope() {
    // If it were, destroying "the" wrapped key would erase nothing, because
    // every envelope would carry a copy. Crypto-erasure only means anything if
    // there is exactly one thing to destroy.
    let mut rng = VectorRng::new(b"one-copy");
    let kek = RegionalMetadataKek::generate(&mut rng);
    let account_key = AccountMetadataKey::generate(&mut rng);
    let wrapped =
        WrappedAccountMetadataKeyV1::seal(&wrapped_ref(4), &account_key, &kek, &mut rng).unwrap();
    let envelope = AccountMetadataEnvelopeV1::new(key_ref(), 1)
        .seal(&account_key, b"payload", &mut rng)
        .unwrap();

    assert!(
        !envelope
            .windows(wrapped.len())
            .any(|w| w == wrapped.as_slice()),
        "the wrapped key must not be copied into an envelope"
    );
    // Nor may the account key itself appear in either record.
    for record in [&wrapped, &envelope] {
        assert!(
            !record
                .windows(32)
                .any(|w| w == account_key.as_bytes().as_slice()),
            "the account metadata key must never appear in a stored record"
        );
    }
}

#[test]
fn destroying_the_wrapped_key_renders_the_envelope_unrecoverable() {
    // D91's required failing case, and D90's honest statement of what it does
    // and does not cover. What is destroyed here is the *only* remaining
    // decryption path for this ciphertext; plaintext in a log, a search index,
    // a cache, an analytics event or a backup of the unwrapped key is not
    // touched by this and no format decision could touch it.
    let mut rng = VectorRng::new(b"erasure");
    let kek = RegionalMetadataKek::generate(&mut rng);
    let account_key = AccountMetadataKey::generate(&mut rng);
    let wrapped =
        WrappedAccountMetadataKeyV1::seal(&wrapped_ref(4), &account_key, &kek, &mut rng).unwrap();
    let envelope = AccountMetadataEnvelopeV1::new(key_ref(), 1)
        .seal(&account_key, b"the display name", &mut rng)
        .unwrap();

    // With the wrapped key, the envelope opens.
    let (_, recovered) = WrappedAccountMetadataKeyV1::open(&wrapped, &kek).unwrap();
    let (_, plaintext) = AccountMetadataEnvelopeV1::open(&envelope, &recovered).unwrap();
    assert_eq!(plaintext, b"the display name");

    // Destroy it — here, simply by not having it. The KEK alone opens nothing,
    // and the envelope's cleartext header still says which key is missing,
    // which is what makes the loss diagnosable rather than mysterious.
    drop(wrapped);
    let header = AccountMetadataEnvelopeV1::parse(&envelope).unwrap();
    assert_eq!(header.key_ref, key_ref());
    assert_eq!(
        AccountMetadataEnvelopeV1::open(
            &envelope,
            &AccountMetadataKey::from_bytes(*kek.as_bytes())
        )
        .unwrap_err(),
        CryptoError::Commitment
    );
}

#[test]
fn a_key_from_another_hierarchy_does_not_open_this_one() {
    // D30's cryptographic half, over raw bytes rather than over types: the same
    // 32 bytes offered for a different purpose fails, because the purpose is
    // bound into the derivation context.
    let mut rng = VectorRng::new(b"purposes");
    let shared_bytes = [0x5a; 32];
    let kek = RegionalMetadataKek::from_bytes(shared_bytes);
    let account_key = AccountMetadataKey::from_bytes(shared_bytes);

    let wrapped =
        WrappedAccountMetadataKeyV1::seal(&wrapped_ref(4), &account_key, &kek, &mut rng).unwrap();
    // The wrap and the envelope carry different purposes, so even with
    // identical key bytes neither opens as the other.
    assert_eq!(
        AccountMetadataEnvelopeV1::open(&wrapped, &account_key).unwrap_err(),
        CryptoError::PurposeMismatch
    );
    let envelope = AccountMetadataEnvelopeV1::new(key_ref(), 1)
        .seal(&account_key, b"x", &mut rng)
        .unwrap();
    assert_eq!(
        WrappedAccountMetadataKeyV1::open(&envelope, &kek).unwrap_err(),
        CryptoError::PurposeMismatch
    );
}

#[test]
fn an_envelope_from_another_account_or_region_does_not_open() {
    let mut rng = VectorRng::new(b"cross");
    let account_key = AccountMetadataKey::generate(&mut rng);
    let bytes = AccountMetadataEnvelopeV1::new(key_ref(), 1)
        .seal(&account_key, b"payload", &mut rng)
        .unwrap();

    for altered in [
        AccountMetadataKeyRef {
            account_id: Uuid::from_bytes([0xb1; 16]),
            ..key_ref()
        },
        AccountMetadataKeyRef {
            region: Region::EU,
            ..key_ref()
        },
        AccountMetadataKeyRef {
            key_version: 2,
            ..key_ref()
        },
    ] {
        let mut relabelled = AccountMetadataEnvelopeV1::new(altered, 1)
            .seal(&account_key, b"payload", &mut rng)
            .unwrap()[..ENVELOPE_HEADER_LEN]
            .to_vec();
        relabelled.extend_from_slice(&bytes[ENVELOPE_HEADER_LEN..]);
        assert_eq!(
            AccountMetadataEnvelopeV1::open(&relabelled, &account_key).unwrap_err(),
            CryptoError::Commitment
        );
    }
}

#[test]
fn the_record_type_is_bound_even_though_it_is_not_interpreted() {
    let mut rng = VectorRng::new(b"record-type");
    let account_key = AccountMetadataKey::generate(&mut rng);
    let bytes = AccountMetadataEnvelopeV1::new(key_ref(), 1)
        .seal(&account_key, b"payload", &mut rng)
        .unwrap();
    let mut relabelled = AccountMetadataEnvelopeV1::new(key_ref(), 2)
        .seal(&account_key, b"payload", &mut rng)
        .unwrap()[..ENVELOPE_HEADER_LEN]
        .to_vec();
    relabelled.extend_from_slice(&bytes[ENVELOPE_HEADER_LEN..]);
    assert_eq!(
        AccountMetadataEnvelopeV1::open(&relabelled, &account_key).unwrap_err(),
        CryptoError::Commitment
    );
}

#[test]
fn rotating_the_account_key_re_encrypts_and_rotating_the_kek_does_not() {
    let mut rng = VectorRng::new(b"rotation");
    let kek_v4 = RegionalMetadataKek::generate(&mut rng);
    let kek_v5 = RegionalMetadataKek::generate(&mut rng);
    let account_key = AccountMetadataKey::generate(&mut rng);

    let envelope = AccountMetadataEnvelopeV1::new(key_ref(), 9)
        .seal(&account_key, b"tombstone bytes", &mut rng)
        .unwrap();

    // A KEK rotation rewraps one record. The envelope is byte-identical
    // afterwards — it does not name the KEK — and still opens under the same
    // account key.
    let wrapped_v4 =
        WrappedAccountMetadataKeyV1::seal(&wrapped_ref(4), &account_key, &kek_v4, &mut rng)
            .unwrap();
    let (_, recovered) = WrappedAccountMetadataKeyV1::open(&wrapped_v4, &kek_v4).unwrap();
    let wrapped_v5 =
        WrappedAccountMetadataKeyV1::seal(&wrapped_ref(5), &recovered, &kek_v5, &mut rng).unwrap();
    let (_, recovered_v5) = WrappedAccountMetadataKeyV1::open(&wrapped_v5, &kek_v5).unwrap();
    assert_eq!(recovered_v5.as_bytes(), account_key.as_bytes());
    assert_eq!(
        AccountMetadataEnvelopeV1::open(&envelope, &recovered_v5)
            .unwrap()
            .1,
        b"tombstone bytes"
    );

    // An account-key rotation does re-encrypt, and the old key stops working.
    let new_key = AccountMetadataKey::generate(&mut rng);
    let new_ref = AccountMetadataKeyRef {
        key_id: KeyId::from_bytes([0xc1; 16]),
        key_version: 2,
        ..key_ref()
    };
    let rotated = rotate_envelope(&envelope, &account_key, &new_ref, &new_key, &mut rng).unwrap();
    let (parsed, plaintext) = AccountMetadataEnvelopeV1::open(&rotated, &new_key).unwrap();
    assert_eq!(parsed.key_ref, new_ref);
    assert_eq!(parsed.record_type, 9);
    assert_eq!(plaintext, b"tombstone bytes");
    assert!(AccountMetadataEnvelopeV1::open(&rotated, &account_key).is_err());
}

#[test]
fn a_rotation_may_not_move_a_record_between_accounts() {
    let mut rng = VectorRng::new(b"no-move");
    let account_key = AccountMetadataKey::generate(&mut rng);
    let new_key = AccountMetadataKey::generate(&mut rng);
    let envelope = AccountMetadataEnvelopeV1::new(key_ref(), 1)
        .seal(&account_key, b"x", &mut rng)
        .unwrap();
    let elsewhere = AccountMetadataKeyRef {
        account_id: Uuid::from_bytes([0xd1; 16]),
        ..key_ref()
    };
    assert_eq!(
        rotate_envelope(&envelope, &account_key, &elsewhere, &new_key, &mut rng).unwrap_err(),
        CryptoError::ContextMismatch
    );
}

#[test]
fn a_region_is_a_stable_code_and_nothing_else() {
    assert_eq!(Region::parse("us").unwrap(), Region::US);
    assert_eq!(Region::parse("eu").unwrap(), Region::EU);
    assert_eq!(
        Region::from_bytes(*Region::US.as_bytes()).unwrap(),
        Region::US
    );
    for bad in ["", "US", "united states!", "toolongregion", "ü"] {
        assert!(Region::parse(bad).is_err(), "{bad} parsed");
    }
    // Trailing bytes after the padding are a malformed field, not padding.
    assert!(Region::from_bytes(*b"us\0\0\0\0\0x").is_err());
}

#[test]
fn a_truncated_or_extended_envelope_is_refused() {
    let mut rng = VectorRng::new(b"trunc");
    let account_key = AccountMetadataKey::generate(&mut rng);
    let bytes = AccountMetadataEnvelopeV1::new(key_ref(), 1)
        .seal(&account_key, b"payload", &mut rng)
        .unwrap();
    for cut in 0..bytes.len() {
        assert!(
            AccountMetadataEnvelopeV1::open(&bytes[..cut], &account_key).is_err(),
            "a {cut}-byte prefix opened"
        );
    }
    let mut extended = bytes.clone();
    extended.push(0);
    assert!(AccountMetadataEnvelopeV1::open(&extended, &account_key).is_err());
}
