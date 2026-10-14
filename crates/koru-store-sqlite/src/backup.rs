// SPDX-License-Identifier: MPL-2.0

//! Local recovery copies, taken with `VACUUM INTO`.
//!
//! ## Why `VACUUM INTO` rather than copying the file
//!
//! A live WAL database is **two or three files** — `x.db`, `x.db-wal`,
//! `x.db-shm` — and the newest committed data is in the WAL, not the main file.
//! Copying `x.db` alone while the store is running produces a backup missing
//! everything since the last checkpoint, and it looks perfectly valid. Copying
//! all three racing a writer produces a mismatched set, which looks valid until
//! it is opened.
//!
//! `VACUUM INTO` asks SQLite for a **consistent copy of the whole database as
//! of one transaction**, written as a single tidy file, without stopping
//! writers. It is a capability we already link, and hand-rolling it is how a
//! backup silently becomes a snapshot of the wrong moment.
//!
//! ## The restore trap
//!
//! Putting a backup back is **not** "copy the file over the old one". If a
//! stale `-wal` is left beside it, SQLite finds a WAL whose header matches
//! nothing in the restored database and either replays it over the top or
//! refuses the file. Either way the restore is worse than not restoring.
//!
//! So [`SqliteBackups::restore`] removes the sidecars, and it does so **after**
//! the new main file is in place, never before — the sequence is
//! write-beside, swap, then clear, so an interruption at any point leaves
//! either the old database or the new one, and never a main file from one
//! generation with a WAL from another.
//!
//! Nothing may hold the database open across a restore. That is the caller's
//! responsibility and it is checked by the daemon, not here.

use std::path::{Path, PathBuf};

use koru_store_core::errors::{Code, Error, Result};
use koru_store_core::model::Timestamp;
use koru_store_core::ports::{BackupId, BackupInfo, Backups};
use rusqlite::params;

use crate::db;

/// Timestamped copies of one database.
pub struct SqliteBackups {
    source: PathBuf,
    dir: PathBuf,
    prefix: String,
}

impl SqliteBackups {
    /// `source` is the live database; `dir` is where copies go.
    pub fn new(source: impl Into<PathBuf>, dir: impl Into<PathBuf>) -> Self {
        let source = source.into();
        let prefix = source
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("backup")
            .to_string();
        Self {
            source,
            dir: dir.into(),
            prefix,
        }
    }

    fn path_of(&self, id: &BackupId) -> Result<PathBuf> {
        // `BackupId` can arrive from an API request, so it is treated as
        // untrusted: a name, never a path. Anything with a separator, a parent
        // reference or a drive letter is refused rather than resolved.
        let name = &id.0;
        if name.is_empty()
            || name.contains('/')
            || name.contains('\\')
            || name.contains("..")
            || name.contains(':')
        {
            return Err(Error::new(Code::InvalidRequest, "not a backup name"));
        }
        if !name.starts_with(&self.prefix) || !name.ends_with(".db") {
            return Err(Error::bare(Code::BackupNotFound));
        }
        Ok(self.dir.join(name))
    }

    /// `<prefix>-<epoch_ms>.db`.
    ///
    /// Epoch milliseconds rather than `YYYYMMDD-HHMMSS`: it carries no calendar
    /// and no locale, and it sorts lexically only by accident — so [`list`]
    /// sorts on the parsed number instead.
    ///
    /// [`list`]: Backups::list
    fn name_for(&self, at: Timestamp) -> String {
        format!("{}-{}.db", self.prefix, at.for_filename())
    }

    fn taken_at(&self, name: &str) -> Option<Timestamp> {
        name.strip_prefix(&self.prefix)?
            .strip_prefix('-')?
            .strip_suffix(".db")?
            .parse::<i64>()
            .ok()
            .map(Timestamp::from_millis)
    }

    /// Remove `-wal` and `-shm` beside a database file.
    fn clear_sidecars(path: &Path) -> Result<()> {
        for suffix in ["-wal", "-shm"] {
            let mut sidecar = path.as_os_str().to_owned();
            sidecar.push(suffix);
            match std::fs::remove_file(PathBuf::from(sidecar)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(Error::new(
                        Code::StorageFailed,
                        format!("could not clear a stale {suffix} file: {e}"),
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Backups for SqliteBackups {
    fn take(&self, at: Timestamp) -> Result<BackupId> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| Error::new(Code::StorageFailed, format!("backup directory: {e}")))?;
        let name = self.name_for(at);
        let destination = self.dir.join(&name);
        // `VACUUM INTO` refuses to overwrite, so a second backup in the same
        // millisecond would fail. Clearing first makes a repeat idempotent
        // rather than an error nobody can act on.
        let _ = std::fs::remove_file(&destination);

        let connection = db::open(&self.source)?;
        connection
            .execute(
                "VACUUM INTO ?1",
                params![destination.to_string_lossy().as_ref()],
            )
            .map_err(|e| Error::new(Code::StorageFailed, format!("VACUUM INTO: {e}")))?;
        Ok(BackupId(name))
    }

    fn list(&self) -> Result<Vec<BackupInfo>> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            // No directory is no backups, not a failure.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::new(Code::StorageFailed, format!("list: {e}"))),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some(taken_at) = self.taken_at(&name) else {
                continue;
            };
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push(BackupInfo {
                id: BackupId(name),
                taken_at,
                size,
            });
        }
        // On the parsed instant, not on the name.
        out.sort_by(|a, b| b.taken_at.cmp(&a.taken_at));
        Ok(out)
    }

    fn restore(&self, id: &BackupId) -> Result<()> {
        let from = self.path_of(id)?;
        if !from.exists() {
            return Err(Error::bare(Code::BackupNotFound));
        }
        // Verify before replacing anything: a backup that will not open is not
        // a backup, and finding that out *after* overwriting the live database
        // is the worst possible moment.
        //
        // Every failure here is `CorruptState`, including the ones SQLite
        // reports as an open failure. The distinction matters to a caller:
        // `StorageFailed` says the machinery is unwell and a retry might work,
        // where this says *this artefact is not usable* and another one should
        // be tried instead.
        verify(&from).map_err(|e| {
            Error::new(
                Code::CorruptState,
                format!("the backup does not open: {}", e.message()),
            )
        })?;

        // Beside, then swap, then clear. An interruption leaves either the old
        // database or the new one — never a main file from one generation with
        // a WAL from another.
        let staging = self.source.with_extension("restore.part");
        std::fs::copy(&from, &staging)
            .map_err(|e| Error::new(Code::StorageFailed, format!("stage restore: {e}")))?;
        std::fs::rename(&staging, &self.source)
            .map_err(|e| Error::new(Code::StorageFailed, format!("swap in restore: {e}")))?;
        Self::clear_sidecars(&self.source)?;
        Ok(())
    }

    fn prune(&self, keep: usize) -> Result<usize> {
        let all = self.list()?;
        let mut removed = 0;
        for info in all.into_iter().skip(keep) {
            if std::fs::remove_file(self.dir.join(&info.id.0)).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// Open a database file and check it reads back intact.
fn verify(path: &Path) -> Result<()> {
    let connection = db::open(path)?;
    if !db::quick_check(&connection)? {
        return Err(Error::new(
            Code::CorruptState,
            "it does not pass its own integrity check",
        ));
    }
    Ok(())
}

/// How many copies are kept.
///
/// Twenty, matching `koru_docstore::backup::BACKUPS_KEPT`, rather than inventing
/// a second retention policy for the same kind of artefact.
pub const BACKUPS_KEPT: usize = 20;

#[cfg(test)]
mod tests;
