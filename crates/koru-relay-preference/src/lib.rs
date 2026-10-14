// SPDX-License-Identifier: MPL-2.0

//! The machine-local relay on/off preference shared by every installed Koru
//! channel.
//!
//! **Charter: this crate holds exactly one semantic value — whether this
//! machine's Koru installations use relays — with its schema, default, path
//! rule, and last-completed-write-wins storage, and nothing else.** It is not
//! `DesktopPreferences` and must not become a second one: that file already
//! warns that a machine-local bucket two processes write is a file whose
//! loser cannot be told which one lost. This value is different in kind — it
//! is deliberately shared across *channels* (decision №7 of the iroh-relay
//! roadmap), where every other machine-local file is per channel, so it needs
//! its own owner rather than a row in somebody else's.
//!
//! ## Semantics (decisions №6 and №7)
//!
//! - One value, default **on**: [`RelayPreference::relay_enabled`].
//! - The channel that changes it applies the change to its own runtime
//!   immediately; every *other* installed channel reads the shared value on
//!   its **next start**. There is no live cross-channel propagation in v1.
//! - Concurrent channel writers are resolved by **last-completed-write-wins**:
//!   each write lands as an atomic whole-file replacement, so readers see
//!   some complete value and the final rename decides which.
//! - A missing or damaged file reads as the default. A preference must never
//!   stop the network from starting.
//!
//! ## Where it lives
//!
//! `<local data>/Koru/network-relay-v1.json` — beside the channel segment
//! directories, under none of them ([`koru_channel::shared_root`]). Under an
//! explicit `--root R` it is `R/agent/network-relay-v1.json` instead, with
//! the rest of the agent's rooted files, so a test stack or a second rooted
//! stack shares nothing machine-wide. Mobile does not use this crate's paths
//! or adapter; it carries the same semantic value in platform-local storage.
//!
//! ## Pure semantics, injected storage
//!
//! Schema, default, and merge rule are pure ([`RelayPreference`], [`load`],
//! [`save`]); where bytes go is a [`PreferenceStorage`] the caller injects.
//! Repository tests use [`MemoryStorage`] and never touch a disk; the real
//! [`FileStorage`] atomic-replacement path is covered by a documented manual
//! smoke check (see `tests.rs`), not by the suite.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// The schema-versioned file name. The `v1` is part of the schema contract:
/// a future incompatible schema is a new file name, never a re-reading of
/// this one.
pub const PREFERENCE_FILE: &str = "network-relay-v1.json";

/// The one semantic value: whether this machine's Koru installations use
/// relays. Off disables every relay; direct/LAN connectivity remains.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RelayPreference {
    pub relay_enabled: bool,
}

impl Default for RelayPreference {
    /// Relay **on** — decision №6's shipped default.
    fn default() -> Self {
        Self {
            relay_enabled: true,
        }
    }
}

impl RelayPreference {
    /// Pure: bytes → value. Anything unreadable is the default, because a
    /// damaged preference file must never stop the network from starting.
    pub fn decode(bytes: &[u8]) -> Self {
        serde_json::from_slice(bytes).unwrap_or_default()
    }

    /// Pure: value → the bytes a writer stores.
    pub fn encode(self) -> Vec<u8> {
        serde_json::to_vec_pretty(&self).expect("a two-field struct serializes")
    }
}

/// Where preference bytes live. Injected so the semantics above are testable
/// without a filesystem.
pub trait PreferenceStorage: Send + Sync {
    /// The last completely-written value, or `None` when nothing was ever
    /// written (or nothing readable remains).
    fn read(&self) -> Option<Vec<u8>>;

    /// Replace the stored value as one atomic whole. Two concurrent writers
    /// must resolve to whichever **completed** last, never to an interleaving.
    fn write(&self, bytes: &[u8]) -> io::Result<()>;
}

/// Read the preference a storage holds; missing or damaged reads as default.
pub fn load(storage: &dyn PreferenceStorage) -> RelayPreference {
    storage
        .read()
        .map(|bytes| RelayPreference::decode(&bytes))
        .unwrap_or_default()
}

/// Persist the preference into a storage.
pub fn save(storage: &dyn PreferenceStorage, preference: RelayPreference) -> io::Result<()> {
    storage.write(&preference.encode())
}

/// In-memory storage for tests — the injected backend the repository suite
/// uses instead of a disk. Shared between two logical "channels" in a test,
/// it models the machine-wide file both would read.
#[derive(Debug, Default)]
pub struct MemoryStorage {
    value: Mutex<Option<Vec<u8>>>,
}

impl PreferenceStorage for MemoryStorage {
    fn read(&self) -> Option<Vec<u8>> {
        self.value
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        *self.value.lock().unwrap_or_else(|error| error.into_inner()) = Some(bytes.to_vec());
        Ok(())
    }
}

/// The real filesystem adapter: whole-file atomic replacement.
///
/// Each write stages a uniquely named sibling (`.part-<pid>-<n>`, so two
/// channels writing at once cannot share a staging file — the shared
/// `snapshot.loro.part` lesson) and renames it over the destination;
/// `std::fs::rename` replaces on Windows too. The rename is the completion
/// that "last-completed-write-wins" names. Deliberately no `fsync`: a
/// preference is not the store's `Ack`, and `guide/contributing/PRINCIPLES.md` reserves
/// durability waits for operations whose meaning is durability — a
/// power-cut-lost toggle re-reads as a value somebody once chose, or the
/// default.
///
/// **Not exercised by repository tests** (they touch no disk); its smoke
/// check is the documented manual command in `tests.rs`.
#[derive(Debug)]
pub struct FileStorage {
    path: PathBuf,
}

static NEXT_STAGING: AtomicU64 = AtomicU64::new(0);

impl FileStorage {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl PreferenceStorage for FileStorage {
    fn read(&self) -> Option<Vec<u8>> {
        std::fs::read(&self.path).ok()
    }

    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let staging = self.path.with_extension(format!(
            "part-{}-{}",
            std::process::id(),
            NEXT_STAGING.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&staging, bytes)?;
        std::fs::rename(&staging, &self.path).inspect_err(|_| {
            // A failed rename must not leave staging litter beside the file.
            let _ = std::fs::remove_file(&staging);
        })
    }
}

/// Pure: the preference's location under an explicit `--root`.
///
/// `<root>/agent/network-relay-v1.json` — with the rest of the agent's
/// rooted files, so a rooted stack shares nothing machine-wide. Path math
/// only; nothing is created or checked.
pub fn rooted_preference_path(root: &Path) -> PathBuf {
    root.join("agent").join(PREFERENCE_FILE)
}

/// Pure: the shared location under a machine's `Koru` data directory —
/// beside the channel segment directories, under none of them.
pub fn shared_preference_path_under(koru_base: &Path) -> PathBuf {
    koru_base.join(PREFERENCE_FILE)
}

/// Where this process's preference file is: the rooted location when a root
/// was named, otherwise the machine's shared one. Resolution only — the path
/// is not created, checked, or opened.
pub fn preference_path(explicit_root: Option<&Path>) -> Option<PathBuf> {
    match explicit_root {
        Some(root) => Some(rooted_preference_path(root)),
        None => koru_channel::shared_root().map(|base| shared_preference_path_under(&base)),
    }
}

#[cfg(test)]
mod tests;
