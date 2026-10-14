// SPDX-License-Identifier: MPL-2.0

//! `resolve` is a disk read, so `resolve` and `privacy` are the explicit
//! disk exception: each scenario works under a directory named for this
//! process and removed when it ends. `contract` and `notice` are pure.
//! `dependency` shells out to `cargo tree` and nothing else.

use std::path::{Path, PathBuf};

use crate::*;

mod contract;
mod dependency;
mod notice;
mod privacy;
mod resolve;
mod save;

/// A folder name no real path carries, so a leak is unmistakable.
pub(crate) const PATH_SENTINEL: &str = "PATH-SENTINEL-private-install";

/// An installation directory of this process's own, removed when the
/// fixture drops. The sentinel sits in its name so the privacy test can
/// assert the path never surfaces.
pub(crate) struct Installation {
    root: PathBuf,
}

impl Installation {
    pub(crate) fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "koru-delegate-preamble-{}-{name}-{PATH_SENTINEL}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.root
    }

    /// The edited copy's path, whether or not anything is there yet.
    pub(crate) fn preamble_path(&self) -> PathBuf {
        self.root.join(RELATIVE_PATH)
    }

    /// Write `bytes` as the edited copy.
    pub(crate) fn edited(&self, bytes: &[u8]) -> &Self {
        let path = self.preamble_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
        self
    }
}

impl Drop for Installation {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
