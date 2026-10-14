// SPDX-License-Identifier: MPL-2.0

//! The run's authority through the orchestrator (§7.3, D28, ADR-348): what
//! a launch is issued with, what it carries to the provider, what a second
//! attempt does to the first attempt's token, and when the endpoint closes.
//!
//! These are the assertions that would have caught a launch naming a URL
//! with no bearer and an empty allowlist — a provider told it may use no
//! tools, at an endpoint that would refuse it anyway.

use koru_delegate_core::{FailureStage, RunStatus};

use super::*;

/// What the host was handed on the `n`th creation.
fn launched(h: &Harness, n: usize) -> DelegationLaunch {
    h.sessions.lock().unwrap().launched[n].clone()
}

#[test]
fn a_launch_carries_the_six_tools_and_a_bearer_to_the_provider() {
    let mut h = Harness::new("launch-carries");
    let run = h.running(1);

    let launch = launched(&h, 0);
    let tools: Vec<&str> = launch.tools.iter().map(|tool| tool.as_str()).collect();
    assert_eq!(tools, RUN_TOOLS, "the endpoint's six, and only those");
    assert!(
        launch.credential.is_some(),
        "a launch with no bearer reaches an endpoint that answers 401 to every call"
    );
    assert!(launch.mcp_endpoint.as_str().contains(&run.to_storage()));

    // Issued with the run's own workspace, its grant set as the person left
    // it, and the working folder the orchestrator already resolved — never
    // a path this port derived a second time.
    let issued = h.authority.lock().unwrap().issued.clone();
    assert_eq!(issued.len(), 1);
    assert_eq!(issued[0].run, run);
    assert_eq!(issued[0].workspace, workspace());
    assert_eq!(issued[0].grants, BTreeSet::from([workspace()]));
    assert_eq!(
        issued[0].run_workspace,
        koru_delegate_scope::ResolvedScope::of_directory(&h.dir.root.join("project"))
            .unwrap()
            .directory,
        "the working folder is the one the run runs in"
    );
}

/// A fresh attempt gets a fresh generation, which is what retires the token
/// the interrupted attempt's provider may still be holding.
#[test]
fn a_second_attempt_advances_the_generation_and_mints_a_new_bearer() {
    let mut h = Harness::new("second-attempt");
    let run = run_id(1);
    let mut chosen = request(run, h.scope());
    chosen.requested_model =
        Some(koru_delegate_core::ModelName::new("deepseek/deepseek-chat").unwrap());
    h.orchestrator.create(chosen.clone(), workspace()).unwrap();
    h.orchestrator.start(run, StartOrigin::Manual).unwrap();
    assert_eq!(
        launched(&h, 0).model.as_ref().map(|model| model.as_str()),
        Some("deepseek/deepseek-chat")
    );
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.session_event(run, SessionEvent::Exited { code: None });
    h.orchestrator.resume(run, false).unwrap();

    let issued = h.authority.lock().unwrap().issued.clone();
    assert_eq!(issued.len(), 2, "one per attempt");
    assert_eq!(
        launched(&h, 1).model,
        launched(&h, 0).model,
        "resume retains the explicit choice"
    );
    assert_eq!(
        h.orchestrator.header(run).unwrap().requested_model,
        chosen.requested_model
    );
    assert!(
        issued[1].generation > issued[0].generation,
        "{issued:?} — an equal generation would leave the old token live"
    );

    let first = launched(&h, 0).credential.unwrap();
    let second = launched(&h, 1).credential.unwrap();
    assert_ne!(
        first.as_str(),
        second.as_str(),
        "the second attempt reused the first attempt's bearer"
    );
}

/// A run that ends closes its endpoint, whichever way it ended: the record
/// is what every later call reads, so revoking is how a provider that
/// outlived the worker stops being able to publish.
#[test]
fn a_terminal_run_revokes_its_authority_once() {
    for (label, completes) in [("completed", true), ("cancelled", false)] {
        let mut h = Harness::new(label);
        let run = h.running(1);
        if completes {
            h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
            h.registrations.lock().unwrap().next = vec![a_result()];
            h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
            assert_eq!(h.status(run), RunStatus::Completed, "{label}");
        } else {
            // Cancelling outside a turn is terminal at once.
            h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
            h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
            h.orchestrator.cancel(run).unwrap();
            assert_eq!(h.status(run), RunStatus::Cancelled, "{label}");
        }
        assert_eq!(
            h.authority.lock().unwrap().revoked,
            vec![run],
            "{label}: revoked exactly once, when the run ended"
        );
    }
}

/// A run still going has an endpoint that must keep answering.
#[test]
fn a_run_that_is_still_going_keeps_its_authority() {
    let mut h = Harness::new("still-going");
    let run = h.running(1);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    assert!(h.authority.lock().unwrap().revoked.is_empty());
}

/// No authority, no provider. Launching one anyway would leave a run whose
/// every tool call is refused and whose cause is nowhere.
#[test]
fn a_refused_authority_fails_the_start_before_any_provider_exists() {
    let mut h = Harness::new("refused");
    let run = run_id(1);
    h.orchestrator
        .create(request(run, h.scope()), workspace())
        .unwrap();
    h.authority.lock().unwrap().refuse = Some("artifacts_unavailable");

    let started = h.orchestrator.start(run, StartOrigin::Manual).unwrap();
    assert!(started.events.is_none());
    assert_eq!(
        h.status(run),
        RunStatus::Failed {
            stage: FailureStage::Authority
        }
    );
    assert!(
        h.sessions.lock().unwrap().launched.is_empty(),
        "a provider was started without an authority"
    );
}

/// D29's whole point, and the defect this plan closed: a turn that
/// registered something completes the run, with the result in the header
/// and the count on the step. With the endpoint stubbed, every run reached
/// the other arm — finished needing attention, nothing registered.
#[test]
fn a_turn_that_registered_a_result_completes_the_run_with_it() {
    let mut h = Harness::new("registered-completes");
    let run = h.running(1);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.registrations.lock().unwrap().next = vec![a_result()];
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });

    assert_eq!(h.status(run), RunStatus::Completed);
    let header = h.orchestrator.header(run).unwrap();
    assert_eq!(header.artifacts.len(), 1);
    assert_eq!(header.artifacts[0].artifact, a_result().artifact);
    assert!(
        h.steps(run).iter().any(|kind| matches!(
            kind,
            koru_delegate_core::StepKind::TurnCompleted {
                artifacts_registered: 1,
                ..
            }
        )),
        "{:?}",
        h.steps(run)
    );
}

/// One result, as the endpoint would report it back.
fn a_result() -> ResultReference {
    ResultReference {
        artifact: koru_delegate_core::ArtifactVersionId::from_uuid(v4(0x11)),
        held_by: DeviceId::from_uuid(v4(0xe1)),
        title: koru_delegate_core::ArtifactTitle::new("Recommendation").unwrap(),
        size_bytes: 12,
        registered_at_ms: 0,
    }
}
