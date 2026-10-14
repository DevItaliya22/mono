// SPDX-License-Identifier: MPL-2.0

//! §7.6's table, row by row, and the property that matters most: for every
//! input the function answers, and no answer is a failure.

use koru_delegate_core::{AttemptGeneration, PriorAttempt, ProviderSessionKey, RunAttemptId};

use super::*;
use crate::fixtures::{host, key, run_id, session};

fn row(
    phase: AttemptPhase,
    provider_session: Option<ProviderSessionKey>,
    host_instance: Option<HostInstanceId>,
) -> AttemptRow {
    AttemptRow {
        run: run_id(1),
        attempt: RunAttemptId::from_uuid(crate::fixtures::v4(2)),
        generation: AttemptGeneration::FIRST,
        phase,
        provider_session,
        host_instance,
        retired: false,
        started_at_ms: Some(1_000),
        updated_at_ms: 2_000,
    }
}

fn bound(n: u8) -> AttemptRow {
    row(
        AttemptPhase::Starting,
        Some(ProviderSessionKey {
            idempotency: key(n),
            session: Some(session(n)),
        }),
        Some(host(n)),
    )
}

const ENDED: RestartVerdict = RestartVerdict::Interrupted {
    prior: PriorAttempt::Ended,
    resume: ResumePolicy::MayAutoResume,
};
const UNKNOWN: RestartVerdict = RestartVerdict::Interrupted {
    prior: PriorAttempt::Unknown,
    resume: ResumePolicy::PersonDecides,
};

#[test]
fn the_table_of_7_6_row_by_row() {
    let same = host(1);
    let new = host(2);
    // Known: reattach, never a replacement.
    assert_eq!(
        classify_at_restart(&bound(1), LookupAnswer::Known, same),
        RestartVerdict::Reattach
    );
    assert_eq!(
        classify_at_restart(&bound(1), LookupAnswer::Known, new),
        RestartVerdict::Reattach,
        "a live session is a live session whichever incarnation answers"
    );
    // Creating: a process may exist right now.
    assert_eq!(
        classify_at_restart(&bound(1), LookupAnswer::Creating, same),
        RestartVerdict::WaitThenReask
    );
    // Known-ended: reconcile, do not respawn.
    assert_eq!(
        classify_at_restart(&bound(1), LookupAnswer::KnownEnded, same),
        RestartVerdict::ReconcileEnded
    );
    // No evidence, new incarnation: the host restarted, nothing survived.
    assert_eq!(
        classify_at_restart(&bound(1), LookupAnswer::NoEvidence, new),
        ENDED
    );
    // No evidence, same incarnation: nothing is running and nothing can
    // start under the key once retired — but earlier work may exist.
    assert_eq!(
        classify_at_restart(&bound(1), LookupAnswer::NoEvidence, same),
        UNKNOWN
    );
    // No evidence, never bound: no host was ever asked, so nothing can be
    // running for it on any incarnation.
    let unbound = row(AttemptPhase::Created, None, None);
    assert_eq!(
        classify_at_restart(&unbound, LookupAnswer::NoEvidence, same),
        ENDED
    );
    assert_eq!(
        classify_at_restart(&unbound, LookupAnswer::NoEvidence, new),
        ENDED
    );
}

/// The whole input space: every phase, bound or not, on the same, a new
/// or no incarnation, against every lookup answer. The function always
/// answers; no answer is a failure; the unknown verdict is reached only by
/// §7.6's last row; and an interruption on the same incarnation with
/// evidence of a session never auto-resumes.
#[test]
fn no_input_yields_a_failed_verdict() {
    let hosts = [None, Some(host(1)), Some(host(2))];
    let sessions = [
        None,
        Some(ProviderSessionKey {
            idempotency: key(1),
            session: None,
        }),
        Some(ProviderSessionKey {
            idempotency: key(1),
            session: Some(session(1)),
        }),
    ];
    let mut seen = std::collections::BTreeSet::new();
    for phase in AttemptPhase::ALL {
        for provider_session in sessions {
            for host_instance in hosts {
                let attempt = row(phase, provider_session, host_instance);
                for lookup in LookupAnswer::ALL {
                    let verdict = classify_at_restart(&attempt, lookup, host(1));
                    let name = verdict.name();
                    assert_ne!(name, "failed", "{attempt:?} {lookup:?}");
                    assert!(!name.contains("fail"), "{name}");
                    assert!(
                        verdict.records_as() != Some(AttemptPhase::Ended)
                            || lookup == LookupAnswer::KnownEnded
                    );
                    if let RestartVerdict::Interrupted { prior, resume } = verdict {
                        let same_host_with_session =
                            provider_session.is_some() && host_instance == Some(host(1));
                        assert_eq!(
                            resume == ResumePolicy::PersonDecides,
                            same_host_with_session,
                            "{attempt:?} {lookup:?}"
                        );
                        assert_eq!(prior == PriorAttempt::Unknown, same_host_with_session);
                        assert_eq!(lookup, LookupAnswer::NoEvidence);
                    }
                    seen.insert(name);
                }
            }
        }
    }
    assert_eq!(
        seen,
        std::collections::BTreeSet::from([
            "interrupted",
            "interrupted_prior_unknown",
            "reattach",
            "reconcile_ended",
            "wait_then_reask",
        ]),
        "every verdict is reachable and none is a failure"
    );
}

#[test]
fn a_verdict_says_which_phase_the_ledger_records() {
    assert_eq!(RestartVerdict::Reattach.records_as(), None);
    assert_eq!(RestartVerdict::WaitThenReask.records_as(), None);
    assert_eq!(
        RestartVerdict::ReconcileEnded.records_as(),
        Some(AttemptPhase::Ended)
    );
    assert_eq!(ENDED.records_as(), Some(AttemptPhase::Interrupted));
    assert_eq!(UNKNOWN.records_as(), Some(AttemptPhase::Unknown));
    for verdict in [
        RestartVerdict::Reattach,
        RestartVerdict::WaitThenReask,
        RestartVerdict::ReconcileEnded,
        ENDED,
        UNKNOWN,
    ] {
        assert_ne!(
            verdict.records_as().map(|p| p.token()),
            Some("failed"),
            "the domain has no such phase to record"
        );
    }
}
