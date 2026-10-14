// SPDX-License-Identifier: MPL-2.0

//! What a **person's** Artifacts surface decides about a run's
//! `.artifacts/<RunId>/` folder (D19, D29, D37).
//!
//! Two questions and one verb, and none of them is new authority:
//!
//! - **Which runs have a folder to look in?** The authorities of the
//!   workspace, with how many versions each run has already registered — so
//!   D29's *finished having registered nothing* is a fact the surface states
//!   rather than infers.
//! - **Which of the files in one are not results?** A file is a result when
//!   a family derived from `(run, canonical relative path)` exists — the
//!   same identity [`crate::family_id`] gives the run's own `create`, so the
//!   two answers cannot drift.
//! - **Register this one.** [`RunFolderPort::register_file`] is
//!   [`RunToolPort::create`](crate::RunToolPort::create) with a person's
//!   scope check in front of it. Not a second path rule, not a second
//!   provenance rule, not a second idempotency key: the *same call*, so a
//!   person's Register and an agent's `create` can never mean two things.
//!
//! **Why that is not a widening.** A gateway session already holds
//! `artifacts.write`, and with it `POST /v1/artifacts/families` and
//! `.../versions` over any workspace it can reach — it can put arbitrary
//! bytes in a workspace's blob store and register them today. What these
//! add is a *narrower* door: the bytes must be a file already inside the
//! run's own folder, read under the register path rule, and every fact that
//! decides where they land — the workspace, the family, the provenance —
//! comes from the durable authority record. The record's `run_workspace`
//! path is read here and never returned to the caller.
//!
//! **A revoked run still answers.** Revocation withdraws the *agent's*
//! ability to act; it says nothing about the person's ability to adopt a
//! file the agent left behind, and D37 keeps those files for the life of
//! the run. Generation fencing does not apply either: no token is
//! presented, so there is no attempt to fence.

use koru_artifact_core::{
    ArtifactRepository, ArtifactTitle, MediaType, RelativePath, RunAuthorityRecord,
    RunAuthorityRepository, RunId, WorkspaceId,
};
use koru_artifact_wire::Failure;
use koru_artifact_wire::run_folder::RegisteredFile;
use koru_artifact_wire::run_tools::ToolFailure;

use crate::{ArtifactScope, RequestContext, Result, RunToolPort, Service, StagedFile, family_id};

/// One run whose folder the surface may look in: the record the gateway
/// resolves paths against, and how much the run has already delivered.
///
/// The record carries the run's working-scope path. It stays inside the
/// gateway — no route returns it — which is why this type is the API
/// crate's rather than the wire's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunFolderTarget {
    pub record: RunAuthorityRecord,
    /// Versions whose producer is this run.
    pub registered: usize,
}

/// The person's side of a run's results folder.
pub trait RunFolderPort: Send + Sync {
    /// Every run of `workspace` with a folder, newest first.
    fn run_folders(
        &self,
        context: RequestContext,
        workspace: WorkspaceId,
    ) -> Result<Vec<RunFolderTarget>>;

    /// One run's target, for the Register path. Refused with
    /// `artifact_authority_not_found` for a run this device never issued an
    /// authority for — there is no folder to resolve a path against.
    fn run_folder(&self, context: RequestContext, run: RunId) -> Result<RunFolderTarget>;

    /// Of the paths a scan found, the ones that are **not** results yet.
    /// Order is preserved, so the surface draws them as the scan sorted
    /// them.
    fn unregistered(
        &self,
        context: RequestContext,
        run: RunId,
        found: Vec<RelativePath>,
    ) -> Result<Vec<RelativePath>>;

    /// Where a person's Register puts the bytes, before the file is
    /// staged: the run's own workspace — always, whatever the grants say
    /// (D28: an empty set denies workspace *access*, and a result the run
    /// wrote is not access to anything) — and the media type the call named
    /// or `application/octet-stream`.
    ///
    /// Its own method rather than [`RunToolPort::create_target`] because
    /// that one validates a title, and a one-click Register has no title to
    /// validate until the file has been staged and its last segment read.
    fn register_target(
        &self,
        context: RequestContext,
        record: &RunAuthorityRecord,
        mime: Option<&MediaType>,
    ) -> Result<crate::RegistrationTarget>;

    /// D19's one click, over a file the gateway has already staged and
    /// filed. The same decision the run's own `create` makes.
    fn register_file(
        &self,
        context: RequestContext,
        record: &RunAuthorityRecord,
        staged: StagedFile,
        title: Option<&ArtifactTitle>,
        mime: Option<&MediaType>,
    ) -> Result<RegisteredFile>;
}

/// A tool refusal as a route failure. The code is the same word either
/// side — a person and an agent read one vocabulary — and only the two
/// that mean *ask again later* are retryable.
pub fn failure_from(refusal: ToolFailure) -> Failure {
    let retryable = matches!(
        refusal.code.as_str(),
        "artifacts_unavailable" | "artifact_storage_failed" | "store_unavailable"
    );
    Failure::new(refusal.code, refusal.message, retryable)
}

impl<R: ArtifactRepository + RunAuthorityRepository + Send> RunFolderPort for Service<R> {
    fn run_folders(
        &self,
        context: RequestContext,
        workspace: WorkspaceId,
    ) -> Result<Vec<RunFolderTarget>> {
        self.admitted(&context, ArtifactScope::Read)?;
        let repository = self.lock()?;
        let mut targets = Vec::new();
        for record in repository.authorities_in_workspace(workspace)? {
            let registered = repository.versions_produced_by(record.run)?.len();
            targets.push(RunFolderTarget { record, registered });
        }
        Ok(targets)
    }

    fn run_folder(&self, context: RequestContext, run: RunId) -> Result<RunFolderTarget> {
        self.admitted(&context, ArtifactScope::Read)?;
        let repository = self.lock()?;
        let record = repository.authority(run)?;
        let registered = repository.versions_produced_by(run)?.len();
        Ok(RunFolderTarget { record, registered })
    }

    fn unregistered(
        &self,
        context: RequestContext,
        run: RunId,
        found: Vec<RelativePath>,
    ) -> Result<Vec<RelativePath>> {
        self.admitted(&context, ArtifactScope::Read)?;
        let repository = self.lock()?;
        let record = repository.authority(run)?;
        let mut unregistered = Vec::with_capacity(found.len());
        for path in found {
            // The run's own identity for that path. A family under it means
            // the file has been adopted — by the agent's `create`, or by an
            // earlier press of this very button.
            if repository
                .family(family_id(&record, path.as_str()))
                .is_err()
            {
                unregistered.push(path);
            }
        }
        Ok(unregistered)
    }

    fn register_target(
        &self,
        context: RequestContext,
        record: &RunAuthorityRecord,
        mime: Option<&MediaType>,
    ) -> Result<crate::RegistrationTarget> {
        self.admitted(&context, ArtifactScope::Write)?;
        Ok(crate::RegistrationTarget {
            workspace: record.workspace,
            mime: mime.cloned().unwrap_or_else(|| {
                MediaType::new(MediaType::OCTET_STREAM)
                    .expect("the default media type is well-formed")
            }),
        })
    }

    fn register_file(
        &self,
        context: RequestContext,
        record: &RunAuthorityRecord,
        staged: StagedFile,
        title: Option<&ArtifactTitle>,
        mime: Option<&MediaType>,
    ) -> Result<RegisteredFile> {
        self.admitted(&context, ArtifactScope::Write)?;
        // Absent a title, the file's own last segment names the result:
        // the click carries no words, and that is what the person is
        // looking at when they press it.
        let derived;
        let title = match title {
            Some(title) => title.as_str(),
            None => {
                derived = staged
                    .relative_path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&staged.relative_path)
                    .to_owned();
                &derived
            }
        };
        let registered =
            RunToolPort::create(self, record, staged, title, mime.map(MediaType::as_str))
                .map_err(failure_from)?;
        Ok(RegisteredFile {
            version: registered.version,
            family: registered.family,
            retried: registered.retried,
        })
    }
}

#[cfg(test)]
mod tests;
