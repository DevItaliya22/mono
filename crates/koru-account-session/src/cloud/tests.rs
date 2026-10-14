// SPDX-License-Identifier: MPL-2.0

use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};

#[test]
fn shared_cloud_endpoint_check_preserves_admission_acceptance() {
    for byte in 0..=255u8 {
        let bytes = [byte; 32];
        let previous =
            ed25519_dalek::VerifyingKey::from_bytes(&bytes).is_ok_and(|key| !key.is_weak());
        let json = serde_json::to_vec(&bytes).unwrap();
        let mut decoder = serde_json::Deserializer::from_slice(&json);
        assert_eq!(endpoint(&mut decoder).is_ok(), previous, "key byte {byte}");
        // The peer constructor/decoder deliberately retains its former behavior.
        assert_eq!(
            serde_json::from_slice::<ReplicationEndpointId>(&json)
                .unwrap()
                .as_bytes(),
            &bytes
        );
    }
}

fn vectors() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../../../contracts/vectors/cloud-sync-admission-v1/foundation.json"
    ))
    .unwrap()
}

fn verifier(previous: bool) -> TokenCapabilityVerifier {
    let mut keys = vec![(
        "regional".into(),
        SigningKey::from_bytes(&[1; 32]).verifying_key(),
    )];
    if previous {
        keys.push((
            "previous".into(),
            SigningKey::from_bytes(&[2; 32]).verifying_key(),
        ));
    }
    TokenCapabilityVerifier::new(Region::Us, "https://us.example.invalid".into(), keys).unwrap()
}

fn signed(payload: &[u8], kind: &str) -> String {
    let header = serde_json::json!({"alg":"EdDSA","typ":kind,"kid":"regional"});
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap()),
        URL_SAFE_NO_PAD.encode(payload)
    );
    format!(
        "{}.{}",
        input,
        URL_SAFE_NO_PAD.encode(
            SigningKey::from_bytes(&[1; 32])
                .sign(input.as_bytes())
                .to_bytes()
        )
    )
}

#[test]
fn frozen_proofs_and_exact_refusals_ds_d01_d03_d05() {
    let v = vectors();
    let verifier = verifier(true);
    let token = v["credential"].as_str().unwrap().as_bytes();
    let claims = verify_credential(&verifier, token, 1_800_000_000_000).unwrap();
    assert_eq!(
        serde_json::to_value(claims.claims()).unwrap(),
        v["credential_claims"]
    );
    for case in v["negative_tokens"].as_array().unwrap() {
        assert_eq!(
            verify_credential(
                &verifier,
                case["token"].as_str().unwrap().as_bytes(),
                1_800_000_000_000
            )
            .unwrap_err()
            .code(),
            case["code"].as_str().unwrap()
        );
    }
    assert_eq!(
        verify_credential(&verifier, token, 1_800_000_600_000)
            .unwrap_err()
            .code(),
        "credential_expired"
    );
    assert_eq!(
        verify_credential(&verifier, token, 1_799_999_999_999)
            .unwrap_err()
            .code(),
        "credential_not_yet_valid"
    );
    assert_eq!(
        claims.verify_time_at(1_800_000_600_000).unwrap_err().code(),
        "credential_expired"
    );
}

#[test]
fn strict_shape_is_checked_after_signature_and_before_use() {
    let v = vectors();
    let verifier = verifier(false);
    // D231 left exactly one signed Cloud claim type. Its strict shape is still
    // checked only after the signature, and never before use.
    let payload = v["credential_claims"].clone();
    let check = |value: &[u8]| {
        let token = signed(value, "koru-sync-credential+jwt");
        let error = verify_credential(&verifier, token.as_bytes(), 1_800_000_000_000).unwrap_err();
        assert_eq!(error.code(), "credential_malformed");
    };
    for key in payload.as_object().unwrap().keys() {
        let mut bad = payload.clone();
        bad[key] = serde_json::Value::Null;
        check(&serde_json::to_vec(&bad).unwrap());
    }
    let mut bad = payload.clone();
    bad["unexpected"] = serde_json::json!(true);
    check(&serde_json::to_vec(&bad).unwrap());
    let raw = serde_json::to_string(&payload).unwrap();
    check(
        raw.replacen(
            "\"workspace_replica_id\":",
            "\"workspace_replica_id\":\"0c0c0c0c-0c0c-4c0c-8c0c-0c0c0c0c0c0c\",\"workspace_replica_id\":",
            1,
        )
        .as_bytes(),
    );
    assert_eq!(
        verify_credential(&verifier, &vec![b'a'; 10_241], 1_800_000_000_000).unwrap_err(),
        Error::Malformed
    );
    let malformed = signed(b"null", "koru-sync-credential+jwt");
    let mut bytes = malformed.into_bytes();
    let last = bytes.len() - 3;
    bytes[last] = if bytes[last] == b'A' { b'B' } else { b'A' };
    assert_eq!(
        verify_credential(&verifier, &bytes, 1_800_000_000_000).unwrap_err(),
        Error::Signature
    );
}

#[test]
fn rotation_and_combined_region_issuer_keep_shared_precedence() {
    let v = vectors();
    let previous = v["previous_key_credential"].as_str().unwrap().as_bytes();
    verify_credential(&verifier(true), previous, 1_800_000_000_000).unwrap();
    assert_eq!(
        verify_credential(&verifier(false), previous, 1_800_000_000_000)
            .unwrap_err()
            .code(),
        "credential_unknown_key"
    );
    let mut claims = v["credential_claims"].clone();
    claims["region"] = serde_json::json!("EU");
    claims["iss"] = serde_json::json!("https://eu.example.invalid");
    let token = signed(
        &serde_json::to_vec(&claims).unwrap(),
        "koru-sync-credential+jwt",
    );
    assert_eq!(
        verify_credential(&verifier(false), token.as_bytes(), 1_800_000_000_000)
            .unwrap_err()
            .code(),
        "wrong_region"
    );
    let token = signed(&serde_json::to_vec(&v["credential_claims"]).unwrap(), "JWT");
    assert_eq!(
        verify_credential(&verifier(false), token.as_bytes(), 1_800_000_000_000)
            .unwrap_err()
            .code(),
        "wrong_purpose"
    );
}

#[test]
fn matching_eu_trust_configuration_cannot_expand_us_only_cloud_scope() {
    let v = vectors();
    let verifier = TokenCapabilityVerifier::new(
        Region::Eu,
        "https://eu.example.invalid".into(),
        [(
            "regional".into(),
            SigningKey::from_bytes(&[1; 32]).verifying_key(),
        )],
    )
    .unwrap();
    let mut claims = v["credential_claims"].clone();
    claims["region"] = serde_json::json!("EU");
    claims["iss"] = serde_json::json!("https://eu.example.invalid");
    let token = signed(
        &serde_json::to_vec(&claims).unwrap(),
        "koru-sync-credential+jwt",
    );
    assert_eq!(
        verify_credential(&verifier, token.as_bytes(), 1_800_000_000_000).unwrap_err(),
        Error::WrongRegion
    );
}
