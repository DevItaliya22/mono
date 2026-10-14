// SPDX-License-Identifier: MPL-2.0

//! `artifacts.db`, opened through [`koru_sqlite`] — the one policy every
//! Koru database shares (stamp first, refuse a newer one, WAL and `FULL`,
//! forward-only steps).

use std::path::Path;

use koru_sqlite::{Migration, Schema};
use rusqlite::Connection;

pub use koru_sqlite::OpenError;

/// `artifacts.db`, held in `PRAGMA user_version`. Schema 1 is the DDL of
/// `crates/koru-artifact-sqlite/migrations/018_artifacts_initial.sql`, verbatim from the P1.0
/// rehearsal; schema 2 adds the run authority record of §7.3
/// (`019_artifacts_run_authorities.sql`); schema 3 adds `deleted_runs`, so a
/// result that outlived the run that made it says the run was deleted rather
/// than printing an id that resolves to nothing
/// (`021_artifacts_deleted_runs.sql`).
pub const ARTIFACTS_SCHEMA_VERSION: i64 = 3;

const MIGRATIONS: &[Migration] = &[
    Migration {
        to: 1,
        name: "018_artifacts_initial.sql",
        sql: include_str!("../migrations/018_artifacts_initial.sql"),
        columns: &[],
    },
    Migration {
        to: 2,
        name: "019_artifacts_run_authorities.sql",
        sql: include_str!("../migrations/019_artifacts_run_authorities.sql"),
        columns: &[],
    },
    Migration {
        to: 3,
        name: "021_artifacts_deleted_runs.sql",
        sql: include_str!("../migrations/021_artifacts_deleted_runs.sql"),
        columns: &[],
    },
];

const SCHEMA: Schema = Schema {
    database: crate::DATABASE_FILE,
    understood: ARTIFACTS_SCHEMA_VERSION,
    migrations: MIGRATIONS,
};

pub(crate) fn open(path: &Path) -> Result<Connection, OpenError> {
    koru_sqlite::open(&SCHEMA, path)
}

pub(crate) fn open_in_memory() -> Result<Connection, OpenError> {
    koru_sqlite::open_in_memory(&SCHEMA)
}

/// The whole open sequence on a connection already opened.
#[cfg(test)]
pub(crate) fn prepare(connection: &mut Connection) -> Result<(), OpenError> {
    koru_sqlite::prepare(&SCHEMA, connection)
}

#[cfg(test)]
pub(crate) fn user_version(connection: &Connection) -> Result<i64, OpenError> {
    koru_sqlite::user_version(crate::DATABASE_FILE, connection)
}

#[cfg(test)]
mod tests;
