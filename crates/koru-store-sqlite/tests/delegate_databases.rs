// SPDX-License-Identifier: MPL-2.0

//! **The original rehearsal of `runs.db` and `artifacts.db`** — the two
//! device-local databases documented in `docs/architecture/database/README.md`, before
//! either existed. The DDL below retains the original proposal's proof:
//! migration from empty, refuse-
//! newer before any pragma is touched, checked closed domains, two-root
//! isolation, an immutable artifact version, one claim per schedule slot.
//!
//! **Test-only.** Nothing here is a production migration: P1.3 lifts the
//! `runs.db` DDL into the delegate worker's crate and P1.10 lifts
//! `artifacts.db` into the artifact service's, each with its own
//! `*_SCHEMA_VERSION`, its own `include_str!`'d numbered file under
//! its crate's `migrations/` and its own row in `docs/architecture/database/DATABASE.sql`.
//! Until then the fixtures live here, beside the only SQLite crate that
//! already carries the forward-only, refuse-newer convention this copies
//! (`koru-store-sqlite/src/db.rs`, `apps/koru-events-server/src/journal.rs`,
//! `koru-ai-sqlite/src/db.rs`).

use std::path::Path;

use rusqlite::{Connection, params};

/// `runs.db`, schema 1. Opened by `koru-agent.exe --worker delegate` (D12)
/// at `<channel-root>/delegate/runs.db`, beside `ai/ai.db` and `events/`.
///
/// One canonical run status lives in the Run document; these rows are how
/// far *this machine* got (§7.1). Closed domains are `CHECK`ed, the way
/// `016_agent_profile_cleanup.sql` does it, because SQLite has no enum.
const RUNS_SCHEMA_VERSION: i64 = 1;
const RUNS_V1: &str = "
    BEGIN IMMEDIATE;
    -- The creation intent, with the whole delegation payload a restart needs
    -- to finish the job. Pending until the Run document is committed; listed
    -- as a request and never as a run (§7.1 steps 1-4).
    CREATE TABLE run_intents (
      run_id             TEXT PRIMARY KEY,
      workspace_id       TEXT NOT NULL,
      payload_json       TEXT NOT NULL,
      state              TEXT NOT NULL CHECK (state IN ('pending', 'satisfied', 'abandoned')),
      run_document_id    TEXT,
      created_at_ms      INTEGER NOT NULL,
      updated_at_ms      INTEGER NOT NULL,
      CHECK (state <> 'satisfied' OR run_document_id IS NOT NULL)
    );
    -- Execution-attempt state per generation. `retired` is the fence of
    -- §7.6: a delayed create carrying a retired key is refused.
    CREATE TABLE run_attempts (
      run_id             TEXT NOT NULL REFERENCES run_intents(run_id),
      generation         INTEGER NOT NULL CHECK (generation >= 1),
      state              TEXT NOT NULL CHECK (state IN (
        'created', 'starting', 'running', 'interrupted', 'ended', 'unknown'
      )),
      session_key        TEXT UNIQUE,
      session_id         TEXT,
      host_instance_id   TEXT,
      retired            INTEGER NOT NULL DEFAULT 0 CHECK (retired IN (0, 1)),
      started_at_ms      INTEGER,
      updated_at_ms      INTEGER NOT NULL,
      PRIMARY KEY (run_id, generation)
    );
    -- The retry queue (§7.1): a lost lifecycle publication, a lost artifact
    -- registration reply, an economics fact not yet on the bus. The same
    -- EventId is reused on retry; the journal dedupes it.
    CREATE TABLE pending_publications (
      event_id           TEXT PRIMARY KEY,
      run_id             TEXT NOT NULL,
      kind               TEXT NOT NULL CHECK (kind IN (
        'lifecycle', 'artifact_registration', 'economics'
      )),
      body_json          TEXT NOT NULL,
      attempts           INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
      queued_at_ms       INTEGER NOT NULL,
      acknowledged_at_ms INTEGER
    );
    CREATE INDEX pending_publications_open
      ON pending_publications(queued_at_ms) WHERE acknowledged_at_ms IS NULL;
    -- Where each projection of a run's retained facts has got to (§17.1):
    -- the Run document's timeline index last folded into the header summary
    -- and last published. Device-local; the facts themselves sync.
    CREATE TABLE projection_cursors (
      run_id             TEXT NOT NULL,
      projection         TEXT NOT NULL CHECK (projection IN ('header_summary', 'bus')),
      timeline_index     INTEGER NOT NULL CHECK (timeline_index >= 0),
      updated_at_ms      INTEGER NOT NULL,
      PRIMARY KEY (run_id, projection)
    );
    -- One slot, one run (§7.7): the claim and its planned RunId are one row,
    -- keyed by the slot and never by the revision.
    CREATE TABLE schedule_claims (
      schedule_id        TEXT NOT NULL,
      occurrence_ms      INTEGER NOT NULL,
      revision           INTEGER NOT NULL CHECK (revision >= 1),
      run_id             TEXT NOT NULL UNIQUE,
      outcome            TEXT NOT NULL CHECK (outcome IN (
        'claimed', 'fired', 'skipped_missed', 'skipped_busy', 'skipped_retired'
      )),
      claimed_at_ms      INTEGER NOT NULL,
      PRIMARY KEY (schedule_id, occurrence_ms)
    );
    PRAGMA user_version = 1;
    COMMIT;
";

/// `artifacts.db`, schema 1. Opened by the gateway worker (D13) at
/// `<channel-root>/artifacts/artifacts.db`. Identity (`version_id`) is
/// separate from content (`blob_hash`, in the workspace's content-addressed
/// blob store); a version is immutable, and the trigger is what refuses.
const ARTIFACTS_SCHEMA_VERSION: i64 = 1;
const ARTIFACTS_V1: &str = "
    BEGIN IMMEDIATE;
    CREATE TABLE artifact_families (
      family_id          TEXT PRIMARY KEY,
      workspace_id       TEXT NOT NULL,
      title              TEXT NOT NULL,
      created_by_run_id  TEXT,
      created_at_ms      INTEGER NOT NULL,
      retired_at_ms      INTEGER
    );
    CREATE INDEX artifact_families_by_workspace
      ON artifact_families(workspace_id) WHERE retired_at_ms IS NULL;
    CREATE TABLE artifact_versions (
      version_id         TEXT PRIMARY KEY,
      family_id          TEXT NOT NULL REFERENCES artifact_families(family_id),
      ordinal            INTEGER NOT NULL CHECK (ordinal >= 1),
      blob_hash          TEXT NOT NULL,
      size_bytes         INTEGER NOT NULL CHECK (size_bytes >= 0),
      mime               TEXT NOT NULL,
      filename           TEXT,
      produced_by_run_id TEXT,
      produced_by_generation INTEGER CHECK (produced_by_generation IS NULL OR produced_by_generation >= 1),
      registration_id    TEXT NOT NULL UNIQUE,
      derived_from       TEXT REFERENCES artifact_versions(version_id),
      created_at_ms      INTEGER NOT NULL,
      UNIQUE (family_id, ordinal)
    );
    CREATE TRIGGER artifact_versions_are_immutable
      BEFORE UPDATE ON artifact_versions
      BEGIN SELECT RAISE(ABORT, 'artifact_version_immutable'); END;
    -- The permission vocabulary, modeled now and owner-only reachable (D11).
    CREATE TABLE artifact_permissions (
      family_id          TEXT NOT NULL REFERENCES artifact_families(family_id),
      principal          TEXT NOT NULL,
      access             TEXT NOT NULL CHECK (access IN ('owner', 'read')),
      granted_at_ms      INTEGER NOT NULL,
      PRIMARY KEY (family_id, principal)
    );
    -- Which run consumed or produced which version (provenance; D6, M3).
    CREATE TABLE artifact_run_links (
      run_id             TEXT NOT NULL,
      version_id         TEXT NOT NULL REFERENCES artifact_versions(version_id),
      role               TEXT NOT NULL CHECK (role IN ('input', 'output')),
      linked_at_ms       INTEGER NOT NULL,
      PRIMARY KEY (run_id, version_id, role)
    );
    PRAGMA user_version = 1;
    COMMIT;
";

#[derive(Debug, PartialEq, Eq)]
enum OpenError {
    SchemaTooNew { found: i64, understood: i64 },
    Sql(String),
}

/// The convention every Koru SQLite database follows, restated so the
/// rehearsal opens exactly the way P1.3 and P1.10 will: read `user_version`
/// **before** any pragma is changed, refuse a newer database, then apply the
/// forward-only steps each in its own transaction.
fn open(path: &Path, understood: i64, steps: &[(i64, &str)]) -> Result<Connection, OpenError> {
    let conn = Connection::open(path).map_err(|e| OpenError::Sql(e.to_string()))?;
    let found: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| OpenError::Sql(e.to_string()))?;
    if found > understood {
        return Err(OpenError::SchemaTooNew { found, understood });
    }
    conn.execute_batch(
        "PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL; PRAGMA foreign_keys = ON;",
    )
    .map_err(|e| OpenError::Sql(e.to_string()))?;
    for (to, sql) in steps {
        if found < *to {
            conn.execute_batch(sql)
                .map_err(|e| OpenError::Sql(e.to_string()))?;
        }
    }
    Ok(conn)
}

fn user_version(conn: &Connection) -> i64 {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

fn runs_db(root: &Path) -> Result<Connection, OpenError> {
    let dir = root.join("delegate");
    std::fs::create_dir_all(&dir).unwrap();
    open(&dir.join("runs.db"), RUNS_SCHEMA_VERSION, &[(1, RUNS_V1)])
}

fn artifacts_db(root: &Path) -> Result<Connection, OpenError> {
    let dir = root.join("artifacts");
    std::fs::create_dir_all(&dir).unwrap();
    open(
        &dir.join("artifacts.db"),
        ARTIFACTS_SCHEMA_VERSION,
        &[(1, ARTIFACTS_V1)],
    )
}

// ------------------------------------------------------------------ migration

/// From nothing to schema 1, stamped; a second open finds it and runs no step.
#[test]
fn both_databases_migrate_from_empty_and_reopen_at_their_version() {
    let root = tempfile::tempdir().unwrap();
    for (db, version) in [
        (runs_db(root.path()).unwrap(), RUNS_SCHEMA_VERSION),
        (artifacts_db(root.path()).unwrap(), ARTIFACTS_SCHEMA_VERSION),
    ] {
        assert_eq!(user_version(&db), version);
    }
    // Reopening is idempotent: the tables exist, the version is unchanged.
    assert_eq!(
        user_version(&runs_db(root.path()).unwrap()),
        RUNS_SCHEMA_VERSION
    );
    assert_eq!(
        user_version(&artifacts_db(root.path()).unwrap()),
        ARTIFACTS_SCHEMA_VERSION
    );
}

/// A database stamped by a newer build is refused before any pragma is
/// touched — the `journal_mode` a newer build chose is left as it was — and
/// the refusal names both numbers, as `schema_too_new` does everywhere else.
#[test]
fn a_newer_database_is_refused_before_anything_is_changed() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("delegate").join("runs.db");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        // A "newer build" made this: a different journal mode, a higher stamp.
        let newer = Connection::open(&path).unwrap();
        newer
            .execute_batch("PRAGMA journal_mode = DELETE; PRAGMA user_version = 2;")
            .unwrap();
    }
    assert_eq!(
        runs_db(root.path()).err(),
        Some(OpenError::SchemaTooNew {
            found: 2,
            understood: RUNS_SCHEMA_VERSION
        })
    );
    let untouched = Connection::open(&path).unwrap();
    let mode: String = untouched
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode.to_lowercase(), "delete", "the refusal changed nothing");
    assert_eq!(user_version(&untouched), 2);
}

/// Two roots are two files that know nothing of each other.
#[test]
fn two_roots_hold_two_independent_databases() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let on_a = runs_db(a.path()).unwrap();
    on_a.execute(
        "INSERT INTO run_intents (run_id, workspace_id, payload_json, state, created_at_ms, updated_at_ms)
         VALUES ('run-a', 'ws', '{}', 'pending', 1, 1)",
        [],
    )
    .unwrap();
    let on_b = runs_db(b.path()).unwrap();
    let count: i64 = on_b
        .query_row("SELECT count(*) FROM run_intents", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
    assert!(a.path().join("delegate").join("runs.db").exists());
    assert!(b.path().join("delegate").join("runs.db").exists());
    assert!(!a.path().join("artifacts").exists(), "nothing opened it");
}

// ------------------------------------------------------------------ domains

/// A closed domain is refused by the database, not merely by the Rust type.
#[test]
fn a_value_outside_a_closed_domain_is_refused_by_the_database() {
    let root = tempfile::tempdir().unwrap();
    let db = runs_db(root.path()).unwrap();
    let refused = db.execute(
        "INSERT INTO run_intents (run_id, workspace_id, payload_json, state, created_at_ms, updated_at_ms)
         VALUES ('r', 'ws', '{}', 'finished', 1, 1)",
        [],
    );
    assert!(refused.is_err(), "'finished' is not in the intent domain");
    // A satisfied intent must name the Run document it produced.
    let refused = db.execute(
        "INSERT INTO run_intents (run_id, workspace_id, payload_json, state, created_at_ms, updated_at_ms)
         VALUES ('r', 'ws', '{}', 'satisfied', 1, 1)",
        [],
    );
    assert!(refused.is_err(), "satisfied without a document");
    db.execute(
        "INSERT INTO run_intents (run_id, workspace_id, payload_json, state, run_document_id, created_at_ms, updated_at_ms)
         VALUES ('r', 'ws', '{}', 'satisfied', 'doc', 1, 1)",
        [],
    )
    .unwrap();
    let refused = db.execute(
        "INSERT INTO run_attempts (run_id, generation, state, updated_at_ms)
         VALUES ('r', 1, 'failed', 1)",
        [],
    );
    assert!(refused.is_err(), "'failed' is not an attempt state (D10)");
    db.execute(
        "INSERT INTO run_attempts (run_id, generation, state, updated_at_ms)
         VALUES ('r', 1, 'interrupted', 1)",
        [],
    )
    .unwrap();
}

/// A schedule slot is claimed once: `(schedule_id, occurrence_ms)` is the
/// key and the revision is not part of it (§7.7), so editing a template can
/// never turn one slot into two.
#[test]
fn a_schedule_slot_is_claimed_once_whatever_the_revision() {
    let root = tempfile::tempdir().unwrap();
    let db = runs_db(root.path()).unwrap();
    db.execute(
        "INSERT INTO schedule_claims (schedule_id, occurrence_ms, revision, run_id, outcome, claimed_at_ms)
         VALUES ('s', 900, 3, 'run-1', 'claimed', 1)",
        [],
    )
    .unwrap();
    let second = db.execute(
        "INSERT INTO schedule_claims (schedule_id, occurrence_ms, revision, run_id, outcome, claimed_at_ms)
         VALUES ('s', 900, 4, 'run-2', 'claimed', 2)",
        [],
    );
    assert!(second.is_err(), "the same slot under a newer revision");
}

/// An artifact version is immutable: the trigger refuses every update with
/// a stable code, and a retried registration finds the version it made.
#[test]
fn an_artifact_version_is_immutable_and_a_registration_retry_is_the_same_version() {
    let root = tempfile::tempdir().unwrap();
    let db = artifacts_db(root.path()).unwrap();
    db.execute(
        "INSERT INTO artifact_families (family_id, workspace_id, title, created_at_ms)
         VALUES ('f', 'ws', 'Report', 1)",
        [],
    )
    .unwrap();
    db.execute(
        "INSERT INTO artifact_versions
           (version_id, family_id, ordinal, blob_hash, size_bytes, mime, registration_id, created_at_ms)
         VALUES ('v1', 'f', 1, 'b3:abc', 12, 'text/markdown', 'reg-1', 1)",
        [],
    )
    .unwrap();
    let rewrite = db.execute(
        "UPDATE artifact_versions SET blob_hash = 'b3:def' WHERE version_id = 'v1'",
        [],
    );
    let message = rewrite.unwrap_err().to_string();
    assert!(message.contains("artifact_version_immutable"), "{message}");

    let retry = db.execute(
        "INSERT INTO artifact_versions
           (version_id, family_id, ordinal, blob_hash, size_bytes, mime, registration_id, created_at_ms)
         VALUES ('v2', 'f', 2, 'b3:abc', 12, 'text/markdown', 'reg-1', 2)",
        [],
    );
    assert!(retry.is_err(), "the registration id is the idempotency key");
    let existing: String = db
        .query_row(
            "SELECT version_id FROM artifact_versions WHERE registration_id = ?1",
            params!["reg-1"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(existing, "v1");
}
