// SPDX-License-Identifier: MPL-2.0

//! One cross-process credential owner per installation and authenticated actor.
//! Empty lock files are coordination only; no credentials are written to disk.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use koru_oauth_credentials::{CredentialEntry, CredentialStore, OsCredentialStore, SecretString};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockError {
    Busy,
    Unavailable,
}

pub trait LockGuard {}
impl LockGuard for File {}

pub trait CredentialLocks: Send + Sync {
    fn acquire(&self, entry: &CredentialEntry) -> Result<Box<dyn LockGuard>, LockError>;
}

pub struct RootLocks {
    root: PathBuf,
}
impl RootLocks {
    pub fn new(root: Option<&Path>) -> Result<Self, super::Error> {
        let root = root
            .map(Path::to_path_buf)
            .or_else(koru_channel::channel_root)
            .ok_or(super::Error::AgentUnavailable)?;
        Ok(Self { root })
    }
}
impl CredentialLocks for RootLocks {
    fn acquire(&self, entry: &CredentialEntry) -> Result<Box<dyn LockGuard>, LockError> {
        if cfg!(test) || cfg!(feature = "no-network") {
            return Err(LockError::Unavailable);
        }
        let directory = self.root.join("socials").join("credentials");
        std::fs::create_dir_all(&directory).map_err(|_| LockError::Unavailable)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join(format!("{}.lock", entry.namespace())))
            .map_err(|_| LockError::Unavailable)?;
        fs2::FileExt::try_lock_exclusive(&file).map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock
                || error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
            {
                LockError::Busy
            } else {
                LockError::Unavailable
            }
        })?;
        // Closing the owning handle releases the OS lock, including on crash.
        // Never unlink a lock file: another waiter may already hold its inode.
        Ok(Box::new(file))
    }
}

/// Connection/revocation writes share the publisher's credential lock.
pub struct LockedVault {
    locks: RootLocks,
}
impl LockedVault {
    pub fn new(root: Option<&Path>) -> Result<Self, super::Error> {
        Ok(Self {
            locks: RootLocks::new(root)?,
        })
    }
}
impl CredentialStore for LockedVault {
    fn load(
        &self,
        entry: &CredentialEntry,
    ) -> Result<Option<SecretString>, koru_oauth_credentials::Error> {
        OsCredentialStore.load(entry)
    }
    fn store(
        &self,
        entry: &CredentialEntry,
        secret: SecretString,
    ) -> Result<(), koru_oauth_credentials::Error> {
        let _guard = self
            .locks
            .acquire(entry)
            .map_err(|_| koru_oauth_credentials::Error::Unavailable)?;
        OsCredentialStore.store(entry, secret)
    }
    fn clear(&self, entry: &CredentialEntry) -> Result<(), koru_oauth_credentials::Error> {
        let _guard = self
            .locks
            .acquire(entry)
            .map_err(|_| koru_oauth_credentials::Error::Unavailable)?;
        OsCredentialStore.clear(entry)
    }
}
