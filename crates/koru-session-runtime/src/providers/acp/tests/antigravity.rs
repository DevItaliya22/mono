// SPDX-License-Identifier: MPL-2.0

//! Google's Antigravity ACP server 1.2.1, replayed from sanitized fixtures.
use super::*;

const INITIALIZE: &str = include_str!("fixtures/antigravity-1.2.1/initialize.json");
const NEW_SESSION: &str = include_str!("fixtures/antigravity-1.2.1/new-session.json");

fn antigravity_harness(initialize: Value) -> Harness {
    let mut h = harness(FakeAcpConfig::default(), None, None);
    h.control.config.route = AcpRoute::antigravity();
    h.control.config.executable_version = h.control.config.route.supported_versions[0].clone();
    let mut frames = h.injected.lock().unwrap();
    frames.push_back(initialize);
    frames.push_back(serde_json::from_str(NEW_SESSION).unwrap());
    drop(frames);
    h
}

fn fixture_initialize() -> Value {
    serde_json::from_str(INITIALIZE).unwrap()
}

#[test]
fn a_chat_session_starts_with_its_bridge_and_no_authenticate_call() {
    let mut h = antigravity_harness(fixture_initialize());
    h.control.config.mcp_servers = vec![crate::providers::McpServerLaunch {
        name: "koru".into(),
        command: "koru-mcp".into(),
        args: vec!["serve".into(), "--stdio".into()],
        env: Vec::new(),
    }];
    let caps = start(&mut h.control);
    assert_eq!(caps.provider.as_str(), "antigravity");
    assert_eq!(caps.executable_version.as_str(), "1.2.1");
    assert!(caps.capabilities.supports(capability::PROVIDER_MODELS));
    // Uncontrolled children: never delegatable, which a chat does not need.
    assert_eq!(
        caps.delegation.child_sessions,
        ChildSessionPolicy::Uncontrolled
    );
    let models: Vec<_> = h
        .control
        .model_options()
        .iter()
        .map(|model| (model.id.as_str(), model.group.as_str()))
        .collect();
    assert_eq!(
        models,
        [
            ("model-a", "antigravity"),
            ("model-b", "antigravity"),
            ("model-c", "antigravity")
        ]
    );
    let state = h.state.lock().unwrap();
    // Its OAuth methods open a browser; the server's own settings decide.
    assert!(
        !state
            .sent()
            .iter()
            .any(|frame| frame["method"] == "authenticate")
    );
    let session = state
        .sent()
        .iter()
        .find(|frame| frame["method"] == "session/new")
        .unwrap();
    let bridge = &session["params"]["mcpServers"][0];
    assert_eq!(bridge["name"], "koru");
    assert_eq!(bridge["command"], "koru-mcp");
    assert_eq!(bridge["args"], json!(["serve", "--stdio"]));
    assert!(
        bridge.get("type").is_none(),
        "stdio is ACP's untagged shape"
    );
    assert!(!format!("{caps:?}").contains("private-session"));
}

#[test]
fn the_handshake_must_name_the_server_and_the_version_its_digest_pins() {
    let mut wrong_version = fixture_initialize();
    wrong_version["result"]["agentInfo"]["version"] = json!("1.2.2");
    let mut wrong_name = fixture_initialize();
    wrong_name["result"]["agentInfo"]["name"] = json!("antigravity-acp-wrapper");
    let mut missing = fixture_initialize();
    missing["result"]
        .as_object_mut()
        .unwrap()
        .remove("agentInfo");
    for initialize in [wrong_version, wrong_name, missing] {
        let mut h = antigravity_harness(initialize);
        let error = h
            .control
            .start(AgentStart::fresh(SessionId::mint(), None))
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::ProviderIncompatible);
        assert_eq!(error.diagnostic, ProviderDiagnostic::VersionRejected);
        assert!(
            !h.state
                .lock()
                .unwrap()
                .sent()
                .iter()
                .any(|frame| frame["method"] == "session/new")
        );
    }
}

#[test]
fn shutdown_ends_the_tree_instead_of_waiting_for_an_eof_exit_that_never_comes() {
    let mut h = antigravity_harness(fixture_initialize());
    start(&mut h.control);
    h.control.shutdown().unwrap();
    {
        let state = h.state.lock().unwrap();
        assert_eq!((state.terminations, state.shutdowns), (1, 0));
    }
    // Every other route keeps its orderly EOF shutdown.
    let mut other = harness(FakeAcpConfig::default(), None, None);
    start(&mut other.control);
    other.control.shutdown().unwrap();
    let state = other.state.lock().unwrap();
    assert_eq!((state.terminations, state.shutdowns), (0, 1));
}

#[test]
fn a_caller_budget_never_undercuts_the_servers_measured_start() {
    let version = ProviderVersion::from_str("1.2.1").unwrap();
    let config = AcpConfig::new(
        AcpRoute::antigravity(),
        "agy_acp_server",
        std::env::current_dir().unwrap(),
        version.clone(),
        Arc::new(NoopAttachmentPathResolver),
    )
    .unwrap();
    assert_eq!(config.request_timeout, Duration::from_secs(20));
    let limited = config.with_limits(ProbeLimits::default()).unwrap();
    assert_eq!(limited.request_timeout, Duration::from_secs(20));
    // The readiness budget still applies unchanged to the other routes.
    let other = super::config().with_limits(ProbeLimits::default()).unwrap();
    assert_eq!(other.request_timeout, ProbeLimits::default().timeout);
}

#[test]
fn only_antigravity_is_identified_by_digest_and_ends_by_termination() {
    for route in [
        AcpRoute::claude_code(),
        AcpRoute::opencode(),
        AcpRoute::kimi(),
        AcpRoute::grok(),
    ] {
        assert!(!route.dialect.identified_by_digest());
        assert!(route.dialect.exits_on_eof());
        assert!(!route.dialect.unpacks_into_temp());
    }
    let route = AcpRoute::antigravity();
    assert!(route.dialect.identified_by_digest());
    assert!(!route.dialect.exits_on_eof());
    assert!(route.dialect.unpacks_into_temp());
    assert!(route.cached_auth_method.is_none());
    assert!(route.arguments.is_empty());
}

#[test]
fn the_digest_is_the_lowercase_sha256_a_person_can_check() {
    assert_eq!(
        super::super::antigravity::sha256_hex(std::io::Cursor::new(b"abc")).unwrap(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
