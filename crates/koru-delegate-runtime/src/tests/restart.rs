// SPDX-License-Identifier: MPL-2.0

//! §7.6 per lookup answer: reattach, wait and re-ask, reconcile, or
//! `Interrupted` — with *prior attempt unknown* never auto-resumed — and
//! the resume stub that retires the old key before a fresh attempt.

use koru_delegate_core::{Attention, PriorAttempt, RunStatus, StepKind};
use koru_session_protocol::{CreationLookup, ManagedState};

use super::*;

/// A run mid-turn, as the worker that died left it.
fn mid_turn(label: &str) -> (Harness, RunId) {
    let mut h = Harness::new(label);
    let run = h.running(1);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    (h, run)
}

#[test]
fn known_reattaches_records_a_gap_and_never_starts_a_replacement() {
    let mut h = Harness::new("known");
    let run = run_id(1);
    let mut chosen = request(run, h.scope());
    chosen.requested_model =
        Some(koru_delegate_core::ModelName::new("deepseek/deepseek-chat").unwrap());
    h.orchestrator.create(chosen, workspace()).unwrap();
    h.orchestrator.start(run, StartOrigin::Manual).unwrap();
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    let mut h = restarted(
        h,
        (
            host(1),
            CreationLookup::Known {
                session: summary(ManagedState::Running, Some(turn(1))),
            },
        ),
    );
    let report = h.orchestrator.restart().unwrap();
    assert_eq!(report.reattached.len(), 1);
    assert_eq!(
        h.orchestrator.open(run).unwrap().requested_model.as_deref(),
        Some("deepseek/deepseek-chat")
    );
    assert!(report.reattached[0].events.is_some());
    assert!(report.interrupted.is_empty());
    assert!(matches!(h.status(run), RunStatus::Running { .. }));
    assert!(
        !h.steps(run)
            .iter()
            .any(|kind| matches!(kind, StepKind::Gap { .. })),
        "the host kept the whole tail, so nothing was lost to show"
    );
    let calls = h.calls();
    assert!(!calls.iter().any(|call| matches!(call, Call::Create { .. })));
    assert!(calls.contains(&Call::Attach(session(1))));
    assert!(calls.contains(&Call::Subscribe(session(1))));
    assert_eq!(h.attempt(run).phase, AttemptPhase::Running);
    assert_eq!(h.orchestrator.claims().len(), 1, "the claim is kept");
    assert_eq!(report.claims.kept.len(), 1);
    // The reattached stream is the live one: a turn completing now ends
    // the run through the same table.
    h.registrations.lock().unwrap().next = vec![artifact(1)];
    h.agent(run, AgentEvent::TurnCompleted { turn_id: turn(1) });
    assert_eq!(h.status(run), RunStatus::Completed);
}

/// §7.1: *facts that cannot be recovered are shown as gaps*. The host says
/// it evicted the earlier part of the observation, so the run says so too —
/// and never invents how much.
#[test]
fn a_reattach_whose_observation_tail_was_evicted_records_a_gap() {
    let (h, run) = mid_turn("known-truncated");
    let mut h = restarted(
        h,
        (
            host(1),
            CreationLookup::Known {
                session: summary(ManagedState::Running, Some(turn(1))),
            },
        ),
    );
    h.sessions.lock().unwrap().observation.earlier_truncated = true;
    h.orchestrator.restart().unwrap();
    assert!(h.steps(run).contains(&StepKind::Gap { events_lost: None }));
    assert!(
        h.facts()
            .iter()
            .any(|fact| matches!(fact, RunFact::Gap { .. }))
    );
    assert!(
        !h.calls()
            .iter()
            .any(|call| matches!(call, Call::Submit { .. })),
        "a gap is recorded, never filled by asking again"
    );
}

#[test]
fn known_with_the_turn_already_completed_reconciles_from_the_summary() {
    let (h, run) = mid_turn("known-completed");
    let mut h = restarted(
        h,
        (
            host(1),
            CreationLookup::Known {
                session: summary(ManagedState::Completed, None),
            },
        ),
    );
    h.orchestrator.restart().unwrap();
    match h.status(run) {
        RunStatus::Running { attention, .. } => {
            assert!(attention.contains(Attention::FinishedNeedingAttention))
        }
        other => panic!("{other:?}"),
    }
    assert!(h.steps(run).contains(&StepKind::TurnCompleted {
        originator: koru_delegate_core::TurnOriginator::Orchestrator,
        artifacts_registered: 0
    }));
}

#[test]
fn creating_waits_through_the_seam_and_re_asks_until_it_resolves() {
    let (h, run) = mid_turn("creating");
    let mut h = restarted(h, (host(1), CreationLookup::Creating));
    h.sessions.lock().unwrap().lookup_sequence = VecDeque::from([
        (host(1), CreationLookup::Creating),
        (host(1), CreationLookup::Creating),
        (
            host(1),
            CreationLookup::Known {
                session: summary(ManagedState::Running, Some(turn(1))),
            },
        ),
    ]);
    let report = h.orchestrator.restart().unwrap();
    assert_eq!(h.sleeps.lock().unwrap().len(), 2, "two waits, then known");
    assert_eq!(report.reattached.len(), 1);
    assert!(matches!(h.status(run), RunStatus::Running { .. }));
}

#[test]
fn creating_that_never_resolves_is_left_running_with_its_claim_kept() {
    let (h, run) = mid_turn("creating-forever");
    let mut h = restarted(h, (host(1), CreationLookup::Creating));
    let report = h.orchestrator.restart().unwrap();
    assert_eq!(report.unresolved, vec![run]);
    assert_eq!(h.sleeps.lock().unwrap().len(), 3, "the bounded re-asks");
    assert!(
        matches!(h.status(run), RunStatus::Running { .. }),
        "never the nothing-running branch"
    );
    assert_eq!(h.orchestrator.claims().len(), 1);
    assert!(
        h.steps(run)
            .iter()
            .all(|kind| !matches!(kind, StepKind::Interrupted { .. }))
    );
}

#[test]
fn known_ended_reconciles_without_respawning() {
    let (h, run) = mid_turn("known-ended");
    let mut h = restarted(
        h,
        (
            host(1),
            CreationLookup::KnownEnded {
                session: summary(ManagedState::Failed, None),
            },
        ),
    );
    let report = h.orchestrator.restart().unwrap();
    assert_eq!(report.reconciled, vec![run]);
    assert_eq!(
        h.status(run),
        RunStatus::Interrupted {
            prior_attempt: PriorAttempt::Ended
        }
    );
    assert_eq!(h.attempt(run).phase, AttemptPhase::Ended);
    assert!(
        !h.calls()
            .iter()
            .any(|call| matches!(call, Call::Create { .. }))
    );
    assert!(h.orchestrator.claims().is_empty());
    // A completed session reconciles to the completion.
    let (h, run) = mid_turn("known-ended-completed");
    let mut h = restarted(
        h,
        (
            host(1),
            CreationLookup::KnownEnded {
                session: summary(ManagedState::Completed, None),
            },
        ),
    );
    h.registrations.lock().unwrap().next = vec![artifact(1)];
    h.orchestrator.restart().unwrap();
    assert_eq!(h.status(run), RunStatus::Completed);
}

#[test]
fn no_evidence_from_a_new_host_is_interrupted_and_a_fresh_attempt_is_safe() {
    let (h, run) = mid_turn("new-host");
    let mut h = restarted(h, (host(2), CreationLookup::NoEvidence));
    let report = h.orchestrator.restart().unwrap();
    assert_eq!(report.interrupted, vec![run]);
    assert_eq!(
        h.status(run),
        RunStatus::Interrupted {
            prior_attempt: PriorAttempt::Ended
        }
    );
    assert_eq!(h.attempt(run).phase, AttemptPhase::Interrupted);
    assert!(h.orchestrator.claims().is_empty());
    assert!(
        !h.calls()
            .iter()
            .any(|call| matches!(call, Call::Create { .. })),
        "never auto-resumed here"
    );
    // A person resumes: the old key is retired at the host before the
    // fresh attempt, and the fresh attempt is generation two. Nothing is
    // confirmed — a prior attempt known to have ended needs no confirming.
    let started = h.orchestrator.resume(run, false).unwrap();
    assert_eq!(started.generation, AttemptGeneration::FIRST.next());
    assert!(started.events.is_some());
    let calls = h.calls();
    let retire = calls
        .iter()
        .position(|call| matches!(call, Call::Retire(_)))
        .unwrap();
    let create = calls
        .iter()
        .position(|call| matches!(call, Call::Create { .. }))
        .unwrap();
    assert!(retire < create, "{calls:?}");
    assert!(h.steps(run).contains(&StepKind::Resumed {
        via: koru_delegate_core::ResumeSeam::RecordedContext
    }));
    assert!(matches!(h.status(run), RunStatus::Running { .. }));
    assert_eq!(h.attempt(run).generation, AttemptGeneration::FIRST.next());
}

#[test]
fn no_evidence_from_the_same_host_is_prior_attempt_unknown_and_a_person_decides() {
    let (h, run) = mid_turn("same-host");
    let mut h = restarted(h, (host(1), CreationLookup::NoEvidence));
    let report = h.orchestrator.restart().unwrap();
    assert_eq!(report.interrupted, vec![run]);
    assert_eq!(
        h.status(run),
        RunStatus::Interrupted {
            prior_attempt: PriorAttempt::Unknown
        }
    );
    assert_eq!(h.attempt(run).phase, AttemptPhase::Unknown);
    assert!(
        !h.calls()
            .iter()
            .any(|call| matches!(call, Call::Create { .. })),
        "never auto-resumed"
    );
    assert!(h.steps(run).contains(&StepKind::Interrupted {
        prior_attempt: PriorAttempt::Unknown
    }));
    // Nothing resumes it on its own, not even when asked without the
    // person's answer.
    assert_eq!(
        h.orchestrator.resume(run, false).unwrap_err(),
        RunError::PriorAttemptUnknown
    );
    // Told earlier work may exist, the person resumes deliberately.
    let started = h.orchestrator.resume(run, true).unwrap();
    assert!(started.events.is_some());
}

#[test]
fn an_attempt_that_never_asked_a_host_is_interrupted_whichever_host_answers() {
    let mut h = Harness::new("no-key");
    let run = run_id(1);
    h.orchestrator
        .create(request(run, h.scope()), workspace())
        .unwrap();
    // A row opened and advanced to `starting` by a worker that died before
    // binding a key.
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
    let h = restarted(h, (host(1), CreationLookup::NoEvidence));
    let mut h = h;
    let report = h.orchestrator.restart().unwrap();
    assert_eq!(report.interrupted, vec![run]);
    assert_eq!(
        h.status(run),
        RunStatus::Interrupted {
            prior_attempt: PriorAttempt::Ended
        }
    );
    assert!(
        !h.calls().iter().any(|call| matches!(call, Call::Lookup(_))),
        "nothing to ask about"
    );
}

#[test]
fn a_resume_against_a_live_key_is_refused_rather_than_spawning_a_second_provider() {
    let (mut h, run) = mid_turn("live-key");
    h.session_event(run, SessionEvent::Exited { code: None });
    assert!(matches!(h.status(run), RunStatus::Interrupted { .. }));
    let key = h.attempt(run).provider_session.unwrap().idempotency;
    h.sessions.lock().unwrap().lookups.insert(
        key,
        (
            host(1),
            CreationLookup::Known {
                session: summary(ManagedState::Running, Some(turn(1))),
            },
        ),
    );
    h.sessions.lock().unwrap().calls.clear();
    assert_eq!(
        h.orchestrator.resume(run, false).unwrap_err(),
        RunError::AttemptLive
    );
    let calls = h.calls();
    assert!(
        calls.iter().any(|call| matches!(call, Call::Retire(_))),
        "{calls:?}"
    );
    assert!(
        !calls.iter().any(|call| matches!(call, Call::Create { .. })),
        "{calls:?}"
    );
}

#[test]
fn a_controller_found_in_place_at_reattach_is_a_takeover_recorded_at_discovery() {
    // §7.4's crash between release and attach: the worker released the
    // lease for a handover and died before the person's window attached.
    // On recovery a controller exists that this orchestrator did not
    // leave, and the only honest record is the takeover it discovered.
    let (h, run) = mid_turn("takeover-discovered");
    h.sessions.lock().unwrap().attach_fails = true;
    let mut h = restarted(
        h,
        (
            host(1),
            CreationLookup::Known {
                session: summary(ManagedState::Running, Some(turn(1))),
            },
        ),
    );
    let report = h.orchestrator.restart().unwrap();

    assert_eq!(report.reattached.len(), 1, "the attempt is not abandoned");
    assert!(report.unresolved.is_empty());
    assert!(
        h.steps(run)
            .contains(&StepKind::ControlTaken { discovered: true }),
        "marked as discovered, because the moment it happened was never seen"
    );
    assert!(
        !h.steps(run)
            .contains(&StepKind::ControlTaken { discovered: false }),
        "nothing claims to have watched the lease being granted"
    );
    assert!(matches!(
        h.status(run).control(),
        Some(koru_delegate_core::ControlState::Human { .. })
    ));
    assert!(
        h.facts().iter().any(|fact| matches!(
            fact,
            RunFact::ControlTaken {
                discovered: true,
                ..
            }
        )),
        "the fact says the same"
    );
    // Observation stays live: the timeline does not go blind while a
    // person types.
    assert!(h.calls().contains(&Call::Subscribe(session(1))));
    assert!(report.reattached[0].events.is_some());
    // And the run is still running: a discovery is not a terminal state.
    assert!(matches!(h.status(run), RunStatus::Running { .. }));
}

#[test]
fn a_discovered_takeover_does_not_let_an_unwatched_completion_end_the_run() {
    // The same crash, with the provider's turn already finished while
    // nobody watched. An orchestrator turn cannot end a run held by a
    // person (§7.5), so the completion waits for control to come back
    // rather than being applied over their head.
    let (h, run) = mid_turn("takeover-discovered-completed");
    h.sessions.lock().unwrap().attach_fails = true;
    let mut h = restarted(
        h,
        (
            host(1),
            CreationLookup::Known {
                session: summary(ManagedState::Completed, None),
            },
        ),
    );
    h.orchestrator.restart().unwrap();
    assert!(matches!(h.status(run), RunStatus::Running { .. }));
    assert!(!h.steps(run).iter().any(|kind| kind.is_terminal()));
    assert!(
        h.steps(run)
            .contains(&StepKind::ControlTaken { discovered: true })
    );
}

mod approval_recovery;
