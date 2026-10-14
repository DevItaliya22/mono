// SPDX-License-Identifier: MPL-2.0

use super::*;
use serde_json::json;

#[test]
fn kimi_authentication_is_a_code_not_a_message_or_method_advertisement() {
    let mut fixture: Value =
        serde_json::from_str(include_str!("fixtures/kimi-auth-required.json")).unwrap();
    let failure = acp_probe_failure(&fixture["error"], false);
    assert_eq!(
        failure.diagnostic,
        ProviderDiagnostic::AuthenticationRequired
    );
    assert_eq!(failure.stage, ProviderStage::Initialize);
    fixture["error"]["message"] = json!("UNTRUSTED_SENTINEL");
    assert_eq!(acp_probe_failure(&fixture["error"], false), failure);
    fixture["error"]["code"] = json!(-32099);
    assert_eq!(
        acp_probe_failure(&fixture["error"], false).diagnostic,
        ProviderDiagnostic::HandshakeRejected
    );
    assert_eq!(
        acp_probe_failure(&json!({"authMethods": [{"id": "login"}]}), false).diagnostic,
        ProviderDiagnostic::HandshakeRejected
    );
}

#[test]
fn wrapper_runtime_error_requires_the_exact_wrapper_rpc_context() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/claude-runtime-missing.json")).unwrap();
    assert_eq!(
        acp_probe_failure(&fixture["error"], true).diagnostic,
        ProviderDiagnostic::RuntimeUnavailable
    );
    assert_eq!(
        acp_probe_failure(&fixture["error"], false).diagnostic,
        ProviderDiagnostic::HandshakeRejected
    );
    for pointer in ["/error/code", "/error/message", "/error/data/details"] {
        let mut changed = fixture.clone();
        *changed.pointer_mut(pointer).unwrap() = json!("UNTRUSTED_SENTINEL");
        assert_eq!(
            acp_probe_failure(&changed["error"], true).diagnostic,
            ProviderDiagnostic::HandshakeRejected
        );
    }
    let mut changed = fixture.clone();
    changed["error"]["data"]["details"] = json!(format!(
        "prefix {}",
        fixture["error"]["data"]["details"].as_str().unwrap()
    ));
    assert_eq!(
        acp_probe_failure(&changed["error"], true).diagnostic,
        ProviderDiagnostic::HandshakeRejected
    );
}

#[test]
fn normalized_failures_discard_all_provider_content() {
    for code in [-32000, -32603, -32099] {
        let error = json!({
            "code": code,
            "message": "UNTRUSTED_SENTINEL",
            "data": {"details": "UNTRUSTED_SENTINEL", "token": "UNTRUSTED_SENTINEL"}
        });
        let failure = acp_probe_failure(&error, true);
        assert!(!format!("{failure:?} {failure}").contains("UNTRUSTED_SENTINEL"));
    }
}

#[test]
fn unusable_override_requires_complete_wrapper_error_context_and_discards_path() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/claude-override-missing.json")).unwrap();
    let error = &fixture["error"];
    let normalized = acp_probe_failure(error, true);
    assert_eq!(
        normalized.diagnostic,
        ProviderDiagnostic::RuntimeUnavailable
    );
    assert!(!format!("{normalized:?} {normalized}").contains("redacted-path"));
    assert_eq!(
        acp_probe_failure(error, false).diagnostic,
        ProviderDiagnostic::HandshakeRejected
    );
    for pointer in ["/code", "/message", "/data/details"] {
        let mut changed = error.clone();
        *changed.pointer_mut(pointer).unwrap() = json!("OTHER");
        assert_eq!(
            acp_probe_failure(&changed, true).diagnostic,
            ProviderDiagnostic::HandshakeRejected
        );
    }
    let mut changed = error.clone();
    changed["data"]["details"] = json!(format!(
        "prefix {}",
        error["data"]["details"].as_str().unwrap()
    ));
    assert_eq!(
        acp_probe_failure(&changed, true).diagnostic,
        ProviderDiagnostic::HandshakeRejected
    );
}
