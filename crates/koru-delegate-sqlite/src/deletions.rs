// SPDX-License-Identifier: MPL-2.0

//! **A run in the trash, and the trash being emptied.**
//!
//! Two different things, and this file is deliberately only the smaller
//! one. Whether a run *is* in the trash is not stored here: a run is a Run
//! document, its trashing is that document's move into the workspace's bin,
//! and it syncs and comes back out whole like every other document in Koru
//! (ADR-69). Nothing device-local decides it, so nothing device-local
//! records it.
//!
//! What is stored here is the **one question the trash asks a person that
//! this device has to remember**: the run left results behind — keep them,
//! or delete them with it? The answer is taken when the run goes into the
//! trash and acted on when the trash is emptied, and everything in between
//! is somebody deciding they will finish this tomorrow. Trashing itself
//! changes nothing on disk and nothing in the artifact store.
//!
//! [`RunsDb::forget_run`] is the other half: emptying the trash, which is
//! the one operation in this file that removes rows a person cannot get
//! back. It removes **only what this device wrote about the run**. The
//! working folder is `koru_delegate_scope::remove_working_folder`'s
//! decision and is not made here — this file has no path and no opinion
//! about one.

use koru_delegate_core::{ArtifactDisposition, RunId};
use rusqlite::{OptionalExtension, params};

use crate::{LedgerError, RunsDb, WriteOp, WriteOutcome, intents::parse_run};

/// The stored token, back as the vocabulary's value. A token this build
/// does not know is a corrupt row named by column, never by value — the
/// same rule every other closed domain in this file follows.
fn parse_disposition(token: &str) -> Result<ArtifactDisposition, LedgerError> {
    ArtifactDisposition::parse(token).ok_or(LedgerError::Corrupt {
        table: "run_deletions",
        column: "artifacts",
    })
}

/// A run this device is holding a deletion decision for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DeletionRecord {
    pub run: RunId,
    pub artifacts: ArtifactDisposition,
    pub decided_at_ms: i64,
}

impl RunsDb {
    /// Record what to do with a trashed run's results. Idempotent on the
    /// same answer; a second, different answer replaces the first, because
    /// a person is allowed to change their mind while the run is still in
    /// the trash and nothing has happened yet.
    pub fn record_deletion(
        &mut self,
        run: RunId,
        artifacts: ArtifactDisposition,
    ) -> Result<WriteOutcome, LedgerError> {
        self.write(WriteOp::RecordDeletion, |tx, now| {
            let existing: Option<String> = tx
                .query_row(
                    "SELECT artifacts FROM run_deletions WHERE run_id = ?1",
                    params![run.to_storage()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| LedgerError::sql("read a deletion", error))?;
            if existing.as_deref() == Some(artifacts.as_str()) {
                return Ok(WriteOutcome::AlreadyApplied);
            }
            tx.execute(
                "INSERT INTO run_deletions (run_id, artifacts, decided_at_ms)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(run_id) DO UPDATE
                    SET artifacts = excluded.artifacts,
                        decided_at_ms = excluded.decided_at_ms",
                params![run.to_storage(), artifacts.as_str(), now],
            )
            .map_err(|error| LedgerError::sql("record a deletion", error))?;
            Ok(WriteOutcome::Applied)
        })
    }

    /// The run came back out of the trash: the decision goes with it.
    /// Idempotent — a run that was never in the trash has none to clear.
    pub fn clear_deletion(&mut self, run: RunId) -> Result<WriteOutcome, LedgerError> {
        self.write(WriteOp::ClearDeletion, |tx, _| {
            let removed = tx
                .execute(
                    "DELETE FROM run_deletions WHERE run_id = ?1",
                    params![run.to_storage()],
                )
                .map_err(|error| LedgerError::sql("clear a deletion", error))?;
            Ok(match removed {
                0 => WriteOutcome::AlreadyApplied,
                _ => WriteOutcome::Applied,
            })
        })
    }

    /// What was decided about this run, if anything was.
    pub fn deletion(&self, run: RunId) -> Result<Option<DeletionRecord>, LedgerError> {
        self.conn()
            .query_row(
                "SELECT artifacts, decided_at_ms FROM run_deletions WHERE run_id = ?1",
                params![run.to_storage()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(|error| LedgerError::sql("read a deletion", error))?
            .map(|(artifacts, decided_at_ms)| {
                Ok(DeletionRecord {
                    run,
                    artifacts: parse_disposition(&artifacts)?,
                    decided_at_ms,
                })
            })
            .transpose()
    }

    /// Every decision this device is holding, oldest first. What a
    /// diagnostic reads; the trash itself reads the documents.
    pub fn deletions(&self) -> Result<Vec<DeletionRecord>, LedgerError> {
        let mut statement = self
            .conn()
            .prepare(
                "SELECT run_id, artifacts, decided_at_ms FROM run_deletions
                  ORDER BY decided_at_ms, run_id",
            )
            .map_err(|error| LedgerError::sql("list deletions", error))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(|error| LedgerError::sql("list deletions", error))?;
        let mut out = Vec::new();
        for row in rows {
            let (run, artifacts, decided_at_ms) =
                row.map_err(|error| LedgerError::sql("list deletions", error))?;
            out.push(DeletionRecord {
                run: parse_run(&run)?,
                artifacts: parse_disposition(&artifacts)?,
                decided_at_ms,
            });
        }
        Ok(out)
    }

    /// **Emptying the trash: every row this device holds about the run
    /// leaves the ledger, in one transaction.**
    ///
    /// Six tables, children before parents, because `run_attempts`
    /// references `run_intents` and a delete in the wrong order is refused
    /// by the foreign key rather than half-applied. One transaction, so a
    /// crash between two of them cannot leave a run that is partly
    /// forgotten — which would be a run nothing can find and nothing can
    /// finish deleting.
    ///
    /// `AlreadyApplied` when the run was not in this ledger at all: a run
    /// that arrived by sync, or one already forgotten. Deleting something
    /// twice is not an error; it is the same answer.
    ///
    /// **It removes no file.** The working folder is decided by scope, not
    /// by this table, and the run's results are the Artifact Store's.
    pub fn forget_run(&mut self, run: RunId) -> Result<WriteOutcome, LedgerError> {
        self.write(WriteOp::ForgetRun, |tx, _| {
            let id = run.to_storage();
            let mut removed = 0usize;
            for statement in [
                "DELETE FROM run_deletions WHERE run_id = ?1",
                "DELETE FROM projection_cursors WHERE run_id = ?1",
                "DELETE FROM pending_publications WHERE run_id = ?1",
                "DELETE FROM schedule_claims WHERE run_id = ?1",
                "DELETE FROM run_attempts WHERE run_id = ?1",
                "DELETE FROM run_intents WHERE run_id = ?1",
            ] {
                removed += tx
                    .execute(statement, params![id])
                    .map_err(|error| LedgerError::sql("forget a run", error))?;
            }
            Ok(match removed {
                0 => WriteOutcome::AlreadyApplied,
                _ => WriteOutcome::Applied,
            })
        })
    }
}

#[cfg(test)]
mod tests;
