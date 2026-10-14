// SPDX-License-Identifier: MPL-2.0

//! Durable schedule publications: prepare, authoritative confirmation, retry,
//! acknowledgement and restart reconciliation.

use rusqlite::params;

use super::*;
use crate::{CrashPoint, Moment, fixtures};

const ENVELOPE_SENTINEL: &str = "SCHEDULE-ENVELOPE-SENTINEL";

fn schedule_ref(n: u8) -> SchedulePublicationRef {
    SchedulePublicationRef {
        schedule: fixtures::schedule(n),
        workspace: fixtures::workspace(n.wrapping_add(0x20)),
        document: fixtures::document(n.wrapping_add(0x40)),
    }
}

fn body(n: u8) -> String {
    format!("{{\"event\":\"{ENVELOPE_SENTINEL}\",\"n\":{n}}}")
}

fn prepared(db: &mut RunsDb, event_n: u8, fact: SchedulePublicationFact) -> EventId {
    let event = fixtures::event(event_n);
    db.prepare_schedule_publication(event, schedule_ref(1), fact, &body(event_n))
        .unwrap();
    event
}

#[test]
fn intent_is_not_publishable_until_the_authoritative_mutation_is_confirmed() {
    let root = fixtures::Root::new();
    let mut db = root.open();
    let event = prepared(&mut db, 1, SchedulePublicationFact::Created { revision: 1 });

    assert!(db.ready_schedule_publications(10).unwrap().is_empty());
    let intents = db.prepared_schedule_publications(10).unwrap();
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].event, event);
    assert_eq!(intents[0].ready_at_ms, None);
    assert_eq!(
        db.acknowledge_schedule_publication(event),
        Err(LedgerError::PublicationNotReady)
    );
    assert_eq!(
        db.bump_schedule_publication_attempts(event),
        Err(LedgerError::PublicationNotReady)
    );

    root.tick(5);
    assert_eq!(
        db.mark_schedule_publication_ready(event).unwrap(),
        WriteOutcome::Applied
    );
    assert_eq!(
        db.mark_schedule_publication_ready(event).unwrap(),
        WriteOutcome::AlreadyApplied
    );
    assert!(db.prepared_schedule_publications(10).unwrap().is_empty());
    let ready = db.ready_schedule_publications(10).unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].event, event);
    assert_eq!(ready[0].body.as_str(), body(1));
    assert_eq!(ready[0].ready_at_ms, Some(root.now_ms()));

    assert_eq!(db.bump_schedule_publication_attempts(event).unwrap(), 1);
    root.tick(5);
    assert_eq!(
        db.acknowledge_schedule_publication(event).unwrap(),
        WriteOutcome::Applied
    );
    assert_eq!(
        db.acknowledge_schedule_publication(event).unwrap(),
        WriteOutcome::AlreadyApplied
    );
    assert!(db.ready_schedule_publications(10).unwrap().is_empty());
    assert_eq!(
        db.bump_schedule_publication_attempts(event),
        Err(LedgerError::PublicationAlreadyAcknowledged)
    );
    assert_eq!(db.schedule_publication(event).unwrap().unwrap().attempts, 1);
}

#[test]
fn exact_event_lookup_and_prepared_cursor_use_their_indexed_identities() {
    let root = fixtures::Root::new();
    let mut db = root.open();
    let mut events = Vec::new();
    for n in 1..=3 {
        let event = fixtures::event(n);
        db.prepare_schedule_publication(
            event,
            schedule_ref(n),
            SchedulePublicationFact::Created { revision: 1 },
            &body(n),
        )
        .unwrap();
        events.push(event);
        root.tick(1);
    }

    let exact = db.schedule_publication(events[1]).unwrap().unwrap();
    assert_eq!(exact.event, events[1]);
    assert_eq!(exact.schedule, schedule_ref(2));
    assert_eq!(exact.body.as_str(), body(2));
    assert!(
        db.schedule_publication(fixtures::event(99))
            .unwrap()
            .is_none()
    );

    let first = db.prepared_schedule_publications_after(None, 2).unwrap();
    assert_eq!(first.len(), 2);
    let cursor = SchedulePublicationCursor::from(first.last().unwrap());
    let second = db
        .prepared_schedule_publications_after(Some(cursor), 2)
        .unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].event, events[2]);
}

#[test]
fn retries_reuse_the_first_event_identity_and_exact_body() {
    let root = fixtures::Root::new();
    let mut db = root.open();
    let schedule = schedule_ref(1);
    let fact = SchedulePublicationFact::Updated { revision: 2 };
    assert_eq!(
        db.prepare_schedule_publication(fixtures::event(1), schedule, fact, &body(1))
            .unwrap()
            .outcome,
        WriteOutcome::Applied
    );
    assert_eq!(
        db.prepare_schedule_publication(fixtures::event(1), schedule, fact, &body(1))
            .unwrap()
            .outcome,
        WriteOutcome::AlreadyApplied,
        "an exact retry returns the durable row"
    );
    assert_eq!(
        db.prepare_schedule_publication(fixtures::event(1), schedule, fact, &body(9)),
        Err(LedgerError::SchedulePublicationConflict),
        "an EventId can never acquire another body"
    );
    assert_eq!(
        db.schedule_publication(fixtures::event(1))
            .unwrap()
            .unwrap()
            .body
            .as_str(),
        body(1)
    );
    let semantic_retry = db
        .prepare_schedule_publication(fixtures::event(2), schedule, fact, &body(1))
        .unwrap();
    assert_eq!(semantic_retry.outcome, WriteOutcome::AlreadyApplied);
    assert_eq!(semantic_retry.publication.event, fixtures::event(1));
    assert_eq!(semantic_retry.publication.body.as_str(), body(1));

    db.prepare_schedule_publication(
        fixtures::event(2),
        schedule,
        SchedulePublicationFact::Paused { revision: 3 },
        &body(2),
    )
    .unwrap();
    let colliding_retry = db
        .prepare_schedule_publication(fixtures::event(2), schedule, fact, &body(1))
        .unwrap();
    assert_eq!(colliding_retry.outcome, WriteOutcome::AlreadyApplied);
    assert_eq!(colliding_retry.publication.event, fixtures::event(1));
    assert_eq!(colliding_retry.publication.body.as_str(), body(1));
    assert_eq!(
        colliding_retry.publication.fact, fact,
        "the offered EventId must not redirect semantic replay to another fact"
    );
    assert_eq!(
        db.prepare_schedule_publication(fixtures::event(5), schedule, fact, &body(2)),
        Err(LedgerError::SchedulePublicationConflict),
        "a semantic retry cannot replace the durable envelope"
    );

    let fired = SchedulePublicationFact::Fired {
        revision: 2,
        occurrence_ms: 9_000,
    };
    db.prepare_schedule_publication(fixtures::event(3), schedule, fired, &body(3))
        .unwrap();
    assert_eq!(
        db.prepare_schedule_publication(
            fixtures::event(4),
            schedule,
            SchedulePublicationFact::Skipped {
                revision: 3,
                occurrence_ms: 9_000,
            },
            &body(4),
        ),
        Err(LedgerError::SchedulePublicationConflict),
        "revision cannot turn one occurrence into a second outcome"
    );
}

#[test]
fn restart_queries_are_bounded_ordered_and_keep_acknowledged_history() {
    let root = fixtures::Root::new();
    {
        let mut db = root.open();
        let created = prepared(&mut db, 1, SchedulePublicationFact::Created { revision: 1 });
        db.mark_schedule_publication_ready(created).unwrap();
        db.acknowledge_schedule_publication(created).unwrap();
        root.tick(5);
        let updated = prepared(&mut db, 2, SchedulePublicationFact::Updated { revision: 2 });
        db.mark_schedule_publication_ready(updated).unwrap();
        root.tick(5);
        let paused = prepared(&mut db, 3, SchedulePublicationFact::Paused { revision: 3 });
        db.mark_schedule_publication_ready(paused).unwrap();
        db.acknowledge_schedule_publication(paused).unwrap();
        root.tick(5);
        prepared(
            &mut db,
            4,
            SchedulePublicationFact::Fired {
                revision: 3,
                occurrence_ms: 10_000,
            },
        );
    }

    let restarted = root.open();
    let prepared = restarted.prepared_schedule_publications(1).unwrap();
    assert_eq!(
        prepared.iter().map(|row| row.event).collect::<Vec<_>>(),
        vec![fixtures::event(4)]
    );
    let ready = restarted.ready_schedule_publications(1).unwrap();
    assert_eq!(
        ready.iter().map(|row| row.event).collect::<Vec<_>>(),
        vec![fixtures::event(2)]
    );
    let all = restarted
        .schedule_publications(schedule_ref(1).schedule)
        .unwrap();
    assert_eq!(all.len(), 4, "acknowledgement never erases a fact");
    assert_eq!(all[2].event, fixtures::event(3));
    assert!(all[2].acknowledged_at_ms.is_some());
    assert!(
        restarted
            .prepared_schedule_publications(0)
            .unwrap()
            .is_empty()
    );
    assert!(restarted.ready_schedule_publications(0).unwrap().is_empty());
}

#[test]
fn a_child_never_drains_before_its_created_chain_root() {
    let root = fixtures::Root::new();
    let mut db = root.open();
    let schedule = schedule_ref(1);
    let root_event = fixtures::event(9);
    let child_event = fixtures::event(1);
    db.prepare_schedule_publication(
        root_event,
        schedule,
        SchedulePublicationFact::Created { revision: 1 },
        &body(9),
    )
    .unwrap();
    db.prepare_schedule_publication(
        child_event,
        schedule,
        SchedulePublicationFact::Updated { revision: 2 },
        &body(1),
    )
    .unwrap();
    db.mark_schedule_publication_ready(child_event).unwrap();
    assert!(
        db.ready_schedule_publications(10).unwrap().is_empty(),
        "a ready child cannot bypass a root that is still prepared"
    );

    db.mark_schedule_publication_ready(root_event).unwrap();
    let ready = db.ready_schedule_publications(10).unwrap();
    assert_eq!(
        ready.iter().map(|row| row.event).collect::<Vec<_>>(),
        vec![root_event],
        "same-ms EventId ordering cannot put the child first"
    );
    db.acknowledge_schedule_publication(root_event).unwrap();
    assert_eq!(
        db.ready_schedule_publications(10).unwrap()[0].event,
        child_event
    );
}

#[test]
fn an_older_eligible_child_is_not_starved_by_a_newer_root_on_another_schedule() {
    let root = fixtures::Root::new();
    let mut db = root.open();
    let first_schedule = schedule_ref(1);
    let first_root = fixtures::event(1);
    let old_child = fixtures::event(2);
    db.prepare_schedule_publication(
        first_root,
        first_schedule,
        SchedulePublicationFact::Created { revision: 1 },
        &body(1),
    )
    .unwrap();
    db.mark_schedule_publication_ready(first_root).unwrap();
    db.acknowledge_schedule_publication(first_root).unwrap();
    root.tick(5);
    db.prepare_schedule_publication(
        old_child,
        first_schedule,
        SchedulePublicationFact::Updated { revision: 2 },
        &body(2),
    )
    .unwrap();
    db.mark_schedule_publication_ready(old_child).unwrap();

    root.tick(5);
    let newer_root = fixtures::event(3);
    db.prepare_schedule_publication(
        newer_root,
        schedule_ref(2),
        SchedulePublicationFact::Created { revision: 1 },
        &body(3),
    )
    .unwrap();
    db.mark_schedule_publication_ready(newer_root).unwrap();

    assert_eq!(
        db.ready_schedule_publications(1).unwrap()[0].event,
        old_child,
        "eligible rows are ordered by age, not globally by kind"
    );
}

#[test]
fn one_schedule_id_cannot_change_workspace_or_document_subject() {
    let root = fixtures::Root::new();
    let mut db = root.open();
    let subject = schedule_ref(1);
    let root_event = fixtures::event(1);
    db.prepare_schedule_publication(
        root_event,
        subject,
        SchedulePublicationFact::Created { revision: 1 },
        &body(1),
    )
    .unwrap();

    let wrong_workspace = SchedulePublicationRef {
        workspace: fixtures::workspace(0xf1),
        ..subject
    };
    assert_eq!(
        db.prepare_schedule_publication(
            fixtures::event(2),
            wrong_workspace,
            SchedulePublicationFact::Updated { revision: 2 },
            &body(2),
        ),
        Err(LedgerError::SchedulePublicationConflict)
    );
    let wrong_document = SchedulePublicationRef {
        document: fixtures::document(0xf2),
        ..subject
    };
    assert_eq!(
        db.prepare_schedule_publication(
            fixtures::event(3),
            wrong_document,
            SchedulePublicationFact::Updated { revision: 2 },
            &body(3),
        ),
        Err(LedgerError::SchedulePublicationConflict)
    );

    db.mark_schedule_publication_ready(root_event).unwrap();
    db.acknowledge_schedule_publication(root_event).unwrap();
    assert!(
        db.ready_schedule_publications(10).unwrap().is_empty(),
        "acknowledging the real root cannot bless a cross-subject child"
    );
}

#[test]
fn only_a_prepared_intent_can_be_abandoned_after_reconciliation() {
    let root = fixtures::Root::new();
    let mut db = root.open();
    let abandoned = prepared(&mut db, 1, SchedulePublicationFact::Created { revision: 1 });
    assert_eq!(
        db.abandon_schedule_publication(abandoned).unwrap(),
        WriteOutcome::Applied
    );
    assert_eq!(
        db.abandon_schedule_publication(abandoned).unwrap(),
        WriteOutcome::AlreadyApplied
    );
    assert!(db.schedule_publication(abandoned).unwrap().is_none());

    let ready = prepared(&mut db, 2, SchedulePublicationFact::Created { revision: 1 });
    db.mark_schedule_publication_ready(ready).unwrap();
    assert_eq!(
        db.abandon_schedule_publication(ready),
        Err(LedgerError::PublicationAlreadyReady)
    );
    db.acknowledge_schedule_publication(ready).unwrap();
    assert_eq!(
        db.abandon_schedule_publication(ready),
        Err(LedgerError::PublicationAlreadyReady)
    );
}

#[test]
fn the_envelope_is_retried_byte_for_byte_and_queue_time_is_not_its_stamp() {
    let root = fixtures::Root::new();
    let mut db = root.open();
    let exact =
        "{ \"event_id\": \"00000000-0000-4000-8000-000000000001\", \"occurred_at_ms\": 17 }\n";
    db.prepare_schedule_publication(
        fixtures::event(1),
        schedule_ref(1),
        SchedulePublicationFact::Created { revision: 1 },
        exact,
    )
    .unwrap();
    db.mark_schedule_publication_ready(fixtures::event(1))
        .unwrap();
    let row = db.ready_schedule_publications(1).unwrap().pop().unwrap();
    assert_eq!(row.body.as_str().as_bytes(), exact.as_bytes());
    assert_eq!(row.queued_at_ms, root.now_ms());
    assert_ne!(row.queued_at_ms, 17, "queue time is not event occurred_at");
}

#[test]
fn all_five_kinds_round_trip_and_body_never_renders() {
    let root = fixtures::Root::new();
    let mut db = root.open();
    let facts = [
        SchedulePublicationFact::Created { revision: 1 },
        SchedulePublicationFact::Updated { revision: 2 },
        SchedulePublicationFact::Paused { revision: 3 },
        SchedulePublicationFact::Fired {
            revision: 3,
            occurrence_ms: 10_000,
        },
        SchedulePublicationFact::Skipped {
            revision: 3,
            occurrence_ms: 20_000,
        },
    ];
    for (index, fact) in facts.into_iter().enumerate() {
        db.prepare_schedule_publication(
            fixtures::event(index as u8 + 1),
            schedule_ref(1),
            fact,
            &body(index as u8 + 1),
        )
        .unwrap();
    }
    let rows = db.schedule_publications(schedule_ref(1).schedule).unwrap();
    assert_eq!(rows.iter().map(|row| row.fact).collect::<Vec<_>>(), facts);
    assert!(rows[0].body.as_str().contains(ENVELOPE_SENTINEL));
    for rendered in [
        format!("{rows:?}"),
        format!("{:?}", rows[0]),
        format!("{:?}", rows[0].body),
        format!("{:?}", db),
    ] {
        assert!(!rendered.contains(ENVELOPE_SENTINEL), "{rendered}");
    }
    assert_eq!(
        format!("{:?}", rows[0].body),
        format!("SchedulePublicationBody(<{} bytes>)", body(1).len())
    );
}

#[test]
fn rust_and_sql_refuse_invalid_facts() {
    let root = fixtures::Root::new();
    let mut db = root.open();
    assert_eq!(
        db.prepare_schedule_publication(
            fixtures::event(1),
            schedule_ref(1),
            SchedulePublicationFact::Created { revision: 0 },
            &body(1),
        ),
        Err(LedgerError::RevisionZero)
    );
    assert_eq!(
        db.prepare_schedule_publication(
            fixtures::event(1),
            schedule_ref(1),
            SchedulePublicationFact::Created { revision: 2 },
            &body(1),
        ),
        Err(LedgerError::InvalidSchedulePublicationFact)
    );
    assert_eq!(
        db.prepare_schedule_publication(
            fixtures::event(1),
            schedule_ref(1),
            SchedulePublicationFact::Created { revision: 1 },
            "not json",
        ),
        Err(LedgerError::BodyNotJson)
    );

    let subject = schedule_ref(1);
    let insert = |kind: &str,
                  revision: i64,
                  occurrence: Option<i64>,
                  attempts: i64,
                  ready: Option<i64>,
                  acknowledged: Option<i64>| {
        db.conn().execute(
            "INSERT INTO schedule_publications
               (event_id, schedule_id, workspace_id, schedule_document_id, kind,
                revision, occurrence_ms, body_json, attempts, queued_at_ms,
                ready_at_ms, acknowledged_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, '{}', ?8, 1, ?9, ?10)",
            params![
                uuid::Uuid::new_v4().to_string(),
                subject.schedule.to_storage(),
                subject.workspace.to_storage(),
                subject.document.to_storage(),
                kind,
                revision,
                occurrence,
                attempts,
                ready,
                acknowledged,
            ],
        )
    };
    assert!(insert("unknown", 1, None, 0, None, None).is_err());
    assert!(insert("created", 1, Some(1), 0, None, None).is_err());
    assert!(insert("fired", 1, None, 0, None, None).is_err());
    assert!(insert("created", 0, None, 0, None, None).is_err());
    assert!(insert("created", 2, None, 0, None, None).is_err());
    assert!(insert("created", 1, None, -1, None, None).is_err());
    assert!(insert("created", 1, None, 0, None, Some(2)).is_err());

    insert("created", 1, None, 0, None, None).unwrap();
    assert!(
        insert("created", 2, None, 0, None, None).is_err(),
        "one schedule has one Created chain root"
    );
}

#[test]
fn crash_boundaries_leave_absent_prepared_ready_or_acknowledged_states() {
    for moment in Moment::ALL {
        let root = fixtures::Root::new();
        let mut db = root.open();
        let event = fixtures::event(1);
        db.arm_crash(CrashPoint {
            op: WriteOp::PrepareSchedulePublication,
            at: moment,
        });
        assert!(matches!(
            db.prepare_schedule_publication(
                event,
                schedule_ref(1),
                SchedulePublicationFact::Created { revision: 1 },
                &body(1),
            ),
            Err(LedgerError::Crashed(_))
        ));
        drop(db);
        let mut restarted = root.open();
        let exists = restarted.schedule_publication(event).unwrap().is_some();
        assert_eq!(
            exists,
            moment == Moment::AfterCommit,
            "prepare at {moment:?}"
        );
        restarted
            .prepare_schedule_publication(
                event,
                schedule_ref(1),
                SchedulePublicationFact::Created { revision: 1 },
                &body(1),
            )
            .unwrap();

        restarted.arm_crash(CrashPoint {
            op: WriteOp::MarkSchedulePublicationReady,
            at: moment,
        });
        assert!(matches!(
            restarted.mark_schedule_publication_ready(event),
            Err(LedgerError::Crashed(_))
        ));
        drop(restarted);
        let mut restarted = root.open();
        let ready = restarted
            .schedule_publication(event)
            .unwrap()
            .unwrap()
            .ready_at_ms
            .is_some();
        assert_eq!(ready, moment == Moment::AfterCommit, "ready at {moment:?}");
        restarted.mark_schedule_publication_ready(event).unwrap();

        restarted.arm_crash(CrashPoint {
            op: WriteOp::AcknowledgeSchedulePublication,
            at: moment,
        });
        assert!(matches!(
            restarted.acknowledge_schedule_publication(event),
            Err(LedgerError::Crashed(_))
        ));
        drop(restarted);
        let mut restarted = root.open();
        let acknowledged = restarted
            .schedule_publication(event)
            .unwrap()
            .unwrap()
            .acknowledged_at_ms
            .is_some();
        assert_eq!(
            acknowledged,
            moment == Moment::AfterCommit,
            "acknowledge at {moment:?}"
        );
        restarted.acknowledge_schedule_publication(event).unwrap();
    }
}

#[test]
fn abandonment_is_atomic_and_a_lost_reply_is_idempotent() {
    for moment in Moment::ALL {
        let root = fixtures::Root::new();
        let mut db = root.open();
        let event = prepared(&mut db, 1, SchedulePublicationFact::Created { revision: 1 });
        db.arm_crash(CrashPoint {
            op: WriteOp::AbandonSchedulePublication,
            at: moment,
        });
        assert!(matches!(
            db.abandon_schedule_publication(event),
            Err(LedgerError::Crashed(_))
        ));
        drop(db);
        let mut restarted = root.open();
        assert_eq!(
            restarted.schedule_publication(event).unwrap().is_none(),
            moment == Moment::AfterCommit
        );
        restarted.abandon_schedule_publication(event).unwrap();
        assert!(restarted.schedule_publication(event).unwrap().is_none());
    }
}
