// SPDX-License-Identifier: MPL-2.0

//! §7.5, row by row: which provider event means what to the run, what is
//! written and what is not, D29, and the provider's end.

use koru_delegate_core::{
    ApprovalOutcome, Attention, ControlState, FailureStage, PriorAttempt, RunStatus, StepKind,
    TurnOriginator,
};
use koru_session_protocol::{ApprovalDecision, ApprovalId, AttachmentId, ErrorCode, ToolCallId};

use super::*;

fn approval(n: u8) -> ApprovalId {
    v4(0x70 + n).hyphenated().to_string().parse().unwrap()
}

fn attachment(n: u8) -> AttachmentId {
    v4(0x60 + n).hyphenated().to_string().parse().unwrap()
}

#[test]
fn a_turn_that_registered_a_result_completes_the_run_and_ends_the_session() {
    let mut h = Harness::new("complete");
    let run = h.running(1);
    assert_eq!(
        h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) }),
        HandleOutcome::Continue
    );
    h.registrations.lock().unwrap().next = vec![artifact(1)];
    assert_eq!(
        h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) }),
        HandleOutcome::Ended
    );
    assert_eq!(h.status(run), RunStatus::Completed);
    let steps = h.steps(run);
    assert_eq!(
        steps,
        vec![
            StepKind::RunCreated,
            StepKind::Started,
            StepKind::TurnStarted {
                originator: TurnOriginator::Orchestrator
            },
            StepKind::ArtifactRegistered,
            StepKind::TurnCompleted {
                originator: TurnOriginator::Orchestrator,
                artifacts_registered: 1
            },
            StepKind::Completed,
        ]
    );
    let header = h.orchestrator.header(run).unwrap();
    assert_eq!(header.artifacts.len(), 1);
    assert!(header.economics.elapsed_ms.is_some());
    let row = h.attempt(run);
    assert_eq!(row.phase, AttemptPhase::Ended);
    assert!(row.retired);
    assert!(h.calls().contains(&Call::Terminate(session(1))));
    assert_eq!(h.capture.lock().unwrap().cleaned.len(), 1);
    assert!(h.orchestrator.claims().is_empty());
    assert!(
        h.facts()
            .iter()
            .any(|fact| matches!(fact, RunFact::Completed { artifacts: 1, .. }))
    );
    // The registrations were asked for by this turn.
    let asked = h.registrations.lock().unwrap().asked.clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].2, Some(turn(1)));
}

#[test]
fn a_turn_that_registered_nothing_is_finished_needing_attention_not_a_result() {
    let mut h = Harness::new("d29");
    let run = h.running(2);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    assert_eq!(
        h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) }),
        HandleOutcome::Continue
    );
    match h.status(run) {
        RunStatus::Running { attention, control } => {
            assert!(attention.contains(Attention::FinishedNeedingAttention));
            assert_eq!(control, ControlState::Orchestrator);
        }
        other => panic!("{other:?}"),
    }
    // Not delivered: the session stays for a person to look at, the
    // attempt is still live, the claim still held.
    assert!(!h.calls().contains(&Call::Terminate(session(1))));
    assert_eq!(h.attempt(run).phase, AttemptPhase::Running);
    assert_eq!(h.orchestrator.claims().len(), 1);
    assert!(!h.steps(run).contains(&StepKind::Completed));
}

#[test]
fn every_row_of_the_table_writes_what_it_says_and_nothing_else() {
    let mut h = Harness::new("table");
    let run = h.running(3);
    let t = turn(1);
    h.agent(run, AgentEvent::TurnStarted { turn_id: t });
    // Never a write.
    h.agent(
        run,
        AgentEvent::MessageDelta {
            turn_id: t,
            text: "MESSAGE-DELTA-SENTINEL".into(),
        },
    );
    h.agent(
        run,
        AgentEvent::ToolStarted {
            turn_id: t,
            tool_call_id: ToolCallId::mint(),
            label: "tool".into(),
            input: None,
        },
    );
    let commits_before = h.documents.lock().unwrap().commits;
    // Three completions, one failed: coalesced into one step at the end.
    for succeeded in [true, false, true] {
        h.agent(
            run,
            AgentEvent::ToolCompleted {
                turn_id: t,
                tool_call_id: ToolCallId::mint(),
                succeeded,
                tool: Some(ToolKind::CommandExecution),
                failure: (!succeeded).then_some(ToolFailure::Errored),
                detail: None,
                output: None,
            },
        );
    }
    assert_eq!(
        h.documents.lock().unwrap().commits,
        commits_before,
        "tools coalesce"
    );
    h.agent(
        run,
        AgentEvent::ApprovalRequired {
            turn_id: t,
            approval_id: approval(1),
            summary: "APPROVAL-SUMMARY-SENTINEL".into(),
            choices: Vec::new(),
        },
    );
    assert!(
        h.status(run)
            .attention()
            .unwrap()
            .contains(Attention::ApprovalRequired)
    );
    h.agent(
        run,
        AgentEvent::ApprovalResolved {
            turn_id: t,
            approval_id: approval(1),
            decision: ApprovalDecision::Approve,
            choice: None,
        },
    );
    assert!(
        !h.status(run)
            .attention()
            .unwrap()
            .contains(Attention::ApprovalRequired)
    );
    h.agent(
        run,
        AgentEvent::ApprovalRequired {
            turn_id: t,
            approval_id: approval(2),
            summary: String::new(),
            choices: Vec::new(),
        },
    );
    h.agent(
        run,
        AgentEvent::ApprovalExpired {
            turn_id: t,
            approval_id: approval(2),
        },
    );
    h.agent(run, AgentEvent::WaitingForInput { turn_id: t });
    assert!(
        h.status(run)
            .attention()
            .unwrap()
            .contains(Attention::WaitingForInput)
    );
    h.agent(
        run,
        AgentEvent::AttachmentAccepted {
            turn_id: t,
            attachment_id: attachment(1),
        },
    );
    h.agent(
        run,
        AgentEvent::AttachmentRejected {
            turn_id: t,
            attachment_id: attachment(2),
            code: ErrorCode::InvalidRequest,
        },
    );
    h.agent(
        run,
        AgentEvent::ChildSessionCreated {
            turn_id: t,
            child_session_id: session(9),
        },
    );
    assert!(h.calls().contains(&Call::Terminate(session(9))));
    assert!(
        !h.status(run)
            .attention()
            .unwrap()
            .contains(Attention::ChildSessionDetected)
    );
    h.agent(
        run,
        AgentEvent::ProviderState {
            provider: "codex".parse().unwrap(),
            state: koru_session_protocol::ManagedState::Running,
        },
    );
    h.registrations.lock().unwrap().next = vec![artifact(1)];
    h.agent(run, AgentEvent::TurnCompleted { turn_id: t });
    let steps = h.steps(run);
    assert_eq!(
        steps,
        vec![
            StepKind::RunCreated,
            StepKind::Started,
            StepKind::TurnStarted {
                originator: TurnOriginator::Orchestrator
            },
            StepKind::ApprovalRequired,
            StepKind::ApprovalResolved {
                outcome: ApprovalOutcome::Approved
            },
            StepKind::ApprovalRequired,
            StepKind::ApprovalResolved {
                outcome: ApprovalOutcome::Expired
            },
            StepKind::WaitingForInput,
            StepKind::AttachmentAccepted,
            StepKind::AttachmentRejected,
            StepKind::ChildSessionDetected,
            StepKind::ToolsCompleted {
                calls: 3,
                failed: 1,
                failures: breakdown(&[(ToolKind::CommandExecution, ToolFailure::Errored, 1)])
            },
            StepKind::ArtifactRegistered,
            StepKind::TurnCompleted {
                originator: TurnOriginator::Orchestrator,
                artifacts_registered: 1
            },
            StepKind::Completed,
        ]
    );
    let header = h.orchestrator.header(run).unwrap();
    assert_eq!(header.economics.tool_calls, 3);
    assert_eq!(header.economics.orchestrator_turns, 1);
}

#[test]
fn tool_completions_are_bounded_per_step() {
    let mut h = Harness::new("tools-bound");
    let run = h.running(4);
    let t = turn(1);
    h.agent(run, AgentEvent::TurnStarted { turn_id: t });
    for _ in 0..(crate::TOOL_STEP_BOUND + 5) {
        h.agent(
            run,
            AgentEvent::ToolCompleted {
                turn_id: t,
                tool_call_id: ToolCallId::mint(),
                succeeded: true,
                tool: Some(ToolKind::WebSearch),
                failure: None,
                detail: None,
                output: None,
            },
        );
    }
    h.agent(run, AgentEvent::TurnCompleted { turn_id: t });
    let tool_steps: Vec<_> = h
        .steps(run)
        .into_iter()
        .filter(|kind| matches!(kind, StepKind::ToolsCompleted { .. }))
        .collect();
    assert_eq!(
        tool_steps,
        vec![
            StepKind::ToolsCompleted {
                calls: crate::TOOL_STEP_BOUND,
                failed: 0,
                failures: ToolFailures::default()
            },
            StepKind::ToolsCompleted {
                calls: 5,
                failed: 0,
                failures: ToolFailures::default()
            },
        ]
    );
}

#[test]
fn a_failed_orchestrator_turn_fails_the_run_with_the_turn_stage() {
    let mut h = Harness::new("turn-failed");
    let run = h.running(5);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    assert_eq!(
        h.agent(
            run,
            AgentEvent::TurnFailed {
                turn_id: turn(1),
                error: koru_session_protocol::SessionFailure {
                    code: ErrorCode::ProviderFailed,
                    message: "PROVIDER-MESSAGE-SENTINEL".into(),
                },
            }
        ),
        HandleOutcome::Ended
    );
    assert_eq!(
        h.status(run),
        RunStatus::Failed {
            stage: FailureStage::Turn
        }
    );
    assert!(h.attempt(run).retired);
    assert_eq!(
        h.orchestrator.header(run).unwrap().failure_reason,
        Some(koru_delegate_core::RunFailureReason::ProviderFailed)
    );
    let steps = h.steps(run);
    assert_eq!(
        steps
            .iter()
            .filter(|step| matches!(step, StepKind::Failed { .. }))
            .count(),
        1
    );
    assert!(matches!(
        steps.last(),
        Some(StepKind::Failed {
            reason: Some(koru_delegate_core::RunFailureReason::ProviderFailed),
            ..
        })
    ));
    h.agent(
        run,
        AgentEvent::TurnFailed {
            turn_id: turn(1),
            error: koru_session_protocol::SessionFailure {
                code: ErrorCode::ProviderUnavailable,
                message: "LATE-SECRET".into(),
            },
        },
    );
    assert_eq!(h.steps(run), steps);
    assert_eq!(
        h.orchestrator.header(run).unwrap().failure_reason,
        Some(koru_delegate_core::RunFailureReason::ProviderFailed)
    );
}

#[test]
fn the_providers_end_interrupts_the_run_and_never_fails_it() {
    let mut h = Harness::new("provider-exit");
    let run = h.running(6);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    assert_eq!(
        h.session_event(run, SessionEvent::Exited { code: Some(1) }),
        HandleOutcome::Ended
    );
    assert_eq!(
        h.status(run),
        RunStatus::Interrupted {
            prior_attempt: PriorAttempt::Ended
        }
    );
    let row = h.attempt(run);
    assert_eq!(row.phase, AttemptPhase::Interrupted);
    assert!(!row.retired, "a person's resume retires it");
    assert!(h.steps(run).contains(&StepKind::Interrupted {
        prior_attempt: PriorAttempt::Ended
    }));
    assert!(h.orchestrator.claims().is_empty());
    // A closed stream says the same thing.
    let mut h = Harness::new("stream-closed");
    let run = h.running(7);
    assert_eq!(
        h.orchestrator
            .handle(run, SessionUpdate::Closed { error: None })
            .unwrap(),
        HandleOutcome::Ended
    );
    assert!(matches!(h.status(run), RunStatus::Interrupted { .. }));
}

#[test]
fn a_gap_in_the_stream_is_recorded_as_a_gap() {
    let mut h = Harness::new("gap");
    let run = h.running(8);
    h.orchestrator
        .handle(
            run,
            SessionUpdate::AgentGap {
                expected: StreamSequence::new(2).unwrap(),
                actual: StreamSequence::new(9).unwrap(),
            },
        )
        .unwrap();
    assert!(h.steps(run).contains(&StepKind::Gap { events_lost: None }));
}

#[test]
fn a_cancel_during_a_turn_reaches_the_session_and_ends_on_the_cancelled_turn() {
    let mut h = Harness::new("cancel-turn");
    let run = h.running(9);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    let header = h.orchestrator.cancel(run).unwrap();
    assert!(
        matches!(header.status, RunStatus::Running { .. }),
        "not yet"
    );
    assert!(h.calls().contains(&Call::CancelTurn {
        session: session(1),
        turn: turn(1),
        reason: koru_session_protocol::CancelReason::Run
    }));
    assert_eq!(
        h.agent(
            run,
            AgentEvent::TurnCancelled {
                turn_id: turn(1),
                reason: koru_session_protocol::CancelReason::Run,
            }
        ),
        HandleOutcome::Ended
    );
    assert_eq!(h.status(run), RunStatus::Cancelled);
    assert!(h.steps(run).contains(&StepKind::Cancelled {
        reason: koru_delegate_core::CancelReason::Run
    }));
    assert!(h.attempt(run).retired);
}

#[test]
fn a_cancel_outside_a_turn_is_cancelled_at_once_and_ends_the_session() {
    let mut h = Harness::new("cancel-idle");
    let run = h.running(10);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
    let header = h.orchestrator.cancel(run).unwrap();
    assert_eq!(header.status, RunStatus::Cancelled);
    assert!(h.calls().contains(&Call::Terminate(session(1))));
    assert!(h.attempt(run).retired);
}

#[test]
fn events_after_the_end_touch_nothing_but_usage() {
    let mut h = Harness::new("after-end");
    let run = h.running(11);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.registrations.lock().unwrap().next = vec![artifact(1)];
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
    let steps = h.steps(run).len();
    assert_eq!(
        h.agent(run, AgentEvent::TurnStarted { turn_id: turn(2) }),
        HandleOutcome::Ended
    );
    assert_eq!(h.steps(run).len(), steps);
    assert_eq!(h.status(run), RunStatus::Completed);
}

// ------------------------------ which tools failed, folded into the step

/// **The fold, end to end.** Six calls, two failed: the step says so, and
/// under it says which tool and how — including the one the provider
/// described in a way the adapter could not place, and the one an older
/// peer reported with neither field.
#[test]
fn the_coalesced_step_says_which_tools_failed_and_how() {
    let mut h = Harness::new("tool-failures");
    let run = h.running(1);
    let t = turn(1);
    h.agent(run, AgentEvent::TurnStarted { turn_id: t });
    let calls: [(bool, Option<ToolKind>, Option<ToolFailure>); 6] = [
        (true, Some(ToolKind::CommandExecution), None),
        (
            false,
            Some(ToolKind::CommandExecution),
            Some(ToolFailure::Errored),
        ),
        (true, Some(ToolKind::FileChange), None),
        (
            false,
            Some(ToolKind::CommandExecution),
            Some(ToolFailure::Errored),
        ),
        // The provider failed it in a way the adapter could not place.
        (
            false,
            Some(ToolKind::McpTool),
            Some(ToolFailure::Unclassified),
        ),
        // An older peer: no tool, no class, still a failure.
        (false, None, None),
    ];
    for (succeeded, tool, failure) in calls {
        h.agent(
            run,
            AgentEvent::ToolCompleted {
                turn_id: t,
                tool_call_id: ToolCallId::mint(),
                succeeded,
                tool,
                failure,
                detail: None,
                output: None,
            },
        );
    }
    h.agent(run, AgentEvent::TurnCompleted { turn_id: t });
    let step = h
        .steps(run)
        .into_iter()
        .find(|kind| matches!(kind, StepKind::ToolsCompleted { .. }))
        .expect("a tool step");
    let StepKind::ToolsCompleted {
        calls,
        failed,
        failures,
    } = step
    else {
        unreachable!()
    };
    assert_eq!((calls, failed), (6, 4));
    assert_eq!(
        failures.iter().collect::<Vec<_>>(),
        breakdown(&[
            (ToolKind::CommandExecution, ToolFailure::Errored, 2),
            (ToolKind::McpTool, ToolFailure::Unclassified, 1),
            (ToolKind::Other, ToolFailure::Unclassified, 1),
        ])
        .iter()
        .collect::<Vec<_>>()
    );
    // **The parts add up to the whole.** Whatever a provider said or did
    // not say, the breakdown accounts for every failure the count claims.
    assert_eq!(failures.total(), failed);
}

/// A turn in which nothing failed writes the step it always wrote, with
/// nothing under it to read.
#[test]
fn a_turn_with_no_failures_records_no_breakdown() {
    let mut h = Harness::new("tool-no-failures");
    let run = h.running(2);
    let t = turn(1);
    h.agent(run, AgentEvent::TurnStarted { turn_id: t });
    for _ in 0..3 {
        h.agent(
            run,
            AgentEvent::ToolCompleted {
                turn_id: t,
                tool_call_id: ToolCallId::mint(),
                succeeded: true,
                tool: Some(ToolKind::WebSearch),
                failure: None,
                detail: None,
                output: None,
            },
        );
    }
    h.agent(run, AgentEvent::TurnCompleted { turn_id: t });
    assert!(h.steps(run).contains(&StepKind::ToolsCompleted {
        calls: 3,
        failed: 0,
        failures: ToolFailures::default(),
    }));
}

/// The breakdown is per step, not per turn: a turn that runs past the tool
/// bound writes two steps, and neither borrows the other's failures.
#[test]
fn each_coalesced_step_carries_only_its_own_failures() {
    let mut h = Harness::new("tool-failures-bound");
    let run = h.running(3);
    let t = turn(1);
    h.agent(run, AgentEvent::TurnStarted { turn_id: t });
    for index in 0..(crate::TOOL_STEP_BOUND + 2) {
        let failed = index == 0;
        h.agent(
            run,
            AgentEvent::ToolCompleted {
                turn_id: t,
                tool_call_id: ToolCallId::mint(),
                succeeded: !failed,
                tool: Some(ToolKind::FileChange),
                failure: failed.then_some(ToolFailure::Denied),
                detail: None,
                output: None,
            },
        );
    }
    h.agent(run, AgentEvent::TurnCompleted { turn_id: t });
    let steps: Vec<_> = h
        .steps(run)
        .into_iter()
        .filter(|kind| matches!(kind, StepKind::ToolsCompleted { .. }))
        .collect();
    assert_eq!(
        steps,
        vec![
            StepKind::ToolsCompleted {
                calls: crate::TOOL_STEP_BOUND,
                failed: 1,
                failures: breakdown(&[(ToolKind::FileChange, ToolFailure::Denied, 1)]),
            },
            StepKind::ToolsCompleted {
                calls: 2,
                failed: 0,
                failures: ToolFailures::default(),
            },
        ]
    );
}
