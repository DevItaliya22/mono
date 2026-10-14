// SPDX-License-Identifier: MPL-2.0

//! Rows, read with the worker stopped: nothing here opens a ledger for
//! writing while it reads one, and every expectation is a row this file
//! actually holds.

use koru_delegate_core::PublicationKind;
use rusqlite::Connection;

use super::*;
use crate::{
    AttemptPhase, SlotOutcome,
    fixtures::{
        BODY_SENTINEL, FOLDER_SENTINEL, OBJECTIVE_SENTINEL, Root, created_run, event, request,
        run_id, running_attempt, schedule, workspace,
    },
};

/// The strong one: a ledger written by a worker that is now gone, read
/// whole, with every row a person would act on.
#[test]
fn the_whole_ledger_reads_with_no_worker_running() {
    let root = Root::new();
    let mut db = root.open();
    let live = created_run(&mut db, 1);
    let stuck = created_run(&mut db, 2);
    running_attempt(&mut db, live, 1);
    let unknown = running_attempt(&mut db, stuck, 2).generation;
    db.advance(stuck, unknown, AttemptPhase::Unknown).unwrap();
    db.record_intent(&request(3), workspace(0xa2)).unwrap();
    root.tick(90_000);
    db.enqueue(event(1), live, PublicationKind::Lifecycle, "{}")
        .unwrap();
    db.bump_attempts(event(1)).unwrap();
    db.bump_attempts(event(1)).unwrap();
    let claimed = db.claim_slot(schedule(0x51), 1_700_000_000_000, 4).unwrap();
    let settled = db.claim_slot(schedule(0x51), 1_700_000_060_000, 4).unwrap();
    db.record_outcome(schedule(0x51), 1_700_000_060_000, SlotOutcome::Fired)
        .unwrap();
    let now = root.now_ms();
    // The worker stops. Everything below reads the file it left behind.
    drop(db);

    let report = report(&root.db_path(), now).unwrap();
    assert!(!report.is_quiet(), "{report:?}");
    assert_eq!(report.summary.live_attempts, 1);
    assert_eq!(report.summary.unknown_attempts, 1);

    let phases: Vec<_> = report
        .attempts
        .iter()
        .map(|attempt| (attempt.run, attempt.generation.get(), attempt.phase))
        .collect();
    assert!(
        phases.contains(&(live, 1, AttemptPhase::Running)),
        "{phases:?}"
    );
    assert!(
        phases.contains(&(stuck, 1, AttemptPhase::Unknown)),
        "{phases:?}"
    );
    assert!(
        report
            .attempts
            .iter()
            .all(|attempt| attempt.session_bound && attempt.host_bound)
    );

    assert_eq!(
        report.intents.iter().map(|i| i.run).collect::<Vec<_>>(),
        vec![run_id(3)]
    );

    assert_eq!(report.publications.len(), 1);
    assert_eq!(report.publications[0].attempts, 2);
    assert_eq!(report.publications[0].waiting_ms, 0);
    assert_eq!(report.publications[0].kind, PublicationKind::Lifecycle);

    // Claimed and not settled: one row, and the fired slot is not in it.
    assert_eq!(
        report.slots.iter().map(|s| s.run).collect::<Vec<_>>(),
        vec![claimed.run]
    );
    assert!(report.slots.iter().all(|s| s.state == SlotState::Claimed));
    assert_ne!(claimed.run, settled.run);
}

/// An empty ledger reports emptiness rather than erroring, and so does a
/// file the migration never reached.
#[test]
fn an_empty_ledger_reports_emptiness() {
    let root = Root::new();
    drop(root.open());
    let empty = report(&root.db_path(), 10_000).unwrap();
    assert!(empty.is_quiet(), "{empty:?}");
    assert_eq!(empty.summary.schema_version, crate::RUNS_SCHEMA_VERSION);

    let unmigrated = Root::new();
    std::fs::create_dir_all(unmigrated.db_path().parent().unwrap()).unwrap();
    Connection::open(unmigrated.db_path()).unwrap();
    let never_migrated = report(&unmigrated.db_path(), 10_000).unwrap();
    assert!(never_migrated.is_quiet(), "{never_migrated:?}");
    assert_eq!(never_migrated.summary.schema_version, 0);

    let missing = Root::new();
    assert_eq!(report(&missing.db_path(), 1), Err(InspectError::Absent));
}

#[test]
fn a_newer_ledger_is_refused_by_both_depths() {
    let root = Root::new();
    let path = root.db_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    Connection::open(&path)
        .unwrap()
        .execute_batch("PRAGMA user_version = 99;")
        .unwrap();
    let refusal = InspectError::SchemaTooNew {
        discovered: 99,
        supported: crate::RUNS_SCHEMA_VERSION,
    };
    assert_eq!(report(&path, 1), Err(refusal.clone()));
    assert_eq!(run_report(&path, 1, run_id(1)), Err(refusal));
}

/// The per-run form: every generation, not only the live one.
#[test]
fn one_run_reports_every_generation_and_what_is_still_open() {
    let root = Root::new();
    let mut db = root.open();
    let run = created_run(&mut db, 7);
    let first = running_attempt(&mut db, run, 7).generation;
    db.advance(run, first, AttemptPhase::Interrupted).unwrap();
    db.retire(run, first).unwrap();
    running_attempt(&mut db, run, 8);
    db.enqueue(event(2), run, PublicationKind::Usage, "{}")
        .unwrap();
    db.advance_cursor(run, crate::Projection::Bus, 12).unwrap();
    let now = root.now_ms();
    drop(db);

    let one = run_report(&root.db_path(), now, run).unwrap();
    assert!(one.known());
    assert_eq!(
        one.attempts
            .iter()
            .map(|a| (a.generation.get(), a.phase, a.retired))
            .collect::<Vec<_>>(),
        vec![
            (1, AttemptPhase::Interrupted, true),
            (2, AttemptPhase::Running, false),
        ]
    );
    assert_eq!(one.intent.unwrap().state, crate::IntentState::Satisfied);
    assert!(one.intent.unwrap().run_document.is_some());
    assert_eq!(one.publications.len(), 1);
    assert_eq!(
        one.cursors
            .iter()
            .map(|c| (c.projection, c.timeline_index))
            .collect::<Vec<_>>(),
        vec![(crate::Projection::Bus, 12)]
    );

    // A run this device never heard of is a report saying so, not an error.
    let stranger = run_report(&root.db_path(), now, run_id(0x5f)).unwrap();
    assert!(!stranger.known());
    assert!(stranger.attempts.is_empty());
}

/// Ids, tokens, counts and durations — never the objective, the folder or
/// a publication body.
#[test]
fn rows_carry_no_content() {
    let root = Root::new();
    let mut db = root.open();
    let run = created_run(&mut db, 1);
    running_attempt(&mut db, run, 1);
    db.record_intent(&request(2), workspace(0xa2)).unwrap();
    db.enqueue(
        event(1),
        run,
        PublicationKind::Lifecycle,
        &format!("{{\"s\":\"{BODY_SENTINEL}\"}}"),
    )
    .unwrap();
    db.claim_slot(schedule(0x51), 1_700_000_000_000, 1).unwrap();
    let now = root.now_ms();
    drop(db);

    let rendered = format!(
        "{:#?}{:#?}",
        report(&root.db_path(), now).unwrap(),
        run_report(&root.db_path(), now, run).unwrap()
    );
    for sentinel in [OBJECTIVE_SENTINEL, FOLDER_SENTINEL, BODY_SENTINEL] {
        assert!(
            !rendered.contains(sentinel),
            "{sentinel} leaked: {rendered}"
        );
    }
}

#[test]
fn schedule_diagnostics_count_every_outcome_and_bound_skip_details() {
    let root = Root::new();
    let mut db = root.open();

    db.claim_slot(schedule(1), 100, 1).unwrap();
    db.record_outcome(schedule(1), 100, SlotOutcome::Fired)
        .unwrap();
    db.claim_slot(schedule(1), 200, 2).unwrap();

    let omitted_planned = run_id(0x70);
    db.claim_skip(
        schedule(2),
        300,
        3,
        omitted_planned,
        SlotOutcome::SkippedMissed,
        None,
    )
    .unwrap();
    for index in 1..=SCHEDULE_SKIP_ROWS {
        root.tick(1);
        db.claim_skip(
            schedule(2),
            300 + i64::try_from(index).unwrap(),
            3,
            run_id(u8::try_from(0x70 + index).unwrap()),
            SlotOutcome::SkippedMissed,
            None,
        )
        .unwrap();
    }
    root.tick(1);
    let busy_planned = run_id(0x90);
    let holder = run_id(0x91);
    db.claim_skip(
        schedule(3),
        900,
        4,
        busy_planned,
        SlotOutcome::SkippedBusy,
        Some(holder),
    )
    .unwrap();
    root.tick(1);
    db.claim_slot(schedule(4), 1_000, 5).unwrap();
    db.conn()
        .execute(
            "UPDATE schedule_claims SET outcome = 'skipped_retired'
              WHERE schedule_id = ?1 AND occurrence_ms = 1000",
            [schedule(4).to_storage()],
        )
        .unwrap();
    let now = root.now_ms();
    drop(db);

    let report = report(&root.db_path(), now).unwrap();
    assert_eq!(
        report.schedule_totals,
        ScheduleTotals {
            fired: 1,
            missed: 21,
            busy: 1,
            retired: 1,
            unsettled: 1,
        }
    );
    assert_eq!(report.recent_schedule_skips.len(), SCHEDULE_SKIP_ROWS);
    assert_eq!(report.recent_schedule_skips[0].schedule, schedule(4));
    assert_eq!(
        report.recent_schedule_skips[0].reason,
        SlotOutcome::SkippedRetired
    );
    assert!(
        report
            .recent_schedule_skips
            .iter()
            .any(|row| row.busy_holder == Some(holder))
    );
    assert!(
        report
            .recent_schedule_skips
            .iter()
            .all(|row| row.reason != SlotOutcome::Fired)
    );

    // The oldest row fell outside the bound. More importantly, the public
    // row type has no planned-run field; the only RunId it can carry is the
    // actionable holder on a busy skip. The Agent rendering test proves the
    // two planned ids never render.
    assert!(
        report
            .recent_schedule_skips
            .iter()
            .all(|row| row.busy_holder != Some(omitted_planned)
                && row.busy_holder != Some(busy_planned))
    );
}

#[test]
fn schedule_diagnostics_read_both_older_schemas_without_the_holder_column() {
    for schema_version in [1_usize, 2] {
        let root = Root::new();
        std::fs::create_dir_all(root.db_path().parent().unwrap()).unwrap();
        let conn = Connection::open(root.db_path()).unwrap();
        for migration in &crate::db::MIGRATIONS[..schema_version] {
            conn.execute_batch(migration.sql).unwrap();
        }
        conn.execute(
            "INSERT INTO schedule_claims
               (schedule_id, occurrence_ms, revision, run_id, outcome, claimed_at_ms)
             VALUES (?1, 300, 3, ?2, 'skipped_missed', 400)",
            rusqlite::params![schedule(2).to_storage(), run_id(0x70).to_storage()],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", i64::try_from(schema_version).unwrap())
            .unwrap();
        drop(conn);

        let report = report(&root.db_path(), 500).unwrap();
        assert_eq!(
            report.summary.schema_version,
            i64::try_from(schema_version).unwrap()
        );
        assert_eq!(report.schedule_totals.missed, 1);
        assert_eq!(report.recent_schedule_skips.len(), 1);
        assert_eq!(report.recent_schedule_skips[0].busy_holder, None);
    }
}

#[test]
fn an_invalid_busy_holder_row_is_found_beyond_the_recent_skip_bound_in_every_schema() {
    for schema_version in [1_usize, 2, 3] {
        let root = Root::new();
        std::fs::create_dir_all(root.db_path().parent().unwrap()).unwrap();
        let conn = Connection::open(root.db_path()).unwrap();
        for migration in &crate::db::MIGRATIONS[..schema_version] {
            conn.execute_batch(migration.sql).unwrap();
        }
        // This is deliberately the oldest row. Schema 1/2 cannot hold the
        // required holder at all; schema 3 permits a migrated null that the
        // Rust boundary must continue to reject.
        conn.execute(
            "INSERT INTO schedule_claims
               (schedule_id, occurrence_ms, revision, run_id, outcome, claimed_at_ms)
             VALUES (?1, 1, 1, ?2, 'skipped_busy', 1)",
            rusqlite::params![schedule(7).to_storage(), run_id(0x9f).to_storage()],
        )
        .unwrap();
        for index in 0..=SCHEDULE_SKIP_ROWS {
            conn.execute(
                "INSERT INTO schedule_claims
                   (schedule_id, occurrence_ms, revision, run_id, outcome, claimed_at_ms)
                 VALUES (?1, ?2, 2, ?3, 'skipped_missed', ?4)",
                rusqlite::params![
                    schedule(8).to_storage(),
                    100 + i64::try_from(index).unwrap(),
                    run_id(u8::try_from(0xa0 + index).unwrap()).to_storage(),
                    100 + i64::try_from(index).unwrap(),
                ],
            )
            .unwrap();
        }
        conn.pragma_update(None, "user_version", i64::try_from(schema_version).unwrap())
            .unwrap();
        drop(conn);

        assert_eq!(
            report(&root.db_path(), 500),
            Err(InspectError::Corrupt {
                table: "schedule_claims",
                column: "busy_holder_run_id",
            }),
            "schema {schema_version}"
        );
    }
}

#[test]
fn a_malformed_busy_holder_is_found_beyond_the_recent_skip_bound() {
    let root = Root::new();
    std::fs::create_dir_all(root.db_path().parent().unwrap()).unwrap();
    let conn = Connection::open(root.db_path()).unwrap();
    for migration in crate::db::MIGRATIONS {
        conn.execute_batch(migration.sql).unwrap();
    }
    conn.execute(
        "INSERT INTO schedule_claims
           (schedule_id, occurrence_ms, revision, run_id, outcome, claimed_at_ms,
            busy_holder_run_id)
         VALUES (?1, 1, 1, ?2, 'skipped_busy', 1, 'not-a-run-id')",
        rusqlite::params![schedule(7).to_storage(), run_id(0x9f).to_storage()],
    )
    .unwrap();
    for index in 0..=SCHEDULE_SKIP_ROWS {
        conn.execute(
            "INSERT INTO schedule_claims
               (schedule_id, occurrence_ms, revision, run_id, outcome, claimed_at_ms)
             VALUES (?1, ?2, 2, ?3, 'skipped_missed', ?4)",
            rusqlite::params![
                schedule(8).to_storage(),
                100 + i64::try_from(index).unwrap(),
                run_id(u8::try_from(0xa0 + index).unwrap()).to_storage(),
                100 + i64::try_from(index).unwrap(),
            ],
        )
        .unwrap();
    }
    conn.pragma_update(None, "user_version", crate::RUNS_SCHEMA_VERSION)
        .unwrap();
    drop(conn);

    assert_eq!(
        report(&root.db_path(), 500),
        Err(InspectError::Corrupt {
            table: "schedule_claims",
            column: "busy_holder_run_id",
        })
    );
}
