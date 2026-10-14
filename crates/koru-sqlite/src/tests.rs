// SPDX-License-Identifier: MPL-2.0

use super::*;

const STEPS: &[Migration] = &[
    Migration {
        to: 1,
        name: "one",
        sql: "CREATE TABLE things (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
        columns: &[],
    },
    Migration {
        to: 2,
        name: "two",
        sql: "",
        columns: &[("things", "size", "INTEGER NOT NULL DEFAULT 0")],
    },
];

const SCHEMA: Schema = Schema {
    database: "things.db",
    understood: 2,
    migrations: STEPS,
};

#[test]
#[ignore = "uses a real filesystem; run explicitly with --ignored"]
fn a_fresh_file_reaches_the_schema_and_reopens_at_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("things.db");
    let connection = open(&SCHEMA, &path).unwrap();
    assert_eq!(user_version("things.db", &connection).unwrap(), 2);
    assert!(has_column("things.db", &connection, "things", "size").unwrap());
    drop(connection);
    let again = open(&SCHEMA, &path).unwrap();
    assert_eq!(user_version("things.db", &again).unwrap(), 2);
}

#[test]
#[ignore = "uses a real filesystem; run explicitly with --ignored"]
fn a_newer_stamp_is_refused_before_the_journal_mode_is_touched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("things.db");
    let newer = Connection::open(&path).unwrap();
    newer.pragma_update(None, "user_version", 9).unwrap();
    drop(newer);
    assert_eq!(
        open(&SCHEMA, &path).err(),
        Some(OpenError::SchemaTooNew {
            database: "things.db",
            discovered: 9,
            supported: 2,
        })
    );
    let untouched = Connection::open(&path).unwrap();
    let journal: String = untouched
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_ne!(
        journal, "wal",
        "a refusal changes nothing, not even the journal mode"
    );
    assert_eq!(
        open(&SCHEMA, &path).unwrap_err().to_string(),
        "things.db schema 9 is newer than supported schema 2"
    );
}

#[test]
#[ignore = "uses a real filesystem; run explicitly with --ignored"]
fn an_open_file_is_wal_and_fully_synchronous() {
    let dir = tempfile::tempdir().unwrap();
    let connection = open(&SCHEMA, &dir.path().join("things.db")).unwrap();
    let journal: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    let synchronous: i64 = connection
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .unwrap();
    assert_eq!(journal, "wal");
    assert_eq!(synchronous, 2, "FULL");
    assert!(quick_check("things.db", &connection).unwrap());
    assert!(size_bytes("things.db", &connection).unwrap() > 0);
}

#[test]
fn a_failing_step_leaves_the_version_where_it_was() {
    const BROKEN: Schema = Schema {
        database: "things.db",
        understood: 2,
        migrations: &[
            Migration {
                to: 1,
                name: "one",
                sql: "CREATE TABLE things (id INTEGER PRIMARY KEY);",
                columns: &[],
            },
            Migration {
                to: 2,
                name: "broken",
                sql: "CREATE TABLE things (id INTEGER PRIMARY KEY);",
                columns: &[],
            },
        ],
    };
    let error = open_in_memory(&BROKEN).unwrap_err();
    assert!(
        matches!(
            error,
            OpenError::Storage {
                operation: "apply a migration of",
                ..
            }
        ),
        "{error}"
    );
    // Partly migrated: resuming applies only what is above the stamp.
    let mut connection = Connection::open_in_memory().unwrap();
    assert_eq!(migrate("things.db", 2, STEPS, &mut connection).unwrap(), 2);
    assert_eq!(
        migrate("things.db", 2, STEPS, &mut connection).unwrap(),
        2,
        "twice changes nothing"
    );
}

#[test]
fn adding_a_column_that_is_already_there_is_a_no_op() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE things (id INTEGER PRIMARY KEY, name TEXT NOT NULL, size INTEGER);",
        )
        .unwrap();
    connection.pragma_update(None, "user_version", 1).unwrap();
    assert_eq!(migrate("things.db", 2, STEPS, &mut connection).unwrap(), 2);
    assert!(has_column("things.db", &connection, "things", "size").unwrap());
}

/// **The durability a file is opened with, read back from the file.** Each
/// pragma is per connection, so this pins what `open` actually leaves behind:
/// WAL, `FULL`, and the Apple flush — which SQLite records on every platform
/// and acts on only where `fsync` alone does not reach the drive.
#[test]
#[ignore = "uses a real filesystem; run explicitly with --ignored"]
fn an_opened_file_is_durable_down_to_the_drive() {
    let dir = tempfile::tempdir().unwrap();
    let connection = open(&SCHEMA, &dir.path().join("things.db")).unwrap();
    let pragma = |name: &str| -> String {
        connection
            .query_row(&format!("PRAGMA {name}"), [], |row| {
                row.get::<_, rusqlite::types::Value>(0)
            })
            .map(|value| match value {
                rusqlite::types::Value::Integer(n) => n.to_string(),
                rusqlite::types::Value::Text(text) => text,
                other => format!("{other:?}"),
            })
            .unwrap()
    };
    assert_eq!(pragma("journal_mode"), "wal");
    assert_eq!(pragma("synchronous"), "2", "synchronous = FULL");
    assert_eq!(pragma("fullfsync"), "1");
    assert_eq!(pragma("checkpoint_fullfsync"), "1");
    assert_eq!(pragma("foreign_keys"), "1");
}
