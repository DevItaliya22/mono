// SPDX-License-Identifier: MPL-2.0

use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use crate::UpdatePaths;

pub struct InstanceLease {
    file: File,
    update_file: File,
    restart_file: Option<File>,
    path: PathBuf,
    directory: PathBuf,
}

impl InstanceLease {
    pub fn acquire(paths: &UpdatePaths, install_dir: &Path) -> Result<Self, String> {
        Self::acquire_inner(paths, install_dir, false)
    }

    /// The updater's token-bearing Agent is the sole process allowed to start
    /// while [`RestartGate`] excludes ordinary shells/chat during health.
    pub fn acquire_restarted(paths: &UpdatePaths, install_dir: &Path) -> Result<Self, String> {
        Self::acquire_inner(paths, install_dir, true)
    }

    fn acquire_inner(
        paths: &UpdatePaths,
        install_dir: &Path,
        updater_restart: bool,
    ) -> Result<Self, String> {
        let directory = paths.instances_dir().join(install_key(install_dir));
        fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let restart_file = if updater_restart {
            None
        } else {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(directory.join("restart.lock"))
                .map_err(|e| e.to_string())?;
            FileExt::try_lock_shared(&file)
                .map_err(|_| "Koru is currently being restarted after an update".to_string())?;
            Some(file)
        };
        let update_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(directory.join("update.lock"))
            .map_err(|e| e.to_string())?;
        FileExt::try_lock_shared(&update_file)
            .map_err(|_| "Koru is currently being updated".to_string())?;
        let path = directory.join(format!("{}.lease", std::process::id()));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        FileExt::lock_exclusive(&file).map_err(|e| e.to_string())?;
        Ok(Self {
            file,
            update_file,
            restart_file,
            path,
            directory,
        })
    }

    /// After publishing health, the updater restart joins the ordinary shared
    /// restart gate before it launches any child process. Until the updater has
    /// observed health and released its exclusive side this returns `false`;
    /// the Agent retries from its maintenance deadline without blocking the UI
    /// loop. Once joined, this lease is indistinguishable from an ordinary
    /// instance for the next update.
    pub fn join_restart_gate(&mut self) -> Result<bool, String> {
        if self.restart_file.is_some() {
            return Ok(true);
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(self.directory.join("restart.lock"))
            .map_err(|e| e.to_string())?;
        match FileExt::try_lock_shared(&file) {
            Ok(()) => {
                self.restart_file = Some(file);
                Ok(true)
            }
            // A lock conflict is reported differently by Windows and Unix.
            // Any failure here is retryable: opening the known file already
            // proved this is not a missing-directory or permission failure.
            Err(_) => Ok(false),
        }
    }

    pub fn other_instances(&self) -> usize {
        let Ok(entries) = fs::read_dir(&self.directory) else {
            return 0;
        };
        let mut active = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path == self.path || path.extension().and_then(|v| v.to_str()) != Some("lease") {
                continue;
            }
            let Ok(file) = OpenOptions::new().read(true).write(true).open(&path) else {
                continue;
            };
            if FileExt::try_lock_exclusive(&file).is_ok() {
                let _ = fs::remove_file(path);
            } else {
                active += 1;
            }
        }
        active
    }
}

impl Drop for InstanceLease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
        let _ = FileExt::unlock(&self.update_file);
        if let Some(file) = &self.restart_file {
            let _ = FileExt::unlock(file);
        }
        let _ = fs::remove_file(&self.path);
    }
}

/// Held by the updater from the last pre-swap quiescence proof through the
/// target build's health decision. Ordinary instances take the shared side and
/// fail fast; the token-bearing restarted Agent bypasses only this gate while
/// still holding the authoritative installation lease.
pub struct RestartGate {
    file: File,
}

impl RestartGate {
    pub fn acquire(paths: &UpdatePaths, install_dir: &Path) -> Result<Self, String> {
        let directory = paths.instances_dir().join(install_key(install_dir));
        fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(directory.join("restart.lock"))
            .map_err(|e| e.to_string())?;
        FileExt::try_lock_exclusive(&file)
            .map_err(|_| "another Koru instance is starting".to_string())?;
        Ok(Self { file })
    }
}

impl Drop for RestartGate {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub struct UpdateLock {
    file: File,
}

impl UpdateLock {
    pub fn acquire(paths: &UpdatePaths, install_dir: &Path) -> Result<Self, String> {
        let directory = paths.instances_dir().join(install_key(install_dir));
        fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(directory.join("update.lock"))
            .map_err(|e| e.to_string())?;
        FileExt::try_lock_exclusive(&file)
            .map_err(|_| "another updater is already installing Koru".to_string())?;
        Ok(Self { file })
    }
}

impl Drop for UpdateLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub fn install_key(install_dir: &Path) -> String {
    let normalized = fs::canonicalize(install_dir).unwrap_or_else(|_| install_dir.to_path_buf());
    let mut digest = Sha256::new();
    digest.update(normalized.to_string_lossy().to_lowercase().as_bytes());
    let bytes = digest.finalize();
    bytes[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_key_is_stable_and_path_specific() {
        assert_eq!(
            install_key(Path::new("C:/Koru")),
            install_key(Path::new("c:/koru"))
        );
        assert_ne!(
            install_key(Path::new("C:/Koru")),
            install_key(Path::new("C:/Other"))
        );
    }

    #[test]
    fn an_instance_refuses_an_active_updater_without_waiting_for_unlock() {
        let directory = tempfile::tempdir().unwrap();
        let install = directory.path().join("install");
        fs::create_dir_all(&install).unwrap();
        let paths = UpdatePaths::under(directory.path());
        let _updater = UpdateLock::acquire(&paths, &install).unwrap();
        let started = std::time::Instant::now();

        let result = InstanceLease::acquire(&paths, &install);

        assert!(result.is_err());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "instance acquisition waited for the updater instead of refusing it"
        );
    }

    #[test]
    fn only_the_token_restart_can_cross_the_health_window_gate() {
        let directory = tempfile::tempdir().unwrap();
        let install = directory.path().join("install");
        fs::create_dir_all(&install).unwrap();
        let paths = UpdatePaths::under(directory.path());

        let ordinary = InstanceLease::acquire(&paths, &install).unwrap();
        assert!(RestartGate::acquire(&paths, &install).is_err());
        drop(ordinary);

        let update_lock = UpdateLock::acquire(&paths, &install).unwrap();
        let restart_gate = RestartGate::acquire(&paths, &install).unwrap();
        assert!(InstanceLease::acquire(&paths, &install).is_err());
        assert!(InstanceLease::acquire_restarted(&paths, &install).is_err());

        // Replacement is complete: the updater releases the authoritative
        // update lock but retains the restart gate until health is decided.
        drop(update_lock);
        assert!(InstanceLease::acquire(&paths, &install).is_err());
        let mut restarted = InstanceLease::acquire_restarted(&paths, &install).unwrap();
        assert!(!restarted.join_restart_gate().unwrap());
        drop(restart_gate);
        assert!(restarted.join_restart_gate().unwrap());
    }
}
