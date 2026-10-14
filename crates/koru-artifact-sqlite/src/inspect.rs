// SPDX-License-Identifier: MPL-2.0

//! Where each run's results are kept, read with nothing running.
//!
//! D37: an unregistered file under `<run-workspace>/.artifacts/<RunId>/`
//! survives as long as its run, including in a temporary workspace — so
//! the disk held that way grows quietly and somebody has to be able to
//! ask how much of it there is. `run_authorities` is the only record that
//! names a run's working folder, which makes this file the one place a
//! diagnostic can start from.
//!
//! It follows the same rule as `koru_delegate_sqlite::inspect`, and for
//! the same reason (PRINCIPLES, *a diagnostic must not depend on the
//! health of what it diagnoses*): the gateway worker owns `artifacts.db`,
//! and a gateway that will not answer is exactly when somebody wants to
//! know what its runs are holding. So this opens the file itself,
//! **read-only**, sets no pragma, writes no row, and refuses a database a
//! newer build stamped without touching it.
//!
//! The path it returns never reaches a rendering. It is the input to a
//! walk whose output is a count and a number of bytes, filed under a run
//! id (P1.17).

use std::{fmt, path::Path};

use koru_artifact_core::{AuthorityState, RunId, RunWorkspace};
use rusqlite::{Connection, OpenFlags};

use crate::{ARTIFACTS_SCHEMA_VERSION, DATABASE_FILE};

/// The schema `run_authorities` arrived in. An older file is readable and
/// simply names no run workspaces.
const AUTHORITIES_SCHEMA: i64 = 2;

/// One run's working folder, as the authority record holds it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RunWorkspaceRow {
    pub run: RunId,
    /// Redacted in `Debug` by its own type; used to walk, never to print.
    pub workspace: RunWorkspace,
    pub state: AuthorityState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectError {
    /// No `artifacts.db` there: no run has ever registered on this root.
    Absent,
    /// A newer build stamped it. Nothing was changed.
    SchemaTooNew { discovered: i64, supported: i64 },
    Storage {
        operation: &'static str,
        detail: String,
    },
    /// A value this build does not understand, named by column and never
    /// by value.
    Corrupt {
        table: &'static str,
        column: &'static str,
    },
}

impl InspectError {
    fn sql(operation: &'static str, error: rusqlite::Error) -> Self {
        Self::Storage {
            operation,
            detail: error.to_string(),
        }
    }
}

impl fmt::Display for InspectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absent => write!(f, "no {DATABASE_FILE} here"),
            Self::SchemaTooNew {
                discovered,
                supported,
            } => write!(
                f,
                "{DATABASE_FILE} schema {discovered} is newer than supported schema {supported}"
            ),
            Self::Storage { operation, detail } => {
                write!(f, "could not {operation} {DATABASE_FILE}: {detail}")
            }
            Self::Corrupt { table, column } => write!(
                f,
                "{DATABASE_FILE} holds a value this build does not understand in {table}.{column}"
            ),
        }
    }
}

impl std::error::Error for InspectError {}

/// Every run this device has issued an authority for, with the folder its
/// results are written to. Ordered by run id, so two readings of an
/// unchanged file are the same reading.
pub fn run_workspaces(path: &Path) -> Result<Vec<RunWorkspaceRow>, InspectError> {
    if !path.is_file() {
        return Err(InspectError::Absent);
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| InspectError::sql("open", error))?;
    conn.busy_timeout(std::time::Duration::from_millis(500))
        .map_err(|error| InspectError::sql("set the busy timeout of", error))?;
    let discovered: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| InspectError::sql("read the schema version of", error))?;
    if discovered > ARTIFACTS_SCHEMA_VERSION {
        return Err(InspectError::SchemaTooNew {
            discovered,
            supported: ARTIFACTS_SCHEMA_VERSION,
        });
    }
    if discovered < AUTHORITIES_SCHEMA {
        // A file from before §7.3 names no run workspaces. That is an
        // answer, not a failure.
        return Ok(Vec::new());
    }
    let mut statement = conn
        .prepare("SELECT run_id, run_workspace_path, state FROM run_authorities ORDER BY run_id")
        .map_err(|error| InspectError::sql("read the run authorities of", error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| InspectError::sql("read the run authorities of", error))?;
    let mut out = Vec::new();
    for row in rows {
        let (run, path, state) =
            row.map_err(|error| InspectError::sql("read the run authorities of", error))?;
        out.push(RunWorkspaceRow {
            run: run.parse().map_err(|_| InspectError::Corrupt {
                table: "run_authorities",
                column: "run_id",
            })?,
            workspace: RunWorkspace::new(Path::new(&path)).map_err(|_| InspectError::Corrupt {
                table: "run_authorities",
                column: "run_workspace_path",
            })?,
            state: AuthorityState::parse(&state).ok_or(InspectError::Corrupt {
                table: "run_authorities",
                column: "state",
            })?,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
