// SPDX-License-Identifier: MPL-2.0

//! The route surface, and the grant matrix in both directions.
//!
//! Every settings path here comes from a temp root: the authority is
//! constructed with `SettingsPaths::under_root`, so no test resolves a real
//! preferences or local-app-data location.

use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt as _;

use super::*;
use crate::{now_millis, router};

/// A gateway with a settings authority rooted in a temp directory, plus two
/// tokens: one with the ordinary default set, one that asked for the write
/// scope by name.
fn state() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    Arc<GatewayState>,
    String,
    String,
) {
    let store_root = tempfile::tempdir().unwrap();
    let settings_root = tempfile::tempdir().unwrap();
    let worker = koru_store_server::StoreWorker::new(koru_store_server::WorkerConfig::embedded(
        store_root.path().to_path_buf(),
        "internal-debug",
    ))
    .spawn()
    .unwrap();
    let state = GatewayState::new("internal-debug".into(), worker);
    state.set_settings_authority(koru_settings::SettingsAuthority::new(
        koru_settings::SettingsPaths::under_root(settings_root.path()),
    ));
    let (reader, _) = state.sessions.mint("test", &[], now_millis());
    let (writer, _) = state.sessions.mint(
        "first-party",
        &[SETTINGS_READ_SCOPE.into(), SETTINGS_WRITE_SCOPE.into()],
        now_millis(),
    );
    (store_root, settings_root, state, reader, writer)
}

async fn call(
    state: &Arc<GatewayState>,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"));
    let request = match body {
        Some(body) => request
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => request.body(Body::empty()).unwrap(),
    };
    let response = router(Arc::clone(state)).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, value)
}

#[test]
fn the_settings_scope_literals_agree() {
    // Spelled in two crates that do not link one another, exactly like
    // `CONTROL_SCOPE`. This is the line that keeps them one name.
    assert_eq!(SETTINGS_READ_SCOPE, koru_settings::SETTINGS_READ_SCOPE);
    assert_eq!(SETTINGS_WRITE_SCOPE, koru_settings::SETTINGS_WRITE_SCOPE);
}

#[test]
fn settings_fault_is_off_by_default_and_cannot_arm_for_a_released_build() {
    let fault = SettingsWriteFault::with_hold(std::time::Duration::ZERO);
    assert_eq!(fault.state(), FaultState::Off);
    assert_eq!(fault.configure(true, None), FaultConfiguration::Off);
    assert_eq!(
        fault.configure(false, Some("global-dictation-enable")),
        FaultConfiguration::RefusedReleasedBuild
    );
    assert_eq!(fault.state(), FaultState::Off);
    assert_eq!(
        fault.configure(true, Some("enable-everything")),
        FaultConfiguration::UnknownOperation
    );
    assert_eq!(fault.state(), FaultState::Off);
}

#[test]
fn settings_fault_is_named_and_one_shot() {
    let fault = SettingsWriteFault::with_hold(std::time::Duration::from_millis(1));
    assert_eq!(
        fault.configure(true, Some("global-dictation-enable")),
        FaultConfiguration::Armed
    );
    assert_eq!(
        fault.take(Some(FaultOperation::GlobalDictationDisable)),
        None
    );
    assert_eq!(
        fault.state(),
        FaultState::Armed(FaultOperation::GlobalDictationEnable)
    );
    assert!(
        fault
            .take(Some(FaultOperation::GlobalDictationEnable))
            .is_some()
    );
    assert_eq!(
        fault.state(),
        FaultState::Fired(FaultOperation::GlobalDictationEnable)
    );
    assert_eq!(
        fault.take(Some(FaultOperation::GlobalDictationEnable)),
        None
    );
}

#[test]
fn only_the_three_exact_agent_patches_have_fault_names() {
    assert_eq!(
        FaultOperation::from_patch(&GlobalPatch {
            global_dictation_enabled: Some(true),
            start_on_login: Some(true),
            ..Default::default()
        }),
        Some(FaultOperation::GlobalDictationEnable)
    );
    assert_eq!(
        FaultOperation::from_patch(&GlobalPatch {
            global_dictation_enabled: Some(false),
            ..Default::default()
        }),
        Some(FaultOperation::GlobalDictationDisable)
    );
    assert_eq!(
        FaultOperation::from_patch(&GlobalPatch {
            start_on_login: Some(false),
            ..Default::default()
        }),
        Some(FaultOperation::StartOnLoginDisable)
    );
    assert_eq!(
        FaultOperation::from_patch(&GlobalPatch {
            global_dictation_enabled: Some(true),
            automatic_update_checks: Some(false),
            ..Default::default()
        }),
        None,
        "an unrelated field must not spend the named one-shot fault"
    );
}

#[test]
fn the_write_scope_is_never_granted_by_default_and_the_read_scope_always_is() {
    // The owner's decision, as a test. A default token — which is what MCP,
    // plugins and scripts hold — reads and cannot write.
    let default = crate::effective_scopes(&[], crate::Admission::Ordinary);
    assert!(default.iter().any(|scope| scope == SETTINGS_READ_SCOPE));
    assert!(
        !default.iter().any(|scope| scope == SETTINGS_WRITE_SCOPE),
        "an MCP tool or a script must not be able to silently change what \
         every window looks like"
    );
    // And it is requestable, which is how a first-party window gets it — the
    // same mechanism `ai.manage_providers` uses.
    assert_eq!(
        crate::effective_scopes(&[SETTINGS_WRITE_SCOPE.into()], crate::Admission::Ordinary),
        vec![SETTINGS_WRITE_SCOPE.to_owned()]
    );
}

#[tokio::test]
async fn a_default_token_reads_every_section_and_writes_none() {
    let (_store, _settings, state, reader, _writer) = state();
    for section in ["global", "terminal"] {
        let (status, body) = call(
            &state,
            &reader,
            "GET",
            &format!("/v1/settings/{section}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{section}");
        assert_eq!(body["section"], section);
    }
    for (section, patch) in [
        ("global", serde_json::json!({"theme": "dark"})),
        ("terminal", serde_json::json!({"zoom": 1.5})),
        ("ai", serde_json::json!({"ollama_reasoning": "high"})),
    ] {
        let (status, body) = call(
            &state,
            &reader,
            "PATCH",
            &format!("/v1/settings/{section}"),
            Some(patch),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{section}");
        assert_eq!(body["code"], "permission_denied");
    }
    // And nothing moved.
    let (_, body) = call(&state, &reader, "GET", "/v1/settings/global", None).await;
    assert_eq!(body["theme"], "system");
}

#[tokio::test]
async fn a_first_party_token_writes_and_reads_back() {
    let (_store, _settings, state, reader, writer) = state();
    let (status, body) = call(
        &state,
        &writer,
        "PATCH",
        "/v1/settings/global",
        Some(serde_json::json!({"theme": "dark", "density": "compact"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["theme"], "dark");
    assert_eq!(body["density"], "compact");

    // A second, ordinary session sees it — the write went to the authority,
    // not to a per-session cache.
    let (_, seen) = call(&state, &reader, "GET", "/v1/settings/global", None).await;
    assert_eq!(seen["theme"], "dark");
    assert_eq!(seen["density"], "compact");
}

#[tokio::test]
async fn a_write_scope_alone_does_not_grant_a_read() {
    // `effective_scopes` filters rather than unions, so a caller that names
    // one scope holds exactly that one. The routes must not assume otherwise.
    let (_store, _settings, state, _reader, _writer) = state();
    let (write_only, _) = state
        .sessions
        .mint("odd", &[SETTINGS_WRITE_SCOPE.into()], now_millis());
    let (status, _) = call(&state, &write_only, "GET", "/v1/settings/global", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_unauthenticated_request_is_rejected_before_the_section_matters() {
    let (_store, _settings, state, _reader, _writer) = state();
    let response = router(Arc::clone(&state))
        .oneshot(
            Request::builder()
                .uri("/v1/settings/global")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_unknown_section_is_a_404_and_not_a_store_request() {
    // `.fallback(forward_store)` means an unregistered path is proxied to the
    // Store. A settings path that fell through would answer something
    // confusing about workspaces.
    let (_store, _settings, state, reader, _writer) = state();
    let (status, body) = call(&state, &reader, "GET", "/v1/settings/windows", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
}

#[tokio::test]
async fn a_patch_naming_a_field_that_is_not_a_setting_is_refused() {
    let (_store, _settings, state, _reader, writer) = state();
    for body in [
        serde_json::json!({"themee": "dark"}),
        serde_json::json!({"global_search_height": 900}),
        serde_json::json!({"emoji_recents": "x"}),
    ] {
        let (status, answer) = call(
            &state,
            &writer,
            "PATCH",
            "/v1/settings/global",
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(answer["code"], "invalid_request");
    }
}

#[tokio::test]
async fn a_patch_carrying_a_value_the_registry_refuses_changes_nothing() {
    let (_store, _settings, state, _reader, writer) = state();
    let (status, _) = call(
        &state,
        &writer,
        "PATCH",
        "/v1/settings/global",
        Some(serde_json::json!({"theme": "dark", "zoom": 9.0})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, body) = call(&state, &writer, "GET", "/v1/settings/global", None).await;
    assert_eq!(
        body["theme"], "system",
        "a refused patch leaves the disk as it was, including the valid \
         fields that travelled with the invalid one"
    );
}

#[tokio::test]
async fn a_side_effect_that_cannot_be_performed_is_a_conflict_carrying_the_truth() {
    let (_store, _settings, state, _reader, writer) = state();
    let (status, body) = call(
        &state,
        &writer,
        "PATCH",
        "/v1/settings/global",
        Some(serde_json::json!({"start_on_login": true})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "no host installed a start-on-login port, so the value cannot commit"
    );
    assert_eq!(
        body["authoritative"]["start_on_login"], false,
        "the failure body carries what is actually true, so a control can \
         reconcile instead of showing a value it only hoped for"
    );
}

/// A host that can perform the OS side effect, and one that tries and fails.
struct StubStartOnLogin {
    allowed: bool,
    performed: std::sync::Arc<std::sync::Mutex<Vec<bool>>>,
}

impl koru_settings::StartOnLoginPort for StubStartOnLogin {
    fn set(&self, enabled: bool) -> Result<(), String> {
        if !self.allowed {
            return Err("the startup entry could not be written".into());
        }
        self.performed.lock().expect("stub port").push(enabled);
        Ok(())
    }
}

async fn assert_faulted_voice_write(
    operation: &str,
    seed: Option<GlobalPatch>,
    body: serde_json::Value,
    expected_enabled: Option<bool>,
    expected_startup: bool,
) {
    let (_store, _settings, mut state, _reader, writer) = state();
    Arc::get_mut(&mut state)
        .expect("unique test state")
        .settings_write_fault = SettingsWriteFault::with_hold(std::time::Duration::from_millis(75));
    state
        .settings()
        .expect("settings authority")
        .set_start_on_login_port(std::sync::Arc::new(StubStartOnLogin {
            allowed: true,
            performed: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }));
    if let Some(seed) = seed {
        state
            .settings()
            .expect("settings authority")
            .patch_global(seed)
            .expect("seed global settings");
    }
    assert_eq!(
        state.configure_settings_write_fault(true, Some(operation)),
        FaultConfiguration::Armed
    );

    let request = call(
        &state,
        &writer,
        "PATCH",
        "/v1/settings/global",
        Some(body.clone()),
    );
    tokio::pin!(request);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(15), &mut request)
            .await
            .is_err(),
        "the reply is held"
    );
    let current = state.settings().expect("settings authority").global();
    assert_eq!(current.global_dictation_enabled, expected_enabled);
    assert_eq!(current.start_on_login, expected_startup);
    assert!(matches!(
        state.settings_write_fault.state(),
        FaultState::Fired(_)
    ));

    let (status, _) = tokio::time::timeout(std::time::Duration::from_secs(1), &mut request)
        .await
        .expect("held reply released");
    assert_eq!(status, StatusCode::OK);
    let (status, _) = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        call(&state, &writer, "PATCH", "/v1/settings/global", Some(body)),
    )
    .await
    .expect("one-shot fault does not hold a retry");
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn enable_commits_before_the_internal_fault_holds_its_reply() {
    assert_faulted_voice_write(
        "global-dictation-enable",
        None,
        serde_json::json!({"global_dictation_enabled": true, "start_on_login": true}),
        Some(true),
        true,
    )
    .await;
}

#[tokio::test]
async fn disable_commits_before_the_internal_fault_holds_its_reply() {
    assert_faulted_voice_write(
        "global-dictation-disable",
        Some(GlobalPatch {
            global_dictation_enabled: Some(true),
            start_on_login: Some(true),
            ..Default::default()
        }),
        serde_json::json!({"global_dictation_enabled": false}),
        Some(false),
        true,
    )
    .await;
}

#[tokio::test]
async fn startup_opt_out_commits_before_the_internal_fault_holds_its_reply() {
    assert_faulted_voice_write(
        "start-on-login-disable",
        Some(GlobalPatch {
            start_on_login: Some(true),
            ..Default::default()
        }),
        serde_json::json!({"start_on_login": false}),
        None,
        false,
    )
    .await;
}

#[tokio::test]
async fn rejected_requests_do_not_spend_the_armed_fault() {
    let (_store, _settings, mut state, reader, writer) = state();
    Arc::get_mut(&mut state)
        .expect("unique test state")
        .settings_write_fault = SettingsWriteFault::with_hold(std::time::Duration::from_millis(75));
    assert_eq!(
        state.configure_settings_write_fault(true, Some("start-on-login-disable")),
        FaultConfiguration::Armed
    );

    let (status, _) = call(
        &state,
        &reader,
        "PATCH",
        "/v1/settings/global",
        Some(serde_json::json!({"start_on_login": false})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &state,
        &writer,
        "PATCH",
        "/v1/settings/global",
        Some(serde_json::json!({"start_on_login": "not-a-boolean"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &state,
        &writer,
        "PATCH",
        "/v1/settings/global",
        Some(serde_json::json!({"start_on_login": false})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        state.settings_write_fault.state(),
        FaultState::Armed(FaultOperation::StartOnLoginDisable)
    );

    state
        .settings()
        .expect("settings authority")
        .set_start_on_login_port(std::sync::Arc::new(StubStartOnLogin {
            allowed: true,
            performed: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }));
    let request = call(
        &state,
        &writer,
        "PATCH",
        "/v1/settings/global",
        Some(serde_json::json!({"start_on_login": false})),
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(15), request)
            .await
            .is_err(),
        "the first successful matching write spends the hold"
    );
    assert_eq!(
        state.settings_write_fault.state(),
        FaultState::Fired(FaultOperation::StartOnLoginDisable)
    );
}

/// **The side effect goes first, and the value commits only if it worked.**
///
/// A stored `true` the registry never got is a lie the person discovers at the
/// next sign-in, so this is the ordering the whole port exists for — and with a
/// host that installs one, the field stops answering 409.
#[tokio::test]
async fn a_host_that_installs_the_port_can_commit_start_on_login() {
    let (_store, _settings, state, _reader, writer) = state();
    let performed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    state
        .settings()
        .expect("the helper installed one")
        .set_start_on_login_port(std::sync::Arc::new(StubStartOnLogin {
            allowed: true,
            performed: std::sync::Arc::clone(&performed),
        }));

    let (status, body) = call(
        &state,
        &writer,
        "PATCH",
        "/v1/settings/global",
        Some(serde_json::json!({"start_on_login": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["start_on_login"], true);
    assert_eq!(
        *performed.lock().expect("stub port"),
        vec![true],
        "the OS was asked exactly once, before the value was stored"
    );

    // Reasserting a value that is already true touches the OS again. That is
    // the repair path after a prior persistence+compensation double failure,
    // where disk can already say true while the startup entry does not.
    let (status, _) = call(
        &state,
        &writer,
        "PATCH",
        "/v1/settings/global",
        Some(serde_json::json!({"start_on_login": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(*performed.lock().expect("stub port"), vec![true, true]);
}

/// A port that refuses is a value that does not move — and the failure body
/// carries what is actually true, so a checkbox reconciles rather than showing
/// something it only hoped for.
#[tokio::test]
async fn a_side_effect_that_fails_leaves_the_value_where_it_was() {
    let (_store, _settings, state, _reader, writer) = state();
    state
        .settings()
        .expect("the helper installed one")
        .set_start_on_login_port(std::sync::Arc::new(StubStartOnLogin {
            allowed: false,
            performed: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }));

    let (status, body) = call(
        &state,
        &writer,
        "PATCH",
        "/v1/settings/global",
        Some(serde_json::json!({"start_on_login": true})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["authoritative"]["start_on_login"], false);

    let (_, seen) = call(&state, &writer, "GET", "/v1/settings/global", None).await;
    assert_eq!(seen["start_on_login"], false, "and nothing was stored");
}

#[tokio::test]
async fn a_terminal_patch_refuses_a_proportional_font() {
    let (_store, _settings, state, _reader, writer) = state();
    let (status, body) = call(
        &state,
        &writer,
        "PATCH",
        "/v1/settings/terminal",
        Some(serde_json::json!({"font": {"chosen": "lora"}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("font"));

    let (status, body) = call(
        &state,
        &writer,
        "PATCH",
        "/v1/settings/terminal",
        Some(serde_json::json!({"font": {"chosen": "sourcecodepro"}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["font"]["chosen"], "sourcecodepro");
}

#[tokio::test]
async fn different_fields_patched_at_once_from_two_sessions_both_land() {
    let (_store, _settings, state, _reader, writer) = state();
    let (second, _) = state.sessions.mint(
        "second-window",
        &[SETTINGS_READ_SCOPE.into(), SETTINGS_WRITE_SCOPE.into()],
        now_millis(),
    );
    let one = call(
        &state,
        &writer,
        "PATCH",
        "/v1/settings/global",
        Some(serde_json::json!({"theme": "dark"})),
    );
    let two = call(
        &state,
        &second,
        "PATCH",
        "/v1/settings/global",
        Some(serde_json::json!({"caret_color": "amber"})),
    );
    let (one, two) = tokio::join!(one, two);
    assert_eq!(one.0, StatusCode::OK);
    assert_eq!(two.0, StatusCode::OK);

    let (_, body) = call(&state, &writer, "GET", "/v1/settings/global", None).await;
    assert_eq!(body["theme"], "dark");
    assert_eq!(body["caret_color"], "amber");
}

#[tokio::test]
async fn the_ai_section_is_unavailable_rather_than_absent_without_a_worker() {
    // The API exists whether or not the worker spawned, so a build that has
    // no AI domain says so instead of answering a misleading 404.
    let (_store, _settings, state, reader, _writer) = state();
    let (status, body) = call(&state, &reader, "GET", "/v1/settings/ai", None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "capability_unavailable");
}

#[tokio::test]
async fn a_host_with_no_settings_authority_says_so() {
    let store_root = tempfile::tempdir().unwrap();
    let worker = koru_store_server::StoreWorker::new(koru_store_server::WorkerConfig::embedded(
        store_root.path().to_path_buf(),
        "internal-debug",
    ))
    .spawn()
    .unwrap();
    let state = GatewayState::new("internal-debug".into(), worker);
    let (token, _) = state.sessions.mint("test", &[], now_millis());
    let (status, body) = call(&state, &token, "GET", "/v1/settings/global", None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "capability_unavailable");
}

#[test]
fn the_authority_is_handed_down_exactly_once() {
    let store_root = tempfile::tempdir().unwrap();
    let settings_root = tempfile::tempdir().unwrap();
    let worker = koru_store_server::StoreWorker::new(koru_store_server::WorkerConfig::embedded(
        store_root.path().to_path_buf(),
        "internal-debug",
    ))
    .spawn()
    .unwrap();
    let state = GatewayState::new("internal-debug".into(), worker);
    let authority = || {
        koru_settings::SettingsAuthority::new(koru_settings::SettingsPaths::under_root(
            settings_root.path(),
        ))
    };
    assert!(state.set_settings_authority(authority()));
    assert!(
        !state.set_settings_authority(authority()),
        "one authority per process is what `nothing else reads or writes the \
         settings files` means in practice"
    );
}

/// A gateway with a real events journal, so the facts can be read back out of
/// it the way a window would.
fn state_with_journal() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    Arc<GatewayState>,
    String,
) {
    let root = tempfile::tempdir().unwrap();
    let settings_root = tempfile::tempdir().unwrap();
    let worker = koru_store_server::StoreWorker::new(koru_store_server::WorkerConfig::embedded(
        root.path().to_path_buf(),
        "internal-debug",
    ))
    .spawn()
    .unwrap();
    let events = koru_events_server::EventsHandle::open(koru_events_server::WorkerConfig::quiet(
        root.path().join("events"),
    ))
    .unwrap();
    let state =
        GatewayState::new_with_events("internal-debug".into(), worker, Some(events.clone()));
    state.set_settings_authority(koru_settings::SettingsAuthority::new(
        koru_settings::SettingsPaths::under_root(settings_root.path()),
    ));
    let (token, _) = state.sessions.mint(
        "first-party",
        &[
            SETTINGS_READ_SCOPE.into(),
            SETTINGS_WRITE_SCOPE.into(),
            "events.read".into(),
        ],
        crate::now_millis(),
    );
    (root, settings_root, state, token)
}

/// Every settings fact in the journal, oldest first.
async fn journal_facts(state: &Arc<GatewayState>, token: &str) -> Vec<serde_json::Value> {
    let (status, page) = call(state, token, "GET", "/v1/events?after=0&limit=1000", None).await;
    assert_eq!(status, StatusCode::OK);
    page["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|stored| stored["envelope"].clone())
        .filter(|envelope| {
            envelope["name"]
                .as_str()
                .is_some_and(|name| name.starts_with("settings."))
        })
        .collect()
}

async fn journal_names(state: &Arc<GatewayState>, token: &str) -> Vec<String> {
    journal_facts(state, token)
        .await
        .into_iter()
        .map(|envelope| envelope["name"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn a_committed_change_is_in_the_journal_by_the_time_the_route_answers() {
    // The ordering the whole design rests on: an event is a fact, so a window
    // that reacts to one never races the write it reports.
    let (_root, _settings, state, token) = state_with_journal();
    assert!(journal_names(&state, &token).await.is_empty());

    let (status, _) = call(
        &state,
        &token,
        "PATCH",
        "/v1/settings/global",
        Some(serde_json::json!({"theme": "dark"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        journal_names(&state, &token).await,
        ["settings.global_changed"]
    );

    let (status, _) = call(
        &state,
        &token,
        "PATCH",
        "/v1/settings/terminal",
        Some(serde_json::json!({"zoom": 1.5})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        journal_names(&state, &token).await,
        ["settings.global_changed", "settings.terminal_changed"]
    );
}

/// **Two windows converge on one theme.**
///
/// The whole point of the change fact, end to end and in one process: a second
/// window is holding a section it read before anything moved, the first window
/// commits a change, and the fact in the journal is what tells the second one
/// its copy is stale. It then *refetches* — the fact carries no value, so
/// refetching is the only way it can learn one, and that is deliberate: a
/// window can never apply a value the authority has not committed.
#[tokio::test]
async fn a_change_in_one_window_reaches_the_other_through_the_fact_it_refetches_on() {
    let (_root, _settings, state, first) = state_with_journal();
    // A second window, with its own token — as far as the gateway is concerned
    // that is all "another window" means.
    let (second, _) = state.sessions.mint(
        "first-party",
        &[SETTINGS_READ_SCOPE.into(), crate::EVENTS_READ_SCOPE.into()],
        crate::now_millis(),
    );

    let (status, before) = call(&state, &second, "GET", "/v1/settings/global", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(before["theme"], "system");
    let watermark = call(
        &state,
        &second,
        "GET",
        "/v1/events?after=0&limit=1000",
        None,
    )
    .await
    .1["events"]
        .as_array()
        .map_or(0, Vec::len);

    let (status, _) = call(
        &state,
        &first,
        "PATCH",
        "/v1/settings/global",
        Some(serde_json::json!({"theme": "dark"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // What the second window's subscription sees: one row, naming its section
    // and nothing else.
    let facts = journal_facts(&state, &second).await;
    assert_eq!(facts.len(), watermark + 1);
    assert_eq!(facts.last().unwrap()["name"], "settings.global_changed");

    // And the refetch the invalidation triggers shows the change.
    let (status, after) = call(&state, &second, "GET", "/v1/settings/global", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(after["theme"], "dark");
    assert_eq!(
        after["density"], before["density"],
        "a field nobody named did not move"
    );
}

/// The other half of convergence: the second window may *read* without holding
/// the write scope, so a settings change reaches every window while only the
/// one that was clicked could have caused it.
#[tokio::test]
async fn a_reader_only_window_still_sees_the_change() {
    let (_root, _settings, state, first) = state_with_journal();
    let (reader, _) = state.sessions.mint("test", &[], crate::now_millis());
    call(
        &state,
        &first,
        "PATCH",
        "/v1/settings/global",
        Some(serde_json::json!({"density": "compact"})),
    )
    .await;
    let (status, section) = call(&state, &reader, "GET", "/v1/settings/global", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(section["density"], "compact");
}

#[tokio::test]
async fn a_refused_change_says_nothing() {
    let (_root, _settings, state, token) = state_with_journal();
    for body in [
        serde_json::json!({"zoom": 9.0}),
        serde_json::json!({"themee": "dark"}),
        serde_json::json!({"start_on_login": true}),
    ] {
        call(&state, &token, "PATCH", "/v1/settings/global", Some(body)).await;
    }
    assert!(
        journal_names(&state, &token).await.is_empty(),
        "nothing happened, so nothing is said"
    );
}

#[tokio::test]
async fn a_change_fact_carries_its_section_and_no_value() {
    let (_root, _settings, state, token) = state_with_journal();
    call(
        &state,
        &token,
        "PATCH",
        "/v1/settings/global",
        Some(serde_json::json!({"theme": "dark", "density": "compact"})),
    )
    .await;
    let facts = journal_facts(&state, &token).await;
    let fact = facts
        .iter()
        .find(|envelope| envelope["name"] == "settings.global_changed")
        .expect("the fact is in the journal");
    assert_eq!(
        fact["attrs"]["section"],
        serde_json::json!({"t": "token", "v": "global"}),
        "a closed token, which is the only thing a settings fact carries"
    );
    let serialized = fact.to_string();
    for forbidden in ["dark", "compact", "theme", "density"] {
        assert!(
            !serialized.contains(forbidden),
            "the fact leaked `{forbidden}`: what somebody's fonts and theme \
             actually are is theirs, and a bus is the wrong place to learn it"
        );
    }
}

#[test]
fn every_settings_route_declares_the_scope_it_takes() {
    // The frozen grant inventory for this surface: reads on the ordinary
    // scope, writes on the narrow one, and nothing undeclared.
    let matrix: Vec<_> = ROUTES
        .iter()
        .map(|route| (route.method, route.path, route.scope))
        .collect();
    assert_eq!(
        matrix,
        vec![
            ("GET", "/v1/settings/{section}", SETTINGS_READ_SCOPE),
            ("PATCH", "/v1/settings/{section}", SETTINGS_WRITE_SCOPE),
        ]
    );
}

#[test]
fn the_settings_routes_are_documented_with_their_scopes() {
    let document = crate::public_openapi();
    let path = &document["paths"]["/v1/settings/{section}"];
    assert_eq!(
        path["get"]["security"],
        serde_json::json!([{"bearerAuth": [SETTINGS_READ_SCOPE]}])
    );
    assert_eq!(
        path["patch"]["security"],
        serde_json::json!([{"bearerAuth": [SETTINGS_WRITE_SCOPE]}])
    );
    assert!(path["patch"]["requestBody"]["required"].as_bool().unwrap());
    assert!(path["get"]["requestBody"].is_null());
}

#[test]
fn the_documented_settings_schemas_never_mention_a_secret() {
    let document = crate::public_openapi().to_string();
    for forbidden in ["api_key", "key_account", "vault_label", "openai_secret"] {
        assert!(!document.contains(forbidden), "leaked `{forbidden}`");
    }
    assert!(
        document.contains("credential_configured"),
        "the configured-state flag is the whole of what is said about a key"
    );
}
