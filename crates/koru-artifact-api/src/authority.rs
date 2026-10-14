// SPDX-License-Identifier: MPL-2.0

//! The run authority port and its service (ROADMAP §7.3).
//!
//! The orchestrator issues an authority before it starts a provider, hands
//! the provider the endpoint and the token, advances the generation when it
//! resumes and revokes when the run ends. Every one of those is a write to
//! `artifacts.db` and nothing else: no callback into a worker, no in-memory
//! grant that a crash would lose. The endpoint then asks [`RunAuthority::resolve`]
//! on every call, and what comes back is the record, never anything the
//! call said about itself.
//!
//! **The token is minted here and stored nowhere.** Two v4 UUIDs' worth of
//! randomness under a fixed prefix, hashed with BLAKE3 before it reaches the
//! repository; the bearer crosses to the caller once, in [`IssuedEndpoint`],
//! and is redacted from that type's `Debug`. A lost reply cannot be
//! answered again — the orchestrator advances the generation instead, which
//! is also what makes an old token stale rather than merely forgotten.

use std::fmt;

use koru_artifact_core::{
    ArtifactError, ArtifactRepository, AttemptGeneration, AuthorityRefusal, NewAuthority,
    RunAuthorityRecord, RunAuthorityRepository, RunId, TokenHash,
};
use koru_artifact_wire::authority::{
    AuthorityView, ForgetAuthorityResponse, IssueAuthorityRequest, RunVersionsResponse,
    RunWorkspaceResponse,
};

use crate::{ArtifactScope, RequestContext, Result, Service};

/// The prefix every run token carries, so a bearer that lands in the wrong
/// header is recognisable for what it is — and refused, since the gateway's
/// session authority never minted it.
pub const TOKEN_PREFIX: &str = "koru_run_";

/// `/v1/runs/<RunId>/mcp`: the path of a run's endpoint, stable across
/// attempts. The gateway prefixes its own base.
pub fn endpoint_path(run: RunId) -> String {
    format!("/v1/runs/{}/mcp", run.to_storage())
}

/// The BLAKE3 digest of a bearer, in the one spelling the store keys on.
pub fn hash_token(token: &str) -> TokenHash {
    TokenHash::from_hex(blake3::hash(token.as_bytes()).to_hex().to_string())
        .expect("a BLAKE3 hex digest is sixty-four lowercase hex characters")
}

fn mint_token() -> String {
    format!(
        "{TOKEN_PREFIX}{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// What issuing or advancing answers: where the provider connects, and the
/// bearer that authenticates this attempt there. Shown once.
#[derive(Clone, PartialEq, Eq)]
pub struct IssuedEndpoint {
    pub run: RunId,
    pub generation: AttemptGeneration,
    pub path: String,
    pub token: String,
}

impl fmt::Debug for IssuedEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IssuedEndpoint")
            .field("run", &self.run)
            .field("generation", &self.generation)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// What a bearer resolves to: the authority, or the reason it was refused.
/// The two are one value so a caller cannot forget to check the second.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    Authorized(RunAuthorityRecord),
    Refused(AuthorityRefusal),
}

/// The authority port: the orchestrator's four verbs over a gateway
/// session, and the endpoint's one question over a bearer.
pub trait RunAuthority: Send + Sync {
    /// Issue a run's authority and its first token. Refused with
    /// `authority_exists` for a run that has one.
    fn issue(
        &self,
        context: RequestContext,
        request: IssueAuthorityRequest,
    ) -> Result<IssuedEndpoint>;

    /// A fresh attempt: a strictly newer generation under a fresh token.
    /// The token that was current is retired and answers `stale_generation`
    /// from then on. The grants are the record's, re-read and never widened.
    fn advance_generation(
        &self,
        context: RequestContext,
        run: RunId,
        generation: AttemptGeneration,
    ) -> Result<IssuedEndpoint>;

    /// Revoke the run's authority: a state change on the record, read by
    /// every later call. Idempotent.
    fn revoke(&self, context: RequestContext, run: RunId) -> Result<AuthorityView>;

    fn authority(&self, context: RequestContext, run: RunId) -> Result<AuthorityView>;

    /// **Where the run worked**, as this device resolved it when it started.
    /// The one read that carries a path back out of the store, for the one
    /// caller entitled to it: emptying the trash, which has to remove that
    /// folder when Koru made it. Behind the *write* scope, because reading
    /// it is not what it is for.
    fn workspace_of(&self, context: RequestContext, run: RunId) -> Result<RunWorkspaceResponse>;

    /// **The run was deleted.** The record, its grants and its tokens go;
    /// one row saying the run was deleted takes their place. Idempotent, and
    /// not an error for a run that never had an authority.
    fn forget(&self, context: RequestContext, run: RunId) -> Result<ForgetAuthorityResponse>;

    /// The versions a run has registered so far, oldest first — the hook
    /// the orchestrator reads at a turn boundary (D29).
    fn versions_produced_by(
        &self,
        context: RequestContext,
        run: RunId,
    ) -> Result<RunVersionsResponse>;

    /// The endpoint's question, asked on every call with the bearer the
    /// call presented. Takes no context: the bearer *is* the context. The
    /// outer failure is the store's; a refusal is an answer.
    fn resolve(&self, token: &str) -> Result<Resolution>;
}

impl<R: ArtifactRepository + RunAuthorityRepository + Send> RunAuthority for Service<R> {
    fn issue(
        &self,
        context: RequestContext,
        request: IssueAuthorityRequest,
    ) -> Result<IssuedEndpoint> {
        self.admitted(&context, ArtifactScope::Write)?;
        let token = mint_token();
        let mut repository = self.lock()?;
        let record = repository.issue_authority(NewAuthority {
            run: request.run,
            workspace: request.workspace,
            generation: request.generation,
            grants: request.grants,
            run_workspace: request.run_workspace,
            token_hash: hash_token(&token),
            created_at_ms: (self.clock)(),
        })?;
        Ok(IssuedEndpoint {
            run: record.run,
            generation: record.generation,
            path: endpoint_path(record.run),
            token,
        })
    }

    fn advance_generation(
        &self,
        context: RequestContext,
        run: RunId,
        generation: AttemptGeneration,
    ) -> Result<IssuedEndpoint> {
        self.admitted(&context, ArtifactScope::Write)?;
        let token = mint_token();
        let mut repository = self.lock()?;
        let record =
            repository.advance_authority(run, generation, hash_token(&token), (self.clock)())?;
        Ok(IssuedEndpoint {
            run: record.run,
            generation: record.generation,
            path: endpoint_path(record.run),
            token,
        })
    }

    fn revoke(&self, context: RequestContext, run: RunId) -> Result<AuthorityView> {
        self.admitted(&context, ArtifactScope::Write)?;
        let mut repository = self.lock()?;
        Ok(repository.revoke_authority(run, (self.clock)())?.into())
    }

    fn authority(&self, context: RequestContext, run: RunId) -> Result<AuthorityView> {
        self.admitted(&context, ArtifactScope::Read)?;
        let repository = self.lock()?;
        Ok(repository.authority(run)?.into())
    }

    fn workspace_of(&self, context: RequestContext, run: RunId) -> Result<RunWorkspaceResponse> {
        self.admitted(&context, ArtifactScope::Write)?;
        let repository = self.lock()?;
        let record = repository.authority(run)?;
        Ok(RunWorkspaceResponse {
            run: record.run,
            run_workspace: record.run_workspace,
        })
    }

    fn forget(&self, context: RequestContext, run: RunId) -> Result<ForgetAuthorityResponse> {
        self.admitted(&context, ArtifactScope::Write)?;
        let mut repository = self.lock()?;
        // Whether there was one is the answer, not a precondition: a run
        // that never started never had an authority, and deleting it is
        // still a delete.
        let held_authority = repository.authority(run).is_ok();
        repository.forget_authority(run, (self.clock)())?;
        Ok(ForgetAuthorityResponse {
            run,
            held_authority,
        })
    }

    fn versions_produced_by(
        &self,
        context: RequestContext,
        run: RunId,
    ) -> Result<RunVersionsResponse> {
        self.admitted(&context, ArtifactScope::Read)?;
        let repository = self.lock()?;
        Ok(RunVersionsResponse {
            versions: repository.versions_produced_by(run)?,
        })
    }

    fn resolve(&self, token: &str) -> Result<Resolution> {
        // A bearer that is not a run token is unknown before the store is
        // asked — no hash of a gateway session bearer ever reaches the
        // token table.
        if !token.starts_with(TOKEN_PREFIX) {
            return Ok(Resolution::Refused(AuthorityRefusal::Unknown));
        }
        let repository = self.lock()?;
        let lookup = repository
            .lookup_token(&hash_token(token))
            .map_err(|error| match error {
                ArtifactError::Storage { .. } => error,
                other => ArtifactError::Storage {
                    operation: "resolve a token",
                    detail: other.code().to_owned(),
                },
            })?;
        Ok(match AuthorityRefusal::decide(lookup) {
            Ok(record) => Resolution::Authorized(record),
            Err(refusal) => Resolution::Refused(refusal),
        })
    }
}

#[cfg(test)]
pub(crate) mod tests;
