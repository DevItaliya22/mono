// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::{CredentialIssuance, REFRESH_ABSOLUTE_MILLIS};

const NOW: i64 = 1_000_000;
const LEGACY: &str = "https://api.koru.do";
const REGIONAL: &str = "https://us.api.koru.do";

fn policy(issuance: CredentialIssuance) -> RegionalCredentialPolicy {
    RegionalCredentialPolicy {
        region: Region::Us,
        legacy_issuer: LEGACY.into(),
        regional_issuer: REGIONAL.into(),
        legacy_cutover_millis: NOW,
        issuance,
        legacy_access_kids: ["old-access".into()].into(),
        legacy_attestation_kids: ["old-attestation".into()].into(),
    }
}

fn claims(issuer: &str) -> DeviceAttestationClaimsV1 {
    DeviceAttestationClaimsV1 {
        version: DEVICE_ATTESTATION_VERSION,
        issuer: issuer.into(),
        audience: DEVICE_ATTESTATION_AUDIENCE.into(),
        koru_identity_id: "00000000-0000-4000-8000-000000000001".parse().unwrap(),
        device_id: DeviceId::parse("00000000-0000-4000-8000-000000000002").unwrap(),
        signing_public_key: DevicePublicKey::from_bytes([11; 32]),
        endpoint_id: ReplicationEndpointId::from_bytes([12; 32]),
        authorized_profile_ids: vec![
            ProfileId::parse("00000000-0000-4000-8000-000000000003").unwrap(),
        ],
        session_id: "00000000-0000-4000-8000-000000000004".parse().unwrap(),
        token_id: "proof".into(),
        issued_at_millis: NOW,
        expires_at_millis: NOW + DEVICE_ATTESTATION_MAX_MILLIS,
    }
}

fn access(issuer: &str) -> AccessTokenClaims {
    let attestation = claims(issuer);
    AccessTokenClaims {
        issuer: issuer.into(),
        audience: "koru-native".into(),
        subject: attestation.koru_identity_id,
        session_id: attestation.session_id,
        token_id: "access".into(),
        issued_at_millis: NOW,
        expires_at_millis: NOW + ACCESS_TOKEN_MILLIS,
    }
}

fn payload(compact: &str) -> serde_json::Value {
    serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(compact.split('.').nth(1).unwrap())
            .unwrap(),
    )
    .unwrap()
}

fn rewrite(compact: &str, value: serde_json::Value, key: &SigningKey) -> String {
    let input = format!(
        "{}.{}",
        compact.split('.').next().unwrap(),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&value).unwrap())
    );
    format!(
        "{input}.{}",
        URL_SAFE_NO_PAD.encode(key.sign(input.as_bytes()).to_bytes())
    )
}

#[test]
fn issuance_is_opt_in_and_regional_credentials_have_a_real_wire_version() {
    let old = Ed25519KeyRing::new(
        "old-access".into(),
        [("old-access".into(), SigningKey::from_bytes(&[1; 32]))],
    )
    .unwrap();
    let legacy = old.sign_access_token(&access(LEGACY)).unwrap();
    let legacy_wire = payload(std::str::from_utf8(legacy.compact.expose_secret()).unwrap());
    assert!(legacy_wire.get("region").is_none());
    assert!(legacy_wire.get("ver").is_none());
    let policy = policy(CredentialIssuance::Regional);
    let regional = Ed25519KeyRing::new(
        "new-access".into(),
        [("new-access".into(), SigningKey::from_bytes(&[2; 32]))],
    )
    .unwrap()
    .with_regional_policy(policy.clone())
    .unwrap();
    let token = regional.sign_access_token(&access(REGIONAL)).unwrap();
    let wire = payload(std::str::from_utf8(token.compact.expose_secret()).unwrap());
    assert_eq!(wire["region"], "US");
    assert_eq!(wire["ver"], 2);
    assert!(
        verify_access_token(
            token.compact.expose_secret(),
            &regional.verification_keys(),
            REGIONAL,
            "koru-native",
            NOW
        )
        .is_err()
    );
    assert!(
        verify_access_token_with_policy(
            token.compact.expose_secret(),
            &regional.verification_keys(),
            REGIONAL,
            "koru-native",
            NOW,
            Some(&policy)
        )
        .is_ok()
    );
    let ring = DeviceAttestationKeyRing::new(
        "new-attestation".into(),
        [("new-attestation".into(), SigningKey::from_bytes(&[3; 32]))],
    )
    .unwrap()
    .with_regional_policy(policy.clone())
    .unwrap();
    let signed = ring.sign(&claims(REGIONAL)).unwrap();
    assert_eq!(payload(&signed.compact)["ver"], 2);
    assert_eq!(payload(&signed.compact)["region"], "US");
    assert!(verify_device_attestation(&signed, &ring.verification_keys(), REGIONAL, NOW).is_err());
    assert_eq!(
        verify_device_attestation_with_policy(
            &signed,
            &ring.verification_keys(),
            REGIONAL,
            NOW,
            Some(&policy)
        )
        .unwrap()
        .claims()
        .version,
        2
    );
}

#[test]
fn legacy_evidence_keeps_its_expiry_and_receipt_inspection_cannot_become_admission() {
    let old = DeviceAttestationKeyRing::new(
        "old-attestation".into(),
        [("old-attestation".into(), SigningKey::from_bytes(&[3; 32]))],
    )
    .unwrap();
    let signed = old.sign(&claims(LEGACY)).unwrap();
    let original = signed.compact.clone();
    let policy = policy(CredentialIssuance::Regional);
    let keys = old.verification_keys();
    assert!(
        verify_device_attestation_with_policy(&signed, &keys, REGIONAL, NOW, Some(&policy)).is_ok()
    );
    let expired = NOW + DEVICE_ATTESTATION_MAX_MILLIS;
    assert!(
        verify_device_attestation_with_policy(&signed, &keys, REGIONAL, expired, Some(&policy))
            .is_err()
    );
    assert!(
        inspect_device_attestation_with_policy(&signed, &keys, REGIONAL, expired, Some(&policy))
            .is_ok()
    );
    assert!(
        inspect_device_attestation_with_policy(
            &signed,
            &keys,
            REGIONAL,
            NOW + REFRESH_ABSOLUTE_MILLIS - 1,
            Some(&policy)
        )
        .is_ok()
    );
    assert!(
        inspect_device_attestation_with_policy(
            &signed,
            &keys,
            REGIONAL,
            NOW + REFRESH_ABSOLUTE_MILLIS,
            Some(&policy)
        )
        .is_err()
    );
    assert_eq!(signed.compact, original);
    let mut future = claims(LEGACY);
    future.issued_at_millis += 1_000;
    future.expires_at_millis += 1_000;
    assert!(
        inspect_device_attestation_with_policy(
            &old.sign(&future).unwrap(),
            &keys,
            REGIONAL,
            NOW + 1_000,
            Some(&policy)
        )
        .is_err()
    );
}

#[test]
fn signature_valid_mixed_shapes_and_cross_region_claims_are_rejected() {
    let key = SigningKey::from_bytes(&[3; 32]);
    let policy = policy(CredentialIssuance::Regional);
    let ring = DeviceAttestationKeyRing::new(
        "new-attestation".into(),
        [("new-attestation".into(), key.clone())],
    )
    .unwrap()
    .with_regional_policy(policy.clone())
    .unwrap();
    let signed = ring.sign(&claims(REGIONAL)).unwrap();
    for (field, value) in [
        ("ver", serde_json::json!(1)),
        ("region", serde_json::json!("EU")),
        ("region", serde_json::json!("us")),
        ("region", serde_json::Value::Null),
        ("iss", serde_json::json!(LEGACY)),
        ("aud", serde_json::json!("koru-native")),
    ] {
        let mut wire = payload(&signed.compact);
        wire[field] = value;
        let changed = SignedDeviceAttestationV1 {
            compact: rewrite(&signed.compact, wire, &key),
        };
        assert!(
            verify_device_attestation_with_policy(
                &changed,
                &ring.verification_keys(),
                REGIONAL,
                NOW,
                Some(&policy)
            )
            .is_err(),
            "{field}"
        );
    }
    let mut wire = payload(&signed.compact);
    wire.as_object_mut().unwrap().remove("region");
    assert!(
        verify_device_attestation_with_policy(
            &SignedDeviceAttestationV1 {
                compact: rewrite(&signed.compact, wire, &key)
            },
            &ring.verification_keys(),
            REGIONAL,
            NOW,
            Some(&policy)
        )
        .is_err()
    );
}

#[test]
fn public_overlap_keys_cannot_be_selected_to_sign_and_legacy_signing_stops_at_cutover() {
    let old = SigningKey::from_bytes(&[1; 32]);
    let ring = Ed25519KeyRing::new(
        "new-access".into(),
        [("new-access".into(), SigningKey::from_bytes(&[2; 32]))],
    )
    .unwrap()
    .with_verification_keys([("old-access".into(), old.verifying_key())])
    .unwrap();
    assert_eq!(ring.verification_keys().len(), 2);
    assert!(!ring.keys.contains_key("old-access"));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&ring.public_jwks().unwrap()).unwrap()["keys"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let bridge = Ed25519KeyRing::new("old-access".into(), [("old-access".into(), old)])
        .unwrap()
        .with_regional_policy(policy(CredentialIssuance::Legacy))
        .unwrap();
    assert!(bridge.sign_access_token(&access(LEGACY)).is_ok());
    let mut late = access(LEGACY);
    late.issued_at_millis += 1_000;
    late.expires_at_millis += 1_000;
    assert!(bridge.sign_access_token(&late).is_err());
}

#[test]
fn opposite_purpose_legacy_keys_cannot_issue_or_admit_regional_credentials() {
    let policy = policy(CredentialIssuance::Regional);
    let key = SigningKey::from_bytes(&[5; 32]);
    assert!(
        Ed25519KeyRing::new(
            "old-attestation".into(),
            [("old-attestation".into(), key.clone())]
        )
        .unwrap()
        .with_regional_policy(policy.clone())
        .is_err()
    );
    assert!(
        DeviceAttestationKeyRing::new("old-access".into(), [("old-access".into(), key.clone())])
            .unwrap()
            .with_regional_policy(policy.clone())
            .is_err()
    );
    // Deliberately contaminate the verification map and sign the regional wire
    // with an old key of the other purpose. Signature validity grants nothing.
    let ring = Ed25519KeyRing::new(
        "old-attestation".into(),
        [("old-attestation".into(), key.clone())],
    )
    .unwrap();
    let token = ring.sign_access_token(&access(REGIONAL)).unwrap();
    let compact = std::str::from_utf8(token.compact.expose_secret()).unwrap();
    let mut wire = payload(compact);
    wire["ver"] = serde_json::json!(2);
    wire["region"] = serde_json::json!("US");
    assert!(
        verify_access_token_with_policy(
            rewrite(compact, wire, &key).as_bytes(),
            &ring.verification_keys(),
            REGIONAL,
            "koru-native",
            NOW,
            Some(&policy)
        )
        .is_err()
    );
}

#[test]
fn legacy_null_fields_and_access_expiry_do_not_get_bridge_exceptions() {
    let key = SigningKey::from_bytes(&[1; 32]);
    let ring =
        Ed25519KeyRing::new("old-access".into(), [("old-access".into(), key.clone())]).unwrap();
    let token = ring.sign_access_token(&access(LEGACY)).unwrap();
    let policy = policy(CredentialIssuance::Regional);
    assert!(
        verify_access_token_with_policy(
            token.compact.expose_secret(),
            &ring.verification_keys(),
            REGIONAL,
            "koru-native",
            NOW + ACCESS_TOKEN_MILLIS - 1,
            Some(&policy)
        )
        .is_ok()
    );
    assert!(
        verify_access_token_with_policy(
            token.compact.expose_secret(),
            &ring.verification_keys(),
            REGIONAL,
            "koru-native",
            NOW + ACCESS_TOKEN_MILLIS,
            Some(&policy)
        )
        .is_err()
    );
    let compact = std::str::from_utf8(token.compact.expose_secret()).unwrap();
    for field in ["region", "ver"] {
        let mut wire = payload(compact);
        wire[field] = serde_json::Value::Null;
        assert!(
            verify_access_token_with_policy(
                rewrite(compact, wire, &key).as_bytes(),
                &ring.verification_keys(),
                REGIONAL,
                "koru-native",
                NOW,
                Some(&policy)
            )
            .is_err()
        );
    }
}

#[test]
fn issuer_native_adapter_preserves_ordinary_policy_and_has_exact_authenticated_denials() {
    use crate::TokenCapabilityVerificationError as Error;
    let key = SigningKey::from_bytes(&[2; 32]);
    let policy = policy(CredentialIssuance::Regional);
    let ring = Ed25519KeyRing::new("new-access".into(), [("new-access".into(), key.clone())])
        .unwrap()
        .with_regional_policy(policy.clone())
        .unwrap();
    let token = ring.sign_access_token(&access(REGIONAL)).unwrap();
    let compact = std::str::from_utf8(token.compact.expose_secret()).unwrap();
    let keys = ring.verification_keys();
    let verify = |bytes: &[u8], now| {
        verify_access_token_for_sync_issuance(
            bytes,
            &keys,
            REGIONAL,
            "koru-native",
            now,
            Some(&policy),
        )
    };
    assert_eq!(
        verify(token.compact.expose_secret(), NOW).unwrap(),
        access(REGIONAL)
    );
    for (field, value, error) in [
        ("aud", serde_json::json!("koru-sync"), Error::WrongAudience),
        ("region", serde_json::json!("EU"), Error::WrongRegion),
        (
            "iss",
            serde_json::json!("https://foreign.invalid"),
            Error::WrongIssuer,
        ),
        ("sub", serde_json::json!("bad-id"), Error::InvalidClaims),
        ("extra", serde_json::json!(true), Error::Malformed),
    ] {
        let mut wire = payload(compact);
        wire[field] = value;
        let signed = rewrite(compact, wire, &key);
        assert_eq!(verify(signed.as_bytes(), NOW), Err(error), "{field}");
        let ordinary = verify_access_token_with_policy(
            signed.as_bytes(),
            &keys,
            REGIONAL,
            "koru-native",
            NOW,
            Some(&policy),
        );
        assert_eq!(
            ordinary,
            Err(if error == Error::Malformed {
                JwtVerificationError::Malformed
            } else {
                JwtVerificationError::Claims
            })
        );
    }
    // EU policy was already invalid before this producer; do not manufacture
    // an ordinary EU success or describe the issuer check as an EU bug fix.
    let mut eu = policy.clone();
    eu.region = Region::Eu;
    assert_eq!(eu.validate(), Err(CryptoError::InvalidInput));
    assert_eq!(
        verify(token.compact.expose_secret(), NOW + ACCESS_TOKEN_MILLIS),
        Err(Error::CredentialExpired)
    );
    assert_eq!(
        verify(token.compact.expose_secret(), NOW - 1),
        Err(Error::CredentialNotYetValid)
    );
    assert_eq!(verify(b"not-a-credential", NOW), Err(Error::Malformed));
    let foreign = Ed25519KeyRing::new(
        "foreign".into(),
        [("foreign".into(), SigningKey::from_bytes(&[4; 32]))],
    )
    .unwrap();
    assert_eq!(
        verify(
            foreign
                .sign_access_token(&access(REGIONAL))
                .unwrap()
                .compact
                .expose_secret(),
            NOW
        ),
        Err(Error::UnknownKey)
    );
    // Known kid, wrong signing material: never inspect its audience as authority.
    let impostor = rewrite(compact, payload(compact), &SigningKey::from_bytes(&[4; 32]));
    assert_eq!(verify(impostor.as_bytes(), NOW), Err(Error::Signature));
    let attestation = DeviceAttestationKeyRing::new(
        "attestation".into(),
        [("attestation".into(), SigningKey::from_bytes(&[6; 32]))],
    )
    .unwrap();
    assert_eq!(
        verify(
            attestation
                .sign(&claims(REGIONAL))
                .unwrap()
                .compact
                .as_bytes(),
            NOW
        ),
        Err(Error::WrongPurpose)
    );
}

#[test]
fn issuer_native_adapter_keeps_the_existing_finite_legacy_bridge() {
    use crate::TokenCapabilityVerificationError as Error;
    let ring = Ed25519KeyRing::new(
        "old-access".into(),
        [("old-access".into(), SigningKey::from_bytes(&[1; 32]))],
    )
    .unwrap();
    let token = ring.sign_access_token(&access(LEGACY)).unwrap();
    let policy = policy(CredentialIssuance::Regional);
    let keys = ring.verification_keys();
    assert!(
        verify_access_token_for_sync_issuance(
            token.compact.expose_secret(),
            &keys,
            REGIONAL,
            "koru-native",
            NOW,
            Some(&policy)
        )
        .is_ok()
    );
    assert_eq!(
        verify_access_token_for_sync_issuance(
            token.compact.expose_secret(),
            &keys,
            REGIONAL,
            "koru-native",
            NOW + ACCESS_TOKEN_MILLIS,
            Some(&policy)
        ),
        Err(Error::WrongIssuer)
    );
}
