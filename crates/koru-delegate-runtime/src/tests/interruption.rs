// SPDX-License-Identifier: MPL-2.0

//! One test per failure domain, and the assertion the roadmap names.
//!
//! Something died. Which something is the domain, and each one has a
//! different amount of evidence to work from:
//!
//! - **the delegate worker** died mid-turn and came back to a live session
//!   (`restart::known_reattaches_…`): it reattaches, records a gap only for
//!   what the host says it lost, and never submits a second time;
//! - **the worker and the session host** both died
//!   (`restart::no_evidence_from_a_new_host_…`): a new incarnation proves
//!   nothing survived, and a fresh attempt is safe;
//! - **the session host** died under a live worker: the run reads
//!   `Interrupted` and the worker starts nothing on its own;
//! - **the provider process** exited: `Interrupted` through the §7.5 exit
//!   row, never `Failed`;
//! - **a creation** was still in flight (`restart::creating_…`): the
//!   reservation is waited on and re-asked, bounded;
//! - **nothing is provable** (`restart::no_evidence_from_the_same_host_…`):
//!   `Interrupted`, flagged, never auto-resumed.
//!
//! The tests here are the ones the classification table does not already
//! state: the two domains where nothing is asked of a host, the sweep that
//! proves no restart path ends a run `Failed` merely because a process was
//! not running, and cancellation reaching the process tree while a person
//! holds the lease.

use koru_delegate_core::{ControlState, PriorAttempt, RunStatus, StepKind};
use koru_session_protocol::{CreationLookup, HandoverState, ManagedState};

use super::*;

/// The session host went away under a live worker: the stream closes with
/// nothing to reconcile from.
#[test]
fn the_session_host_dying_interrupts_the_run_and_starts_nothing_on_its_own() {
    let mut h = Harness::new("host-gone");
    let run = h.running(1);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.sessions.lock().unwrap().calls.clear();
    assert_eq!(
        h.orchestrator
            .handle(run, SessionUpdate::Closed { error: None })
            .unwrap(),
        HandleOutcome::Ended
    );
    assert_eq!(
        h.status(run),
        RunStatus::Interrupted {
            prior_attempt: PriorAttempt::Ended
        }
    );
    let calls = h.calls();
    assert!(
        !calls
            .iter()
            .any(|call| matches!(call, Call::Create { .. } | Call::Submit { .. })),
        "the worker respawns no provider of its own accord: {calls:?}"
    );
    // The row stays live and unretired: a person's resume is what retires
    // it, and until then nothing may open a second generation.
    let row = h.attempt(run);
    assert_eq!(row.phase, AttemptPhase::Interrupted);
    assert!(!row.retired);
    assert!(h.orchestrator.claims().is_empty(), "the folder is free");
}

/// The provider process exited. §7.5's exit row, D10's rule.
#[test]
fn the_provider_exiting_interrupts_the_run_and_never_fails_it() {
    for code in [None, Some(0), Some(1)] {
        let mut h = Harness::new("provider-exit");
        let run = h.running(2);
        h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
        assert_eq!(
            h.session_event(run, SessionEvent::Exited { code }),
            HandleOutcome::Ended
        );
        assert_eq!(
            h.status(run),
            RunStatus::Interrupted {
                prior_attempt: PriorAttempt::Ended
            },
            "exit code {code:?}"
        );
        assert!(
            !h.steps(run)
                .iter()
                .any(|kind| matches!(kind, StepKind::Failed { .. })),
            "exit code {code:?}"
        );
    }
}

/// The roadmap's own assertion: **no path marks a run `Failed` merely
/// because a process was not running at startup.**
///
/// The classification function's totality is proved beside it in
/// `koru-delegate-sqlite`; this is the orchestrator over it — every lookup
/// answer, from the same host incarnation and from a fresh one, over a run
/// that was starting and a run that was running, plus an attempt that never
/// bound a key at all. `Failed` belongs to a start that failed or a turn
/// that failed, and nothing here is either.
#[test]
fn no_restart_path_fails_a_run_because_a_process_was_not_running() {
    let answers = || {
        [
            (
                "known",
                CreationLookup::Known {
                    session: summary(ManagedState::Running, Some(turn(1))),
                },
            ),
            ("creating", CreationLookup::Creating),
            (
                "known-ended",
                CreationLookup::KnownEnded {
                    session: summary(ManagedState::Failed, None),
                },
            ),
            ("no-evidence", CreationLookup::NoEvidence),
        ]
    };
    for (name, lookup) in answers() {
        for incarnation in [host(1), host(2)] {
            for mid_turn in [false, true] {
                let mut h = Harness::new("sweep");
                let run = h.running(3);
                if mid_turn {
                    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
                }
                let mut h = restarted(h, (incarnation, lookup.clone()));
                let report = h.orchestrator.restart().unwrap();
                let where_ = format!("{name}/{incarnation:?}/mid_turn={mid_turn}");
                assert!(
                    !matches!(h.status(run), RunStatus::Failed { .. }),
                    "{where_}: {:?}",
                    h.status(run)
                );
                assert!(
                    !h.steps(run)
                        .iter()
                        .any(|kind| matches!(kind, StepKind::Failed { .. })),
                    "{where_}"
                );
                assert!(
                    !h.facts()
                        .iter()
                        .any(|fact| matches!(fact, RunFact::Failed { .. })),
                    "{where_}"
                );
                assert!(report.started.is_empty(), "{where_}: nothing is restarted");
            }
        }
    }
    // And the attempt that never asked a host for anything: no key, no
    // lookup, and still not a failure.
    let mut h = Harness::new("sweep-no-key");
    let run = run_id(4);
    h.orchestrator
        .create(request(run, h.scope()), workspace())
        .unwrap();
    let row = h.orchestrator.ports_mut().store.open_attempt(run).unwrap();
    h.orchestrator
        .ports_mut()
        .store
        .advance(run, row.generation, AttemptPhase::Starting)
        .unwrap();
    {
        let state = h.orchestrator.open(run).unwrap();
        state
            .doc
            .commit(&mut |document| document.set_status(&RunStatus::Starting))
            .unwrap();
    }
    let mut h = restarted(h, (host(1), CreationLookup::NoEvidence));
    h.orchestrator.restart().unwrap();
    assert_eq!(
        h.status(run),
        RunStatus::Interrupted {
            prior_attempt: PriorAttempt::Ended
        }
    );
}

/// §7.6: **cancellation reaches the session-owned process tree including
/// while a person holds control.**
///
/// The lease is the person's, so the turn is not cancelled through a lease
/// this orchestrator gave up; the run ends and the session is ended, which
/// is what takes the provider's process tree with it. The timeline says
/// that control came back to the run before it says the run was cancelled.
#[test]
fn a_run_cancellation_reaches_the_process_tree_while_a_person_holds_control() {
    let mut h = Harness::new("cancel-under-control");
    let run = h.running(5);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.session_event(
        run,
        SessionEvent::HandoverChanged {
            handover: HandoverState::Requested,
        },
    );
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
    h.session_event(
        run,
        SessionEvent::HandoverChanged {
            handover: HandoverState::Taken,
        },
    );
    // The person is mid-turn of their own when the run is cancelled.
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(7) });
    assert!(matches!(
        h.status(run).control(),
        Some(ControlState::Human { .. })
    ));
    h.sessions.lock().unwrap().calls.clear();

    let header = h.orchestrator.cancel(run).unwrap();

    assert_eq!(header.status, RunStatus::Cancelled);
    let calls = h.calls();
    assert!(
        calls.contains(&Call::Terminate(session(1))),
        "the session is ended, so the process tree is: {calls:?}"
    );
    assert!(
        !calls
            .iter()
            .any(|call| matches!(call, Call::CancelTurn { .. })),
        "no turn is cancelled through a lease this orchestrator gave up: {calls:?}"
    );
    let steps = h.steps(run);
    let returned = steps
        .iter()
        .position(|kind| *kind == StepKind::ControlReturned)
        .expect("control came back to the run");
    let cancelled = steps
        .iter()
        .position(|kind| matches!(kind, StepKind::Cancelled { .. }))
        .expect("the run was cancelled");
    assert!(returned < cancelled, "{steps:?}");
    assert!(
        h.facts()
            .iter()
            .any(|fact| matches!(fact, RunFact::ControlReturned { .. })),
    );
    let row = h.attempt(run);
    assert_eq!(row.phase, AttemptPhase::Ended);
    assert!(row.retired);
    assert!(h.orchestrator.claims().is_empty());
}

/// The ordinary cancellation still goes through the turn — and a host that
/// refuses it because the turn ended in the race still ends the run.
#[test]
fn a_cancellation_the_host_refuses_still_ends_the_run() {
    let mut h = Harness::new("cancel-raced");
    let run = h.running(6);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.sessions.lock().unwrap().cancel_refused = Some("turn_not_found");
    let header = h.orchestrator.cancel(run).unwrap();
    assert_eq!(header.status, RunStatus::Cancelled);
    let calls = h.calls();
    assert!(calls.iter().any(|call| matches!(
        call,
        Call::CancelTurn {
            reason: CancelReason::Run,
            ..
        }
    )));
    assert!(calls.contains(&Call::Terminate(session(1))));
}
