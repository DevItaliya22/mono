// SPDX-License-Identifier: MPL-2.0

//! §7.4's fences and D36: a pending takeover blocks submission and fences
//! completion; a takeover-boundary cancellation ends the step and never
//! the job; a run cancellation does; a person's turn never completes the
//! objective.

use koru_delegate_core::{
    Attention, CancelReason, ControlState, RunStatus, StepKind, TurnOriginator,
};
use koru_session_protocol::{ErrorCode, HandoverState};

use super::*;

fn handover(h: &mut Harness, run: RunId, state: HandoverState) -> HandleOutcome {
    h.session_event(run, SessionEvent::HandoverChanged { handover: state })
}

#[test]
fn a_pending_takeover_fences_completion_until_control_returns() {
    let mut h = Harness::new("fence");
    let run = h.running(1);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    // The person asks mid-turn: the request waits for the boundary.
    handover(&mut h, run, HandoverState::Requested);
    assert_eq!(
        h.status(run).control(),
        Some(ControlState::HandoverPending {
            objective_satisfied: false
        })
    );
    assert!(
        !h.calls().contains(&Call::Release(session(1))),
        "not before the boundary"
    );
    // The turn completes the objective — and does not complete the run.
    h.registrations.lock().unwrap().next = vec![artifact(1)];
    assert_eq!(
        h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) }),
        HandleOutcome::Continue
    );
    assert_eq!(
        h.status(run).control(),
        Some(ControlState::HandoverPending {
            objective_satisfied: true
        })
    );
    assert!(!h.steps(run).contains(&StepKind::Completed));
    // The lease was released at the boundary, and no submission happened.
    let calls = h.calls();
    assert!(calls.contains(&Call::Release(session(1))));
    let submissions = calls
        .iter()
        .filter(|call| matches!(call, Call::Submit { .. }))
        .count();
    assert_eq!(submissions, 1);
    // Taken is recorded only when the person holds the lease.
    handover(&mut h, run, HandoverState::Taken);
    assert_eq!(
        h.status(run).control(),
        Some(ControlState::Human {
            objective_satisfied: true
        })
    );
    assert!(
        h.steps(run)
            .contains(&StepKind::ControlTaken { discovered: false })
    );
    // A person's turn is recorded and is not the run's result.
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(7) });
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(7) });
    assert!(h.steps(run).contains(&StepKind::TurnStarted {
        originator: TurnOriginator::Human
    }));
    assert!(!h.status(run).is_terminal());
    // Returned: the orchestrator reacquires, records the return, and the
    // run completes on the objective already satisfied — no second turn.
    assert_eq!(
        handover(&mut h, run, HandoverState::Returned),
        HandleOutcome::Ended
    );
    assert_eq!(h.status(run), RunStatus::Completed);
    let calls = h.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|call| matches!(call, Call::Attach(_)))
            .count(),
        2
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| matches!(call, Call::Submit { .. }))
            .count(),
        1,
        "no orchestrator submission between the handovers"
    );
    let steps = h.steps(run);
    let taken = steps
        .iter()
        .position(|kind| matches!(kind, StepKind::ControlTaken { .. }))
        .unwrap();
    let returned = steps
        .iter()
        .position(|kind| *kind == StepKind::ControlReturned)
        .unwrap();
    let completed = steps
        .iter()
        .position(|kind| *kind == StepKind::Completed)
        .unwrap();
    assert!(taken < returned && returned < completed);
    let header = h.orchestrator.header(run).unwrap();
    assert_eq!(
        header.economics.interventions, 2,
        "a takeover and a human turn"
    );
    assert_eq!(header.economics.human_turns, 1);
}

#[test]
fn a_takeover_boundary_cancellation_ends_the_step_and_never_the_job() {
    let mut h = Harness::new("takeover-cancel");
    let run = h.running(2);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    handover(&mut h, run, HandoverState::Requested);
    // The person cancelled the turn to take control now.
    assert_eq!(
        h.agent(
            run,
            AgentEvent::TurnCancelled {
                turn_id: turn(1),
                reason: koru_session_protocol::CancelReason::TakeoverBoundary,
            }
        ),
        HandleOutcome::Continue
    );
    assert!(matches!(h.status(run), RunStatus::Running { .. }));
    assert!(h.steps(run).contains(&StepKind::TurnCancelled {
        originator: TurnOriginator::Orchestrator,
        reason: CancelReason::TakeoverBoundary
    }));
    assert!(!h.steps(run).iter().any(|kind| kind.is_terminal()));
    assert!(h.calls().contains(&Call::Release(session(1))));
    assert_eq!(h.attempt(run).phase, AttemptPhase::Running);
    // Taken, a person types, returned: the objective was not satisfied, so
    // the orchestrator submits the next turn and the run goes on.
    handover(&mut h, run, HandoverState::Taken);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(8) });
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(8) });
    assert_eq!(
        handover(&mut h, run, HandoverState::Returned),
        HandleOutcome::Continue
    );
    assert_eq!(h.status(run).control(), Some(ControlState::Orchestrator));
    let submissions = h
        .calls()
        .iter()
        .filter(|call| matches!(call, Call::Submit { .. }))
        .count();
    assert_eq!(submissions, 2);
    // And the next orchestrator turn can complete the work.
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(2) });
    h.registrations.lock().unwrap().next = vec![artifact(1)];
    assert_eq!(
        h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(2) }),
        HandleOutcome::Ended
    );
    assert_eq!(h.status(run), RunStatus::Completed);
}

#[test]
fn a_run_cancellation_during_a_turn_terminalizes() {
    let mut h = Harness::new("run-cancel");
    let run = h.running(3);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
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
}

#[test]
fn a_request_with_no_turn_active_releases_at_once() {
    let mut h = Harness::new("idle-request");
    let run = h.running(4);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
    assert!(
        h.status(run)
            .attention()
            .unwrap()
            .contains(Attention::FinishedNeedingAttention)
    );
    handover(&mut h, run, HandoverState::Requested);
    assert!(h.calls().contains(&Call::Release(session(1))));
    handover(&mut h, run, HandoverState::Taken);
    // A person's failed turn is recorded and never terminalizes.
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(9) });
    h.agent(
        run,
        AgentEvent::TurnFailed {
            turn_id: turn(9),
            error: koru_session_protocol::SessionFailure {
                code: ErrorCode::ProviderFailed,
                message: String::new(),
            },
        },
    );
    assert!(matches!(h.status(run), RunStatus::Running { .. }));
    assert!(h.steps(run).contains(&StepKind::TurnFailed {
        originator: TurnOriginator::Human,
        stage: koru_delegate_core::FailureStage::Turn
    }));
}

#[test]
fn a_failed_reattach_records_nothing() {
    let mut h = Harness::new("reattach-fails");
    let run = h.running(5);
    handover(&mut h, run, HandoverState::Requested);
    handover(&mut h, run, HandoverState::Taken);
    h.sessions.lock().unwrap().attach_fails = true;
    handover(&mut h, run, HandoverState::Returned);
    assert!(!h.steps(run).contains(&StepKind::ControlReturned));
    assert!(matches!(
        h.status(run).control(),
        Some(ControlState::Human { .. })
    ));
}

fn approval_event(n: u8) -> AgentEvent {
    AgentEvent::ApprovalRequired {
        turn_id: turn(1),
        approval_id: v4(n).to_string().parse().unwrap(),
        summary: "fixture permission".into(),
        choices: Vec::new(),
    }
}
fn resolve_approval(h: &mut Harness, run: RunId, n: u8) {
    h.agent(
        run,
        AgentEvent::ApprovalResolved {
            turn_id: turn(1),
            approval_id: v4(n).to_string().parse().unwrap(),
            decision: koru_session_protocol::ApprovalDecision::Approve,
            choice: None,
        },
    );
}

#[test]
fn approval_handover_releases_in_both_orders_and_preserves_the_original_turn() {
    for request_first in [false, true] {
        for return_when in 0..3 {
            let mut h = Harness::new("approval-cooperative");
            let run = h.running(1);
            h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
            if request_first {
                handover(&mut h, run, HandoverState::Requested);
            }
            h.agent(run, approval_event(71));
            if !request_first {
                handover(&mut h, run, HandoverState::Requested);
            }
            assert_eq!(
                h.calls()
                    .iter()
                    .filter(|call| **call == Call::Release(session(1)))
                    .count(),
                1
            );
            handover(&mut h, run, HandoverState::Taken);
            if return_when == 0 {
                handover(&mut h, run, HandoverState::Returned);
                assert!(
                    h.status(run)
                        .attention()
                        .unwrap()
                        .contains(Attention::ApprovalRequired)
                );
            }
            resolve_approval(&mut h, run, 71);
            if return_when == 1 {
                handover(&mut h, run, HandoverState::Returned);
            }
            h.registrations.lock().unwrap().next = vec![artifact(1)];
            h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
            if return_when == 2 {
                assert_eq!(
                    h.status(run).control(),
                    Some(ControlState::Human {
                        objective_satisfied: true
                    })
                );
                assert!(!h.status(run).is_terminal());
                handover(&mut h, run, HandoverState::Returned);
            }
            assert_eq!(h.status(run), RunStatus::Completed);
            assert_eq!(
                h.calls()
                    .iter()
                    .filter(|call| matches!(call, Call::Submit { .. }))
                    .count(),
                1,
                "returning the approval lease must never duplicate the original objective"
            );
            assert!(h.steps(run).contains(&StepKind::TurnCompleted {
                originator: TurnOriginator::Orchestrator,
                artifacts_registered: 1,
            }));
            assert!(
                !h.steps(run)
                    .iter()
                    .any(|step| matches!(step, StepKind::TurnCancelled { .. }))
            );
        }
    }
}

#[test]
fn closing_one_of_two_approvals_preserves_attention_and_the_safe_handover() {
    let mut h = Harness::new("two-approvals");
    let run = h.running(1);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.agent(run, approval_event(71));
    h.agent(run, approval_event(72));
    resolve_approval(&mut h, run, 71);
    assert!(
        h.status(run)
            .attention()
            .unwrap()
            .contains(Attention::ApprovalRequired)
    );
    handover(&mut h, run, HandoverState::Requested);
    assert!(h.calls().contains(&Call::Release(session(1))));
    handover(&mut h, run, HandoverState::Taken);
    resolve_approval(&mut h, run, 72);
    assert!(
        !h.status(run)
            .attention()
            .unwrap()
            .contains(Attention::ApprovalRequired)
    );
    handover(&mut h, run, HandoverState::Returned);
    assert_eq!(
        h.calls()
            .iter()
            .filter(|call| matches!(call, Call::Submit { .. }))
            .count(),
        1
    );
}

#[test]
fn an_original_turn_failure_or_run_cancel_still_ends_while_the_approval_lease_is_borrowed() {
    for cancelled in [false, true] {
        let mut h = Harness::new("approval-terminal");
        let run = h.running(1);
        h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
        h.agent(run, approval_event(71));
        handover(&mut h, run, HandoverState::Requested);
        handover(&mut h, run, HandoverState::Taken);
        resolve_approval(&mut h, run, 71);
        let event = if cancelled {
            AgentEvent::TurnCancelled {
                turn_id: turn(1),
                reason: koru_session_protocol::CancelReason::Run,
            }
        } else {
            AgentEvent::TurnFailed {
                turn_id: turn(1),
                error: koru_session_protocol::SessionFailure {
                    code: ErrorCode::ProviderFailed,
                    message: String::new(),
                },
            }
        };
        assert_eq!(h.agent(run, event), HandleOutcome::Ended);
        assert!(h.status(run).is_terminal());
        assert_eq!(
            h.calls()
                .iter()
                .filter(|call| matches!(call, Call::Submit { .. }))
                .count(),
            1
        );
    }
}

#[test]
fn an_immediate_cancel_racing_the_cooperative_release_keeps_its_takeover_reason() {
    let mut h = Harness::new("approval-now-race");
    let run = h.running(1);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.agent(run, approval_event(71));
    handover(&mut h, run, HandoverState::Requested);
    handover(&mut h, run, HandoverState::Taken);
    h.agent(
        run,
        AgentEvent::TurnCancelled {
            turn_id: turn(1),
            reason: koru_session_protocol::CancelReason::TakeoverBoundary,
        },
    );
    assert!(!h.status(run).is_terminal());
    assert!(
        !h.status(run)
            .attention()
            .unwrap()
            .contains(Attention::ApprovalRequired)
    );
    assert!(h.steps(run).contains(&StepKind::TurnCancelled {
        originator: TurnOriginator::Orchestrator,
        reason: CancelReason::TakeoverBoundary,
    }));
    handover(&mut h, run, HandoverState::Returned);
    assert_eq!(
        h.calls()
            .iter()
            .filter(|call| matches!(call, Call::Submit { .. }))
            .count(),
        2,
        "the old turn actually ended, so return can submit the next turn"
    );
}

#[test]
fn a_reply_for_the_wrong_turn_or_a_duplicate_does_not_close_another_permission() {
    let mut h = Harness::new("approval-stale-close");
    let run = h.running(1);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.agent(run, approval_event(71));
    h.agent(
        run,
        AgentEvent::ApprovalResolved {
            turn_id: turn(9),
            approval_id: v4(71).to_string().parse().unwrap(),
            decision: koru_session_protocol::ApprovalDecision::Approve,
            choice: None,
        },
    );
    assert!(
        h.status(run)
            .attention()
            .unwrap()
            .contains(Attention::ApprovalRequired)
    );
    resolve_approval(&mut h, run, 71);
    resolve_approval(&mut h, run, 71);
    assert_eq!(
        h.steps(run)
            .iter()
            .filter(|step| matches!(step, StepKind::ApprovalResolved { .. }))
            .count(),
        1
    );
}

#[test]
fn duplicate_handover_notifications_do_not_acquire_or_submit_twice() {
    let mut h = Harness::new("approval-duplicate-handover");
    let run = h.running(1);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.agent(run, approval_event(71));
    handover(&mut h, run, HandoverState::Requested);
    handover(&mut h, run, HandoverState::Taken);
    handover(&mut h, run, HandoverState::Taken);
    // The session and agent streams are independent: return may be seen
    // before the approval reply and the original turn's completion.
    handover(&mut h, run, HandoverState::Returned);
    let attaches = h
        .calls()
        .iter()
        .filter(|call| matches!(call, Call::Attach(_)))
        .count();
    handover(&mut h, run, HandoverState::Returned);
    assert_eq!(
        h.calls()
            .iter()
            .filter(|call| matches!(call, Call::Attach(_)))
            .count(),
        attaches
    );
    assert_eq!(
        h.calls()
            .iter()
            .filter(|call| matches!(call, Call::Submit { .. }))
            .count(),
        1
    );
    resolve_approval(&mut h, run, 71);
    h.registrations.lock().unwrap().next = vec![artifact(1)];
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
    assert_eq!(
        handover(&mut h, run, HandoverState::Returned),
        HandleOutcome::Ended
    );
    assert_eq!(h.status(run), RunStatus::Completed);
}

#[test]
fn session_handover_can_overtake_the_original_start_and_approval_events() {
    for taken_before_start in [false, true] {
        let mut h = Harness::new("approval-cross-stream-start");
        let run = h.running(1);
        handover(&mut h, run, HandoverState::Requested);
        if taken_before_start {
            handover(&mut h, run, HandoverState::Taken);
        }
        h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
        if !taken_before_start {
            handover(&mut h, run, HandoverState::Taken);
        }
        h.agent(run, approval_event(71));
        assert!(
            !h.calls().contains(&Call::Release(session(1))),
            "a late approval cannot release a lease already held by the person"
        );
        resolve_approval(&mut h, run, 71);
        handover(&mut h, run, HandoverState::Returned);
        h.registrations.lock().unwrap().next = vec![artifact(1)];
        h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
        assert_eq!(h.status(run), RunStatus::Completed);
        assert_eq!(
            h.calls()
                .iter()
                .filter(|call| matches!(call, Call::Submit { .. }))
                .count(),
            1
        );
        assert!(h.steps(run).contains(&StepKind::TurnStarted {
            originator: TurnOriginator::Orchestrator,
        }));
    }
}
