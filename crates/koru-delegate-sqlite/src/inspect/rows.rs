// SPDX-License-Identifier: MPL-2.0

//! The same read-only file as rows: what is live, what is waiting, and
//! for how long.
//!
//! [`crate::inspect`] answers "how many"; this answers "which". The
//! difference matters at an incident: two live attempts is a number, and
//! *this run, generation 3, `unknown` for forty minutes* is the sentence a
//! person acts on. Every field is an id, a closed-domain token, a count or
//! a duration — an age in milliseconds computed against the `now_ms` the
//! caller passed, never a wall clock read here, so a test states time
//! rather than waiting for it.
//!
//! **The rows carry ids and never content.** The intent's payload and a
//! publication bodies are the columns in this file that hold what a person
//! wrote, and none is selected by any statement below. Schedule
//! history similarly omits the planned RunId for work that was skipped.

use koru_delegate_core::{
    AttemptGeneration, DocumentId, EventId, PublicationKind, RunAttemptId, RunId, ScheduleId,
    WorkspaceId,
};
use rusqlite::Connection;

use super::{InspectError, Summary, open_read_only};
use crate::{AttemptPhase, IntentState, Projection, SlotState, domains::parse_publication_kind};

/// How many open publications a whole-ledger report lists. The queue is a
/// retry queue, so a healthy one is short and a sick one is long; twenty
/// rows say which of those it is, and [`Summary::open_publications`] says
/// how many there really are.
pub const PUBLICATION_ROWS: usize = 20;

/// How many durable schedule skips a whole-ledger report lists. Totals remain
/// unbounded; this cap keeps an operator report useful after a long absence.
pub const SCHEDULE_SKIP_ROWS: usize = 20;

/// One execution attempt, as the ledger holds it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AttemptSummary {
    pub run: RunId,
    pub generation: AttemptGeneration,
    pub attempt: RunAttemptId,
    pub phase: AttemptPhase,
    /// §7.6's fence: a retired attempt authorises the next generation and
    /// accepts no write.
    pub retired: bool,
    /// A provider session key is bound — the reservation was taken, so a
    /// process may exist for this attempt.
    pub session_bound: bool,
    /// The host incarnation the session was asked of is recorded, which is
    /// what separates "the host restarted" from "same host, no evidence".
    pub host_bound: bool,
    pub started_at_ms: Option<i64>,
    /// Since the ledger last heard anything about this attempt.
    pub idle_ms: i64,
}

/// One creation intent (§7.1 steps 1 and 4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IntentSummary {
    pub run: RunId,
    pub workspace: WorkspaceId,
    pub state: IntentState,
    /// Named once the intent is satisfied: the run exists.
    pub run_document: Option<DocumentId>,
    /// Since the request was recorded. A pending intent older than a
    /// moment is a request a restart never completed forward.
    pub age_ms: i64,
}

/// One publication the bus has not acknowledged (§7.1's retry queue).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PublicationSummary {
    pub event: EventId,
    pub run: RunId,
    pub kind: PublicationKind,
    /// Delivery attempts so far. Publication lag with a rising attempt
    /// count is a bus that refuses; lag with a flat one is a worker that
    /// is not trying.
    pub attempts: u32,
    pub waiting_ms: i64,
}

/// One schedule slot this device claimed (§7.7) and has not settled yet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SlotSummary {
    pub schedule: ScheduleId,
    pub run: RunId,
    pub occurrence_ms: i64,
    pub revision: u32,
    pub state: SlotState,
    /// Since the claim was written.
    pub held_ms: i64,
}

/// Durable schedule outcomes on this device, across the life of the ledger.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ScheduleTotals {
    pub fired: u64,
    pub missed: u64,
    pub busy: u64,
    pub retired: u64,
    pub unsettled: u64,
}

/// One recent schedule skip, deliberately without the planned skipped RunId.
///
/// A skip never created a run. Excluding that implementation id from the type
/// makes the diagnostic's privacy and product boundary structural rather than
/// a rendering convention.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScheduleSkipSummary {
    pub schedule: ScheduleId,
    pub occurrence_ms: i64,
    pub revision: u32,
    pub reason: crate::SlotOutcome,
    pub busy_holder: Option<RunId>,
}

/// Where one projection of a run's facts has got to (§17.1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CursorSummary {
    pub projection: Projection,
    pub timeline_index: u64,
    pub age_ms: i64,
}

/// The whole ledger: the counts, and the rows behind the ones that matter.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Report {
    pub summary: Summary,
    /// Unretired attempts that are not in a terminal phase, plus every
    /// `unknown` one — §7.6's last row, which is unretired, terminal, and
    /// the single most important thing in this file, because it is waiting
    /// for a person.
    pub attempts: Vec<AttemptSummary>,
    /// Requests that are not yet runs, oldest first.
    pub intents: Vec<IntentSummary>,
    /// Open publications, oldest first, at most [`PUBLICATION_ROWS`].
    pub publications: Vec<PublicationSummary>,
    /// Claimed and not settled, oldest occurrence first.
    pub slots: Vec<SlotSummary>,
    /// All durable firing and skip outcomes, plus unsettled claims.
    pub schedule_totals: ScheduleTotals,
    /// Recent skips ordered by claim time, newest claim first, bounded by
    /// [`SCHEDULE_SKIP_ROWS`].
    pub recent_schedule_skips: Vec<ScheduleSkipSummary>,
}

impl Report {
    /// Whether the ledger holds nothing at all — a worker that has never
    /// run, or one with nothing outstanding. Emptiness is an answer.
    pub fn is_quiet(&self) -> bool {
        self.attempts.is_empty()
            && self.intents.is_empty()
            && self.publications.is_empty()
            && self.slots.is_empty()
            && self.summary.prepared_schedule_publications == 0
            && self.summary.ready_schedule_publications == 0
    }
}

/// One run, in as much detail as the ledger holds — every generation, not
/// only the live one, because the question a person asks about a single
/// run is usually "what happened the time before".
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RunReport {
    pub run: RunId,
    /// `None` when this device has never heard of the run.
    pub intent: Option<IntentSummary>,
    /// Oldest generation first.
    pub attempts: Vec<AttemptSummary>,
    /// Every publication of this run that is still open.
    pub publications: Vec<PublicationSummary>,
    pub cursors: Vec<CursorSummary>,
    /// The slot that minted it, when a schedule did.
    pub slot: Option<SlotSummary>,
}

impl RunReport {
    /// Whether this device knows the run at all.
    pub fn known(&self) -> bool {
        self.intent.is_some()
    }
}

/// Read the whole ledger at `path`, read-only, at `now_ms`.
pub fn report(path: &std::path::Path, now_ms: i64) -> Result<Report, InspectError> {
    let summary = super::inspect(path, now_ms)?;
    if summary.schema_version < 1 {
        // Created but never migrated: there are no tables to select from,
        // and an empty report is the honest answer rather than an error.
        return Ok(Report {
            summary,
            attempts: Vec::new(),
            intents: Vec::new(),
            publications: Vec::new(),
            slots: Vec::new(),
            schedule_totals: ScheduleTotals::default(),
            recent_schedule_skips: Vec::new(),
        });
    }
    let (conn, schema_version) = open_read_only(path)?;
    let has_busy_holder = schema_version >= 3;
    validate_schedule_claims(&conn, has_busy_holder)?;
    Ok(Report {
        summary,
        attempts: attempts(
            &conn,
            now_ms,
            "WHERE (retired = 0 AND state IN ('created', 'starting', 'running'))
                OR (retired = 0 AND state = 'unknown')
              ORDER BY updated_at_ms",
            &[],
        )?,
        intents: intents(
            &conn,
            now_ms,
            "WHERE state = 'pending' ORDER BY created_at_ms",
            &[],
        )?,
        publications: publications(
            &conn,
            now_ms,
            &format!(
                "WHERE acknowledged_at_ms IS NULL
                 ORDER BY queued_at_ms LIMIT {PUBLICATION_ROWS}"
            ),
            &[],
        )?,
        slots: slots(
            &conn,
            now_ms,
            "WHERE outcome = 'claimed' ORDER BY occurrence_ms",
            &[],
        )?,
        schedule_totals: schedule_totals(&conn)?,
        recent_schedule_skips: recent_schedule_skips(&conn, has_busy_holder)?,
    })
}

/// Read one run out of the ledger at `path`, read-only, at `now_ms`.
pub fn run_report(
    path: &std::path::Path,
    now_ms: i64,
    run: RunId,
) -> Result<RunReport, InspectError> {
    let (conn, schema_version) = open_read_only(path)?;
    if schema_version < 1 {
        return Ok(RunReport {
            run,
            intent: None,
            attempts: Vec::new(),
            publications: Vec::new(),
            cursors: Vec::new(),
            slot: None,
        });
    }
    let id = run.to_storage();
    let key: &[&dyn rusqlite::ToSql] = &[&id];
    Ok(RunReport {
        run,
        intent: intents(&conn, now_ms, "WHERE run_id = ?1", key)?
            .into_iter()
            .next(),
        attempts: attempts(&conn, now_ms, "WHERE run_id = ?1 ORDER BY generation", key)?,
        publications: publications(
            &conn,
            now_ms,
            "WHERE run_id = ?1 AND acknowledged_at_ms IS NULL ORDER BY queued_at_ms",
            key,
        )?,
        cursors: cursors(&conn, now_ms, run)?,
        slot: slots(&conn, now_ms, "WHERE run_id = ?1", key)?
            .into_iter()
            .next(),
    })
}

// ------------------------------------------------------------- the readers
//
// One shape each: the columns, a suffix the caller chose from a literal in
// this file, and the parameters. Nothing here interpolates a value.

fn attempts(
    conn: &Connection,
    now_ms: i64,
    suffix: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<Vec<AttemptSummary>, InspectError> {
    let sql = format!(
        "SELECT run_id, generation, attempt_id, state, session_key IS NOT NULL,
                host_instance_id IS NOT NULL, retired, started_at_ms, updated_at_ms
           FROM run_attempts {suffix}"
    );
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| InspectError::sql("read the attempts of", error))?;
    let rows = statement
        .query_map(params, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, bool>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, i64>(8)?,
            ))
        })
        .map_err(|error| InspectError::sql("read the attempts of", error))?;
    let mut out = Vec::new();
    for row in rows {
        let (run, generation, attempt, state, session, host, retired, started, updated) =
            row.map_err(|error| InspectError::sql("read the attempts of", error))?;
        out.push(AttemptSummary {
            run: parse_run(&run, "run_attempts")?,
            generation: u32::try_from(generation)
                .ok()
                .and_then(AttemptGeneration::from_raw)
                .ok_or(InspectError::Corrupt {
                    table: "run_attempts",
                    column: "generation",
                })?,
            attempt: attempt
                .parse::<RunAttemptId>()
                .map_err(|_| InspectError::Corrupt {
                    table: "run_attempts",
                    column: "attempt_id",
                })?,
            phase: AttemptPhase::parse(&state)?,
            retired: retired != 0,
            session_bound: session,
            host_bound: host,
            started_at_ms: started,
            idle_ms: now_ms.saturating_sub(updated),
        });
    }
    Ok(out)
}

fn intents(
    conn: &Connection,
    now_ms: i64,
    suffix: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<Vec<IntentSummary>, InspectError> {
    let sql = format!(
        "SELECT run_id, workspace_id, state, run_document_id, created_at_ms
           FROM run_intents {suffix}"
    );
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| InspectError::sql("read the intents of", error))?;
    let rows = statement
        .query_map(params, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|error| InspectError::sql("read the intents of", error))?;
    let mut out = Vec::new();
    for row in rows {
        let (run, workspace, state, document, created) =
            row.map_err(|error| InspectError::sql("read the intents of", error))?;
        out.push(IntentSummary {
            run: parse_run(&run, "run_intents")?,
            workspace: WorkspaceId::parse(&workspace).ok_or(InspectError::Corrupt {
                table: "run_intents",
                column: "workspace_id",
            })?,
            state: IntentState::parse(&state)?,
            run_document: document
                .map(|id| {
                    DocumentId::parse(&id).ok_or(InspectError::Corrupt {
                        table: "run_intents",
                        column: "run_document_id",
                    })
                })
                .transpose()?,
            age_ms: now_ms.saturating_sub(created),
        });
    }
    Ok(out)
}

fn publications(
    conn: &Connection,
    now_ms: i64,
    suffix: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<Vec<PublicationSummary>, InspectError> {
    let sql = format!(
        "SELECT event_id, run_id, kind, attempts, queued_at_ms
           FROM pending_publications {suffix}"
    );
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| InspectError::sql("read the publications of", error))?;
    let rows = statement
        .query_map(params, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|error| InspectError::sql("read the publications of", error))?;
    let mut out = Vec::new();
    for row in rows {
        let (event, run, kind, attempts, queued) =
            row.map_err(|error| InspectError::sql("read the publications of", error))?;
        out.push(PublicationSummary {
            event: uuid::Uuid::parse_str(&event).map(EventId).map_err(|_| {
                InspectError::Corrupt {
                    table: "pending_publications",
                    column: "event_id",
                }
            })?,
            run: parse_run(&run, "pending_publications")?,
            kind: parse_publication_kind(&kind)?,
            attempts: u32::try_from(attempts).unwrap_or(u32::MAX),
            waiting_ms: now_ms.saturating_sub(queued),
        });
    }
    Ok(out)
}

fn slots(
    conn: &Connection,
    now_ms: i64,
    suffix: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<Vec<SlotSummary>, InspectError> {
    let sql = format!(
        "SELECT schedule_id, occurrence_ms, revision, run_id, outcome, claimed_at_ms
           FROM schedule_claims {suffix}"
    );
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| InspectError::sql("read the schedule claims of", error))?;
    let rows = statement
        .query_map(params, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(|error| InspectError::sql("read the schedule claims of", error))?;
    let mut out = Vec::new();
    for row in rows {
        let (schedule, occurrence_ms, revision, run, outcome, claimed) =
            row.map_err(|error| InspectError::sql("read the schedule claims of", error))?;
        out.push(SlotSummary {
            schedule: schedule.parse().map_err(|_| InspectError::Corrupt {
                table: "schedule_claims",
                column: "schedule_id",
            })?,
            run: parse_run(&run, "schedule_claims")?,
            occurrence_ms,
            revision: u32::try_from(revision).map_err(|_| InspectError::Corrupt {
                table: "schedule_claims",
                column: "revision",
            })?,
            state: SlotState::parse(&outcome)?,
            held_ms: now_ms.saturating_sub(claimed),
        });
    }
    Ok(out)
}

fn schedule_totals(conn: &Connection) -> Result<ScheduleTotals, InspectError> {
    let (fired, missed, busy, retired, unsettled): (i64, i64, i64, i64, i64) = conn
        .query_row(
            "SELECT
                COALESCE(SUM(outcome = 'fired'), 0),
                COALESCE(SUM(outcome = 'skipped_missed'), 0),
                COALESCE(SUM(outcome = 'skipped_busy'), 0),
                COALESCE(SUM(outcome = 'skipped_retired'), 0),
                COALESCE(SUM(outcome = 'claimed'), 0)
               FROM schedule_claims",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .map_err(|error| InspectError::sql("count schedule outcomes in", error))?;
    Ok(ScheduleTotals {
        fired: u64::try_from(fired).unwrap_or(0),
        missed: u64::try_from(missed).unwrap_or(0),
        busy: u64::try_from(busy).unwrap_or(0),
        retired: u64::try_from(retired).unwrap_or(0),
        unsettled: u64::try_from(unsettled).unwrap_or(0),
    })
}

/// Validate the holder relationship across every durable claim before the
/// bounded recent projection can hide an older bad row.
fn validate_schedule_claims(conn: &Connection, has_busy_holder: bool) -> Result<(), InspectError> {
    let invalid: bool = if has_busy_holder {
        conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM schedule_claims
                 WHERE (outcome = 'skipped_busy') <> (busy_holder_run_id IS NOT NULL)
            )",
            [],
            |row| row.get(0),
        )
    } else {
        // Schema 1/2 had no holder column. A busy row therefore cannot be
        // reported truthfully; missed and retired rows remain readable.
        conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM schedule_claims WHERE outcome = 'skipped_busy'
            )",
            [],
            |row| row.get(0),
        )
    }
    .map_err(|error| InspectError::sql("validate schedule claims in", error))?;
    if invalid {
        return Err(InspectError::Corrupt {
            table: "schedule_claims",
            column: "busy_holder_run_id",
        });
    }
    if has_busy_holder {
        let mut statement = conn
            .prepare(
                "SELECT busy_holder_run_id
                   FROM schedule_claims
                  WHERE outcome = 'skipped_busy'",
            )
            .map_err(|error| InspectError::sql("validate schedule claims in", error))?;
        let holders = statement
            .query_map([], |row| row.get::<_, Option<String>>(0))
            .map_err(|error| InspectError::sql("validate schedule claims in", error))?;
        for holder in holders {
            let holder = holder
                .map_err(|error| InspectError::sql("validate schedule claims in", error))?
                .ok_or(InspectError::Corrupt {
                    table: "schedule_claims",
                    column: "busy_holder_run_id",
                })?;
            parse_busy_holder(&holder)?;
        }
    }
    Ok(())
}

fn recent_schedule_skips(
    conn: &Connection,
    has_busy_holder: bool,
) -> Result<Vec<ScheduleSkipSummary>, InspectError> {
    // Schema 3 added the holder. Older supported ledgers still diagnose
    // missed/retired skips; a legacy busy row has no truthful holder to
    // invent and is rejected by the earlier full-table invariant check.
    let busy_holder = if has_busy_holder {
        "busy_holder_run_id"
    } else {
        "NULL"
    };
    let mut statement = conn
        .prepare(&format!(
            "SELECT schedule_id, occurrence_ms, revision, outcome, {busy_holder}
               FROM schedule_claims
              WHERE outcome IN ('skipped_missed', 'skipped_busy', 'skipped_retired')
              ORDER BY claimed_at_ms DESC, schedule_id, occurrence_ms DESC
              LIMIT {SCHEDULE_SKIP_ROWS}"
        ))
        .map_err(|error| InspectError::sql("read recent schedule skips from", error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(|error| InspectError::sql("read recent schedule skips from", error))?;
    let mut out = Vec::new();
    for row in rows {
        let (schedule, occurrence_ms, revision, outcome, busy_holder) =
            row.map_err(|error| InspectError::sql("read recent schedule skips from", error))?;
        let reason = match SlotState::parse(&outcome)? {
            SlotState::Settled(reason) => reason,
            SlotState::Claimed => {
                return Err(InspectError::Corrupt {
                    table: "schedule_claims",
                    column: "outcome",
                });
            }
        };
        let busy_holder = busy_holder.map(|run| parse_busy_holder(&run)).transpose()?;
        if matches!(reason, crate::SlotOutcome::SkippedBusy) != busy_holder.is_some() {
            return Err(InspectError::Corrupt {
                table: "schedule_claims",
                column: "busy_holder_run_id",
            });
        }
        out.push(ScheduleSkipSummary {
            schedule: schedule.parse().map_err(|_| InspectError::Corrupt {
                table: "schedule_claims",
                column: "schedule_id",
            })?,
            occurrence_ms,
            revision: u32::try_from(revision).map_err(|_| InspectError::Corrupt {
                table: "schedule_claims",
                column: "revision",
            })?,
            reason,
            busy_holder,
        });
    }
    Ok(out)
}

fn cursors(conn: &Connection, now_ms: i64, run: RunId) -> Result<Vec<CursorSummary>, InspectError> {
    let mut statement = conn
        .prepare(
            "SELECT projection, timeline_index, updated_at_ms
               FROM projection_cursors WHERE run_id = ?1 ORDER BY projection",
        )
        .map_err(|error| InspectError::sql("read the cursors of", error))?;
    let rows = statement
        .query_map([run.to_storage()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|error| InspectError::sql("read the cursors of", error))?;
    let mut out = Vec::new();
    for row in rows {
        let (projection, index, updated) =
            row.map_err(|error| InspectError::sql("read the cursors of", error))?;
        out.push(CursorSummary {
            projection: match projection.as_str() {
                "header_summary" => Projection::HeaderSummary,
                "bus" => Projection::Bus,
                _ => {
                    return Err(InspectError::Corrupt {
                        table: "projection_cursors",
                        column: "projection",
                    });
                }
            },
            timeline_index: u64::try_from(index).unwrap_or(0),
            age_ms: now_ms.saturating_sub(updated),
        });
    }
    Ok(out)
}

fn parse_run(id: &str, table: &'static str) -> Result<RunId, InspectError> {
    id.parse().map_err(|_| InspectError::Corrupt {
        table,
        column: "run_id",
    })
}

fn parse_busy_holder(id: &str) -> Result<RunId, InspectError> {
    id.parse().map_err(|_| InspectError::Corrupt {
        table: "schedule_claims",
        column: "busy_holder_run_id",
    })
}

#[cfg(test)]
mod tests;
