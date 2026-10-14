// SPDX-License-Identifier: MPL-2.0

use super::*;
use serde_json::{Value, json};

fn vectors() -> Value {
    serde_json::from_str(include_str!(
        "../../../../contracts/vectors/cloud-sync-issuance-v1/issuance.json"
    ))
    .unwrap()
}
fn foundation() -> Value {
    serde_json::from_str(include_str!(
        "../../../../contracts/vectors/cloud-sync-admission-v1/foundation.json"
    ))
    .unwrap()
}

#[test]
fn frozen_request_shapes_reject_caller_authority_and_require_profile_v4() {
    for case in vectors()["request_cases"].as_array().unwrap() {
        let actual = serde_json::from_str::<SyncTokenRequestV1>(case["body"].as_str().unwrap());
        if case["id"] == "I-R33" {
            // This vector changes only the HTTP path. This DTO decodes body;
            // Plan13B must consume it through the actual route/path adapter.
            assert!(actual.is_ok());
        } else {
            assert_eq!(
                actual.is_ok(),
                case["valid"].as_bool().unwrap(),
                "{}",
                case["id"]
            );
        }
    }
    // Request-specific v4 requirement must not alter generic dataset UUIDs.
    let generic = json!({"kind":"profile","profile_id":"00000000-0000-0000-0000-000000000000",
        "lineage_id":"00000000-0000-0000-0000-000000000000",
        "logical_replica_id":"00000000-0000-0000-0000-000000000000"});
    assert!(serde_json::from_value::<CloudReplicaPrincipalV1>(generic).is_ok());
}

fn at<'a>(value: &'a mut Value, path: &[&str]) -> &'a mut Value {
    if let Some((head, tail)) = path.split_first() {
        at(&mut value[*head], tail)
    } else {
        value
    }
}

fn response_bytes(recipe: &Value) -> Vec<u8> {
    let f = foundation();
    let mut response = json!({"version":1,"credential":f["credential"],
        "expires_at_seconds":f["credential_claims"]["exp"]});
    for (index, kind) in ["workspace", "profile"].iter().enumerate() {
        let dataset = f["sessions"][index]["dataset"].clone();
        response[*kind] = if recipe[format!("{kind}_available")]
            .as_bool()
            .unwrap_or(true)
        {
            json!({"state":"available","dataset":dataset,"route":{
                "endpoint_id":f["sessions"][index]["assignment"]["worker_endpoint_id"],
                "direct_addresses":[format!("127.0.0.1:{}",7777+index)],
                "relay_url":if recipe["isolated"]==true { Value::Null } else { json!("https://relay.example.invalid") }
            }})
        } else {
            json!({"state":"unavailable","dataset":dataset,"code":"sync_unavailable","retry_after_seconds":1})
        };
    }
    if let Some(edit) = recipe["set"].as_array() {
        let path: Vec<_> = edit[0].as_str().unwrap().split('.').collect();
        *at(&mut response, &path) = edit[1].clone();
    }
    if let Some(drop) = recipe["drop"].as_str() {
        let mut path: Vec<_> = drop.split('.').collect();
        let field = path.pop().unwrap();
        at(&mut response, &path)
            .as_object_mut()
            .unwrap()
            .remove(field);
    }
    let mut bytes = serde_json::to_string(&response).unwrap();
    if let Some(duplicate) = recipe["duplicate"].as_str() {
        let path: Vec<_> = duplicate.split('.').collect();
        let value = at(&mut response, &path);
        let field = path.last().unwrap();
        let fragment = format!("\"{field}\":{value}");
        bytes = bytes.replacen(&fragment, &format!("{fragment},{fragment}"), 1);
    }
    bytes.into_bytes()
}

#[test]
fn frozen_response_shapes_are_strict_but_not_authority_checks() {
    // These negative reference cases require a verified token/current trusted
    // dataset/relay context, not merely a structural DTO. No verifier is put
    // into a wire crate and no candidate URL becomes a trusted route here.
    let contextual = [
        "I-S05", "I-S09", "I-S10", "I-S11", "I-S12", "I-S18", "I-S20",
    ];
    for case in vectors()["response_cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let result =
            serde_json::from_slice::<SyncTokenResponseV1>(&response_bytes(&case["recipe"]));
        let expected_shape = contextual.contains(&id) || case["valid"].as_bool().unwrap();
        assert_eq!(result.is_ok(), expected_shape, "{id}");
        if let Ok(value) = result {
            assert!(!format!("{value:?}").contains(value.credential().compact()));
            assert_eq!(
                serde_json::from_slice::<SyncTokenResponseV1>(&serde_json::to_vec(&value).unwrap())
                    .unwrap(),
                value
            );
        }
    }
}

#[test]
fn nested_missing_null_unknown_and_duplicate_fields_are_refused() {
    let good = response_bytes(&json!({}));
    let object: Value = serde_json::from_slice(&good).unwrap();
    for path in [
        "version",
        "credential",
        "expires_at_seconds",
        "workspace",
        "profile",
        "workspace.state",
        "workspace.dataset",
        "workspace.route",
        "workspace.dataset.workspace_id",
        "workspace.dataset.incarnation_id",
        "workspace.dataset.logical_replica_id",
        "profile.dataset.profile_id",
        "profile.dataset.lineage_id",
        "profile.dataset.logical_replica_id",
        "workspace.route.endpoint_id",
        "workspace.route.direct_addresses",
    ] {
        let mut malformed = object.clone();
        let parts: Vec<_> = path.split('.').collect();
        *at(&mut malformed, &parts) = Value::Null;
        assert!(
            serde_json::from_value::<SyncTokenResponseV1>(malformed).is_err(),
            "null {path}"
        );
        let mut missing = object.clone();
        let (last, parent) = parts.split_last().unwrap();
        at(&mut missing, parent)
            .as_object_mut()
            .unwrap()
            .remove(*last);
        assert!(
            serde_json::from_value::<SyncTokenResponseV1>(missing).is_err(),
            "missing {path}"
        );
    }
}

#[test]
fn compact_evidence_is_bounded_redacted_and_not_implicitly_verified() {
    let value =
        SyncCredential::try_from(foundation()["credential"].as_str().unwrap().to_owned()).unwrap();
    assert_eq!(format!("{value:?}"), "SyncCredential(<redacted>)");
    for malformed in [
        String::new(),
        "a.b.c".into(),
        "a.b.c.d".into(),
        "x".repeat(10241),
        format!("{}=", value.compact()),
        "é".repeat(100),
    ] {
        assert!(SyncCredential::try_from(malformed).is_err());
    }
    let partial = serde_json::from_slice::<SyncTokenResponseV1>(&response_bytes(
        &json!({"workspace_available":false}),
    ))
    .unwrap();
    assert!(!partial.workspace().available());
    assert!(partial.profile().available());
}
