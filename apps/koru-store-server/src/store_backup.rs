// SPDX-License-Identifier: MPL-2.0

//! One-file backup of the authoritative Koru Store.
//!
//! Derived search/projection databases, logs and recovery checkpoints are not
//! included: `catalog.db`, each `workspace.db`, and blobs are sufficient to
//! reconstruct everything a client can see. SQLite files are captured with
//! `VACUUM INTO`, so committed WAL pages are part of the backup.
//!
//! **Blobs are two places, and both are taken.** `<root>/blobs/objects/` is
//! where every blob now lives; `<root>/workspaces/<id>/blobs/` is where an
//! older build filed them, and a store that has not been read since the
//! upgrade still has bytes only there. Taking one and not the other would make
//! a backup that opens cleanly and has lost the pictures. Staging is excluded
//! — a half-written upload is not content — and quarantine is kept, because it
//! is the only copy of bytes somebody may still want to look at.

use std::fs::{self, File};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use koru_store_core::errors::{Code, Error, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;

const FORMAT: u32 = 1;
// The backup format is persistent data, not the host executable's name.
const EXTENSION: &str = "koru-store";

/// The daemon-side implementation handed to the authenticated HTTP API.
pub struct LiveBackup {
    root: PathBuf,
}

impl LiveBackup {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }
}

impl koru_store_api::routes::admin::BackupService for LiveBackup {
    fn create(&self, destination: &Path) -> Result<()> {
        create(&self.root, destination)
    }
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: u32,
    created_at: i64,
}

pub fn backup_from_cli(root: &Path, destination: Option<&Path>) -> Result<()> {
    if root_is_claimed(root) {
        return Err(Error::new(
            Code::WorkspaceInUse,
            "the Store is running; use Backup Store from its tray menu or quit it first",
        ));
    }
    let destination = destination.map(Path::to_path_buf).unwrap_or_else(|| {
        PathBuf::from(format!(
            "Koru Store {}.{EXTENSION}",
            stamp(koru_store_core::model::Timestamp::now().millis())
        ))
    });
    create(root, &destination)?;
    println!("Store backup written to {}", destination.display());
    Ok(())
}

pub fn restore_from_cli(root: &Path, source: &Path) -> Result<()> {
    if root_is_claimed(root) {
        return Err(Error::new(
            Code::WorkspaceInUse,
            "quit the Koru Store before restoring a backup",
        ));
    }
    let previous = restore(root, source)?;
    println!("Store restored from {}", source.display());
    if let Some(previous) = previous {
        println!("Previous Store kept at {}", previous.display());
    }
    Ok(())
}

fn root_is_claimed(root: &Path) -> bool {
    koru_ipc::Endpoint::for_service_root(koru_ipc::STORE, koru_channel::segment(), root)
        .is_ok_and(|endpoint| koru_ipc::probe(&endpoint))
}

pub fn create(root: &Path, destination: &Path) -> Result<()> {
    let staging =
        tempfile::tempdir().map_err(|error| failed("create backup staging directory", error))?;
    let catalog = root.join(crate::daemon::CATALOG_FILE);
    if catalog.exists() {
        snapshot(&catalog, &staging.path().join("catalog.db"))?;
    }

    // The object store, minus its staging directory.
    let objects = root.join(koru_store_sqlite::backend::BLOBS_DIR);
    if let Ok(entries) = fs::read_dir(&objects) {
        for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
            if entry.file_name() == koru_store_sqlite::blobs::STAGING_DIR {
                continue;
            }
            copy_tree(
                &entry.path(),
                &staging
                    .path()
                    .join(koru_store_sqlite::backend::BLOBS_DIR)
                    .join(entry.file_name()),
            )?;
        }
    }

    let workspaces = root.join(koru_store_sqlite::backend::WORKSPACES_DIR);
    if let Ok(entries) = fs::read_dir(&workspaces) {
        for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
            let name = entry.file_name();
            let source = entry.path();
            let target = staging.path().join("workspaces").join(&name);
            let database = source.join(koru_store_sqlite::backend::WORKSPACE_FILE);
            if database.exists() {
                fs::create_dir_all(&target)
                    .map_err(|error| failed("create workspace staging directory", error))?;
                snapshot(
                    &database,
                    &target.join(koru_store_sqlite::backend::WORKSPACE_FILE),
                )?;
            }
            copy_tree(
                &source.join(koru_store_sqlite::backend::BLOBS_DIR),
                &target.join(koru_store_sqlite::backend::BLOBS_DIR),
            )?;
        }
    }

    let manifest = Manifest {
        format: FORMAT,
        created_at: koru_store_core::model::Timestamp::now().millis(),
    };
    fs::write(
        staging.path().join("backup.json"),
        serde_json::to_vec_pretty(&manifest)
            .map_err(|error| Error::new(Code::Internal, error.to_string()))?,
    )
    .map_err(|error| failed("write backup manifest", error))?;

    if let Some(parent) = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|error| failed("create backup destination", error))?;
    }
    let part = part_path(destination);
    let result = (|| {
        let file = File::create(&part).map_err(|error| failed("create backup file", error))?;
        let mut archive = zip::ZipWriter::new(file);
        add_tree(&mut archive, staging.path(), staging.path())?;
        let file = archive
            .finish()
            .map_err(|error| Error::new(Code::StorageFailed, format!("finish backup: {error}")))?;
        file.sync_all()
            .map_err(|error| failed("flush backup", error))?;
        replace(&part, destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result
}

pub fn restore(root: &Path, source: &Path) -> Result<Option<PathBuf>> {
    let file = File::open(source).map_err(|error| failed("open backup", error))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| Error::new(Code::BundleFormatUnsupported, error.to_string()))?;
    let manifest: Manifest = {
        let mut entry = archive
            .by_name("backup.json")
            .map_err(|_| Error::bare(Code::BundleFormatUnsupported))?;
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| failed("read backup manifest", error))?;
        serde_json::from_slice(&bytes)
            .map_err(|error| Error::new(Code::BundleFormatUnsupported, error.to_string()))?
    };
    if manifest.format != FORMAT {
        return Err(Error::bare(Code::BundleFormatUnsupported));
    }

    let parent = root.parent().ok_or_else(|| {
        Error::new(
            Code::InvalidRequest,
            "the Store root has no parent directory",
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| failed("create Store parent", error))?;
    let leaf = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("store");
    let staged = parent.join(format!(".{leaf}.restore-{}", std::process::id()));
    if staged.exists() {
        fs::remove_dir_all(&staged).map_err(|error| failed("clear restore staging", error))?;
    }
    fs::create_dir_all(&staged).map_err(|error| failed("create restore staging", error))?;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| Error::new(Code::BundleFormatUnsupported, error.to_string()))?;
        let Some(relative) = entry.enclosed_name() else {
            return Err(Error::new(
                Code::BundleFormatUnsupported,
                "unsafe archive path",
            ));
        };
        let target = staged.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&target)
                .map_err(|error| failed("create restored directory", error))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| failed("create restored file directory", error))?;
        }
        let mut output =
            File::create(&target).map_err(|error| failed("create restored file", error))?;
        std::io::copy(&mut entry, &mut output).map_err(|error| failed("extract backup", error))?;
        output
            .sync_all()
            .map_err(|error| failed("flush restored file", error))?;
    }

    // Opening the catalog proves its header/schema before the live directory is
    // touched. Workspace databases are likewise opened from staging.
    let _ = koru_store_sqlite::SqliteCatalog::open(&staged.join("catalog.db"))?;
    let workspace_root = staged.join("workspaces");
    if let Ok(entries) = fs::read_dir(&workspace_root) {
        for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
            let database = entry
                .path()
                .join(koru_store_sqlite::backend::WORKSPACE_FILE);
            if database.exists() {
                let _ = koru_store_sqlite::SqliteWorkspace::open(&database)?;
            }
        }
    }

    let previous = root.exists().then(|| {
        parent.join(format!(
            "{leaf}.before-restore-{}",
            stamp(koru_store_core::model::Timestamp::now().millis())
        ))
    });
    if let Some(previous) = &previous {
        fs::rename(root, previous).map_err(|error| failed("preserve previous Store", error))?;
    }
    if let Err(error) = fs::rename(&staged, root) {
        if let Some(previous) = &previous {
            let _ = fs::rename(previous, root);
        }
        return Err(failed("install restored Store", error));
    }
    Ok(previous)
}

fn snapshot(source: &Path, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| failed("create snapshot directory", error))?;
    }
    let _ = fs::remove_file(destination);
    let connection = rusqlite::Connection::open(source)
        .map_err(|error| Error::new(Code::StorageFailed, format!("open database: {error}")))?;
    connection
        .execute(
            "VACUUM INTO ?1",
            params![destination.to_string_lossy().as_ref()],
        )
        .map_err(|error| Error::new(Code::StorageFailed, format!("snapshot database: {error}")))?;
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    let Ok(entries) = fs::read_dir(source) else {
        return Ok(());
    };
    fs::create_dir_all(destination).map_err(|error| failed("create blob staging", error))?;
    for entry in entries.flatten() {
        let target = destination.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target).map_err(|error| failed("copy blob", error))?;
        }
    }
    Ok(())
}

fn add_tree<W: Write + Seek>(
    archive: &mut zip::ZipWriter<W>,
    root: &Path,
    at: &Path,
) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(at)
        .map_err(|error| failed("read backup staging", error))?
        .flatten()
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            add_tree(archive, root, &path)?;
            continue;
        }
        let name = path
            .strip_prefix(root)
            .expect("entry is below root")
            .to_string_lossy()
            .replace('\\', "/");
        archive
            .start_file(
                name,
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
            )
            .map_err(|error| {
                Error::new(Code::StorageFailed, format!("write backup entry: {error}"))
            })?;
        let mut input =
            File::open(&path).map_err(|error| failed("open staged backup entry", error))?;
        std::io::copy(&mut input, archive).map_err(|error| failed("write backup entry", error))?;
    }
    Ok(())
}

fn part_path(destination: &Path) -> PathBuf {
    let mut name = destination.as_os_str().to_owned();
    name.push(format!(".{}.part", std::process::id()));
    PathBuf::from(name)
}

fn replace(part: &Path, destination: &Path) -> Result<()> {
    if destination.exists() {
        fs::remove_file(destination).map_err(|error| failed("replace previous backup", error))?;
    }
    fs::rename(part, destination).map_err(|error| failed("publish backup", error))
}

fn stamp(millis: i64) -> String {
    // Stable, sortable and filesystem-safe without pulling a calendar into the
    // daemon. Milliseconds also avoid two backups naming one file.
    millis.to_string()
}

fn failed(context: &str, error: impl std::fmt::Display) -> Error {
    Error::new(Code::StorageFailed, format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use koru_store_core::model::Timestamp;
    use koru_store_core::service::Store;
    use koru_store_core::text::Locale;
    use koru_store_sqlite::{SqliteBackend, SqliteCatalog};

    fn open(root: &Path) -> Store<SqliteCatalog, SqliteBackend> {
        Store::open(
            SqliteCatalog::open(&root.join("catalog.db")).unwrap(),
            SqliteBackend::new(root),
        )
        .unwrap()
    }

    #[test]
    fn one_file_restores_catalog_workspace_database_and_blob() {
        let source = tempfile::tempdir().unwrap();
        let archive_dir = tempfile::tempdir().unwrap();
        let archive = archive_dir.path().join("everything.koru-store");
        let restored = archive_dir.path().join("restored-store");
        let at = Timestamp::from_millis(1_770_000_000_000);

        let (workspace, hash) = {
            let mut store = open(source.path());
            let profile = store
                .create_profile("Owner", None, None, Locale::parse("en").unwrap(), at)
                .unwrap();
            let workspace = store.create_workspace(profile.id, "Kept", at).unwrap().id;
            let blob = store
                .put_blob(
                    workspace,
                    b"image bytes",
                    "image/png",
                    Some("one.png"),
                    at,
                    None,
                )
                .unwrap();
            (workspace, blob.hash)
        };

        create(source.path(), &archive).unwrap();
        assert!(archive.is_file());
        assert_eq!(restore(&restored, &archive).unwrap(), None);

        let mut store = open(&restored);
        assert_eq!(store.workspaces().unwrap()[0].name, "Kept");
        assert_eq!(store.get_blob(workspace, &hash).unwrap(), b"image bytes");
    }
}
