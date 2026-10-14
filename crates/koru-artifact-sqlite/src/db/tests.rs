// SPDX-License-Identifier: MPL-2.0

//! The open sequence: migration from empty, idempotent reopen, and a newer
//! database refused before anything about it is changed.

use rusqlite::Connection;

use super::*;

const SCHEMA_ONE_OBJECTS: &[&str] = &[
    "artifact_families",
    "artifact_families_by_workspace",
    "artifact_versions",
    "artifact_versions_are_immutable",
    "artifact_permissions",
    "artifact_run_links",
];

const SCHEMA_TWO_OBJECTS: &[&str] = &[
    "run_authorities",
    "run_authority_grants",
    "run_authority_tokens",
    "run_authority_tokens_by_run",
];

const SCHEMA_THREE_OBJECTS: &[&str] = &["deleted_runs"];

fn has_object(connection: &Connection, object: &str) -> bool {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = ?1)",
            [object],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn a_fresh_database_migrates_to_the_current_schema_with_every_object() {
    let connection = open_in_memory().unwrap();
    assert_eq!(user_version(&connection).unwrap(), ARTIFACTS_SCHEMA_VERSION);
    for object in SCHEMA_ONE_OBJECTS
        .iter()
        .chain(SCHEMA_TWO_OBJECTS)
        .chain(SCHEMA_THREE_OBJECTS)
    {
        assert!(
            has_object(&connection, object),
            "missing schema object {object}"
        );
    }
    let synchronous: i64 = connection
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        synchronous, 2,
        "FULL: a version is a fact, not an observation"
    );
}

/// A database stamped 1 by the P1.10 build — the exact previous on-disk
/// shape, made by that build's own migration — opens through the production
/// sequence, gains every later schema's objects and nothing else changes:
/// the families and versions it held are still there.
#[test]
fn a_schema_one_database_migrates_forward_to_two_keeping_its_rows() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("artifacts").join(crate::DATABASE_FILE);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let stamped = Connection::open(&path).unwrap();
        stamped.execute_batch(MIGRATIONS[0].sql).unwrap();
        stamped.pragma_update(None, "user_version", 1).unwrap();
        stamped
            .execute_batch(
                "INSERT INTO artifact_families
                   (family_id, workspace_id, title, created_at_ms)
                 VALUES ('00000000-0000-4000-8000-000000000001',
                         '00000000-0000-4000-8000-0000000000a0', 'Report', 1);",
            )
            .unwrap();
        for object in SCHEMA_TWO_OBJECTS.iter().chain(SCHEMA_THREE_OBJECTS) {
            assert!(!has_object(&stamped, object), "{object} exists at schema 1");
        }
    }
    let migrated = open(&path).unwrap();
    assert_eq!(user_version(&migrated).unwrap(), ARTIFACTS_SCHEMA_VERSION);
    for object in SCHEMA_ONE_OBJECTS
        .iter()
        .chain(SCHEMA_TWO_OBJECTS)
        .chain(SCHEMA_THREE_OBJECTS)
    {
        assert!(
            has_object(&migrated, object),
            "missing {object} after the walk forward"
        );
    }
    let families: i64 = migrated
        .query_row("SELECT COUNT(*) FROM artifact_families", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(families, 1, "the migration kept the rows");
    let authorities: i64 = migrated
        .query_row("SELECT COUNT(*) FROM run_authorities", [], |row| row.get(0))
        .unwrap();
    assert_eq!(authorities, 0);
}

#[test]
fn reopening_runs_no_step_and_keeps_the_stamp() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("artifacts").join(crate::DATABASE_FILE);
    drop(open(&path).unwrap());
    let again = open(&path).unwrap();
    assert_eq!(user_version(&again).unwrap(), ARTIFACTS_SCHEMA_VERSION);
    let mut connection = Connection::open(&path).unwrap();
    prepare(&mut connection).unwrap();
    assert_eq!(user_version(&connection).unwrap(), ARTIFACTS_SCHEMA_VERSION);
}

/// A database stamped by a newer build is refused before any pragma is
/// touched — the `journal_mode` that build chose is left as it was — and
/// the refusal names both numbers.
#[test]
fn a_newer_database_is_refused_before_anything_is_changed() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("artifacts").join(crate::DATABASE_FILE);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let newer = Connection::open(&path).unwrap();
        newer
            .execute_batch("PRAGMA journal_mode = DELETE; PRAGMA user_version = 99;")
            .unwrap();
    }
    assert_eq!(
        open(&path).err(),
        Some(OpenError::SchemaTooNew {
            database: crate::DATABASE_FILE,
            discovered: 99,
            supported: ARTIFACTS_SCHEMA_VERSION,
        })
    );
    let untouched = Connection::open(&path).unwrap();
    let mode: String = untouched
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode.to_lowercase(), "delete", "the refusal changed nothing");
    assert_eq!(user_version(&untouched).unwrap(), 99);
    assert_eq!(
        OpenError::SchemaTooNew {
            database: crate::DATABASE_FILE,
            discovered: 99,
            supported: 3
        }
        .to_string(),
        "artifacts.db schema 99 is newer than supported schema 3"
    );
}

#[test]
fn the_migration_file_owns_neither_the_transaction_nor_the_stamp() {
    for migration in MIGRATIONS {
        // Statements only: the header comment names the stamp it does not set.
        let sql: String = migration
            .sql
            .lines()
            .filter(|line| !line.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join(
                "
",
            )
            .to_ascii_uppercase();
        // `BEGIN` alone is a trigger body; a transaction is these spellings.
        for transaction in ["BEGIN;", "BEGIN IMMEDIATE", "BEGIN TRANSACTION", "COMMIT"] {
            assert!(!sql.contains(transaction), "{}", migration.name);
        }
        assert!(!sql.contains("USER_VERSION"), "{}", migration.name);
    }
}
