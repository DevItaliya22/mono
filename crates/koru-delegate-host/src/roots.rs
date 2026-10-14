// SPDX-License-Identifier: MPL-2.0

//! Where a run's working folder is: the session host's projects for a
//! project scope, and a directory of this worker's own for a temporary
//! one (D37: it survives as long as the run does).
//!
//! The project registry is the host's; this asks it on its own connection
//! rather than caching a root, because a root can be set or moved while a
//! run is being admitted and a cached answer would be a stored guess.
//!
//! **The temporary base is read the same way, and for the same reason**
//! (ADR-365). A person can change where Koru makes these folders while runs
//! are going; a base captured when this worker started would send the next run
//! somewhere they had already stopped choosing. It is one small device-local
//! file read per run start, on this worker's own thread — never on a thread
//! that answers a person.

use std::path::PathBuf;

use koru_delegate_core::{ProjectId, RunId};
use koru_delegate_scope::{RootLookup, ScopeRoots};
use koru_session_client::SessionClient;

pub struct HostRoots {
    endpoint: koru_ipc::Endpoint,
    /// Where this stack's settings are, so the chosen base can be read at the
    /// moment it is needed. `None` when no settings location could be
    /// resolved, which falls back to the platform's temporary directory — the
    /// same answer as "nothing chosen", and never a guessed folder.
    settings: Option<koru_settings::SettingsPaths>,
}

impl HostRoots {
    pub fn new(
        endpoint: koru_ipc::Endpoint,
        settings: Option<koru_settings::SettingsPaths>,
    ) -> Self {
        Self { endpoint, settings }
    }

    fn client(&self) -> Option<SessionClient> {
        SessionClient::connect(
            &self.endpoint,
            "koru-delegate-roots",
            crate::sessions::offer(),
        )
        .ok()
    }

    /// The folder a run's own directory is made inside.
    ///
    /// The chosen one, or the platform's temporary directory — `%TMP%` on
    /// Windows, `/tmp` on Unix. [`koru_settings::delegate_temporary_base`] is
    /// the only place that fallback is spelled, because the modal that shows a
    /// person where their work will go and this worker that puts it there are
    /// two processes and must not be able to disagree about it.
    fn temporary_base(&self) -> PathBuf {
        let chosen = self
            .settings
            .clone()
            .map(koru_settings::SettingsAuthority::new)
            .and_then(|authority| authority.global().delegate_temporary_root);
        koru_settings::delegate_temporary_base(chosen.as_deref())
    }

    pub fn planned_temporary(
        &self,
        run: RunId,
    ) -> Result<koru_delegate_scope::ResolvedScope, koru_delegate_scope::ScopeRefusal> {
        let runs = self.temporary_base().join(koru_settings::RUNS_FOLDER);
        std::fs::create_dir_all(&runs)
            .map_err(|_| koru_delegate_scope::ScopeRefusal::NoTemporaryDirectory)?;
        let child = PathBuf::from(run.to_storage()).join("workspace");
        koru_delegate_scope::ResolvedScope::planned_under(&runs, &child)
    }
}

impl ScopeRoots for HostRoots {
    fn planned_temporary(
        &self,
        run: RunId,
    ) -> Result<koru_delegate_scope::ResolvedScope, koru_delegate_scope::ScopeRefusal> {
        HostRoots::planned_temporary(self, run)
    }

    fn project_root(&self, project: ProjectId) -> RootLookup {
        let Some(client) = self.client() else {
            return RootLookup::UnknownProject;
        };
        let Ok(projects) = client.list_projects() else {
            return RootLookup::UnknownProject;
        };
        match projects.into_iter().find(|summary| summary.id == project) {
            Some(summary) => match summary.root_directory {
                Some(root) => RootLookup::Directory(PathBuf::from(root)),
                None => RootLookup::NoRoot,
            },
            None => RootLookup::UnknownProject,
        }
    }

    /// `<base>/koru-runs/<run>/workspace`.
    ///
    /// The middle segment is [`koru_settings::RUNS_FOLDER`] and it earns its
    /// place: the base is usually a directory Koru does not own — a person's
    /// `%TMP%` has a hundred other things in it — and one named folder is the
    /// difference between a tree somebody can find and delete and a scatter of
    /// bare identifiers nobody can attribute. The run's own directory is named
    /// for the run, and `workspace` inside it is what the agent is pointed at,
    /// which leaves room beside it for anything the run owns that is not the
    /// agent's to see.
    ///
    /// **Making it is what resolving it means** — admission never sees a
    /// missing directory — so a base that cannot be written to fails here,
    /// which is exactly why the choosing end refuses one before it is stored.
    fn temporary_directory(&self, run: RunId) -> Option<PathBuf> {
        let directory = run_directory(&self.temporary_base(), run);
        std::fs::create_dir_all(&directory).ok()?;
        Some(directory)
    }
}

/// The arithmetic on its own, so it can be read and tested without making a
/// directory: `<base>/koru-runs/<run>/workspace`.
fn run_directory(base: &std::path::Path, run: RunId) -> PathBuf {
    base.join(koru_settings::RUNS_FOLDER)
        .join(run.to_storage())
        .join("workspace")
}

#[cfg(test)]
mod tests;
