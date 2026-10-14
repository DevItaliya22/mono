// SPDX-License-Identifier: MPL-2.0

//! The bodies of `/v1/artifacts/authorities` — how the orchestrator issues,
//! advances and revokes a run's authority (ROADMAP §7.3).
//!
//! The token crosses exactly once, in [`IssuedAuthority`], and is redacted
//! from that type's `Debug`; every later read answers with
//! [`AuthorityView`], which carries no token and no path.

use std::collections::BTreeSet;
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use koru_artifact_core::{AttemptGeneration, AuthorityState, RunWorkspace};

use crate::{RunId, WorkspaceId};

/// `POST /v1/artifacts/authorities`. The run's own workspace, the attempt
/// this token is for, the grant set as the person left it in the modal
/// (D28), and the working scope whose `.artifacts/<RunId>/` the tools
/// accept paths from.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueAuthorityRequest {
    pub run: RunId,
    pub workspace: WorkspaceId,
    pub generation: AttemptGeneration,
    #[serde(default)]
    pub grants: BTreeSet<WorkspaceId>,
    pub run_workspace: RunWorkspace,
}

impl fmt::Debug for IssueAuthorityRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IssueAuthorityRequest")
            .field("run", &self.run)
            .field("workspace", &self.workspace)
            .field("generation", &self.generation)
            .field("grants", &self.grants.len())
            .finish_non_exhaustive()
    }
}

/// `POST /v1/artifacts/authorities/{run}/generation`: the next attempt.
/// Strictly newer than the current one; the grants are re-read from the
/// record and cannot be sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdvanceGenerationRequest {
    pub generation: AttemptGeneration,
}

/// The answer to an issue or an advance: the endpoint the provider is
/// handed, and the bearer that authenticates this attempt at it. The
/// bearer is shown nowhere else — not in a later read, not in `Debug`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IssuedAuthority {
    pub run: RunId,
    pub generation: AttemptGeneration,
    /// `<gateway base>/v1/runs/<RunId>/mcp` — stable per run across
    /// attempts.
    pub url: String,
    pub token: String,
}

impl fmt::Debug for IssuedAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IssuedAuthority")
            .field("run", &self.run)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// `GET /v1/artifacts/authorities/{run}` and the answer to a revocation:
/// the record without its token and without its path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthorityView {
    pub run: RunId,
    pub workspace: WorkspaceId,
    pub generation: AttemptGeneration,
    pub grants: BTreeSet<WorkspaceId>,
    pub state: AuthorityState,
    pub created_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at_ms: Option<i64>,
}

impl From<koru_artifact_core::RunAuthorityRecord> for AuthorityView {
    fn from(record: koru_artifact_core::RunAuthorityRecord) -> Self {
        Self {
            run: record.run,
            workspace: record.workspace,
            generation: record.generation,
            grants: record.grants,
            state: record.state,
            created_at_ms: record.created_at_ms,
            revoked_at_ms: record.revoked_at_ms,
        }
    }
}

/// `GET /v1/artifacts/authorities/{run}/workspace`: **the one answer that
/// carries the run's working folder back out of the store.**
///
/// [`AuthorityView`] deliberately carries no path, and that is still the
/// rule for every read a surface makes. This is the exception, for the one
/// caller that needs it: emptying the trash has to remove the folder a
/// deleted run worked in, and the record is the only durable place that
/// folder is written down — the scope says what *kind* of folder was asked
/// for, and only the device that ran it knows which directory that turned
/// out to be. Re-deriving it from the scope now would name a different
/// directory whenever a project has moved or a temporary base has been
/// repointed, which is how a delete removes the wrong thing.
///
/// It is behind the **write** scope, not the read one. Reading it is not
/// what it is for: a person's absolute path leaves this store only into the
/// hand that is about to delete the folder, and the fence should say so.
///
/// Its `Debug` prints no path, like every other type that holds one.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunWorkspaceResponse {
    pub run: RunId,
    pub run_workspace: RunWorkspace,
}

impl fmt::Debug for RunWorkspaceResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunWorkspaceResponse")
            .field("run", &self.run)
            .finish_non_exhaustive()
    }
}

/// `POST /v1/artifacts/authorities/{run}/forget`: the run was deleted.
///
/// The authority record, its grants and its tokens are removed — there is
/// nothing left for them to authorize — and one row saying the run was
/// deleted takes their place, so a result the person chose to keep can say
/// *a run that was deleted* instead of printing an id that resolves to
/// nothing.
///
/// `held_authority` is whether there was a record to remove. `false` is an
/// ordinary answer, not a failure: a run that never started never had one,
/// and neither does a run that reached this device by sync.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForgetAuthorityResponse {
    pub run: RunId,
    pub held_authority: bool,
}

/// `GET /v1/artifacts/runs/{run}/versions`: what a run has registered so
/// far, oldest first — the hook the orchestrator reads at a turn boundary
/// (D29) and publishes `delegate.artifact.registered` from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunVersionsResponse {
    pub versions: Vec<crate::ArtifactVersion>,
}
