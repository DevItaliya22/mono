// SPDX-License-Identifier: MPL-2.0

//! The SQLite implementation of [`RunAuthorityRepository`] (§7.3).
//!
//! Three tables, one `BEGIN IMMEDIATE` per write. The record is one row
//! per run; the grants are rows in a child table, written as a complete
//! set at issue and never touched again (D28: frozen for an attempt, a
//! resume re-reads them); the tokens are one row per attempt, keyed by the
//! hash of the bearer, and a superseded row is *retired* by a mark so a
//! stale token still resolves to "stale" rather than "unknown".

use std::collections::BTreeSet;

use koru_artifact_core::{
    ArtifactError, AttemptGeneration, AuthorityState, NewAuthority, Result, RunAuthorityRecord,
    RunAuthorityRepository, RunId, RunWorkspace, TokenHash, TokenLookup, WorkspaceId,
};
use rusqlite::{Connection, OptionalExtension, Row, params};

use super::{Repository, decode, map_sql};

const AUTHORITY_COLUMNS: &str = "run_id, workspace_id, attempt_generation, run_workspace_path, \
     state, created_at_ms, revoked_at_ms";

type AuthorityRow = (String, String, i64, String, String, i64, Option<i64>);

fn authority_row(row: &Row<'_>) -> rusqlite::Result<AuthorityRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn grants_of(connection: &Connection, run: RunId) -> Result<BTreeSet<WorkspaceId>> {
    let mut statement = connection
        .prepare("SELECT workspace_id FROM run_authority_grants WHERE run_id = ?1")
        .map_err(|error| map_sql("read the grants", error))?;
    let rows = statement
        .query_map(params![run.to_storage()], |row| row.get::<_, String>(0))
        .map_err(|error| map_sql("read the grants", error))?;
    let mut grants = BTreeSet::new();
    for row in rows {
        let workspace = row.map_err(|error| map_sql("read the grants", error))?;
        grants.insert(decode("decode a grant", WorkspaceId::parse(&workspace))?);
    }
    Ok(grants)
}

fn authority_from(
    connection: &Connection,
    (run, workspace, generation, path, state, created_at_ms, revoked_at_ms): AuthorityRow,
) -> Result<RunAuthorityRecord> {
    const OP: &str = "decode an authority row";
    let run: RunId = decode(OP, run.parse().ok())?;
    Ok(RunAuthorityRecord {
        run,
        workspace: decode(OP, WorkspaceId::parse(&workspace))?,
        generation: decode(
            OP,
            u32::try_from(generation)
                .ok()
                .and_then(AttemptGeneration::from_raw),
        )?,
        grants: grants_of(connection, run)?,
        run_workspace: RunWorkspace::new(std::path::Path::new(&path))?,
        state: decode(OP, AuthorityState::parse(&state))?,
        created_at_ms,
        revoked_at_ms,
    })
}

fn find_authority(connection: &Connection, run: RunId) -> Result<RunAuthorityRecord> {
    let row = connection
        .query_row(
            &format!("SELECT {AUTHORITY_COLUMNS} FROM run_authorities WHERE run_id = ?1"),
            params![run.to_storage()],
            authority_row,
        )
        .optional()
        .map_err(|error| map_sql("read an authority", error))?
        .ok_or(ArtifactError::AuthorityNotFound)?;
    authority_from(connection, row)
}

fn insert_token(
    connection: &Connection,
    run: RunId,
    generation: AttemptGeneration,
    token: &TokenHash,
    issued_at_ms: i64,
) -> Result<()> {
    connection
        .execute(
            "INSERT INTO run_authority_tokens (token_hash, run_id, attempt_generation, issued_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                token.as_str(),
                run.to_storage(),
                i64::from(generation.get()),
                issued_at_ms
            ],
        )
        .map_err(|error| map_sql("issue a token", error))?;
    Ok(())
}

/// Which of `runs` this device has recorded as deleted.
///
/// Asked of a set the caller already has rather than by listing the table:
/// the answer a provenance sentence needs is about the handful of runs it
/// names, and a store that has deleted a thousand runs must not pay for
/// nine hundred and ninety of them to draw one line.
pub(crate) fn deleted_among(
    connection: &Connection,
    runs: &BTreeSet<RunId>,
) -> Result<BTreeSet<RunId>> {
    if runs.is_empty() {
        return Ok(BTreeSet::new());
    }
    let mut statement = connection
        .prepare("SELECT 1 FROM deleted_runs WHERE run_id = ?1")
        .map_err(|error| map_sql("read the deleted runs", error))?;
    let mut deleted = BTreeSet::new();
    for run in runs {
        let found: Option<i64> = statement
            .query_row(params![run.to_storage()], |row| row.get(0))
            .optional()
            .map_err(|error| map_sql("read the deleted runs", error))?;
        if found.is_some() {
            deleted.insert(*run);
        }
    }
    Ok(deleted)
}

impl RunAuthorityRepository for Repository {
    fn issue_authority(&mut self, authority: NewAuthority) -> Result<RunAuthorityRecord> {
        let transaction = self.transaction()?;
        if find_authority(&transaction, authority.run).is_ok() {
            return Err(ArtifactError::AuthorityExists);
        }
        transaction
            .execute(
                "INSERT INTO run_authorities
                   (run_id, workspace_id, attempt_generation, run_workspace_path, state,
                    created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    authority.run.to_storage(),
                    authority.workspace.to_storage(),
                    i64::from(authority.generation.get()),
                    authority.run_workspace.as_str(),
                    AuthorityState::Active.as_str(),
                    authority.created_at_ms,
                ],
            )
            .map_err(|error| map_sql("issue an authority", error))?;
        for workspace in &authority.grants {
            transaction
                .execute(
                    "INSERT INTO run_authority_grants (run_id, workspace_id) VALUES (?1, ?2)",
                    params![authority.run.to_storage(), workspace.to_storage()],
                )
                .map_err(|error| map_sql("record a grant", error))?;
        }
        insert_token(
            &transaction,
            authority.run,
            authority.generation,
            &authority.token_hash,
            authority.created_at_ms,
        )?;
        let issued = find_authority(&transaction, authority.run)?;
        transaction
            .commit()
            .map_err(|error| map_sql("commit an authority", error))?;
        Ok(issued)
    }

    fn authority(&self, run: RunId) -> Result<RunAuthorityRecord> {
        find_authority(&self.connection, run)
    }

    fn authorities_in_workspace(&self, workspace: WorkspaceId) -> Result<Vec<RunAuthorityRecord>> {
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT {AUTHORITY_COLUMNS} FROM run_authorities WHERE workspace_id = ?1
                 ORDER BY created_at_ms DESC, run_id DESC"
            ))
            .map_err(|error| map_sql("list the authorities", error))?;
        let rows = statement
            .query_map(params![workspace.to_storage()], authority_row)
            .map_err(|error| map_sql("list the authorities", error))?;
        let mut found = Vec::new();
        for row in rows {
            let row = row.map_err(|error| map_sql("list the authorities", error))?;
            found.push(authority_from(&self.connection, row)?);
        }
        Ok(found)
    }

    fn advance_authority(
        &mut self,
        run: RunId,
        generation: AttemptGeneration,
        token_hash: TokenHash,
        advanced_at_ms: i64,
    ) -> Result<RunAuthorityRecord> {
        let transaction = self.transaction()?;
        let current = find_authority(&transaction, run)?;
        if current.state == AuthorityState::Revoked {
            return Err(ArtifactError::AuthorityRevoked);
        }
        if generation <= current.generation {
            return Err(ArtifactError::StaleGeneration);
        }
        transaction
            .execute(
                "UPDATE run_authority_tokens SET retired_at_ms = ?2
                 WHERE run_id = ?1 AND retired_at_ms IS NULL",
                params![run.to_storage(), advanced_at_ms],
            )
            .map_err(|error| map_sql("retire a token", error))?;
        transaction
            .execute(
                "UPDATE run_authorities SET attempt_generation = ?2 WHERE run_id = ?1",
                params![run.to_storage(), i64::from(generation.get())],
            )
            .map_err(|error| map_sql("advance a generation", error))?;
        insert_token(&transaction, run, generation, &token_hash, advanced_at_ms)?;
        let advanced = find_authority(&transaction, run)?;
        transaction
            .commit()
            .map_err(|error| map_sql("commit a generation", error))?;
        Ok(advanced)
    }

    fn revoke_authority(&mut self, run: RunId, revoked_at_ms: i64) -> Result<RunAuthorityRecord> {
        let transaction = self.transaction()?;
        let current = find_authority(&transaction, run)?;
        if current.state == AuthorityState::Active {
            transaction
                .execute(
                    "UPDATE run_authorities SET state = ?2, revoked_at_ms = ?3 WHERE run_id = ?1",
                    params![
                        run.to_storage(),
                        AuthorityState::Revoked.as_str(),
                        revoked_at_ms
                    ],
                )
                .map_err(|error| map_sql("revoke an authority", error))?;
        }
        let revoked = find_authority(&transaction, run)?;
        transaction
            .commit()
            .map_err(|error| map_sql("commit a revocation", error))?;
        Ok(revoked)
    }

    fn forget_authority(&mut self, run: RunId, deleted_at_ms: i64) -> Result<()> {
        let transaction = self.transaction()?;
        // Children before the parent: both reference `run_authorities`.
        for statement in [
            "DELETE FROM run_authority_tokens WHERE run_id = ?1",
            "DELETE FROM run_authority_grants WHERE run_id = ?1",
            "DELETE FROM run_authorities WHERE run_id = ?1",
        ] {
            transaction
                .execute(statement, params![run.to_storage()])
                .map_err(|error| map_sql("forget an authority", error))?;
        }
        // The moment of the *first* deletion stands: a second call is the
        // same delete, not a later one.
        transaction
            .execute(
                "INSERT INTO deleted_runs (run_id, deleted_at_ms) VALUES (?1, ?2)
                 ON CONFLICT(run_id) DO NOTHING",
                params![run.to_storage(), deleted_at_ms],
            )
            .map_err(|error| map_sql("record a deleted run", error))?;
        transaction
            .commit()
            .map_err(|error| map_sql("commit a deletion", error))?;
        Ok(())
    }

    fn deleted_runs(&self, runs: &BTreeSet<RunId>) -> Result<BTreeSet<RunId>> {
        deleted_among(&self.connection, runs)
    }

    fn lookup_token(&self, token: &TokenHash) -> Result<Option<TokenLookup>> {
        let found = self
            .connection
            .query_row(
                "SELECT run_id, attempt_generation, retired_at_ms FROM run_authority_tokens
                 WHERE token_hash = ?1",
                params![token.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| map_sql("look up a token", error))?;
        let Some((run, generation, retired_at_ms)) = found else {
            return Ok(None);
        };
        const OP: &str = "decode a token row";
        let run: RunId = decode(OP, run.parse().ok())?;
        Ok(Some(TokenLookup {
            authority: find_authority(&self.connection, run)?,
            token_generation: decode(
                OP,
                u32::try_from(generation)
                    .ok()
                    .and_then(AttemptGeneration::from_raw),
            )?,
            retired: retired_at_ms.is_some(),
        }))
    }
}

#[cfg(test)]
mod tests;
