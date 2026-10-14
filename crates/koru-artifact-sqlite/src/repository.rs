// SPDX-License-Identifier: MPL-2.0

//! The SQLite implementation of [`ArtifactRepository`].
//!
//! Every write is one `BEGIN IMMEDIATE` transaction, so a registration that
//! checks the family, the registration id and the `derived_from` target
//! decides against the same state it inserts into. The store mints the
//! version id and assigns the next ordinal; the caller's registration id is
//! the idempotency key. There is no update path: the trigger in the schema
//! refuses one, and [`map_sql`] turns its message into the same stable code
//! the service answers with.

use std::path::Path;

use koru_artifact_core::{
    Access, ArtifactError, ArtifactFamily, ArtifactFamilyId, ArtifactRepository, ArtifactTitle,
    ArtifactVersion, ArtifactVersionId, AttemptGeneration, BlobHash, Filename, LinkRole, MediaType,
    NewFamily, NewVersion, Ordinal, Permission, Principal, ProducedBy, Provenance, Registered,
    RegistrationId, Result, RunId, RunLink, WorkspaceId,
};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

use crate::{DATABASE_FILE, OpenError};

mod authority;

pub struct Repository {
    connection: Connection,
}

impl Repository {
    /// Open, or create and migrate, `artifacts.db` inside `dir` — the
    /// directory `koru_channel::artifacts_root_beside` names.
    pub fn open(dir: &Path) -> std::result::Result<Self, OpenError> {
        Ok(Self {
            connection: crate::db::open(&dir.join(DATABASE_FILE))?,
        })
    }

    /// A private in-memory database at the current schema, for a test or a
    /// rehearsal that needs the real repository and no file. Nothing in a
    /// product path calls this.
    pub fn open_in_memory() -> std::result::Result<Self, OpenError> {
        Ok(Self {
            connection: crate::db::open_in_memory()?,
        })
    }

    /// The raw connection, for tests that must say something at the SQL
    /// layer — that the trigger refuses an `UPDATE`, for one.
    #[cfg(test)]
    pub(crate) fn connection(&self) -> &Connection {
        &self.connection
    }

    fn transaction(&mut self) -> Result<Transaction<'_>> {
        self.connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| map_sql("begin a transaction", error))
    }
}

/// Every SQLite failure through one function, so the immutability trigger
/// is recognised in one place.
fn map_sql(operation: &'static str, error: rusqlite::Error) -> ArtifactError {
    if let rusqlite::Error::SqliteFailure(_, Some(message)) = &error
        && message.contains(ArtifactError::VersionImmutable.code())
    {
        return ArtifactError::VersionImmutable;
    }
    ArtifactError::Storage {
        operation,
        detail: error.to_string(),
    }
}

fn decode<T>(operation: &'static str, value: Option<T>) -> Result<T> {
    value.ok_or(ArtifactError::Storage {
        operation,
        detail: "a stored value did not parse".into(),
    })
}

type FamilyRow = (String, String, String, Option<String>, i64, Option<i64>);

fn family_row(row: &Row<'_>) -> rusqlite::Result<FamilyRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}

const FAMILY_COLUMNS: &str =
    "family_id, workspace_id, title, created_by_run_id, created_at_ms, retired_at_ms";

fn family_from(
    (id, workspace, title, run, created_at_ms, retired_at_ms): FamilyRow,
) -> Result<ArtifactFamily> {
    const OP: &str = "decode a family row";
    Ok(ArtifactFamily {
        id: decode(OP, id.parse().ok())?,
        workspace: decode(OP, WorkspaceId::parse(&workspace))?,
        title: decode(OP, ArtifactTitle::new(title).ok())?,
        created_by_run: match run {
            Some(run) => Some(decode(OP, run.parse().ok())?),
            None => None,
        },
        created_at_ms,
        retired_at_ms,
    })
}

const VERSION_COLUMNS: &str = "version_id, family_id, ordinal, blob_hash, size_bytes, mime, \
     filename, produced_by_run_id, produced_by_generation, registration_id, derived_from, \
     created_at_ms";

type VersionRow = (
    String,
    String,
    i64,
    String,
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    String,
    Option<String>,
    i64,
);

fn version_row(row: &Row<'_>) -> rusqlite::Result<VersionRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
    ))
}

fn version_from(
    (
        id,
        family,
        ordinal,
        blob,
        size_bytes,
        mime,
        filename,
        run,
        generation,
        registration,
        derived_from,
        created_at_ms,
    ): VersionRow,
) -> Result<ArtifactVersion> {
    const OP: &str = "decode a version row";
    let produced_by = match (run, generation) {
        (Some(run), Some(generation)) => Some(ProducedBy {
            run: decode(OP, run.parse().ok())?,
            generation: decode(
                OP,
                u32::try_from(generation)
                    .ok()
                    .and_then(AttemptGeneration::from_raw),
            )?,
        }),
        _ => None,
    };
    Ok(ArtifactVersion {
        id: decode(OP, id.parse().ok())?,
        family: decode(OP, family.parse().ok())?,
        ordinal: decode(OP, u32::try_from(ordinal).ok().and_then(Ordinal::from_raw))?,
        blob: decode(OP, BlobHash::parse(&blob).ok())?,
        size_bytes: decode(OP, u64::try_from(size_bytes).ok())?,
        mime: decode(OP, MediaType::new(mime).ok())?,
        filename: match filename {
            Some(name) => Some(decode(OP, Filename::new(name).ok())?),
            None => None,
        },
        produced_by,
        registration_id: decode(OP, registration.parse().ok())?,
        derived_from: match derived_from {
            Some(from) => Some(decode(OP, from.parse().ok())?),
            None => None,
        },
        created_at_ms,
    })
}

fn find_family(connection: &Connection, id: ArtifactFamilyId) -> Result<ArtifactFamily> {
    connection
        .query_row(
            &format!("SELECT {FAMILY_COLUMNS} FROM artifact_families WHERE family_id = ?1"),
            params![id.to_storage()],
            family_row,
        )
        .optional()
        .map_err(|error| map_sql("read a family", error))?
        .ok_or(ArtifactError::FamilyNotFound)
        .and_then(family_from)
}

fn find_version(connection: &Connection, id: ArtifactVersionId) -> Result<ArtifactVersion> {
    version_where(connection, "version_id = ?1", &id.to_storage())?
        .ok_or(ArtifactError::VersionNotFound)
}

fn version_where(
    connection: &Connection,
    predicate: &str,
    value: &str,
) -> Result<Option<ArtifactVersion>> {
    connection
        .query_row(
            &format!("SELECT {VERSION_COLUMNS} FROM artifact_versions WHERE {predicate}"),
            params![value],
            version_row,
        )
        .optional()
        .map_err(|error| map_sql("read a version", error))?
        .map(version_from)
        .transpose()
}

fn links_of(connection: &Connection, version: ArtifactVersionId) -> Result<Vec<RunLink>> {
    let mut statement = connection
        .prepare(
            "SELECT run_id, role, linked_at_ms FROM artifact_run_links
             WHERE version_id = ?1 ORDER BY linked_at_ms, run_id, role",
        )
        .map_err(|error| map_sql("read run links", error))?;
    let rows = statement
        .query_map(params![version.to_storage()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|error| map_sql("read run links", error))?;
    let mut links = Vec::new();
    for row in rows {
        let (run, role, linked_at_ms) = row.map_err(|error| map_sql("read run links", error))?;
        const OP: &str = "decode a run link";
        links.push(RunLink {
            run: decode(OP, run.parse::<RunId>().ok())?,
            version,
            role: decode(OP, LinkRole::parse(&role))?,
            linked_at_ms,
        });
    }
    Ok(links)
}

impl ArtifactRepository for Repository {
    fn create_family(&mut self, family: NewFamily, owner: Principal) -> Result<ArtifactFamily> {
        let transaction = self.transaction()?;
        if find_family(&transaction, family.id).is_ok() {
            return Err(ArtifactError::FamilyExists);
        }
        transaction
            .execute(
                "INSERT INTO artifact_families
                   (family_id, workspace_id, title, created_by_run_id, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    family.id.to_storage(),
                    family.workspace.to_storage(),
                    family.title.as_str(),
                    family.created_by_run.map(RunId::to_storage),
                    family.created_at_ms,
                ],
            )
            .map_err(|error| map_sql("create a family", error))?;
        transaction
            .execute(
                "INSERT INTO artifact_permissions (family_id, principal, access, granted_at_ms)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    family.id.to_storage(),
                    owner.to_storage(),
                    Access::Owner.as_str(),
                    family.created_at_ms,
                ],
            )
            .map_err(|error| map_sql("grant the owner", error))?;
        let created = find_family(&transaction, family.id)?;
        transaction
            .commit()
            .map_err(|error| map_sql("commit a family", error))?;
        Ok(created)
    }

    fn family(&self, id: ArtifactFamilyId) -> Result<ArtifactFamily> {
        find_family(&self.connection, id)
    }

    fn list_families(
        &self,
        workspace: WorkspaceId,
        include_retired: bool,
    ) -> Result<Vec<ArtifactFamily>> {
        let filter = if include_retired {
            ""
        } else {
            " AND retired_at_ms IS NULL"
        };
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT {FAMILY_COLUMNS} FROM artifact_families
                 WHERE workspace_id = ?1{filter} ORDER BY created_at_ms, family_id"
            ))
            .map_err(|error| map_sql("list families", error))?;
        let rows = statement
            .query_map(params![workspace.to_storage()], family_row)
            .map_err(|error| map_sql("list families", error))?;
        rows.map(|row| {
            row.map_err(|error| map_sql("list families", error))
                .and_then(family_from)
        })
        .collect()
    }

    fn retire_family(
        &mut self,
        id: ArtifactFamilyId,
        retired_at_ms: i64,
    ) -> Result<ArtifactFamily> {
        let transaction = self.transaction()?;
        let family = find_family(&transaction, id)?;
        if !family.is_retired() {
            transaction
                .execute(
                    "UPDATE artifact_families SET retired_at_ms = ?2 WHERE family_id = ?1",
                    params![id.to_storage(), retired_at_ms],
                )
                .map_err(|error| map_sql("retire a family", error))?;
        }
        let retired = find_family(&transaction, id)?;
        transaction
            .commit()
            .map_err(|error| map_sql("commit a retirement", error))?;
        Ok(retired)
    }

    fn register_version(&mut self, version: NewVersion) -> Result<Registered> {
        let transaction = self.transaction()?;
        if let Some(existing) = version_where(
            &transaction,
            "registration_id = ?1",
            &version.registration_id.to_storage(),
        )? {
            // §7.1: a retry returns the existing version, never a duplicate —
            // and a "retry" under another family or with other bytes is not
            // a retry at all.
            if existing.family == version.family && existing.blob == version.blob {
                return Ok(Registered {
                    version: existing,
                    retried: true,
                });
            }
            return Err(ArtifactError::RegistrationConflict);
        }
        let family = find_family(&transaction, version.family)?;
        if family.is_retired() {
            return Err(ArtifactError::FamilyRetired);
        }
        if let Some(from) = version.derived_from {
            find_version(&transaction, from)?;
        }
        let ordinal: i64 = transaction
            .query_row(
                "SELECT COALESCE(MAX(ordinal), 0) + 1 FROM artifact_versions WHERE family_id = ?1",
                params![version.family.to_storage()],
                |row| row.get(0),
            )
            .map_err(|error| map_sql("assign an ordinal", error))?;
        let id = ArtifactVersionId::mint();
        transaction
            .execute(
                "INSERT INTO artifact_versions
                   (version_id, family_id, ordinal, blob_hash, size_bytes, mime, filename,
                    produced_by_run_id, produced_by_generation, registration_id, derived_from,
                    created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    id.to_storage(),
                    version.family.to_storage(),
                    ordinal,
                    version.blob.as_str(),
                    i64::try_from(version.size_bytes).map_err(|_| ArtifactError::InvalidRequest)?,
                    version.mime.as_str(),
                    version.filename.as_ref().map(Filename::as_str),
                    version.produced_by.map(|by| by.run.to_storage()),
                    version.produced_by.map(|by| i64::from(by.generation.get())),
                    version.registration_id.to_storage(),
                    version.derived_from.map(ArtifactVersionId::to_storage),
                    version.created_at_ms,
                ],
            )
            .map_err(|error| map_sql("register a version", error))?;
        let registered = find_version(&transaction, id)?;
        transaction
            .commit()
            .map_err(|error| map_sql("commit a registration", error))?;
        Ok(Registered {
            version: registered,
            retried: false,
        })
    }

    fn version(&self, id: ArtifactVersionId) -> Result<ArtifactVersion> {
        find_version(&self.connection, id)
    }

    fn version_by_registration(&self, id: RegistrationId) -> Result<Option<ArtifactVersion>> {
        version_where(&self.connection, "registration_id = ?1", &id.to_storage())
    }

    fn versions_produced_by(&self, run: RunId) -> Result<Vec<ArtifactVersion>> {
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT {VERSION_COLUMNS} FROM artifact_versions
                 WHERE produced_by_run_id = ?1 ORDER BY created_at_ms, version_id"
            ))
            .map_err(|error| map_sql("list a run's versions", error))?;
        let rows = statement
            .query_map(params![run.to_storage()], version_row)
            .map_err(|error| map_sql("list a run's versions", error))?;
        rows.map(|row| {
            row.map_err(|error| map_sql("list a run's versions", error))
                .and_then(version_from)
        })
        .collect()
    }

    fn versions(&self, family: ArtifactFamilyId) -> Result<Vec<ArtifactVersion>> {
        find_family(&self.connection, family)?;
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT {VERSION_COLUMNS} FROM artifact_versions
                 WHERE family_id = ?1 ORDER BY ordinal"
            ))
            .map_err(|error| map_sql("list versions", error))?;
        let rows = statement
            .query_map(params![family.to_storage()], version_row)
            .map_err(|error| map_sql("list versions", error))?;
        rows.map(|row| {
            row.map_err(|error| map_sql("list versions", error))
                .and_then(version_from)
        })
        .collect()
    }

    fn link_run(&mut self, link: RunLink) -> Result<()> {
        let transaction = self.transaction()?;
        find_version(&transaction, link.version)?;
        transaction
            .execute(
                "INSERT OR IGNORE INTO artifact_run_links (run_id, version_id, role, linked_at_ms)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    link.run.to_storage(),
                    link.version.to_storage(),
                    link.role.as_str(),
                    link.linked_at_ms,
                ],
            )
            .map_err(|error| map_sql("link a run", error))?;
        transaction
            .commit()
            .map_err(|error| map_sql("commit a run link", error))
    }

    fn provenance(&self, id: ArtifactVersionId) -> Result<Provenance> {
        let subject = find_version(&self.connection, id)?;
        let mut runs = links_of(&self.connection, subject.id)?;
        let mut ancestors = Vec::new();
        let mut seen = std::collections::BTreeSet::from([subject.id]);
        let mut next = subject.derived_from;
        while let Some(from) = next {
            // Immutable rows referencing earlier rows cannot form a cycle;
            // the guard is for a database edited by hand, not for the store.
            if !seen.insert(from) {
                break;
            }
            let ancestor = find_version(&self.connection, from)?;
            runs.extend(links_of(&self.connection, ancestor.id)?);
            next = ancestor.derived_from;
            ancestors.push(ancestor);
        }
        // **Which of the runs this walk names are gone.** Asked of the runs
        // the sentence will actually print — the producers and the links —
        // rather than of the whole table, so the cost is the walk's own.
        let mut named: std::collections::BTreeSet<RunId> =
            runs.iter().map(|link| link.run).collect();
        named.extend(
            std::iter::once(&subject)
                .chain(ancestors.iter())
                .filter_map(|version| version.produced_by.map(|by| by.run)),
        );
        let deleted_runs = authority::deleted_among(&self.connection, &named)?;
        Ok(Provenance {
            subject,
            ancestors,
            runs,
            deleted_runs,
        })
    }

    fn permissions(&self, family: ArtifactFamilyId) -> Result<Vec<Permission>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT principal, access, granted_at_ms FROM artifact_permissions
                 WHERE family_id = ?1 ORDER BY granted_at_ms, principal",
            )
            .map_err(|error| map_sql("read permissions", error))?;
        let rows = statement
            .query_map(params![family.to_storage()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(|error| map_sql("read permissions", error))?;
        let mut permissions = Vec::new();
        for row in rows {
            let (principal, access, granted_at_ms) =
                row.map_err(|error| map_sql("read permissions", error))?;
            const OP: &str = "decode a permission";
            permissions.push(Permission {
                family,
                principal: decode(OP, Principal::parse(&principal).ok())?,
                access: decode(OP, Access::parse(&access))?,
                granted_at_ms,
            });
        }
        Ok(permissions)
    }
}

#[cfg(test)]
mod tests;
