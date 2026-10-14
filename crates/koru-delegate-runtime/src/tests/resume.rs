// SPDX-License-Identifier: MPL-2.0

//! The resume verb: what it refuses, what it fences, which seam it takes,
//! and what the fresh attempt is told (§7.6, §7.2, D10, D48).

use koru_delegate_core::{PriorAttempt, ResumeSeam, RunStatus, StepKind};
use koru_session_protocol::CreationLookup;

use super::*;

/// A run interrupted by its provider exiting mid-turn — the state a person
/// finds a run in when they come back to it.
fn interrupted(label: &str) -> (Harness, RunId) {
    let mut h = Harness::new(label);
    let run = h.running(1);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    h.session_event(run, SessionEvent::Exited { code: None });
    assert_eq!(
        h.status(run),
        RunStatus::Interrupted {
            prior_attempt: PriorAttempt::Ended
        }
    );
    h.sessions.lock().unwrap().calls.clear();
    h.ledger.lock().unwrap().ops.clear();
    (h, run)
}

#[test]
fn only_an_interrupted_run_resumes() {
    let mut h = Harness::new("not-interrupted");
    let run = h.running(1);
    assert_eq!(
        h.orchestrator.resume(run, false).unwrap_err(),
        RunError::NotInterrupted { status: "running" }
    );
    assert_eq!(
        h.orchestrator.resume(run, false).unwrap_err().code(),
        "not_interrupted"
    );
    // And a run that finished is not resumed either.
    h.orchestrator.cancel(run).unwrap();
    h.agent(
        run,
        AgentEvent::TurnCancelled {
            turn_id: turn(1),
            reason: CancelReason::Run,
        },
    );
    assert_eq!(
        h.orchestrator.resume(run, false).unwrap_err(),
        RunError::NotInterrupted {
            status: "cancelled"
        }
    );
    assert!(
        !h.calls().iter().any(|call| matches!(call, Call::Retire(_))),
        "a refused resume fences nothing"
    );
}

/// D48: a source that no longer resolves for anything new refuses the
/// resume — **before** anything is retired, so a person who restores it
/// can still resume the run they had.
#[test]
fn a_source_in_the_trash_refuses_the_resume_and_retires_nothing() {
    for (reason, code) in [
        (SourceRefusalReason::InTrash, "source_in_trash"),
        (SourceRefusalReason::Unavailable, "source_unavailable"),
        (SourceRefusalReason::Unanswered, "source_unanswered"),
    ] {
        let (mut h, run) = interrupted("trashed");
        h.capture.lock().unwrap().refuse_admission = Some(SourceRefusal { item: 0, reason });
        let error = h.orchestrator.resume(run, false).unwrap_err();
        assert_eq!(error.code(), code);
        assert_eq!(
            error,
            RunError::SourceRefused(SourceRefusal { item: 0, reason })
        );
        assert!(
            matches!(h.status(run), RunStatus::Interrupted { .. }),
            "the run is where it was"
        );
        let calls = h.calls();
        assert!(
            !calls
                .iter()
                .any(|call| matches!(call, Call::Retire(_) | Call::Create { .. })),
            "{calls:?}"
        );
        let row = h.attempt(run);
        assert_eq!(row.generation, AttemptGeneration::FIRST);
        assert!(!row.retired);
        // Restored, the same resume goes through.
        h.capture.lock().unwrap().refuse_admission = None;
        assert!(h.orchestrator.resume(run, false).is_ok());
    }
}

/// §7.2, D10: **resume keeps the original pins.** Admission is asked about
/// exactly what the header names, and the fresh attempt materializes the
/// same items — nothing is re-resolved to whatever those documents say now.
#[test]
fn a_resume_keeps_the_original_pins() {
    let (mut h, run) = interrupted("original-pins");
    let pins = h.orchestrator.header(run).unwrap().context;
    h.orchestrator.resume(run, false).unwrap();
    let capture = h.capture.lock().unwrap();
    assert_eq!(capture.admitted, vec![(run, pins.clone())]);
    assert_eq!(h.orchestrator.header(run).unwrap().context, pins);
}

/// The fence, in the order §7.6 states it: the old key is retired **at the
/// host** before the ledger opens the next generation, so a create delayed
/// in the host's blocking lane is refused rather than becoming a second
/// provider.
#[test]
fn the_old_key_is_retired_at_the_host_before_the_next_generation_opens() {
    let (mut h, run) = interrupted("fence-order");
    let started = h.orchestrator.resume(run, false).unwrap();
    assert_eq!(started.generation, AttemptGeneration::FIRST.next());

    let calls = h.calls();
    let retire = calls
        .iter()
        .position(|call| matches!(call, Call::Retire(_)))
        .expect("the host was asked to retire the key");
    let create = calls
        .iter()
        .position(|call| matches!(call, Call::Create { .. }))
        .expect("a fresh session was created");
    assert!(retire < create, "{calls:?}");

    let ops = h.ops();
    let ledger_retire = ops.iter().position(|op| *op == "retire").unwrap();
    let opened = ops.iter().position(|op| *op == "open_attempt").unwrap();
    assert!(ledger_retire < opened, "{ops:?}");
    assert!(h.attempt(run).generation == AttemptGeneration::FIRST.next());
}

/// No adapter in this tree declares native resume — Codex declares
/// `Unsupported` — so this is the path every resume takes today: a fresh
/// session whose first submission is the preamble, what this attempt is,
/// the artifacts registered so far, and the objective. **The timeline says
/// which seam it was.**
#[test]
fn the_recorded_context_fallback_names_the_artifacts_so_far() {
    let (mut h, run) = interrupted("fallback");
    h.registrations.lock().unwrap().next = vec![artifact(1), artifact(2)];
    h.orchestrator.resume(run, false).unwrap();

    assert!(h.steps(run).contains(&StepKind::Resumed {
        via: ResumeSeam::RecordedContext
    }));
    assert!(h.facts().iter().any(|fact| matches!(
        fact,
        RunFact::Resumed {
            via: ResumeSeam::RecordedContext,
            ..
        }
    )));
    assert!(
        h.calls()
            .iter()
            .any(|call| matches!(call, Call::Create { resumes: false, .. })),
        "no token, so nothing is handed to the adapter"
    );

    let submission = h.submissions().pop().expect("the first submission");
    assert!(submission.contains(PREAMBLE_SENTINEL));
    assert!(submission.contains(OBJECTIVE_SENTINEL));
    assert!(submission.contains("interrupted"));
    assert!(submission.contains("Artifacts registered so far (2)"));
    assert!(submission.contains(&artifact(1).artifact.to_string()));
    assert!(submission.contains(&artifact(2).artifact.to_string()));
    assert!(matches!(h.status(run), RunStatus::Running { .. }));
}

/// The adapter's seam, when an adapter has one: the token is handed to the
/// fresh creation, and the timeline says `native` rather than
/// `recorded_context`.
#[test]
fn the_adapter_seam_hands_the_token_to_the_fresh_creation() {
    let (mut h, run) = interrupted("native");
    h.sessions
        .lock()
        .unwrap()
        .resume_tokens
        .insert(session(1), "resume-token".parse().unwrap());
    h.orchestrator.resume(run, false).unwrap();
    assert!(h.steps(run).contains(&StepKind::Resumed {
        via: ResumeSeam::Native
    }));
    assert!(
        h.calls()
            .iter()
            .any(|call| matches!(call, Call::Create { resumes: true, .. })),
        "{:?}",
        h.calls()
    );
    // The seam is asked about the session that ended, after the key is
    // fenced and never before.
    let calls = h.calls();
    let retire = calls
        .iter()
        .position(|call| matches!(call, Call::Retire(_)))
        .unwrap();
    let asked = calls
        .iter()
        .position(|call| matches!(call, Call::ResumeToken(_)))
        .unwrap();
    assert!(retire < asked, "{calls:?}");
}

/// A long run's first submission stays bounded: the most recent
/// [`RESUME_ARTIFACTS_BOUND`] are named and the total is stated.
///
/// [`RESUME_ARTIFACTS_BOUND`]: crate::orchestrator::resume::RESUME_ARTIFACTS_BOUND
#[test]
fn the_artifact_list_the_fallback_names_is_bounded() {
    let (mut h, run) = interrupted("bounded");
    h.registrations.lock().unwrap().next = (0..40).map(artifact).collect();
    h.orchestrator.resume(run, false).unwrap();
    let submission = h.submissions().pop().unwrap();
    assert!(
        submission.contains("Artifacts registered so far: 40 (the most recent 32 listed)"),
        "{submission}"
    );
    let listed = submission
        .lines()
        .filter(|line| line.starts_with("- `"))
        .count();
    assert_eq!(listed, 32);
    // The oldest fell off; the newest is there.
    assert!(!submission.contains(&artifact(0).artifact.to_string()));
    assert!(submission.contains(&artifact(39).artifact.to_string()));
}

/// A run with nothing registered says so rather than saying nothing.
#[test]
fn a_resume_with_no_artifacts_yet_says_none() {
    let (mut h, run) = interrupted("no-artifacts");
    h.orchestrator.resume(run, false).unwrap();
    let submission = h.submissions().pop().unwrap();
    assert!(submission.contains("Artifacts registered so far: none."));
}

/// §7.6's last row, end to end: nothing resumes it on its own, a person's
/// explicit answer is what opens the fresh attempt, and the key is retired
/// first all the same — the confirmation is not what makes it safe.
#[test]
fn a_prior_attempt_that_is_unknown_needs_the_person_and_still_takes_the_fence() {
    let mut h = Harness::new("unknown-prior");
    let run = h.running(1);
    h.agent(run, AgentEvent::TurnStarted { turn_id: turn(1) });
    let mut h = restarted(h, (host(1), CreationLookup::NoEvidence));
    h.orchestrator.restart().unwrap();
    assert_eq!(
        h.status(run),
        RunStatus::Interrupted {
            prior_attempt: PriorAttempt::Unknown
        }
    );
    h.sessions.lock().unwrap().calls.clear();
    assert_eq!(
        h.orchestrator.resume(run, false).unwrap_err(),
        RunError::PriorAttemptUnknown
    );
    assert_eq!(
        h.orchestrator.resume(run, false).unwrap_err().code(),
        "prior_attempt_unknown"
    );
    assert!(
        h.calls().is_empty(),
        "nothing is asked of the host for a resume that will not happen"
    );

    h.orchestrator.resume(run, true).unwrap();
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
    assert_eq!(h.attempt(run).generation, AttemptGeneration::FIRST.next());
}

/// A run whose prior attempt is known to have ended needs no confirming:
/// the evidence is the host's, not the person's.
#[test]
fn a_prior_attempt_known_to_have_ended_needs_no_confirmation() {
    let (mut h, run) = interrupted("ended-prior");
    assert!(h.orchestrator.resume(run, false).is_ok());
}
