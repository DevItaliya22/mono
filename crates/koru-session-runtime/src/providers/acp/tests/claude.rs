// SPDX-License-Identifier: MPL-2.0

use super::*;

fn wrapper(auth: Option<&str>, before_initialize: bool) -> Harness {
    let mut h = harness(FakeAcpConfig::default(), None, None);
    h.control.config.route = AcpRoute::claude_code();
    h.control.config.executable_version = ProviderVersion::from_str("0.76.0").unwrap();
    h.control.config.request_timeout = Duration::from_millis(10);
    let initialized = response(
        1,
        json!({
            "protocolVersion":1,
            "agentInfo":{"name":"<wrapper>","version":"0.76.0"},
            "agentCapabilities":{"_meta":{"authStatus":{}},"mcpCapabilities":{"http":true}}
        }),
    );
    let session = response(2, json!({"sessionId":"<session>"}));
    let auth = auth.map(|kind| json!({"jsonrpc":"2.0","method":"_auth/status_update","params":{
        "authStatus":{"kind":kind,"label":"PRIVATE_AUTH","account":{"email":"PRIVATE_AUTH","organization":"PRIVATE_AUTH"}}
    }}));
    let mut queue = h.injected.lock().unwrap();
    if before_initialize && let Some(frame) = &auth {
        queue.push_back(frame.clone());
    }
    queue.push_back(initialized);
    queue.push_back(session);
    if !before_initialize && let Some(frame) = auth {
        queue.push_back(frame);
    }
    drop(queue);
    h
}

#[test]
fn signed_in_evidence_before_or_after_session_new_is_required_and_private() {
    for kind in ["account", "api_key"] {
        for before in [false, true] {
            let mut h = wrapper(Some(kind), before);
            let caps = start(&mut h.control);
            assert!(caps.capabilities.supports(capability::MANAGED_EVENTS));
            assert_eq!(caps.delegation.mcp, McpSupport::Supported);
            assert_eq!(caps.delegation.child_sessions, ChildSessionPolicy::Disabled);
            let verdict = DelegationVerdict::evaluate(&caps.capabilities, &caps.delegation);
            assert!(verdict.delegatable());
            assert!(!format!("{caps:?} {:?}", h.control.diagnostics()).contains("PRIVATE_AUTH"));
        }
    }
}

#[test]
fn successful_session_new_with_none_is_signed_out_but_unknown_or_absent_evidence_is_failed() {
    for (kind, diagnostic) in [
        (Some("none"), ProviderDiagnostic::AuthenticationRequired),
        (Some("external"), ProviderDiagnostic::HandshakeRejected),
        (Some("gateway"), ProviderDiagnostic::HandshakeRejected),
        (Some("future"), ProviderDiagnostic::HandshakeRejected),
        (None, ProviderDiagnostic::Timeout),
    ] {
        let mut h = wrapper(kind, false);
        let error = h
            .control
            .start(AgentStart::fresh(SessionId::mint(), None))
            .unwrap_err();
        assert_eq!(error.stage, ProviderStage::Initialize);
        assert_eq!(error.diagnostic, diagnostic);
        assert_eq!(
            error.code,
            if kind == Some("none") {
                ErrorCode::ProviderUnavailable
            } else {
                ErrorCode::ProviderFailed
            },
        );
        assert!(!format!("{error:?} {error}").contains("PRIVATE_AUTH"));
        assert!(h.control.capabilities.is_none());
        assert!(h.control.submit(input(AgentTurnId::mint())).is_err());
    }
}

#[test]
fn wrapper_handshake_requires_exact_pin_and_marker_not_a_status_in_marker() {
    for (version, marker) in [("0.77.0", json!({})), ("0.76.0", Value::Null)] {
        let mut h = wrapper(Some("account"), true);
        for frame in h.injected.lock().unwrap().iter_mut() {
            if frame["id"] == 1 {
                frame["result"]["agentInfo"]["version"] = json!(version);
                frame["result"]["agentCapabilities"]["_meta"]["authStatus"] = marker.clone();
            }
        }
        assert!(
            h.control
                .start(AgentStart::fresh(SessionId::mint(), None))
                .is_err()
        );
    }
}

#[test]
fn child_exclusions_union_and_invalid_metadata_never_discards_restrictions() {
    let mut route = AcpRoute::claude_code();
    route.session_new_meta = Some(
        json!({"claudeCode":{"options":{"disallowedTools":["Bash","Agent"],"maxTurns":3}},"other":true}),
    );
    let cfg = AcpConfig::new(
        route,
        "never-started",
        std::env::current_dir().unwrap(),
        ProviderVersion::from_str("0.76.0").unwrap(),
        Arc::new(NoopAttachmentPathResolver),
    )
    .unwrap();
    let meta = cfg.route.session_new_meta.as_ref().unwrap();
    assert_eq!(
        meta.pointer("/claudeCode/options/disallowedTools").unwrap(),
        &json!(["Bash", "Agent", "Task"])
    );
    assert_eq!(meta.pointer("/claudeCode/options/maxTurns").unwrap(), 3);
    assert_eq!(meta["other"], true);
    for meta in [
        json!([]),
        json!({"claudeCode":false}),
        json!({"claudeCode":{"options":{"disallowedTools":false}}}),
    ] {
        let mut route = AcpRoute::claude_code();
        route.session_new_meta = Some(meta);
        assert!(
            AcpConfig::new(
                route,
                "never-started",
                std::env::current_dir().unwrap(),
                ProviderVersion::from_str("0.76.0").unwrap(),
                Arc::new(NoopAttachmentPathResolver)
            )
            .is_err()
        );
    }
}

#[test]
fn unqualified_children_refuse_delegation_before_transmitting_endpoint() {
    let mut h = wrapper(Some("account"), false);
    h.control.config.route.child_sessions = ChildSessionPolicy::Uncontrolled;
    h.control.config.delegation = Some(delegation());
    let error = h
        .control
        .start(AgentStart::fresh(SessionId::mint(), None))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::CapabilityNotNegotiated);
    assert!(h.state.lock().unwrap().sent().is_empty());
}

#[test]
fn recorded_auth_carriers_and_runtime_refusals_normalize_through_start() {
    for (fixture, expected) in [
        (
            include_str!("fixtures/claude-0.76.0/auth-account.json"),
            None,
        ),
        (
            include_str!("fixtures/claude-0.76.0/auth-none.json"),
            Some(ProviderDiagnostic::AuthenticationRequired),
        ),
        (
            include_str!("fixtures/claude-0.76.0/auth-future.json"),
            Some(ProviderDiagnostic::HandshakeRejected),
        ),
    ] {
        let mut h = wrapper(None, false);
        h.injected
            .lock()
            .unwrap()
            .push_back(serde_json::from_str(fixture).unwrap());
        let result = h.control.start(AgentStart::fresh(SessionId::mint(), None));
        match expected {
            None => {
                assert!(result.is_ok());
            }
            Some(diagnostic) => assert_eq!(result.unwrap_err().diagnostic, diagnostic),
        }
    }
    for fixture in [
        include_str!("../../acp_failure/fixtures/claude-runtime-missing.json"),
        include_str!("../../acp_failure/fixtures/claude-override-missing.json"),
    ] {
        let mut h = wrapper(None, false);
        let mut frame: Value = serde_json::from_str(fixture).unwrap();
        frame["id"] = json!(2);
        *h.injected.lock().unwrap().get_mut(1).unwrap() = frame;
        let error = h
            .control
            .start(AgentStart::fresh(SessionId::mint(), None))
            .unwrap_err();
        assert_eq!(error.diagnostic, ProviderDiagnostic::RuntimeUnavailable);
        assert!(!format!("{error:?}").contains("redacted-path"));
    }
}

#[test]
fn uncontrolled_native_children_preserve_managed_support_but_fail_cancel_verdict() {
    let mut h = wrapper(Some("account"), false);
    h.control.config.route.child_sessions = ChildSessionPolicy::Uncontrolled;
    let caps = start(&mut h.control);
    assert!(caps.capabilities.supports(capability::MANAGED_EVENTS));
    assert_eq!(caps.delegation.mcp, McpSupport::Supported);
    let verdict = DelegationVerdict::evaluate(&caps.capabilities, &caps.delegation);
    assert_eq!(verdict.missing(), vec![DelegationRequirement::Cancel]);
    for policy in [
        ChildSessionPolicy::Disabled,
        ChildSessionPolicy::CannotDisable,
        ChildSessionPolicy::NotOffered,
    ] {
        let mut facts = caps.delegation.clone();
        facts.child_sessions = policy;
        assert!(DelegationVerdict::evaluate(&caps.capabilities, &facts).delegatable());
    }
}

#[cfg(windows)]
#[test]
fn wrapper_windows_shim_uses_profile_launcher_and_refuses_authored_arguments() {
    let executable = std::ffi::OsString::from(r"C:\fixture\claude-agent-acp.cmd");
    let expected =
        koru_session_profiles::platform_launch_argv(std::path::PathBuf::from(&executable), &[])
            .unwrap();
    let config = AcpConfig::new(
        AcpRoute::claude_code(),
        executable.clone(),
        std::path::PathBuf::from(r"C:\fixture"),
        ProviderVersion::from_str("0.76.0").unwrap(),
        Arc::new(crate::NoopAttachmentPathResolver),
    )
    .unwrap();
    assert_eq!(config.executable, expected[0]);
    assert_eq!(config.route.arguments, expected[1..]);
    let mut route = AcpRoute::claude_code();
    route.arguments.push("authored & command".into());
    assert!(
        AcpConfig::new(
            route,
            executable,
            std::path::PathBuf::from(r"C:\fixture"),
            ProviderVersion::from_str("0.76.0").unwrap(),
            Arc::new(crate::NoopAttachmentPathResolver),
        )
        .is_err()
    );
}
