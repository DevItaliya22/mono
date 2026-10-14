// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::{TokenCapabilityKeyRing, TokenCapabilityVerificationError, TokenCapabilityVerifier};
use ed25519_dalek::SigningKey;
use serde_json::{Value, json};

fn foundation() -> Value {
    serde_json::from_str(include_str!(
        "../../../../../contracts/vectors/cloud-sync-admission-v1/foundation.json"
    ))
    .unwrap()
}
fn vectors() -> Value {
    serde_json::from_str(include_str!(
        "../../../../../contracts/vectors/cloud-sync-issuance-v1/issuance.json"
    ))
    .unwrap()
}
fn keys() -> TokenCapabilityKeyRing {
    TokenCapabilityKeyRing::new(
        Region::Us,
        "regional".into(),
        [("regional".into(), SigningKey::from_bytes(&[1; 32]))],
    )
    .unwrap()
}
fn claims() -> SyncCredentialClaimsV1 {
    serde_json::from_value(foundation()["credential_claims"].clone()).unwrap()
}
fn number(value: &Value) -> i64 {
    value
        .as_i64()
        .unwrap_or_else(|| value.as_str().unwrap().parse().unwrap())
}
fn boundary(value: &Value) -> EntitlementBoundary {
    if value.is_null() {
        EntitlementBoundary::Unbounded
    } else {
        EntitlementBoundary::UntilSeconds(number(value))
    }
}

fn assert_invalid(result: Result<SignedSyncCredential, CryptoError>) {
    assert!(matches!(result, Err(CryptoError::InvalidInput)));
}

#[test]
fn finite_entitlement_boundary_reaches_real_signed_expiry_and_verifier_equality() {
    let keys = keys();
    let mut claims = claims();
    let cutoff = claims.iat + 73;
    claims.exp =
        sync_credential_expiry(claims.iat, EntitlementBoundary::UntilSeconds(cutoff)).unwrap();
    let signed = keys
        .sign_sync_credential("https://us.example.invalid", &claims)
        .unwrap();
    let verifier = keys.verifier("https://us.example.invalid".into()).unwrap();
    let verified =
        super::super::verify_credential(&verifier, signed.compact(), cutoff * 1000 - 1).unwrap();
    assert_eq!(signed.expires_at_seconds(), cutoff);
    assert_eq!(verified.claims().exp, cutoff);
    assert_eq!(
        verified.verify_time_at(cutoff * 1000).unwrap_err(),
        TokenCapabilityVerificationError::CredentialExpired
    );
    assert_eq!(
        super::super::verify_credential(&verifier, signed.compact(), cutoff * 1000).unwrap_err(),
        TokenCapabilityVerificationError::CredentialExpired
    );
    assert_eq!(
        sync_credential_expiry(claims.iat, EntitlementBoundary::UntilSeconds(claims.iat)),
        Err(SyncExpiryError::NoRemainingLifetime)
    );
}

#[test]
fn key_rotation_overlap_accepts_both_but_retirement_refuses_live_previous_token() {
    let old = TokenCapabilityKeyRing::new(
        Region::Us,
        "previous".into(),
        [("previous".into(), SigningKey::from_bytes(&[2; 32]))],
    )
    .unwrap();
    let current = TokenCapabilityKeyRing::new(
        Region::Us,
        "regional".into(),
        [
            ("regional".into(), SigningKey::from_bytes(&[1; 32])),
            ("previous".into(), SigningKey::from_bytes(&[2; 32])),
        ],
    )
    .unwrap();
    let claims = claims();
    let before = old
        .sign_sync_credential("https://us.example.invalid", &claims)
        .unwrap();
    let after = current
        .sign_sync_credential("https://us.example.invalid", &claims)
        .unwrap();
    let overlap = current
        .verifier("https://us.example.invalid".into())
        .unwrap();
    let now = claims.iat * 1000 + 1;
    assert!(now < claims.exp * 1000);
    super::super::verify_credential(&overlap, before.compact(), now).unwrap();
    super::super::verify_credential(&overlap, after.compact(), now).unwrap();
    assert_eq!(
        before.compact(),
        foundation()["previous_key_credential"]
            .as_str()
            .unwrap()
            .as_bytes()
    );
    let retired = keys()
        .verifier("https://us.example.invalid".into())
        .unwrap();
    assert_eq!(
        super::super::verify_credential(&retired, before.compact(), now).unwrap_err(),
        TokenCapabilityVerificationError::UnknownKey
    );
    super::super::verify_credential(&retired, after.compact(), now).unwrap();
}

#[test]
fn frozen_expiry_arithmetic_and_dormant_entitlement_boundary() {
    for case in vectors()["expiry_cases"].as_array().unwrap() {
        let duration = case["shorter_seconds"].as_i64().unwrap_or(600);
        let result = expiry_with_duration(
            number(&case["iat"]),
            duration,
            boundary(&case["entitlement_cutoff"]),
        );
        assert_eq!(
            result.ok(),
            (!case["exp"].is_null()).then(|| number(&case["exp"])),
            "{}",
            case["id"]
        );
    }
    let supplement: Value = serde_json::from_str(include_str!(
        "../../../../../contracts/vectors/cloud-sync-issuance-v1/lifecycle-boundaries-v1/cases.json"
    ))
    .unwrap();
    for case in supplement["entitlement_cases"].as_array().unwrap() {
        let result = sync_credential_expiry(number(&case["iat"]), boundary(&case["cutoff"]));
        assert_eq!(result.ok(), case["exp"].as_i64(), "{}", case["id"]);
        assert!(case["public_denial_code"].is_null());
    }
}

#[test]
fn real_signer_reproduces_existing_frozen_token_and_verifies_every_bound_claim() {
    let keys = keys();
    let claims = claims();
    let signed = keys
        .sign_sync_credential("https://us.example.invalid", &claims)
        .unwrap();
    assert_eq!(
        signed.compact(),
        foundation()["credential"].as_str().unwrap().as_bytes()
    );
    assert_eq!(signed.key_id(), "regional");
    assert_eq!(signed.expires_at_seconds(), claims.exp);
    let verifier = keys.verifier("https://us.example.invalid".into()).unwrap();
    let verified =
        super::super::verify_credential(&verifier, signed.compact(), claims.iat * 1000).unwrap();
    assert_eq!(
        serde_json::to_value(verified.claims()).unwrap(),
        foundation()["credential_claims"]
    );
    assert_eq!(format!("{signed:?}"), "SignedSyncCredential(<redacted>)");
    assert_eq!(
        super::super::verify_credential(&verifier, signed.compact(), claims.exp * 1000)
            .unwrap_err(),
        TokenCapabilityVerificationError::CredentialExpired
    );
    assert_eq!(
        super::super::verify_credential(&verifier, signed.compact(), claims.iat * 1000 - 1)
            .unwrap_err(),
        TokenCapabilityVerificationError::CredentialNotYetValid
    );
    for field in ["worker_id", "lease_id", "lease_epoch", "route"] {
        assert!(
            serde_json::to_value(verified.claims())
                .unwrap()
                .get(field)
                .is_none(),
            "I-C02 {field}"
        );
    }
}

#[test]
fn purpose_specific_signer_rejects_invalid_typed_claims_before_compact_signing() {
    let keys = keys();
    for (field, value) in [
        ("ver", json!(2)),
        ("purpose", json!("koru:sync:route:v1")),
        ("aud", json!("koru-sync-worker")),
        ("region", json!("EU")),
        ("iss", json!("https://other.example.invalid")),
        ("iat", json!(-1)),
        ("exp", json!(1800000000)),
        ("exp", json!(1800000601)),
    ] {
        let mut wire = foundation()["credential_claims"].clone();
        wire[field] = value;
        let candidate: SyncCredentialClaimsV1 = serde_json::from_value(wire).unwrap();
        assert_invalid(keys.sign_sync_credential("https://us.example.invalid", &candidate));
    }
    let mut bad = claims();
    bad.membership_epoch = -1;
    assert_invalid(keys.sign_sync_credential("https://us.example.invalid", &bad));
    bad = claims();
    bad.client_endpoint_id = serde_json::from_value(json!(vec![0u8; 32])).unwrap();
    assert_invalid(keys.sign_sync_credential("https://us.example.invalid", &bad));
    bad = claims();
    bad.iat = i64::MAX - 600;
    bad.exp = sync_credential_expiry(bad.iat, EntitlementBoundary::Unbounded).unwrap();
    assert_eq!(
        bad.exp,
        i64::MAX,
        "I-T14 is arithmetic, not an issuable millis timestamp"
    );
    assert_invalid(keys.sign_sync_credential("https://us.example.invalid", &bad));
    let eu = TokenCapabilityKeyRing::new(
        Region::Eu,
        "regional".into(),
        [("regional".into(), SigningKey::from_bytes(&[1; 32]))],
    )
    .unwrap();
    assert_invalid(eu.sign_sync_credential("https://us.example.invalid", &claims()));
}

#[test]
fn signed_evidence_has_key_and_purpose_separation_without_pooling_verifiers() {
    let keys = keys();
    let token = keys
        .sign_sync_credential("https://us.example.invalid", &claims())
        .unwrap();
    let foreign = TokenCapabilityVerifier::new(
        Region::Us,
        "https://us.example.invalid".into(),
        [(
            "access".into(),
            SigningKey::from_bytes(&[7; 32]).verifying_key(),
        )],
    )
    .unwrap();
    assert_eq!(
        super::super::verify_credential(&foreign, token.compact(), 1800000000000).unwrap_err(),
        TokenCapabilityVerificationError::UnknownKey
    );
    let same_id_foreign = TokenCapabilityVerifier::new(
        Region::Us,
        "https://us.example.invalid".into(),
        [(
            "regional".into(),
            SigningKey::from_bytes(&[7; 32]).verifying_key(),
        )],
    )
    .unwrap();
    assert_eq!(
        super::super::verify_credential(&same_id_foreign, token.compact(), 1800000000000)
            .unwrap_err(),
        TokenCapabilityVerificationError::Signature
    );
    assert!(
        crate::verify_access_token(
            token.compact(),
            &keys.verification_keys(),
            "https://us.example.invalid",
            "koru",
            1800000000000
        )
        .is_err()
    );
    let eu = TokenCapabilityVerifier::new(
        Region::Eu,
        "https://eu.example.invalid".into(),
        keys.verification_keys(),
    )
    .unwrap();
    assert_eq!(
        super::super::verify_credential(&eu, token.compact(), 1800000000000).unwrap_err(),
        TokenCapabilityVerificationError::WrongRegion
    );
}

#[test]
fn generic_verifier_ceiling_is_not_an_issuance_configuration() {
    let keys = keys();
    let verifier = keys.verifier("https://us.example.invalid".into()).unwrap();
    for case in vectors()["credential_lifetime_cases"].as_array().unwrap() {
        let mut claims = claims();
        claims.exp = claims.iat + case["seconds"].as_i64().unwrap();
        // Test-only raw encoder: production's purpose-specific signer refuses
        // durations beyond600. The verifier intentionally accepts up to900.
        let raw = crate::Ed25519KeyRing::new(
            "regional".into(),
            [("regional".into(), SigningKey::from_bytes(&[1; 32]))],
        )
        .unwrap()
        .sign_compact("koru-sync-credential+jwt", &claims)
        .unwrap();
        assert_eq!(
            super::super::verify_credential(
                &verifier,
                raw.compact.expose_secret(),
                claims.iat * 1000
            )
            .is_ok(),
            case["accepted"].as_bool().unwrap(),
            "{}",
            case["id"]
        );
        assert_eq!(
            keys.sign_sync_credential("https://us.example.invalid", &claims)
                .is_ok(),
            (1..=600).contains(&case["seconds"].as_i64().unwrap())
        );
    }
}

#[test]
fn newly_signed_epochs_are_retained_not_treated_as_live_authority() {
    let keys = keys();
    let mut claims = claims();
    claims.authorization_version += 10;
    claims.membership_epoch += 20;
    claims.cloud_access_epoch += 30;
    let token = keys
        .sign_sync_credential("https://us.example.invalid", &claims)
        .unwrap();
    let verifier = keys.verifier("https://us.example.invalid".into()).unwrap();
    let verified =
        super::super::verify_credential(&verifier, token.compact(), claims.iat * 1000).unwrap();
    assert_eq!(verified.claims().membership_epoch, claims.membership_epoch);
    assert_eq!(
        verified.claims().cloud_access_epoch,
        claims.cloud_access_epoch
    );
    assert_eq!(
        verified.claims().authorization_version,
        claims.authorization_version
    );
    // DS-D13/14 still need the hosted/current-state producer. Signature evidence
    // cannot decide which epoch is current and this test does not pretend it can.
}
