// SPDX-License-Identifier: MPL-2.0

//! The diagnostics seam: counts, read-only, with no worker running.
//!
//! PRINCIPLES: *a diagnostic must not depend on the health of what it
//! diagnoses.* A delegate worker that will not answer is exactly when
//! somebody wants to know what it was holding, so [`inspect`] opens the
//! file itself, read-only, and counts. It changes nothing — not a pragma,
//! not a row — and it returns ids, counts and ages only: no payload, no
//! body, no path. `koru-agent delegate` (P1.17) renders it.
//!
//! Two depths, one connection policy. [`inspect`] answers the aggregate
//! counts — the numbers a person reads first, and the ones the existing
//! summary already promised. [`rows`] answers the same file as rows: which
//! attempts are live, which are `unknown` and waiting for a person, which
//! requests are still requests, which publications have not been
//! acknowledged and for how long, and which schedule slots are claimed but
//! not settled. Both open the file read-only and change nothing.

mod rows;

use std::{fmt, path::Path};

use rusqlite::{Connection, OpenFlags};

pub use rows::{
    AttemptSummary, CursorSummary, IntentSummary, PUBLICATION_ROWS, PublicationSummary, Report,
    RunReport, SCHEDULE_SKIP_ROWS, ScheduleSkipSummary, ScheduleTotals, SlotSummary, report,
    run_report,
};

use crate::{DATABASE_NAME, RUNS_SCHEMA_VERSION};

/// What the ledger holds, as numbers.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Summary {
    pub schema_version: i64,
    /// Unretired attempts in `created`, `starting` or `running`.
    pub live_attempts: u64,
    /// Unretired attempts in `unknown`: each is waiting for a person.
    pub unknown_attempts: u64,
    /// Requests that are not yet runs.
    pub pending_intents: u64,
    /// Publications not yet acknowledged.
    pub open_publications: u64,
    /// How long the oldest open publication has waited, or `None` when the
    /// queue is empty. Publication lag, in one number.
    pub oldest_open_publication_age_ms: Option<i64>,
    /// Schedule event intents awaiting authoritative reconciliation.
    pub prepared_schedule_publications: u64,
    /// Authoritatively confirmed schedule events awaiting terminal journal
    /// acknowledgement (published or permanently not publishable).
    pub ready_schedule_publications: u64,
    pub oldest_prepared_schedule_publication_age_ms: Option<i64>,
    pub oldest_ready_schedule_publication_age_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectError {
    /// No ledger at that path: the worker never opened one here.
    Absent,
    /// A newer build stamped it. Nothing was changed; the numbers are not
    /// this build's to read.
    SchemaTooNew { discovered: i64, supported: i64 },
    /// A token this build does not know, named by column and **never by
    /// value** — the rule `domains.rs` states, kept on the read-only path
    /// because a corrupt row is exactly the kind a person pastes into a
    /// bug report.
    Corrupt {
        table: &'static str,
        column: &'static str,
    },
    Storage {
        operation: &'static str,
        detail: String,
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

impl From<crate::LedgerError> for InspectError {
    /// The closed-domain parsers are the ledger's, and the only failure
    /// they can hand a reader is a token outside the domain.
    fn from(error: crate::LedgerError) -> Self {
        match error {
            crate::LedgerError::Corrupt { table, column } => Self::Corrupt { table, column },
            other => Self::Storage {
                operation: "decode a row of",
                detail: other.to_string(),
            },
        }
    }
}

impl fmt::Display for InspectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absent => write!(f, "no {DATABASE_NAME} here"),
            Self::SchemaTooNew {
                discovered,
                supported,
            } => write!(
                f,
                "{DATABASE_NAME} schema {discovered} is newer than supported schema {supported}"
            ),
            Self::Corrupt { table, column } => write!(
                f,
                "{DATABASE_NAME} holds a value this build does not understand in {table}.{column}"
            ),
            Self::Storage { operation, detail } => {
                write!(f, "could not {operation} {DATABASE_NAME}: {detail}")
            }
        }
    }
}

impl std::error::Error for InspectError {}

/// Count what the ledger at `path` holds, read-only, at `now_ms`.
pub fn inspect(path: &Path, now_ms: i64) -> Result<Summary, InspectError> {
    let (conn, schema_version) = open_read_only(path)?;
    if schema_version < 1 {
        // Created but never migrated: an empty file holds nothing to count.
        return Ok(Summary {
            schema_version,
            ..Summary::default()
        });
    }
    let count = |sql: &str| -> Result<u64, InspectError> {
        let n: i64 = conn
            .query_row(sql, [], |row| row.get(0))
            .map_err(|error| InspectError::sql("count rows of", error))?;
        Ok(u64::try_from(n).unwrap_or(0))
    };
    let oldest: Option<i64> = conn
        .query_row(
            "SELECT min(queued_at_ms) FROM pending_publications WHERE acknowledged_at_ms IS NULL",
            [],
            |row| row.get(0),
        )
        .map_err(|error| InspectError::sql("read the oldest publication of", error))?;
    let (
        prepared_schedule_publications,
        ready_schedule_publications,
        oldest_prepared,
        oldest_ready,
    ) = if schema_version >= 4 {
        let prepared =
            count("SELECT count(*) FROM schedule_publications WHERE ready_at_ms IS NULL")?;
        let ready = count(
            "SELECT count(*) FROM schedule_publications
                  WHERE ready_at_ms IS NOT NULL AND acknowledged_at_ms IS NULL",
        )?;
        let oldest_prepared: Option<i64> = conn
            .query_row(
                "SELECT min(queued_at_ms) FROM schedule_publications
                      WHERE ready_at_ms IS NULL",
                [],
                |row| row.get(0),
            )
            .map_err(|error| {
                InspectError::sql("read the oldest prepared schedule publication of", error)
            })?;
        let oldest_ready: Option<i64> = conn
            .query_row(
                "SELECT min(queued_at_ms) FROM schedule_publications
                      WHERE ready_at_ms IS NOT NULL AND acknowledged_at_ms IS NULL",
                [],
                |row| row.get(0),
            )
            .map_err(|error| {
                InspectError::sql("read the oldest ready schedule publication of", error)
            })?;
        (prepared, ready, oldest_prepared, oldest_ready)
    } else {
        (0, 0, None, None)
    };
    Ok(Summary {
        schema_version,
        live_attempts: count(
            "SELECT count(*) FROM run_attempts
              WHERE retired = 0 AND state IN ('created', 'starting', 'running')",
        )?,
        unknown_attempts: count(
            "SELECT count(*) FROM run_attempts WHERE retired = 0 AND state = 'unknown'",
        )?,
        pending_intents: count("SELECT count(*) FROM run_intents WHERE state = 'pending'")?,
        open_publications: count(
            "SELECT count(*) FROM pending_publications WHERE acknowledged_at_ms IS NULL",
        )?,
        oldest_open_publication_age_ms: oldest.map(|queued| now_ms.saturating_sub(queued)),
        prepared_schedule_publications,
        ready_schedule_publications,
        oldest_prepared_schedule_publication_age_ms: oldest_prepared
            .map(|queued| now_ms.saturating_sub(queued)),
        oldest_ready_schedule_publication_age_ms: oldest_ready
            .map(|queued| now_ms.saturating_sub(queued)),
    })
}

/// Open the ledger read-only and answer with its stamped schema version.
///
/// **The refusal comes before any other pragma**, the way `db.rs` opens it
/// for writing: `journal_mode` persists in the file, and a newer build's
/// choice must survive an older build's visit. Nothing here creates a
/// file, sets a pragma or writes a row.
fn open_read_only(path: &Path) -> Result<(Connection, i64), InspectError> {
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
    let schema_version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| InspectError::sql("read the schema version of", error))?;
    if schema_version > RUNS_SCHEMA_VERSION {
        return Err(InspectError::SchemaTooNew {
            discovered: schema_version,
            supported: RUNS_SCHEMA_VERSION,
        });
    }
    Ok((conn, schema_version))
}

#[cfg(test)]
mod tests;
