// SPDX-License-Identifier: MPL-2.0

//! Projection cursors: how far each projection has folded the run's facts.
//!
//! §17.1 commits a fact in the run's own storage first and projects it
//! twice — into the header's summary and onto the bus. Each projection
//! keeps, per run, the timeline index it has folded up to. A cursor only
//! moves forward: an index below the current one is refused, the current
//! one is already applied, and the facts themselves are never here — they
//! live in the Run document and sync; the cursor is this device's bookmark.

use koru_delegate_core::RunId;
use rusqlite::{OptionalExtension, params};

use crate::{LedgerError, Projection, RunsDb, WriteOp, WriteOutcome};

impl RunsDb {
    /// The index last folded, or `None` if the projection has not started
    /// on this run.
    pub fn cursor(&self, run: RunId, projection: Projection) -> Result<Option<u64>, LedgerError> {
        read(self.conn(), run, projection)
    }

    /// Move the cursor to `timeline_index`. Monotonic: moving back is
    /// [`LedgerError::CursorMovedBackwards`].
    pub fn advance_cursor(
        &mut self,
        run: RunId,
        projection: Projection,
        timeline_index: u64,
    ) -> Result<WriteOutcome, LedgerError> {
        self.write(WriteOp::AdvanceCursor, |tx, now| {
            let offered = i64::try_from(timeline_index).map_err(|_| LedgerError::Corrupt {
                table: "projection_cursors",
                column: "timeline_index",
            })?;
            match read(tx, run, projection)? {
                Some(current) if current > timeline_index => {
                    Err(LedgerError::CursorMovedBackwards {
                        current,
                        offered: timeline_index,
                    })
                }
                Some(current) if current == timeline_index => Ok(WriteOutcome::AlreadyApplied),
                Some(_) => {
                    tx.execute(
                        "UPDATE projection_cursors SET timeline_index = ?3, updated_at_ms = ?4
                          WHERE run_id = ?1 AND projection = ?2",
                        params![run.to_storage(), projection.token(), offered, now],
                    )
                    .map_err(|error| LedgerError::sql("advance a cursor", error))?;
                    Ok(WriteOutcome::Applied)
                }
                None => {
                    tx.execute(
                        "INSERT INTO projection_cursors
                           (run_id, projection, timeline_index, updated_at_ms)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![run.to_storage(), projection.token(), offered, now],
                    )
                    .map_err(|error| LedgerError::sql("start a cursor", error))?;
                    Ok(WriteOutcome::Applied)
                }
            }
        })
    }
}

fn read(
    conn: &rusqlite::Connection,
    run: RunId,
    projection: Projection,
) -> Result<Option<u64>, LedgerError> {
    let index: Option<i64> = conn
        .query_row(
            "SELECT timeline_index FROM projection_cursors
              WHERE run_id = ?1 AND projection = ?2",
            params![run.to_storage(), projection.token()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| LedgerError::sql("read a cursor", error))?;
    index
        .map(|index| {
            u64::try_from(index).map_err(|_| LedgerError::Corrupt {
                table: "projection_cursors",
                column: "timeline_index",
            })
        })
        .transpose()
}

#[cfg(test)]
mod tests;
