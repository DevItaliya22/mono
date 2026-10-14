// SPDX-License-Identifier: MPL-2.0

//! Opt-in gateway -> orchestrator -> registered host factory qualification.
//! No executable, root, or account is touched before the explicit live guard.
use super::*;
use koru_session_client::SessionClient;
use koru_session_protocol::{
    AgentEvent, CapabilitySet, ProcessState, ProtocolOffer, ProtocolRange, SessionId, SessionMode,
    capability,
};
const CONTENT: &[u8] = b"KORU_GENERIC_QUALIFICATION";
const OBJECTIVE: &str = "Use only the tool metadata discovery needed to locate the Koru run MCP create tool, then call create exactly once with path fixture.txt, title Generic qualification, and mime text/plain. The fixture file is already staged in this run's artifact folder. Use no other actions, no shell, no file reads or edits, and no child agents. Then finish.";
const CANCEL_OBJECTIVE: &str = "Without using any tools, shell, file edits, or child agents, write a long numbered explanation of elementary arithmetic with 200 short items.";
const LIVE_DEADLINE: Duration = Duration::from_secs(120);

struct Provider {
    kind: &'static str,
    executable: &'static str,
    package: &'static str,
    version: &'static str,
    maximum_discovery_calls: u64,
}
const CLAUDE: Provider = Provider {
    kind: "claude",
    executable: "claude-agent-acp",
    package: "@agentclientprotocol/claude-agent-acp",
    version: "0.76.0",
    maximum_discovery_calls: 1,
};
const GROK: Provider = Provider {
    kind: "grok",
    executable: "grok",
    package: "",
    version: "1.0.25",
    maximum_discovery_calls: 1,
};
const CODEX: Provider = Provider {
    kind: "codex",
    executable: "codex",
    package: "@openai/codex",
    version: "0.154.0",
    maximum_discovery_calls: 0,
};

#[test]
fn live_registered_claude_run_and_cancel_when_opted_in() {
    qualify(&CLAUDE);
}
#[test]
fn live_registered_grok_run_and_cancel_when_opted_in() {
    qualify(&GROK);
}
#[test]
fn live_registered_codex_run_and_cancel_when_opted_in() {
    qualify(&CODEX);
}

fn runtime_path(provider: &Provider) -> std::ffi::OsString {
    let executable = koru_session_profiles::resolve_executable(provider.executable)
        .expect("install pinned provider on PATH before opting in");
    let bin = executable.parent().unwrap();
    if !provider.package.is_empty() {
        // Local npm shims live in node_modules/.bin; global Windows npm
        // shims live beside node_modules. Qualification must exercise the
        // executable the normal app finds in either installation layout.
        let package = [
            bin.parent().unwrap().join(provider.package),
            bin.join("node_modules").join(provider.package),
        ]
        .into_iter()
        .find(|directory| directory.join("package.json").is_file())
        .expect("provider npm package beside its local or global shim");
        let pin: serde_json::Value = serde_json::from_slice(
            &std::fs::read(package.join("package.json")).expect("pinned npm package"),
        )
        .unwrap();
        assert_eq!(
            pin["version"].as_str(),
            Some(provider.version),
            "only the qualification pin is accepted"
        );
    }
    // Native Grok's complete version/revision is checked by its registered
    // factory. No npm layout or provider configuration is assumed for it.
    let node = koru_session_profiles::resolve_executable("node").expect("Node installed");
    let mut directories = vec![bin.to_path_buf(), node.parent().unwrap().to_path_buf()];
    #[cfg(windows)]
    {
        let windows = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
        directories.extend([
            windows.join("System32"),
            windows.join("System32/WindowsPowerShell/v1.0"),
            windows,
        ]);
    }
    #[cfg(not(windows))]
    directories.extend([PathBuf::from("/usr/bin"), PathBuf::from("/bin")]);
    std::env::join_paths(directories).unwrap()
}

fn session_client(agent: &Agent) -> SessionClient {
    SessionClient::connect(
        &koru_session_host::endpoint(segment(), Some(agent.root())).unwrap(),
        "koru-live-qualification",
        ProtocolOffer {
            versions: ProtocolRange { min: 14, max: 14 },
            capabilities: CapabilitySet::new(
                capability::ALL.iter().map(|name| name.parse().unwrap()),
            ),
        },
    )
    .expect("connect to the actual supervised host")
}
fn session_for(agent: &Agent, run: RunId) -> Option<SessionId> {
    agent
        .runs_db()
        .current_attempt(run)
        .ok()
        .flatten()
        .and_then(|attempt| attempt.provider_session)
        .and_then(|provider| provider.session)
}
fn steps(view: &koru_delegate_api::wire::RunView) -> Vec<StepKind> {
    view.timeline
        .items
        .iter()
        .filter_map(|item| match item {
            TimelineItem::Step(entry) => Some(entry.kind),
            _ => None,
        })
        .collect()
}
fn verify_closed(
    agent: &Agent,
    client: &koru_api_client::Client,
    sessions: &SessionClient,
    run: RunId,
) {
    wait_until("run authority revocation", PROMPT, || {
        client
            .artifacts()
            .run_authority(run)
            .is_ok_and(|authority| {
                authority.state == koru_artifact_wire::authority::AuthorityState::Revoked
            })
    });
    let session = session_for(agent, run).expect("attempt recorded its session");
    wait_until("provider session exit", PROMPT, || {
        sessions.list().is_ok_and(|rows| {
            rows.iter().find(|row| row.id == session).is_none_or(|row| {
                matches!(
                    row.process,
                    ProcessState::Exited { .. } | ProcessState::Failed { .. }
                )
            })
        })
    });
}

struct LiveFixture {
    agent: Agent,
    client: koru_api_client::Client,
    sessions: SessionClient,
    request: CreateRunRequest,
    directory: PathBuf,
}

fn live_fixture(provider: &Provider) -> LiveFixture {
    // Presence only: no credential copying, value inspection, or diagnostics.
    for key in [
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
        "CLAUDE_CODE_SKIP_AUTH_LOGIN",
        "CLAUDE_CONFIG_DIR",
        "OPENAI_BASE_URL",
        "GROK_XAI_API_BASE_URL",
        "GROK_CLI_CHAT_PROXY_BASE_URL",
        "GROK_MODELS_BASE_URL",
        "GROK_MODELS_LIST_URL",
        "GROK_HOME",
        "XAI_API_KEY",
    ] {
        assert!(
            std::env::var_os(key).is_none(),
            "live fixture refuses environment override {key}"
        );
    }
    let path = runtime_path(provider);
    let tree = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let base = tree.join("target/generic-live-fixtures");
    std::fs::create_dir_all(&base).unwrap();
    let root = tempfile::Builder::new()
        .prefix("orchestrated-")
        .tempdir_in(base)
        .unwrap();
    let agent = Agent::with_runtime_path(None, root, Some(path));
    let client = agent.gateway();
    let (profile, workspace) = workspace(&client);
    wait_for_runs(&client, workspace);
    let sessions = session_client(&agent);
    let directory = agent.root().join("qualification-workspace");
    std::fs::create_dir_all(&directory).unwrap();
    let project = koru_session_protocol::ProjectId::mint();
    sessions
        .create_project(
            project,
            "Qualification fixture",
            Some(directory.to_string_lossy().into_owned()),
            Some(koru_session_protocol::WorkspaceScope {
                profile: profile.to_storage().parse().unwrap(),
                workspace: workspace.to_storage().parse().unwrap(),
            }),
        )
        .expect("create isolated project through the host");
    let mut request = request(profile, workspace, Vec::new());
    request.agent = SessionProfileKey::BuiltIn(provider.kind.parse().unwrap());
    request.scope = WorkspaceScope::ProjectRoot { project };
    LiveFixture {
        agent,
        client,
        sessions,
        request,
        directory,
    }
}

fn qualify(provider: &Provider) {
    if !koru_env::switch(koru_env::name::KORU_LIVE_PROVIDER_SMOKE) {
        return;
    }
    let LiveFixture {
        agent,
        client,
        sessions,
        mut request,
        directory,
    } = live_fixture(provider);
    request.objective = Objective::new(OBJECTIVE).unwrap();
    let run = request.run;
    let staging = directory.join(".artifacts").join(run.to_storage());
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("fixture.txt"), CONTENT).unwrap();
    client
        .runs()
        .create(&request)
        .expect("gateway accepts real delegated run");
    let mut completed = None;
    let phase_started = Instant::now();
    let mut diagnosed = false;
    let mut answered = std::collections::BTreeSet::new();
    wait_until(
        "registered provider artifact completion",
        LIVE_DEADLINE,
        || {
            let view = client.runs().run(run, None, 0, 500).unwrap();
            let needs_diagnostic = matches!(&view.header.status,
                RunStatus::Running { attention, .. }
                    if attention.contains(Attention::FinishedNeedingAttention))
                || matches!(
                    &view.header.status,
                    RunStatus::Failed { .. } | RunStatus::Cancelled
                )
                || phase_started.elapsed() >= LIVE_DEADLINE.saturating_sub(Duration::from_secs(3));
            if needs_diagnostic && !diagnosed {
                print_diagnostic(&agent, &sessions, run, &view);
                diagnosed = true;
            }
            assert!(
                !matches!(
                    view.header.status,
                    RunStatus::Failed { .. } | RunStatus::Cancelled
                ),
                "registered run ended unsuccessfully: {}",
                view.header.status.name()
            );
            if view.header.status == RunStatus::Completed {
                completed = Some(view);
                true
            } else {
                answer_known_fixture_approval(&agent, &sessions, run, provider, &mut answered);
                false
            }
        },
    );
    let view = completed.unwrap();
    assert_eq!(view.header.artifacts.len(), 1);
    let result = &view.header.artifacts[0];
    assert_eq!(result.title.as_str(), "Generic qualification");
    assert_eq!(result.size_bytes, CONTENT.len() as u64);
    assert_eq!(
        client.artifacts().content(result.artifact).unwrap(),
        CONTENT
    );
    let versions = client.artifacts().run_versions(run).unwrap();
    assert_eq!(versions.versions.len(), 1);
    assert_eq!(versions.versions[0].id, result.artifact);
    let provenance = versions.versions[0].produced_by.unwrap();
    assert_eq!(provenance.run, run);
    assert_eq!(
        provenance.generation,
        koru_delegate_core::AttemptGeneration::FIRST
    );
    let timeline = steps(&view);
    for expected in [
        StepKind::RunCreated,
        StepKind::Started,
        StepKind::ArtifactRegistered,
        StepKind::Completed,
    ] {
        assert!(
            timeline.contains(&expected),
            "missing normalized lifecycle step"
        );
    }
    assert!(timeline.iter().any(|step| matches!(
        step,
        StepKind::TurnCompleted {
            artifacts_registered: 1,
            ..
        }
    )));
    let completed_calls: u64 = timeline
        .iter()
        .filter_map(|step| match step {
            StepKind::ToolsCompleted {
                calls, failed: 0, ..
            } => Some(u64::from(*calls)),
            _ => None,
        })
        .sum();
    assert!(
        (1..=1 + provider.maximum_discovery_calls).contains(&completed_calls),
        "successful tools={completed_calls}; only one fixture create and the measured optional metadata discovery may complete"
    );
    assert!(
        !timeline.iter().any(|step| matches!(step,
        StepKind::ToolsCompleted { failed, .. } if *failed > 0)),
        "the fixture cannot hide another failed tool call"
    );
    verify_closed(&agent, &client, &sessions, run);

    let mut cancel_request = request.clone();
    cancel_request.run = RunId::mint();
    cancel_request.objective = Objective::new(CANCEL_OBJECTIVE).unwrap();
    let cancel_run = cancel_request.run;
    client
        .runs()
        .create(&cancel_request)
        .expect("create text-only cancellation run");
    let mut observed_session = None;
    let mut observed_turn = None;
    wait_until("real provider text before cancel", LIVE_DEADLINE, || {
        let Some(session) = session_for(&agent, cancel_run) else {
            return false;
        };
        let observation = sessions.observe_agent_events(session).unwrap();
        assert!(
            !observation.earlier_truncated,
            "fixture must fit the retained tail"
        );
        let summary = sessions
            .list()
            .unwrap()
            .into_iter()
            .find(|row| row.id == session);
        let Some(summary) = summary else { return false };
        let active = summary.active_turn_id;
        let streaming = summary.managed == Some(koru_session_protocol::ManagedState::Running)
            && active.is_some()
            && observation.events.iter().any(|event| {
                matches!(&event.event, AgentEvent::MessageDelta { turn_id, text }
                    if Some(*turn_id) == active && !text.is_empty())
            });
        if streaming {
            observed_session = Some(session);
            observed_turn = active;
        }
        streaming
    });
    let session = observed_session.unwrap();
    let summary = sessions
        .list()
        .unwrap()
        .into_iter()
        .find(|row| row.id == session)
        .unwrap();
    assert_eq!(
        summary.mode,
        SessionMode::Managed {
            provider: provider.kind.parse().unwrap()
        }
    );
    assert!(
        summary.active_turn_id == observed_turn
            && observed_turn.is_some()
            && summary.managed == Some(koru_session_protocol::ManagedState::Running),
        "the observed provider turn must still be active immediately before cancel"
    );
    client
        .runs()
        .cancel(cancel_run)
        .expect("gateway cancellation");
    wait_until("orchestrated cancellation", PROMPT, || {
        client
            .runs()
            .run(cancel_run, None, 0, 500)
            .is_ok_and(|view| view.header.status == RunStatus::Cancelled)
    });
    let cancelled = client.runs().run(cancel_run, None, 0, 500).unwrap();
    let timeline = steps(&cancelled);
    assert!(
        timeline
            .iter()
            .any(|step| matches!(step, StepKind::TurnCancelled { .. }))
    );
    assert!(
        timeline
            .iter()
            .any(|step| matches!(step, StepKind::Cancelled { .. }))
    );
    assert!(cancelled.header.artifacts.is_empty());
    verify_closed(&agent, &client, &sessions, cancel_run);
    let log = agent.delegate_log();
    for private in [OBJECTIVE, CANCEL_OBJECTIVE, "koru_run_"] {
        assert!(
            !log.contains(private),
            "private live data reached the delegate log"
        );
    }
    eprintln!(
        "live_qualification provider={} pin={} artifact_bytes={} completed_tools={} provenance=first_generation lifecycle=completed cancellation=landed authority=revoked",
        provider.kind,
        provider.version,
        CONTENT.len(),
        completed_calls
    );
}

/// This matcher is deliberately a fixture allowlist, not product policy.
/// Wrapper 0.76.0's presentation.js/tools.js gives default MCP calls their
/// exact native tool name and Other kind. Never infer from prose or labels.
fn known_fixture_choice(
    provider: &str,
    summary: &str,
    latest_tool_is_other: bool,
    choices: &[koru_session_protocol::ApprovalChoice],
) -> Option<koru_session_protocol::ApprovalChoiceId> {
    if provider != "claude" || summary != "mcp__koru_run__create" || !latest_tool_is_other {
        return None;
    }
    let mut one_time = choices
        .iter()
        .filter(|choice| choice.intent == koru_session_protocol::ApprovalIntent::Allow);
    let selected = one_time.next()?;
    one_time.next().is_none().then_some(selected.id)
}

fn answer_known_fixture_approval(
    agent: &Agent,
    sessions: &SessionClient,
    run: RunId,
    provider: &Provider,
    answered: &mut std::collections::BTreeSet<koru_session_protocol::ApprovalId>,
) {
    use koru_session_protocol::{
        ApprovalDecision, HandoverState, IdempotencyKey, TakeoverUrgency, ViewportGeneration,
    };
    let Some(session) = session_for(agent, run) else {
        return;
    };
    let observation = match sessions.observe_agent_events(session) {
        Ok(observation) => observation,
        // The run document and session teardown are independent observations.
        // A completed run may close between reading its view and this request.
        Err(koru_session_client::ClientError::Session(error))
            if error.code == koru_session_protocol::ErrorCode::SessionNotFound =>
        {
            return;
        }
        Err(_) => panic!("approval observation failed"),
    };
    assert!(
        !observation.earlier_truncated,
        "approval fixture requires complete retained evidence"
    );
    let mut latest_other = std::collections::BTreeMap::new();
    let mut pending = std::collections::BTreeMap::new();
    for item in &observation.events {
        match &item.event {
            AgentEvent::ToolStarted { turn_id, label, .. } => {
                latest_other.insert(*turn_id, label == "other");
            }
            AgentEvent::ApprovalRequired {
                turn_id,
                approval_id,
                summary,
                choices,
            } => {
                pending.insert(
                    *approval_id,
                    known_fixture_choice(
                        provider.kind,
                        summary,
                        latest_other.get(turn_id).copied().unwrap_or(false),
                        choices,
                    ),
                );
            }
            AgentEvent::ApprovalResolved { approval_id, .. }
            | AgentEvent::ApprovalExpired { approval_id, .. } => {
                pending.remove(approval_id);
            }
            _ => {}
        }
    }
    for (approval, choice) in pending {
        if answered.contains(&approval) {
            continue;
        }
        assert!(
            answered.is_empty(),
            "second distinct approval refused before action"
        );
        let choice = choice.expect("unidentified approval refused; no native content retained");
        sessions
            .request_control(session, TakeoverUrgency::AtTurnBoundary)
            .unwrap();
        wait_until("approval boundary releases the controller", PROMPT, || {
            sessions
                .list()
                .unwrap()
                .iter()
                .find(|row| row.id == session)
                .is_some_and(|row| row.handover == HandoverState::Released)
        });
        let (lease, _subscription) = sessions
            .attach_live(session, ViewportGeneration::new(1).unwrap(), 80, 1)
            .unwrap();
        let answer = sessions.answer_approval(
            session,
            lease.token,
            IdempotencyKey::mint(),
            approval,
            ApprovalDecision::Approve,
            Some(choice),
        );
        // Return even on refusal so a failed fixture never strands the lease.
        let returned = sessions.return_control(session, lease.token);
        answer.expect("known one-time fixture approval accepted");
        returned.expect("controller returned through normal handover");
        answered.insert(approval);
        eprintln!(
            "live_fixture_approval: known_create=true, one_time=true, controller_returned=true"
        );
    }
}

#[test]
fn fixture_approval_never_infers_remembered_or_unknown_actions() {
    use koru_session_protocol::{ApprovalChoice, ApprovalChoiceId, ApprovalIntent};
    let choice = |intent| ApprovalChoice {
        id: ApprovalChoiceId::mint(),
        intent,
        label: "<label>".into(),
    };
    let allowed = choice(ApprovalIntent::Allow);
    assert_eq!(
        known_fixture_choice(
            "claude",
            "mcp__koru_run__create",
            true,
            std::slice::from_ref(&allowed)
        ),
        Some(allowed.id)
    );
    for (provider, summary, other) in [
        ("grok", "mcp__koru_run__create", true),
        ("claude", "shell", true),
        ("claude", "mcp__koru_run__create", false),
    ] {
        assert!(
            known_fixture_choice(provider, summary, other, std::slice::from_ref(&allowed))
                .is_none()
        );
    }
    assert!(
        known_fixture_choice(
            "claude",
            "mcp__koru_run__create",
            true,
            &[choice(ApprovalIntent::AllowRemembered)]
        )
        .is_none()
    );
    assert!(
        known_fixture_choice(
            "claude",
            "mcp__koru_run__create",
            true,
            &[allowed, choice(ApprovalIntent::Allow)]
        )
        .is_none()
    );
}

fn print_diagnostic(
    agent: &Agent,
    sessions: &SessionClient,
    run: RunId,
    view: &koru_delegate_api::wire::RunView,
) {
    let session = session_for(agent, run);
    let summary = session.and_then(|id| sessions.list().ok()?.into_iter().find(|row| row.id == id));
    let observation = session.and_then(|id| sessions.observe_agent_events(id).ok());
    let mut events = std::collections::BTreeMap::<&str, u64>::new();
    let mut failed_tools = 0_u64;
    if let Some(observation) = &observation {
        for event in &observation.events {
            let kind = match &event.event {
                AgentEvent::TurnStarted { .. } => "turn_started",
                AgentEvent::MessageDelta { .. } => "message_delta",
                AgentEvent::ToolStarted { .. } => "tool_started",
                AgentEvent::ToolCompleted { succeeded, .. } => {
                    failed_tools += u64::from(!succeeded);
                    "tool_completed"
                }
                AgentEvent::ApprovalRequired { .. } => "approval_required",
                AgentEvent::ApprovalResolved { .. } => "approval_resolved",
                AgentEvent::ApprovalExpired { .. } => "approval_expired",
                AgentEvent::WaitingForInput { .. } => "waiting_for_input",
                AgentEvent::TurnCompleted { .. } => "turn_completed",
                AgentEvent::TurnCancelled { .. } => "turn_cancelled",
                AgentEvent::TurnFailed { .. } => "turn_failed",
                AgentEvent::ProviderState { .. } => "provider_state",
                _ => "other",
            };
            *events.entry(kind).or_default() += 1;
        }
    }
    let attention: Vec<_> = match &view.header.status {
        RunStatus::Running { attention, .. } => attention.iter().collect(),
        _ => Vec::new(),
    };
    let value = serde_json::json!({
        "run_state":view.header.status.name(), "attention":attention,
        "managed_state":summary.as_ref().and_then(|row| row.managed.clone()),
        "active_turn":summary.as_ref().is_some_and(|row| row.active_turn_id.is_some()),
        "session_present":summary.is_some(), "events":events,
        "events_truncated":observation.as_ref().map(|value| value.earlier_truncated),
        "registration_count":view.header.artifacts.len(), "failed_tool_count":failed_tools
    });
    eprintln!("live_diagnostic: {value}");
}

#[path = "live_generic/failure.rs"]
mod failure;
