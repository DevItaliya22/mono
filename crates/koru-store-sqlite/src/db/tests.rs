// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_store_core::errors::Code;

const STEPS: &[Migration] = &[
    Migration {
        to: 1,
        name: "step 1",
        sql: "CREATE TABLE thing (id TEXT PRIMARY KEY);",
        columns: &[],
    },
    Migration {
        to: 2,
        name: "step 2",
        sql: "",
        columns: &[("thing", "note", "TEXT")],
    },
];

fn count_columns(c: &Connection, table: &str) -> i64 {
    c.query_row(
        &format!("SELECT count(*) FROM pragma_table_info('{table}')"),
        [],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn a_fresh_database_migrates_to_the_latest_version() {
    let mut c = open_in_memory().unwrap();
    assert_eq!(user_version(&c).unwrap(), 0);
    assert_eq!(migrate(&mut c, "test.db", STEPS, 2).unwrap(), 2);
    assert_eq!(count_columns(&c, "thing"), 2);
}

#[test]
fn migrating_twice_changes_nothing() {
    // Idempotence is what makes a start-up safe to repeat after a crash.
    let mut c = open_in_memory().unwrap();
    migrate(&mut c, "test.db", STEPS, 2).unwrap();
    migrate(&mut c, "test.db", STEPS, 2).unwrap();
    assert_eq!(user_version(&c).unwrap(), 2);
    assert_eq!(count_columns(&c, "thing"), 2);
}

#[test]
fn a_partly_migrated_database_resumes_where_it_stopped() {
    let mut c = open_in_memory().unwrap();
    migrate(&mut c, "test.db", &STEPS[..1], 1).unwrap();
    assert_eq!(count_columns(&c, "thing"), 1);
    assert_eq!(migrate(&mut c, "test.db", STEPS, 2).unwrap(), 2);
    assert_eq!(count_columns(&c, "thing"), 2);
}

#[test]
fn a_newer_database_is_refused_rather_than_rewritten() {
    // The rule the module exists for: an older binary must not attempt to
    // reshape a database it only half understands.
    let mut c = open_in_memory().unwrap();
    migrate(&mut c, "test.db", STEPS, 2).unwrap();
    let err = migrate(&mut c, "test.db", &STEPS[..1], 1).unwrap_err();
    assert_eq!(err.code(), Code::SchemaTooNew);
    // And it is still at the version it was: nothing was touched.
    assert_eq!(user_version(&c).unwrap(), 2);
}

#[test]
fn a_failing_migration_leaves_the_version_where_it_was() {
    const BAD: &[Migration] = &[
        Migration {
            to: 1,
            name: "step 1",
            sql: "CREATE TABLE ok (id TEXT);",
            columns: &[],
        },
        Migration {
            to: 2,
            name: "step 2",
            sql: "THIS IS NOT SQL;",
            columns: &[],
        },
    ];
    let mut c = open_in_memory().unwrap();
    assert!(migrate(&mut c, "test.db", BAD, 2).is_err());
    // Step one committed, step two did not. Half a step is what a transaction
    // per migration exists to prevent.
    assert_eq!(user_version(&c).unwrap(), 1);
    assert_eq!(count_columns(&c, "ok"), 1);
}

#[test]
fn a_real_file_is_wal_and_fully_synchronous() {
    // `synchronous` is meaningless in memory, so this is the one test that
    // needs a disk. FULL rather than NORMAL is the difference between an
    // acknowledgement being true and being nearly true.
    let dir = tempfile::tempdir().unwrap();
    let c = open(&dir.path().join("nested").join("t.db")).unwrap();

    let journal: String = c
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(journal.to_lowercase(), "wal");

    // 2 == FULL. 1 would be NORMAL, which under WAL does not fsync per commit.
    let sync: i64 = c.query_row("PRAGMA synchronous", [], |r| r.get(0)).unwrap();
    assert_eq!(sync, 2, "synchronous must be FULL, not NORMAL");
}

#[test]
fn opening_creates_the_directory_it_was_pointed_at() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a").join("b").join("c.db");
    open(&path).unwrap();
    assert!(path.exists());
}

#[test]
fn a_healthy_database_passes_its_own_check() {
    let mut c = open_in_memory().unwrap();
    migrate(&mut c, "test.db", STEPS, 2).unwrap();
    assert!(quick_check(&c).unwrap());
}

#[test]
fn size_is_reported_in_bytes() {
    let mut c = open_in_memory().unwrap();
    migrate(&mut c, "test.db", STEPS, 2).unwrap();
    let size = size_bytes(&c).unwrap();
    assert!(size > 0, "an empty database still has a header");
    assert_eq!(size % 512, 0, "a size is whole pages: {size}");
}

/// **A column that is already there is not a failure.**
///
/// The trap this exists for: `documents.trashed_by` was appended to a migration
/// that had already run, so it reached every database created afterwards and
/// none created before. The fix is a *later* migration adding it — which then
/// has to be a no-op on the databases that got it early, or the fix fails on
/// exactly the machines that did not need it.
#[test]
fn adding_a_column_that_is_already_there_is_a_no_op() {
    const FIRST: &[Migration] = &[
        Migration {
            to: 1,
            name: "step 1",
            sql: "CREATE TABLE thing (id TEXT PRIMARY KEY);",
            columns: &[],
        },
        // The edited migration: two columns where it once had one.
        Migration {
            to: 2,
            name: "step 2",
            sql: "",
            columns: &[("thing", "note", "TEXT"), ("thing", "who", "TEXT")],
        },
    ];
    // ...and the corrected shape, which adds the second one again a version later.
    const THEN: &[Migration] = &[
        Migration {
            to: 1,
            name: "step 1",
            sql: "CREATE TABLE thing (id TEXT PRIMARY KEY);",
            columns: &[],
        },
        Migration {
            to: 2,
            name: "step 2",
            sql: "",
            columns: &[("thing", "note", "TEXT"), ("thing", "who", "TEXT")],
        },
        Migration {
            to: 3,
            name: "step 3",
            sql: "",
            columns: &[("thing", "who", "TEXT")],
        },
    ];

    // The database that got both columns early still migrates.
    let mut early = open_in_memory().unwrap();
    assert_eq!(migrate(&mut early, "test.db", FIRST, 2).unwrap(), 2);
    assert_eq!(migrate(&mut early, "test.db", THEN, 3).unwrap(), 3);
    assert_eq!(
        count_columns(&early, "thing"),
        3,
        "a column was added twice"
    );

    // And so does the one that never saw it.
    let mut late = open_in_memory().unwrap();
    late.pragma_update(None, "user_version", 1).unwrap();
    late.execute_batch(
        "CREATE TABLE thing (id TEXT PRIMARY KEY); ALTER TABLE thing ADD COLUMN note TEXT;",
    )
    .unwrap();
    assert_eq!(migrate(&mut late, "test.db", THEN, 3).unwrap(), 3);
    assert_eq!(
        count_columns(&late, "thing"),
        3,
        "the missing column never arrived"
    );
}
