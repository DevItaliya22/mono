// SPDX-License-Identifier: MPL-2.0

//! The registry and the overlap rule are pure and tested in memory. Path
//! identity is the thing under test, so `identity`, `reconcile` and
//! `privacy` are the explicit disk exception: each scenario works under a
//! directory named for this process and removed when it ends, and nothing
//! reaches a store, a process or a network. `dependency` shells out to
//! `cargo tree` and nothing else.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use uuid::Uuid;

use crate::*;

mod claims;
mod dependency;
mod identity;
mod privacy;
mod reconcile;
mod removal;

/// A deterministic v4 UUID — version nibble 4, variant 8 — so strict ids
/// accept it and a fixture is the same on every run.
pub(crate) fn v4(n: u8) -> Uuid {
    format!("00000000-0000-4000-8000-0000000000{n:02x}")
        .parse()
        .unwrap()
}

pub(crate) fn run_id(n: u8) -> RunId {
    RunId::from_uuid(v4(n))
}

pub(crate) fn attempt_id(n: u8) -> RunAttemptId {
    RunAttemptId::from_uuid(v4(n))
}

pub(crate) fn project_id(n: u8) -> ProjectId {
    v4(n).hyphenated().to_string().parse().unwrap()
}

pub(crate) fn schedule(n: u8) -> ScheduleRef {
    ScheduleRef::from_uuid(v4(n))
}

pub(crate) fn scheduled(n: u8) -> StartOrigin {
    StartOrigin::Scheduled {
        schedule: schedule(n),
    }
}

/// A folder name no real path carries, so a leak is unmistakable.
pub(crate) const PATH_SENTINEL: &str = "PATH-SENTINEL-private-folder";

/// A directory of this process's own, removed when the fixture drops. Every
/// disk scenario lives under one, and the sentinel sits in its name so the
/// privacy test can assert the path never surfaces.
pub(crate) struct TempDir {
    root: PathBuf,
}

impl TempDir {
    pub(crate) fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "koru-delegate-scope-{}-{name}-{PATH_SENTINEL}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.root
    }

    /// Create `relative` under the root and return its path.
    pub(crate) fn dir(&self, relative: &str) -> PathBuf {
        let path = self.root.join(relative);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    pub(crate) fn file(&self, relative: &str) -> PathBuf {
        let path = self.root.join(relative);
        std::fs::write(&path, b"not a directory").unwrap();
        path
    }

    /// A directory junction `link` → `target`, made the way a person makes
    /// one (`mklink /J`, which needs no privilege). `None`, with the reason
    /// printed, if this machine refuses — the test then skips rather than
    /// asserting something it could not set up.
    #[cfg(windows)]
    pub(crate) fn junction(&self, link: &str, target: &Path) -> Option<PathBuf> {
        let link = self.root.join(link);
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(target)
            .output();
        match output {
            Ok(output) if output.status.success() => Some(link),
            Ok(output) => {
                eprintln!(
                    "skipping: mklink /J refused with status {:?}",
                    output.status.code()
                );
                None
            }
            Err(error) => {
                eprintln!("skipping: cmd could not be started ({:?})", error.kind());
                None
            }
        }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The roots port, filled by hand.
#[derive(Default)]
pub(crate) struct Roots {
    projects: HashMap<ProjectId, RootLookup>,
    temporary: HashMap<RunId, PathBuf>,
}

impl Roots {
    pub(crate) fn project(mut self, project: ProjectId, root: &Path) -> Self {
        self.projects
            .insert(project, RootLookup::Directory(root.to_path_buf()));
        self
    }

    pub(crate) fn project_without_root(mut self, project: ProjectId) -> Self {
        self.projects.insert(project, RootLookup::NoRoot);
        self
    }

    pub(crate) fn temporary(mut self, run: RunId, directory: &Path) -> Self {
        self.temporary.insert(run, directory.to_path_buf());
        self
    }
}

impl ScopeRoots for Roots {
    fn project_root(&self, project: ProjectId) -> RootLookup {
        self.projects
            .get(&project)
            .cloned()
            .unwrap_or(RootLookup::UnknownProject)
    }

    fn temporary_directory(&self, run: RunId) -> Option<PathBuf> {
        self.temporary.get(&run).cloned()
    }
}

pub(crate) fn root_scope(n: u8) -> WorkspaceScope {
    WorkspaceScope::ProjectRoot {
        project: project_id(n),
    }
}

pub(crate) fn folder_scope(n: u8, folder: &str) -> WorkspaceScope {
    WorkspaceScope::ProjectFolder {
        project: project_id(n),
        folder: RelativeFolder::new(folder).unwrap(),
    }
}

/// The durable port, filled by hand; `Err` stands in for a database that
/// could not be read.
pub(crate) struct FakeDurable {
    pub(crate) claims: Result<Vec<DurableClaim>, &'static str>,
}

impl DurableClaims for FakeDurable {
    type Error = &'static str;

    fn claims_live_at_shutdown(&self) -> Result<Vec<DurableClaim>, Self::Error> {
        self.claims.clone()
    }
}

pub(crate) fn identity(path: &Path) -> ScopeIdentity {
    ScopeIdentity::of_directory(path).unwrap()
}
