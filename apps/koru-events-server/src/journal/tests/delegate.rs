// SPDX-License-Identifier: MPL-2.0

//! A delegated run's rows through the journal itself (P1.8): the
//! per-subject read matches one Run document, a re-publication dedupes and
//! an amendment does not, pruning past the domain window never reuses a
//! sequence, and — `Never` policy — nothing reaches the telemetry outbox.
//!
//! These extend `append_dedupes_by_event_id`,
//! `pruning_never_reuses_a_sequence` and `never_policy_events_never_reach_the_outbox`
//! with the run's own rows rather than restating them: the mechanism is the
//! journal's, and this is the proof that the delegate rows ride it.

use koru_delegate_core::{
    AttemptGeneration, DocumentId, RunAttemptId, RunId, TurnIdentity, TurnOriginator, WorkspaceId,
};
use koru_delegate_events::{
    CachedInputRelationship, RunChain, RunCreated, RunRef, RunStarted, Stamp, TurnStarted,
    TurnUsage,
};
use koru_events::SubjectKind;

use super::*;

fn v4(n: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 | n)
}

fn stamp(n: u128) -> Stamp {
    Stamp {
        event_id: EventId(v4(0xdd00 + n)),
        occurred_at_ms: T0 + n as i64,
    }
}

fn run_ref() -> RunRef {
    RunRef {
        run: RunId::from_uuid(v4(0x1)),
        document: DocumentId::from_uuid(v4(0x2)),
        workspace: WorkspaceId::from_uuid(v4(0x3)),
    }
}

fn attempt() -> RunAttemptId {
    RunAttemptId::from_uuid(v4(0x4))
}

fn chain() -> (Envelope, RunChain) {
    RunChain::created(
        &ctx(),
        run_ref(),
        stamp(0),
        RunCreated {
            requested_by: koru_delegate_core::ActorRef::Orchestrator { run: run_ref().run },
            agent: koru_delegate_events::DelegatedAgent::Claude,
            scope: koru_delegate_events::ScopeKind::Temporary,
            context_items: 0,
        },
    )
}

fn started(chain: &RunChain) -> Envelope {
    chain.started(
        stamp(1),
        RunStarted {
            attempt: attempt(),
            generation: AttemptGeneration::FIRST,
            session: v4(0x5).hyphenated().to_string().parse().unwrap(),
        },
    )
}

fn usage(chain: &RunChain, stamp: Stamp, input: u64, revision: u32) -> Envelope {
    chain.turn_usage(
        stamp,
        &TurnUsage {
            fact: koru_delegate_core::UsageFact {
                identity: koru_delegate_core::UsageFactId::from_uuid(v4(0x6)),
                attempt: attempt(),
                turn: v4(0x7).hyphenated().to_string().parse().unwrap(),
                revision: koru_delegate_core::AmendmentRevision(revision),
                input: Some(input),
                output: None,
                cached_input: None,
                model: koru_delegate_core::ModelRef::from_uuid(v4(0x8)),
                completeness: koru_delegate_core::Completeness::Complete,
            },
            cached_relationship: CachedInputRelationship::Undeclared,
        },
    )
}

fn subject_names(journal: &Journal) -> Vec<String> {
    journal
        .read_subject(
            SubjectKind::Document,
            &run_ref().document.to_storage(),
            0,
            100,
        )
        .unwrap()
        .events
        .iter()
        .map(|stored| stored.envelope.name.as_str().to_string())
        .collect()
}

#[test]
fn a_republished_row_is_one_row_and_an_amendment_is_two() {
    let mut journal = Journal::in_memory().unwrap();
    let (created, chain) = chain();
    let started = started(&chain);
    let first = journal.append(&[created, started.clone()], T0).unwrap();
    assert_eq!((first.accepted, first.duplicates), (2, 0));

    // The pending list re-publishes `started` under the same id.
    let again = journal
        .append(std::slice::from_ref(&started), T0 + 5)
        .unwrap();
    assert_eq!((again.accepted, again.duplicates), (0, 1));
    assert_eq!(journal.last_seq().unwrap(), 2);

    // A usage fact, retried, then corrected under a fresh id.
    let fact = usage(&chain, stamp(2), 100, 0);
    journal.append(std::slice::from_ref(&fact), T0).unwrap();
    let retried = journal.append(std::slice::from_ref(&fact), T0).unwrap();
    assert_eq!((retried.accepted, retried.duplicates), (0, 1));
    let corrected = usage(&chain, Stamp::fresh(T0 + 10), 140, 1);
    let accepted = journal.append(&[corrected], T0).unwrap();
    assert_eq!((accepted.accepted, accepted.duplicates), (1, 0));

    assert_eq!(
        subject_names(&journal),
        [
            "delegate.run.created",
            "delegate.run.started",
            "delegate.turn.usage",
            "delegate.turn.usage",
        ]
    );
    // Four rows, in publication order, the correction after the retry. The
    // exact high-water mark is SQLite's (an ignored insert may or may not
    // advance the AUTOINCREMENT counter); what matters is that a retry made
    // no row and every row has its own sequence.
    let page = journal
        .read_subject(
            SubjectKind::Document,
            &run_ref().document.to_storage(),
            0,
            100,
        )
        .unwrap();
    assert!(page.events.windows(2).all(|pair| pair[0].seq < pair[1].seq));
    assert!(journal.last_seq().unwrap() >= page.events[3].seq);
}

#[test]
fn pruning_a_run_past_the_domain_window_never_reuses_its_sequence() {
    let mut journal = Journal::in_memory().unwrap();
    let (created, chain) = chain();
    journal
        .append(
            &[
                created,
                started(&chain),
                chain.turn_started(
                    stamp(3),
                    TurnStarted {
                        turn: TurnIdentity {
                            attempt: attempt(),
                            turn: v4(0x7).hyphenated().to_string().parse().unwrap(),
                            originator: TurnOriginator::Orchestrator,
                        },
                    },
                ),
            ],
            T0,
        )
        .unwrap();
    assert_eq!(journal.last_seq().unwrap(), 3);

    // A day past the domain window: the write path's sweep empties the run's
    // history from the journal — the Run document keeps it (D32) — and the
    // AUTOINCREMENT high-water mark survives.
    journal
        .prune_now(T0 + (RETENTION_DAYS as i64 + 1) * 86_400_000)
        .unwrap();
    assert!(subject_names(&journal).is_empty());

    let late = chain.run_rated(
        stamp(4),
        koru_delegate_events::RunRated {
            rated_by: koru_delegate_core::ActorRef::Orchestrator { run: run_ref().run },
            rating: koru_delegate_core::Rating::Good,
            has_note: false,
        },
    );
    journal.append(&[late], T0).unwrap();
    let page = journal
        .read_subject(
            SubjectKind::Document,
            &run_ref().document.to_storage(),
            0,
            100,
        )
        .unwrap();
    assert_eq!(page.events.len(), 1);
    assert!(
        page.events[0].seq > 3,
        "a pruned seq must never come back; got {}",
        page.events[0].seq
    );
}

/// D41: consent granted, and still no delegate row reaches the outbox.
#[test]
fn delegate_rows_never_reach_the_outbox() {
    let mut journal = Journal::in_memory().unwrap();
    journal.set_consent(true, "consent-1").unwrap();
    let (created, chain) = chain();
    journal
        .append(
            &[created, started(&chain), usage(&chain, stamp(5), 1, 0)],
            T0,
        )
        .unwrap();
    assert_eq!(journal.status().unwrap().domain_events, 3);
    assert!(journal.telemetry_pending(100).unwrap().is_empty());
}
