// SPDX-License-Identifier: MPL-2.0

use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
use koru_account_core::{AccessTokenClaims, ProfileId, TokenSigner};
use koru_device::{DeviceId, DevicePublicKey, ReplicationEndpointId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    CredentialIssuance, DEVICE_ATTESTATION_AUDIENCE, DEVICE_ATTESTATION_VERSION,
    DeviceAttestationClaimsV1, DeviceAttestationKeyRing, Ed25519KeyRing, RegionalCredentialPolicy,
    TokenCapabilityKeyRing,
};

const ISSUER: &str = "https://us.api.koru.do";
const NOW: i64 = 1_000_000;
const TYPE: &str = "test-capability+jwt";
const PURPOSE: &str = "test:capability";

// A test decoder only, not a proposed Plan 13 wire shape. These opaque bindings
// make loss of resource/epoch information through the verifier observable.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureClaims {
    ver: u32,
    purpose: String,
    iss: String,
    aud: String,
    region: Region,
    iat: i64,
    exp: i64,
    bindings: FixtureBindings,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureBindings {
    workspace: uuid::Uuid,
    replica: uuid::Uuid,
    incarnation: uuid::Uuid,
    endpoint: [u8; 32],
    membership_epoch: u64,
    cloud_access_epoch: u64,
}

impl TokenCapabilityClaims for FixtureClaims {
    fn verification_fields(&self) -> TokenCapabilityFields<'_> {
        TokenCapabilityFields {
            version: self.ver,
            purpose: &self.purpose,
            issuer: &self.iss,
            audience: &self.aud,
            region: self.region,
            issued_at_seconds: self.iat,
            expires_at_seconds: self.exp,
        }
    }
}

fn claims() -> FixtureClaims {
    FixtureClaims {
        ver: 1,
        purpose: PURPOSE.into(),
        iss: ISSUER.into(),
        aud: "koru-sync".into(),
        region: Region::Us,
        iat: NOW / 1_000,
        exp: NOW / 1_000 + 900,
        bindings: FixtureBindings {
            workspace: uuid::Uuid::from_u128(1),
            replica: uuid::Uuid::from_u128(2),
            incarnation: uuid::Uuid::from_u128(3),
            endpoint: [4; 32],
            membership_epoch: 5,
            cloud_access_epoch: 6,
        },
    }
}

fn requirements(audience: &str) -> TokenCapabilityRequirements {
    TokenCapabilityRequirements::new(TYPE.into(), 1, PURPOSE.into(), audience.into(), 900_000)
        .unwrap()
}

fn verifier() -> TokenCapabilityVerifier {
    TokenCapabilityKeyRing::new(
        Region::Us,
        "capability".into(),
        [("capability".into(), SigningKey::from_bytes(&[7; 32]))],
    )
    .unwrap()
    .verifier(ISSUER.into())
    .unwrap()
}

fn sign_raw(header: &str, payload: &str, key: &SigningKey) -> Vec<u8> {
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header),
        URL_SAFE_NO_PAD.encode(payload)
    );
    format!(
        "{input}.{}",
        URL_SAFE_NO_PAD.encode(key.sign(input.as_bytes()).to_bytes())
    )
    .into_bytes()
}

fn sign(payload: &Value) -> Vec<u8> {
    sign_raw(
        &json!({"alg":"EdDSA", "typ":TYPE, "kid":"capability"}).to_string(),
        &payload.to_string(),
        &SigningKey::from_bytes(&[7; 32]),
    )
}

fn refusal(token: &[u8], audience: &str, now: i64) -> &'static str {
    verifier()
        .verify::<FixtureClaims>(token, &requirements(audience), now)
        .unwrap_err()
        .code()
}

#[test]
fn complete_bindings_survive_and_verified_evidence_does_not_disclose_them_in_debug() {
    let expected = claims();
    let token = sign(&serde_json::to_value(&expected).unwrap());
    let verified = verifier()
        .verify::<FixtureClaims>(&token, &requirements("koru-sync"), NOW)
        .unwrap();
    assert_eq!(verified.claims(), &expected);
    assert_eq!(
        format!("{verified:?}"),
        "VerifiedTokenCapability(<verified-evidence>)"
    );
    // No lookup or authority object is involved: these epochs can be stale.
    // Consumers must compare all of them with their current resource authority.
    assert_eq!(verified.claims().bindings.membership_epoch, 5);
}

#[test]
fn ds_d01_d02_authenticated_audience_axis_has_exact_refusal_both_directions() {
    let mut payload = serde_json::to_value(claims()).unwrap();
    payload["aud"] = json!("koru-native");
    assert_eq!(refusal(&sign(&payload), "koru-sync", NOW), "wrong_audience");
    payload["aud"] = json!("koru-sync");
    assert_eq!(
        refusal(&sign(&payload), "koru-native", NOW),
        "wrong_audience"
    );
}

#[test]
fn ds_d03_region_is_explicit_and_checked_before_issuer() {
    let mut payload = serde_json::to_value(claims()).unwrap();
    payload["region"] = json!("EU");
    assert_eq!(refusal(&sign(&payload), "koru-sync", NOW), "wrong_region");
    payload["iss"] = json!("https://eu.api.koru.do");
    assert_eq!(refusal(&sign(&payload), "koru-sync", NOW), "wrong_region");
    payload["region"] = json!("US");
    assert_eq!(refusal(&sign(&payload), "koru-sync", NOW), "wrong_issuer");
}

#[test]
fn ds_d05_expiry_is_exact_at_admission_and_cached_evidence_revalidation() {
    let token = sign(&serde_json::to_value(claims()).unwrap());
    let verified = verifier()
        .verify::<FixtureClaims>(&token, &requirements("koru-sync"), NOW)
        .unwrap();
    assert_eq!(verified.verify_time_at(NOW + 899_999), Ok(()));
    assert_eq!(
        verified.verify_time_at(NOW + 900_000).unwrap_err().code(),
        "credential_expired"
    );
    assert_eq!(
        refusal(&token, "koru-sync", NOW + 900_000),
        "credential_expired"
    );
    assert_eq!(
        refusal(&token, "koru-sync", NOW - 1),
        "credential_not_yet_valid"
    );
    assert_eq!(
        verified.verify_time_at(NOW - 1).unwrap_err().code(),
        "credential_not_yet_valid"
    );
}

#[test]
fn capped_short_lifetime_is_allowed_but_overflow_and_invalid_intervals_are_not() {
    let mut payload = serde_json::to_value(claims()).unwrap();
    payload["exp"] = json!(NOW / 1_000 + 1);
    assert!(
        verifier()
            .verify::<FixtureClaims>(&sign(&payload), &requirements("koru-sync"), NOW)
            .is_ok()
    );
    for (iat, exp) in [
        (1_000, 1_000),
        (1_000, 999),
        (1_000, 1_901),
        (-1, 10),
        (i64::MAX, i64::MAX),
        (0, i64::MAX),
        (i64::MIN, 1_000),
    ] {
        payload["iat"] = json!(iat);
        payload["exp"] = json!(exp);
        assert_eq!(
            refusal(&sign(&payload), "koru-sync", NOW),
            "credential_claims_invalid"
        );
    }
}

#[test]
fn strict_claims_reject_unknown_missing_null_duplicate_and_bad_binding_fields() {
    let payload = serde_json::to_value(claims()).unwrap();
    for name in [
        "ver", "purpose", "iss", "aud", "region", "iat", "exp", "bindings",
    ] {
        let mut changed = payload.clone();
        changed.as_object_mut().unwrap().remove(name);
        assert_eq!(
            refusal(&sign(&changed), "koru-sync", NOW),
            "credential_malformed"
        );
        changed[name] = Value::Null;
        assert_eq!(
            refusal(&sign(&changed), "koru-sync", NOW),
            "credential_malformed"
        );
    }
    for (name, value) in [
        ("unknown", json!(true)),
        ("region", json!("us")),
        ("region", json!("XX")),
    ] {
        let mut changed = payload.clone();
        changed[name] = value;
        assert_eq!(
            refusal(&sign(&changed), "koru-sync", NOW),
            "credential_malformed"
        );
    }
    let mut changed = payload.clone();
    changed["bindings"]["workspace"] = json!("not-a-uuid");
    assert_eq!(
        refusal(&sign(&changed), "koru-sync", NOW),
        "credential_malformed"
    );
    let duplicate = payload
        .to_string()
        .replacen('{', "{\"aud\":\"koru-sync\",", 1);
    let token = sign_raw(
        &json!({"alg":"EdDSA", "typ":TYPE, "kid":"capability"}).to_string(),
        &duplicate,
        &SigningKey::from_bytes(&[7; 32]),
    );
    assert_eq!(refusal(&token, "koru-sync", NOW), "credential_malformed");
}

#[test]
fn signed_common_checks_have_documented_precedence_and_no_legacy_fallback() {
    let mut payload = serde_json::to_value(claims()).unwrap();
    for (name, value, expected) in [
        ("ver", json!(99), "credential_version_unsupported"),
        ("purpose", json!("enrollment"), "wrong_purpose"),
        ("iss", json!("https://api.koru.do"), "wrong_issuer"),
    ] {
        let mut changed = payload.clone();
        changed[name] = value;
        assert_eq!(refusal(&sign(&changed), "koru-sync", NOW), expected);
    }
    payload["ver"] = json!(99);
    payload["purpose"] = json!("enrollment");
    payload["aud"] = json!("koru-native");
    payload["region"] = json!("EU");
    assert_eq!(
        refusal(&sign(&payload), "koru-sync", NOW),
        "credential_version_unsupported"
    );
    payload["ver"] = json!(1);
    assert_eq!(refusal(&sign(&payload), "koru-sync", NOW), "wrong_purpose");
    payload["purpose"] = json!(PURPOSE);
    assert_eq!(refusal(&sign(&payload), "koru-sync", NOW), "wrong_audience");
}

#[test]
fn bounded_parser_refuses_header_confusion_unknown_keys_and_tampered_signatures() {
    let payload = serde_json::to_value(claims()).unwrap().to_string();
    for (header, expected) in [
        (
            json!({"alg":"none", "typ":TYPE, "kid":"capability"}),
            "credential_claims_invalid",
        ),
        (
            json!({"alg":"EdDSA", "typ":"JWT", "kid":"capability"}),
            "wrong_purpose",
        ),
        (
            json!({"alg":"EdDSA", "typ":TYPE, "kid":"unknown"}),
            "credential_unknown_key",
        ),
        (
            json!({"alg":"EdDSA", "typ":TYPE, "kid":"../capability"}),
            "credential_claims_invalid",
        ),
        (
            json!({"alg":"EdDSA", "typ":TYPE, "kid":"capability", "jku":"https://untrusted.invalid"}),
            "credential_malformed",
        ),
    ] {
        let token = sign_raw(
            &header.to_string(),
            &payload,
            &SigningKey::from_bytes(&[7; 32]),
        );
        assert_eq!(refusal(&token, "koru-sync", NOW), expected);
    }
    let header = json!({"alg":"EdDSA", "typ":TYPE, "kid":"capability"}).to_string();
    let token = sign_raw(&header, &payload, &SigningKey::from_bytes(&[8; 32]));
    assert_eq!(
        refusal(&token, "koru-sync", NOW),
        "credential_signature_invalid"
    );
    // Signature is checked before decoding even a malformed/unknown-version payload.
    let token = sign_raw(&header, "{", &SigningKey::from_bytes(&[8; 32]));
    assert_eq!(
        refusal(&token, "koru-sync", NOW),
        "credential_signature_invalid"
    );
    for token in [
        vec![255],
        b"a.b.c.d".to_vec(),
        b"...".to_vec(),
        vec![b'x'; 12 * 1024 + 1],
        sign_raw(
            &header,
            &"x".repeat(10 * 1024),
            &SigningKey::from_bytes(&[7; 32]),
        ),
    ] {
        assert_eq!(refusal(&token, "koru-sync", NOW), "credential_malformed");
    }
}

#[test]
fn verification_key_overlap_needs_no_private_key_and_retirement_refuses_old_key() {
    let old = SigningKey::from_bytes(&[7; 32]);
    let current = SigningKey::from_bytes(&[8; 32]);
    let verifier = TokenCapabilityVerifier::new(
        Region::Us,
        ISSUER.into(),
        [
            ("old".into(), old.verifying_key()),
            ("current".into(), current.verifying_key()),
        ],
    )
    .unwrap();
    let payload = serde_json::to_value(claims()).unwrap().to_string();
    for (kid, key) in [("old", &old), ("current", &current)] {
        let token = sign_raw(
            &json!({"alg":"EdDSA", "typ":TYPE, "kid":kid}).to_string(),
            &payload,
            key,
        );
        assert!(
            verifier
                .verify::<FixtureClaims>(&token, &requirements("koru-sync"), NOW)
                .is_ok()
        );
    }
    let retired = TokenCapabilityVerifier::new(
        Region::Us,
        ISSUER.into(),
        [("current".into(), current.verifying_key())],
    )
    .unwrap();
    let token = sign_raw(
        &json!({"alg":"EdDSA", "typ":TYPE, "kid":"old"}).to_string(),
        &payload,
        &old,
    );
    assert_eq!(
        retired
            .verify::<FixtureClaims>(&token, &requirements("koru-sync"), NOW)
            .unwrap_err()
            .code(),
        "credential_unknown_key"
    );
}

#[test]
fn real_regional_access_and_attestation_credentials_refuse_at_purpose_boundary() {
    let policy = RegionalCredentialPolicy {
        region: Region::Us,
        regional_issuer: ISSUER.into(),
        legacy_issuer: "https://api.koru.do".into(),
        legacy_cutover_millis: NOW,
        issuance: CredentialIssuance::Regional,
        legacy_access_kids: ["old-access".into()].into(),
        legacy_attestation_kids: ["old-attestation".into()].into(),
    };
    let access_ring = Ed25519KeyRing::new(
        "access".into(),
        [("access".into(), SigningKey::from_bytes(&[8; 32]))],
    )
    .unwrap()
    .with_regional_policy(policy.clone())
    .unwrap();
    let access = access_ring
        .sign_access_token(&AccessTokenClaims {
            issuer: ISSUER.into(),
            audience: "koru-native".into(),
            subject: "00000000-0000-4000-8000-000000000001".parse().unwrap(),
            session_id: "00000000-0000-4000-8000-000000000002".parse().unwrap(),
            token_id: "access".into(),
            issued_at_millis: NOW,
            expires_at_millis: NOW + 900_000,
        })
        .unwrap();
    assert_eq!(
        refusal(&access.compact.0, "koru-sync", NOW),
        "wrong_purpose"
    );
    // Matching only the header type still cannot borrow the access signing ring.
    let jwt_requirements = TokenCapabilityRequirements::new(
        "JWT".into(),
        1,
        PURPOSE.into(),
        "koru-sync".into(),
        900_000,
    )
    .unwrap();
    assert_eq!(
        verifier()
            .verify::<FixtureClaims>(&access.compact.0, &jwt_requirements, NOW)
            .unwrap_err()
            .code(),
        "credential_unknown_key"
    );
    let attestation_ring = DeviceAttestationKeyRing::new(
        "attestation".into(),
        [("attestation".into(), SigningKey::from_bytes(&[9; 32]))],
    )
    .unwrap()
    .with_regional_policy(policy)
    .unwrap();
    let attestation = attestation_ring
        .sign(&DeviceAttestationClaimsV1 {
            version: DEVICE_ATTESTATION_VERSION,
            issuer: ISSUER.into(),
            audience: DEVICE_ATTESTATION_AUDIENCE.into(),
            koru_identity_id: "00000000-0000-4000-8000-000000000001".parse().unwrap(),
            device_id: DeviceId::parse("00000000-0000-4000-8000-000000000003").unwrap(),
            signing_public_key: DevicePublicKey::from_bytes([10; 32]),
            endpoint_id: ReplicationEndpointId::from_bytes([11; 32]),
            authorized_profile_ids: vec![
                ProfileId::parse("00000000-0000-4000-8000-000000000004").unwrap(),
            ],
            session_id: "00000000-0000-4000-8000-000000000002".parse().unwrap(),
            token_id: "attestation".into(),
            issued_at_millis: NOW,
            expires_at_millis: NOW + 300_000,
        })
        .unwrap();
    assert_eq!(
        refusal(attestation.compact.as_bytes(), "koru-sync", NOW),
        "wrong_purpose"
    );
}

#[test]
fn trusted_configuration_cannot_be_empty_ambiguous_or_invalid() {
    let key = SigningKey::from_bytes(&[7; 32]).verifying_key();
    assert!(TokenCapabilityVerifier::new(Region::Us, ISSUER.into(), []).is_err());
    assert!(
        TokenCapabilityVerifier::new(
            Region::Us,
            ISSUER.into(),
            (0..17).map(|index| (format!("key-{index}"), key)),
        )
        .is_err()
    );
    assert!(
        TokenCapabilityVerifier::new(
            Region::Us,
            ISSUER.into(),
            [("same".into(), key), ("same".into(), key)]
        )
        .is_err()
    );
    assert!(
        TokenCapabilityVerifier::new(Region::Us, "bad issuer".into(), [("key".into(), key)])
            .is_err()
    );
    for (typ, version, purpose, audience, maximum) in [
        ("", 1, PURPOSE, "koru-sync", 900_000),
        (TYPE, 0, PURPOSE, "koru-sync", 900_000),
        (TYPE, 1, "", "koru-sync", 900_000),
        (TYPE, 1, PURPOSE, "", 900_000),
        (TYPE, 1, PURPOSE, "koru-sync", 0),
        (TYPE, 1, PURPOSE, "koru-sync", 900_001),
        (TYPE, 1, PURPOSE, "koru-sync", i64::MAX),
    ] {
        assert!(
            TokenCapabilityRequirements::new(
                typ.into(),
                version,
                purpose.into(),
                audience.into(),
                maximum
            )
            .is_err()
        );
    }
}
