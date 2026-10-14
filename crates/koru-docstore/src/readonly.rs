// SPDX-License-Identifier: MPL-2.0

//! Strictly read-only access to a legacy workspace directory.

use std::fs;
use std::path::{Path, PathBuf};

use koru_core::schema::WorkspaceDoc;

use crate::{Result, StoreIoError};

pub struct ReadonlyWorkspace {
    pub doc: WorkspaceDoc,
    pub snapshot: PathBuf,
    pub warnings: Vec<String>,
}

/// Load a snapshot, falling back to the newest readable backup, then replay
/// every complete update frame. This function performs no filesystem writes.
pub fn load_workspace(folder: &Path) -> Result<ReadonlyWorkspace> {
    let primary = folder.join("snapshot.loro");
    let mut candidates = vec![primary.clone()];
    candidates.extend(crate::backup::list(&folder.join("backups")));

    let mut failures = Vec::new();
    let mut loaded = None;
    for path in candidates {
        match fs::read(&path) {
            Ok(bytes) => match WorkspaceDoc::from_snapshot(&bytes) {
                Ok(doc) => {
                    loaded = Some((doc, path));
                    break;
                }
                Err(e) => failures.push(format!("{}: {e}", path.display())),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                failures.push(format!("{}: missing", path.display()))
            }
            Err(e) => failures.push(format!("{}: {e}", path.display())),
        }
    }
    let Some((mut doc, snapshot)) = loaded else {
        return Err(StoreIoError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("no readable legacy snapshot ({})", failures.join("; ")),
        )));
    };

    let mut warnings = Vec::new();
    if snapshot != primary {
        warnings.push(format!(
            "The current snapshot could not be read; using backup {}.",
            snapshot.display()
        ));
    }

    let updates = folder.join("updates");
    let mut logs: Vec<PathBuf> = match fs::read_dir(&updates) {
        Ok(entries) => entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "loro"))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    logs.sort_unstable();

    for path in logs {
        let bytes = fs::read(&path)?;
        let mut cursor = 0usize;
        let mut frames = Vec::new();
        while cursor + 4 <= bytes.len() {
            let length = u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().unwrap()) as usize;
            let start = cursor + 4;
            let Some(end) = start.checked_add(length) else {
                break;
            };
            if end > bytes.len() {
                break;
            }
            let frame = &bytes[start..end];
            if !doc.frame_is_known(frame) {
                frames.push(frame.to_vec());
            }
            cursor = end;
        }
        if cursor != bytes.len() {
            warnings.push(format!(
                "{} ends with an incomplete update; complete updates before it were used.",
                path.display()
            ));
        }
        if !frames.is_empty()
            && let Err(e) = doc.import_frames(&frames)
        {
            warnings.push(format!(
                "{} contains damaged updates and was skipped: {e}",
                path.display()
            ));
        }
    }

    Ok(ReadonlyWorkspace {
        doc,
        snapshot,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use koru_core::{WorkspaceId, node::Node, schema::Parent};

    #[test]
    fn loading_does_not_create_or_change_any_file() {
        let temp = tempfile::tempdir().unwrap();
        let mut doc = WorkspaceDoc::create(WorkspaceId::new(), "Old").unwrap();
        doc.insert(Parent::Root, Node::document("Page").unwrap())
            .unwrap();
        fs::write(
            temp.path().join("snapshot.loro"),
            doc.export_snapshot().unwrap(),
        )
        .unwrap();
        let before = fs::read(temp.path().join("snapshot.loro")).unwrap();
        let loaded = load_workspace(temp.path()).unwrap();
        assert_eq!(loaded.doc.explore().unwrap().len(), 1);
        assert_eq!(fs::read(temp.path().join("snapshot.loro")).unwrap(), before);
        assert!(!temp.path().join("quarantine").exists());
        assert!(!temp.path().join("updates").exists());
    }

    #[test]
    fn falls_back_to_a_backup_and_replays_complete_updates_without_writing() {
        let temp = tempfile::tempdir().unwrap();
        let mut doc = WorkspaceDoc::create(WorkspaceId::new(), "Old").unwrap();
        let before = doc.version();
        let backup = doc.export_snapshot().unwrap();
        let page = doc
            .insert(Parent::Root, Node::document("From updates").unwrap())
            .unwrap();
        let update = doc.export_updates_since(&before).unwrap();

        fs::write(temp.path().join("snapshot.loro"), b"damaged").unwrap();
        fs::create_dir_all(temp.path().join("backups")).unwrap();
        fs::write(
            temp.path()
                .join("backups")
                .join("snapshot-20260813-120000.loro"),
            &backup,
        )
        .unwrap();
        fs::create_dir_all(temp.path().join("updates")).unwrap();
        let mut framed = Vec::new();
        framed.extend_from_slice(&(update.len() as u32).to_le_bytes());
        framed.extend_from_slice(&update);
        let update_path = temp.path().join("updates").join("peer-A.loro");
        fs::write(&update_path, &framed).unwrap();
        let primary_before = fs::read(temp.path().join("snapshot.loro")).unwrap();
        let update_before = fs::read(&update_path).unwrap();

        let loaded = load_workspace(temp.path()).unwrap();
        assert_eq!(
            loaded.doc.read(page).unwrap().content.as_text(),
            Some("From updates")
        );
        assert!(loaded.snapshot.ends_with("snapshot-20260813-120000.loro"));
        assert_eq!(loaded.warnings.len(), 1);
        assert_eq!(
            fs::read(temp.path().join("snapshot.loro")).unwrap(),
            primary_before
        );
        assert_eq!(fs::read(update_path).unwrap(), update_before);
    }
}
