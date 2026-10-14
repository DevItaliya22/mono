// SPDX-License-Identifier: MPL-2.0

//! Generations, the closed phase ladder, the session binding, and §7.6's
//! fence: a fresh attempt opens only after the one before it is retired.

use koru_delegate_core::{AttemptGeneration, ProviderSessionKey, StaleAttempt};

use super::*;
use crate::{
    WriteOutcome,
    fixtures::{
        Root, created_run, host, key, request, reserved, run_id, running_attempt, session,
        workspace,
    },
};

#[test]
fn an_attempt_opens_only_for_a_run_that_exists() {
    let root = Root::new();
    let mut db = root.open();
    assert_eq!(db.open_attempt(run_id(1)), Err(LedgerError::UnknownRun));
    db.record_intent(&request(1), workspace(0xa2)).unwrap();
    assert_eq!(
        db.open_attempt(run_id(1)),
        Err(LedgerError::RunNotCreated),
        "a request is not a run, and nothing starts a provider for one"
    );
    db.abandon_intent(run_id(1)).unwrap();
    assert_eq!(db.open_attempt(run_id(1)), Err(LedgerError::RunNotCreated));

    let run = created_run(&mut db, 2);
    let opened = db.open_attempt(run).unwrap();
    assert_eq!(opened.generation, AttemptGeneration::FIRST);
    assert_eq!(opened.phase, AttemptPhase::Created);
    assert!(!opened.retired);
    assert!(opened.is_live());
    assert_eq!(opened.provider_session, None);
    assert_eq!(opened.started_at_ms, None);
    assert_eq!(db.current_attempt(run).unwrap(), Some(opened));
    assert_eq!(db.attempts(run).unwrap(), vec![opened]);
    assert_eq!(db.current_attempt(run_id(1)).unwrap(), None);
}

#[test]
fn the_phase_ladder_is_walked_one_rung_at_a_time_and_stamps_the_start() {
    let root = Root::new();
    let mut db = root.open();
    let run = created_run(&mut db, 1);
    let g = db.open_attempt(run).unwrap().generation;
    assert_eq!(
        db.advance(run, g, AttemptPhase::Running),
        Err(LedgerError::IllegalAttemptTransition {
            from: AttemptPhase::Created,
            to: AttemptPhase::Running
        })
    );
    root.tick(10);
    let starting = db.advance(run, g, AttemptPhase::Starting).unwrap();
    assert_eq!(starting.phase, AttemptPhase::Starting);
    assert_eq!(starting.started_at_ms, Some(root.now_ms()));
    root.tick(10);
    let again = db.advance(run, g, AttemptPhase::Starting).unwrap();
    assert_eq!(
        again, starting,
        "the same phase is already applied, untouched"
    );
    let running = db.advance(run, g, AttemptPhase::Running).unwrap();
    assert_eq!(
        running.started_at_ms, starting.started_at_ms,
        "the start is stamped once"
    );
    assert_eq!(running.updated_at_ms, root.now_ms());
    assert_eq!(
        db.advance(run, g, AttemptPhase::Starting),
        Err(LedgerError::IllegalAttemptTransition {
            from: AttemptPhase::Running,
            to: AttemptPhase::Starting
        })
    );
    let ended = db.advance(run, g, AttemptPhase::Ended).unwrap();
    assert!(!ended.is_live());
    for to in AttemptPhase::ALL
        .into_iter()
        .filter(|p| *p != AttemptPhase::Ended)
    {
        assert_eq!(
            db.advance(run, g, to),
            Err(LedgerError::IllegalAttemptTransition {
                from: AttemptPhase::Ended,
                to
            })
        );
    }
}

#[test]
fn only_the_current_generation_may_write() {
    let root = Root::new();
    let mut db = root.open();
    let run = created_run(&mut db, 1);
    assert_eq!(
        db.advance(run, AttemptGeneration::FIRST, AttemptPhase::Starting),
        Err(LedgerError::NoAttempt)
    );
    let g = db.open_attempt(run).unwrap().generation;
    let stale = LedgerError::StaleAttempt(StaleAttempt {
        current: g,
        offered: g.next(),
    });
    assert_eq!(
        db.advance(run, g.next(), AttemptPhase::Starting),
        Err(stale.clone()),
        "a newer generation the caller invented is refused like an older one"
    );
    assert_eq!(
        db.bind_session(run, g.next(), reserved(1), host(1)),
        Err(stale.clone())
    );
    assert_eq!(db.retire(run, g.next()), Err(stale));
}

#[test]
fn a_session_binds_once_to_one_attempt() {
    let root = Root::new();
    let mut db = root.open();
    let run = created_run(&mut db, 1);
    let g = db.open_attempt(run).unwrap().generation;
    db.advance(run, g, AttemptPhase::Starting).unwrap();

    // The key, reserved before the host is asked.
    let bound = db.bind_session(run, g, reserved(1), host(1)).unwrap();
    assert_eq!(bound.provider_session, Some(reserved(1)));
    assert_eq!(bound.host_instance, Some(host(1)));
    // Rebinding the same facts is harmless.
    assert_eq!(
        db.bind_session(run, g, reserved(1), host(1)).unwrap(),
        bound
    );
    // The session id, once the host answered.
    let answered = ProviderSessionKey {
        idempotency: key(1),
        session: Some(session(2)),
    };
    let bound = db.bind_session(run, g, answered, host(1)).unwrap();
    assert_eq!(bound.provider_session, Some(answered));
    // The key alone again does not forget the session.
    let kept = db.bind_session(run, g, reserved(1), host(1)).unwrap();
    assert_eq!(kept.provider_session, Some(answered));

    // A different key, session or host is a conflict.
    assert_eq!(
        db.bind_session(run, g, reserved(9), host(1)),
        Err(LedgerError::SessionKeyConflict)
    );
    assert_eq!(
        db.bind_session(
            run,
            g,
            ProviderSessionKey {
                idempotency: key(1),
                session: Some(session(3)),
            },
            host(1)
        ),
        Err(LedgerError::SessionKeyConflict)
    );
    assert_eq!(
        db.bind_session(run, g, answered, host(9)),
        Err(LedgerError::SessionKeyConflict)
    );

    // A key names one attempt, ever: another run cannot take it.
    let other = created_run(&mut db, 2);
    let og = db.open_attempt(other).unwrap().generation;
    assert_eq!(
        db.bind_session(other, og, reserved(1), host(1)),
        Err(LedgerError::SessionKeyConflict)
    );
    // And a terminal attempt binds nothing.
    db.advance(run, g, AttemptPhase::Ended).unwrap();
    assert_eq!(
        db.bind_session(run, g, answered, host(1)),
        Err(LedgerError::AttemptEnded {
            phase: AttemptPhase::Ended
        })
    );
}

/// §7.6's fence: retiring is what authorises a fresh attempt, only a
/// terminal attempt retires, and the retired key stays taken.
#[test]
fn a_fresh_attempt_opens_only_once_the_last_one_is_retired() {
    let root = Root::new();
    let mut db = root.open();
    let run = created_run(&mut db, 1);
    let first = running_attempt(&mut db, run, 1);
    let g = first.generation;
    assert_eq!(
        db.open_attempt(run),
        Err(LedgerError::AttemptLive { generation: g })
    );
    assert_eq!(
        db.retire(run, g),
        Err(LedgerError::AttemptNotEnded {
            phase: AttemptPhase::Running
        }),
        "a read alone cannot exclude the future; the attempt ends first"
    );
    db.advance(run, g, AttemptPhase::Unknown).unwrap();
    assert_eq!(
        db.open_attempt(run),
        Err(LedgerError::AttemptLive { generation: g }),
        "ended is not retired"
    );
    let retired = db.retire(run, g).unwrap();
    assert!(retired.retired);
    assert_eq!(
        db.retire(run, g).unwrap(),
        retired,
        "retiring twice is harmless"
    );
    assert_eq!(
        db.advance(run, g, AttemptPhase::Ended),
        Err(LedgerError::AttemptRetired { generation: g })
    );

    let second = db.open_attempt(run).unwrap();
    assert_eq!(second.generation, g.next());
    assert_ne!(second.attempt, first.attempt, "a resume is a fresh attempt");
    assert_eq!(
        second.provider_session, None,
        "nothing of the old session carries over"
    );
    assert_eq!(
        db.current_attempt(run).unwrap().unwrap().generation,
        g.next()
    );
    assert_eq!(db.attempts(run).unwrap().len(), 2);
    // The retired key is still taken: a delayed create under it cannot
    // become the fresh attempt's session.
    db.advance(run, g.next(), AttemptPhase::Starting).unwrap();
    assert_eq!(
        db.bind_session(run, g.next(), reserved(1), host(1)),
        Err(LedgerError::SessionKeyConflict)
    );
    assert_eq!(
        db.retire(run, g),
        Err(LedgerError::StaleAttempt(StaleAttempt {
            current: g.next(),
            offered: g
        })),
        "the old generation is fenced from every write"
    );
}

#[test]
fn live_attempts_are_the_unretired_non_terminal_ones() {
    let root = Root::new();
    let mut db = root.open();
    let a = created_run(&mut db, 1);
    let b = created_run(&mut db, 2);
    let c = created_run(&mut db, 3);
    running_attempt(&mut db, a, 1);
    root.tick(1);
    let bg = db.open_attempt(b).unwrap().generation;
    root.tick(1);
    let cg = running_attempt(&mut db, c, 3).generation;
    db.advance(c, cg, AttemptPhase::Unknown).unwrap();
    let live: Vec<_> = db
        .live_attempts()
        .unwrap()
        .into_iter()
        .map(|row| row.run)
        .collect();
    assert_eq!(live, vec![a, b], "oldest first; unknown is not live");
    db.advance(b, bg, AttemptPhase::Interrupted).unwrap();
    db.retire(b, bg).unwrap();
    let live: Vec<_> = db
        .live_attempts()
        .unwrap()
        .into_iter()
        .map(|row| row.run)
        .collect();
    assert_eq!(live, vec![a]);
}

/// The vocabulary crate's `AttemptState`, assembled from the row, the
/// run's open publications and the header-summary cursor.
#[test]
fn attempt_state_is_the_core_type_assembled_from_the_ledger() {
    use koru_delegate_core::PublicationKind;

    let root = Root::new();
    let mut db = root.open();
    let run = created_run(&mut db, 1);
    assert_eq!(db.attempt_state(run).unwrap(), None);
    let row = running_attempt(&mut db, run, 1);
    db.enqueue(crate::fixtures::event(5), run, PublicationKind::Usage, "{}")
        .unwrap();
    db.bump_attempts(crate::fixtures::event(5)).unwrap();
    db.advance_cursor(run, Projection::HeaderSummary, 7)
        .unwrap();
    db.advance_cursor(run, Projection::Bus, 3).unwrap();
    let state = db.attempt_state(run).unwrap().unwrap();
    assert_eq!(state.run, run);
    assert_eq!(state.attempt, row.attempt);
    assert_eq!(state.generation, row.generation);
    assert_eq!(state.started_at_ms, row.started_at_ms);
    assert_eq!(state.provider_session, row.provider_session);
    assert_eq!(state.process.host_instance, Some(host(1)));
    assert_eq!(state.process.worker_pid, None);
    assert_eq!(state.process.observed_at_ms, Some(row.updated_at_ms));
    assert_eq!(state.pending_publications.len(), 1);
    assert_eq!(state.pending_publications[0].kind, PublicationKind::Usage);
    assert_eq!(state.pending_publications[0].attempts, 1);
    assert_eq!(
        state.projection_cursor,
        Some(7),
        "the header's cursor, not the bus's"
    );
    assert_eq!(
        db.advance(run, row.generation, AttemptPhase::Ended)
            .unwrap()
            .phase,
        AttemptPhase::Ended
    );
    assert_eq!(
        db.acknowledge(crate::fixtures::event(5)).unwrap(),
        WriteOutcome::Applied
    );
    assert!(
        db.attempt_state(run)
            .unwrap()
            .unwrap()
            .pending_publications
            .is_empty()
    );
}
