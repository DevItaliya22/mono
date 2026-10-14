// SPDX-License-Identifier: MPL-2.0

//! The port a store of artifacts implements.
//!
//! Metadata, identity and provenance only: the port never sees a byte of
//! content, because the bytes live in the workspace's blob store and a
//! version names them by hash. There is **no method that changes a
//! version** — that absence is the immutability rule at the type level, and
//! the database trigger is the same rule one layer down.

use std::collections::BTreeSet;

use crate::{
    ArtifactFamily, ArtifactFamilyId, ArtifactVersion, ArtifactVersionId, AttemptGeneration,
    NewAuthority, NewFamily, NewVersion, Permission, Principal, Provenance, Registered,
    RegistrationId, Result, RunAuthorityRecord, RunId, RunLink, TokenHash, TokenLookup,
    WorkspaceId,
};

pub trait ArtifactRepository {
    /// Create a family and grant `owner` the [`crate::Access::Owner`] on it,
    /// in one transaction. A second family under the same id is
    /// [`crate::ArtifactError::FamilyExists`].
    fn create_family(&mut self, family: NewFamily, owner: Principal) -> Result<ArtifactFamily>;

    fn family(&self, id: ArtifactFamilyId) -> Result<ArtifactFamily>;

    /// The families of a workspace, oldest first. Retired ones only when
    /// asked for.
    fn list_families(
        &self,
        workspace: WorkspaceId,
        include_retired: bool,
    ) -> Result<Vec<ArtifactFamily>>;

    /// Mark a family retired. Its versions stay; a second retirement is
    /// harmless and keeps the first mark.
    fn retire_family(&mut self, id: ArtifactFamilyId, retired_at_ms: i64)
    -> Result<ArtifactFamily>;

    /// Register a version under a family, or find the one an earlier call
    /// with the same registration id already made (`retried`). Refused for
    /// a retired family, an unknown `derived_from`, or a registration id
    /// seen before with a different family or blob.
    fn register_version(&mut self, version: NewVersion) -> Result<Registered>;

    fn version(&self, id: ArtifactVersionId) -> Result<ArtifactVersion>;

    /// The version an earlier registration under this id made, if any —
    /// how a retried `create` finds its family without minting a second.
    fn version_by_registration(&self, id: RegistrationId) -> Result<Option<ArtifactVersion>>;

    /// Every version whose `produced_by` names the run, oldest first. The
    /// hook the orchestrator reads at a turn boundary (D29) and publishes
    /// `delegate.artifact.registered` from.
    fn versions_produced_by(&self, run: RunId) -> Result<Vec<ArtifactVersion>>;

    /// A family's versions, by ordinal.
    fn versions(&self, family: ArtifactFamilyId) -> Result<Vec<ArtifactVersion>>;

    /// Record that a run consumed or produced a version. Idempotent on the
    /// `(run, version, role)` key.
    fn link_run(&mut self, link: RunLink) -> Result<()>;

    /// Walk `derived_from` from a version to its root and collect every run
    /// link along the way.
    fn provenance(&self, id: ArtifactVersionId) -> Result<Provenance>;

    /// The grants on a family, for [`crate::authorize`].
    fn permissions(&self, family: ArtifactFamilyId) -> Result<Vec<Permission>>;
}

/// The port a store of run authorities implements (§7.3). Beside
/// [`ArtifactRepository`] rather than inside it: one is about results, the
/// other about who may register them, and a store may implement both.
///
/// Tokens never cross this port in the clear — only their hashes — and a
/// retired hash is kept, marked, so a stale token can be told from one that
/// was never issued.
pub trait RunAuthorityRepository {
    /// Record a new authority and its first token. A second record for the
    /// same run is [`crate::ArtifactError::AuthorityExists`].
    fn issue_authority(&mut self, authority: NewAuthority) -> Result<RunAuthorityRecord>;

    fn authority(&self, run: RunId) -> Result<RunAuthorityRecord>;

    /// Every authority whose run belongs to `workspace`, newest first.
    ///
    /// The one question a *person's* surface asks that a run never does:
    /// which runs of this workspace have a `.artifacts/<RunId>/` folder to
    /// look in (D19, D37). Revoked records are included — an agent that may
    /// no longer act still left files behind, and adopting one of them is
    /// the person's decision, not the agent's.
    fn authorities_in_workspace(&self, workspace: WorkspaceId) -> Result<Vec<RunAuthorityRecord>>;

    /// Move the run to `generation` under a fresh token, retiring the
    /// token that was current. Refused unless `generation` is strictly
    /// newer ([`crate::ArtifactError::StaleGeneration`]), for a revoked
    /// run, and for a run with no authority. The grants are re-read from
    /// the record and never widened: this takes none.
    fn advance_authority(
        &mut self,
        run: RunId,
        generation: AttemptGeneration,
        token_hash: TokenHash,
        advanced_at_ms: i64,
    ) -> Result<RunAuthorityRecord>;

    /// Mark the run's authority revoked. A second revocation keeps the
    /// first mark.
    fn revoke_authority(&mut self, run: RunId, revoked_at_ms: i64) -> Result<RunAuthorityRecord>;

    /// **The run was deleted: its authority goes, and the fact that it
    /// existed stays.**
    ///
    /// The one place in this store where rows are removed rather than
    /// marked, and it is the exception the rule is built for: revocation
    /// withdraws an agent's reach from a run that still exists, and this is
    /// a run that does not. Keeping a record of what a deleted run was
    /// allowed to do would be keeping its working folder's path, which is
    /// content, about a run nobody can reach.
    ///
    /// What stays is one row in `deleted_runs`: an id and a moment. A
    /// result the person chose to keep still names the run that made it, and
    /// that row is how its provenance says *a run that was deleted* rather
    /// than printing an id that resolves to nothing.
    ///
    /// Idempotent, and never an error for a run this store never issued an
    /// authority for — a run that arrived by sync has none here, and
    /// deleting it is still a delete. The `deleted_runs` row is written
    /// either way.
    fn forget_authority(&mut self, run: RunId, deleted_at_ms: i64) -> Result<()>;

    /// Which of `runs` this store has recorded as deleted. A run absent
    /// from the answer is simply a run — this device's, or another's.
    fn deleted_runs(&self, runs: &BTreeSet<RunId>) -> Result<BTreeSet<RunId>>;

    /// What a token hash names, or `None` for one never issued.
    fn lookup_token(&self, token: &TokenHash) -> Result<Option<TokenLookup>>;
}
