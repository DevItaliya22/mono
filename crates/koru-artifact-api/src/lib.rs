// SPDX-License-Identifier: MPL-2.0

//! The typed internal artifact-service boundary, and the service behind it.
//!
//! **The same seam Koru AI has.** The gateway's routes speak
//! [`ArtifactServerPort`] with a [`RequestContext`] it built from the bearer
//! it admitted; no bearer, HTTP request, database row or blob byte crosses.
//! Where the AI domain answers the port from a worker with its own event
//! loop, an artifact store has no loop to run — it is a database and a
//! permission rule — so the answer lives here as [`Service`], over any
//! [`ArtifactRepository`], and the gateway composes it with the SQLite one.
//!
//! Two decisions are made in this crate and nowhere else:
//!
//! - **Scope, then permission.** A call first needs the scope its route
//!   declares ([`ArtifactScope`]); it then needs the [`Access`] the family's
//!   permission rows grant its principal, decided by
//!   [`koru_artifact_core::authorize`]. In M1 the principal is always
//!   [`Principal::LocalOwner`] and owns every family it creates (D11), so the
//!   second check always passes — but it is *made*, against real rows, which
//!   is what makes M7 a change to the principal rather than to the service.
//! - **The blob's size is the Store's word.** [`VerifiedBlob`] is the
//!   gateway's promise that it asked the workspace's blob store whether the
//!   hash is there and what it holds; a registration never trusts a
//!   caller-stated size.
//!
//! A run is a third caller, beside the person and M7's peers, and it gets
//! its own two ports over the same service: [`RunAuthority`] — issue,
//! advance, revoke, and resolve a bearer to its record on every call — and
//! [`RunToolPort`], the decisions behind the six tools at
//! `/v1/runs/{run}/mcp` (§7.3). [`ArtifactDomain`] is all three ports as
//! one object, which is what the gateway holds.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use koru_artifact_core::{
    Access, ArtifactError, ArtifactFamily, ArtifactFamilyId, ArtifactRepository, ArtifactTitle,
    ArtifactVersion, ArtifactVersionId, NewFamily, NewVersion, Principal, Provenance,
    RunAuthorityRepository, RunLink, WorkspaceId, authorize,
};
use koru_artifact_wire::{
    CreateFamilyRequest, Failure, FamilyResponse, LinkRunRequest, ListFamiliesQuery,
    ListFamiliesResponse, RegisterVersionRequest, RegisterVersionResponse,
};

mod authority;
mod run_folder;
mod run_tools;

pub use authority::{
    IssuedEndpoint, Resolution, RunAuthority, TOKEN_PREFIX, endpoint_path, hash_token,
};
pub use run_folder::{RunFolderPort, RunFolderTarget, failure_from};
pub use run_tools::{
    RegistrationTarget, RunToolPort, StagedFile, ToolResult, family_id, registration_id,
};

/// Every port the artifact domain answers, as one object: the person's
/// routes, the orchestrator's authority verbs, the run's tools, and the
/// person's side of a run's results folder.
pub trait ArtifactDomain: ArtifactServerPort + RunAuthority + RunToolPort + RunFolderPort {}

impl<T: ArtifactServerPort + RunAuthority + RunToolPort + RunFolderPort> ArtifactDomain for T {}

/// The two scopes `/v1/artifacts` declares. Both are in the gateway's
/// ordinary default set: reading what a run produced and registering a
/// result are what a window and an MCP tool do, and a person can see both.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ArtifactScope {
    Read,
    Write,
}

impl ArtifactScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "artifacts.read",
            Self::Write => "artifacts.write",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "artifacts.read" => Some(Self::Read),
            "artifacts.write" => Some(Self::Write),
            _ => None,
        }
    }
}

/// Who is calling: the gateway's label, the artifact scopes it holds, and
/// the [`Principal`] its rows are decided against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactPrincipal {
    pub label: String,
    pub scopes: Vec<ArtifactScope>,
    pub principal: Principal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestContext {
    pub principal: ArtifactPrincipal,
    pub trace_id: String,
}

/// What the gateway learned from the blob store before registering: the
/// hash is there, under the family's workspace, and this is its size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifiedBlob {
    pub size_bytes: u64,
}

pub type Result<T> = std::result::Result<T, Failure>;

/// Host-independent artifact contract used by the gateway's routes.
pub trait ArtifactServerPort: Send + Sync {
    fn list_families(
        &self,
        context: RequestContext,
        query: ListFamiliesQuery,
    ) -> Result<ListFamiliesResponse>;
    fn create_family(
        &self,
        context: RequestContext,
        request: CreateFamilyRequest,
    ) -> Result<ArtifactFamily>;
    fn family(&self, context: RequestContext, family: ArtifactFamilyId) -> Result<FamilyResponse>;
    fn retire_family(
        &self,
        context: RequestContext,
        family: ArtifactFamilyId,
    ) -> Result<ArtifactFamily>;
    /// The first half of a registration: the same scope and permission
    /// decision `register_version` makes, and the workspace whose blob store
    /// the gateway must then ask about the hash. Refused for a retired
    /// family, so no blob is looked up for a registration that cannot land.
    fn registration_target(
        &self,
        context: RequestContext,
        family: ArtifactFamilyId,
    ) -> Result<WorkspaceId>;
    fn register_version(
        &self,
        context: RequestContext,
        family: ArtifactFamilyId,
        request: RegisterVersionRequest,
        blob: VerifiedBlob,
    ) -> Result<RegisterVersionResponse>;
    fn version(
        &self,
        context: RequestContext,
        version: ArtifactVersionId,
    ) -> Result<ArtifactVersion>;
    fn provenance(&self, context: RequestContext, version: ArtifactVersionId)
    -> Result<Provenance>;
    fn link_run(
        &self,
        context: RequestContext,
        version: ArtifactVersionId,
        request: LinkRunRequest,
    ) -> Result<RunLink>;
}

/// The service: scope, permission, clock, and the repository underneath.
///
/// One mutex around the repository, because a SQLite connection is one
/// writer and the gateway calls from `spawn_blocking`; the lock is held for
/// one call and never across an await.
pub struct Service<R> {
    repository: Mutex<R>,
    clock: fn() -> i64,
}

fn wall_clock_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn forbidden() -> Failure {
    Failure::new(
        "forbidden",
        "The gateway session does not permit this artifact operation.",
        false,
    )
}

impl<R: ArtifactRepository + RunAuthorityRepository + Send> Service<R> {
    pub fn new(repository: R) -> Self {
        Self::with_clock(repository, wall_clock_ms)
    }

    /// A service whose "now" is the caller's — for a test that asserts a
    /// timestamp, and for nothing else.
    pub fn with_clock(repository: R, clock: fn() -> i64) -> Self {
        Self {
            repository: Mutex::new(repository),
            clock,
        }
    }

    fn admitted(&self, context: &RequestContext, scope: ArtifactScope) -> Result<()> {
        if context.principal.scopes.contains(&scope) {
            Ok(())
        } else {
            Err(forbidden())
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, R>> {
        self.repository.lock().map_err(|_| {
            Failure::from(ArtifactError::Storage {
                operation: "lock the repository",
                detail: "a previous call panicked while holding it".into(),
            })
        })
    }

    fn permitted(
        repository: &R,
        principal: Principal,
        family: ArtifactFamilyId,
        required: Access,
    ) -> Result<()> {
        let permissions = repository.permissions(family)?;
        authorize(&permissions, principal, required)?;
        Ok(())
    }
}

impl<R: ArtifactRepository + RunAuthorityRepository + Send> ArtifactServerPort for Service<R> {
    fn list_families(
        &self,
        context: RequestContext,
        query: ListFamiliesQuery,
    ) -> Result<ListFamiliesResponse> {
        self.admitted(&context, ArtifactScope::Read)?;
        let repository = self.lock()?;
        let principal = context.principal.principal;
        let mut families = Vec::new();
        for family in repository.list_families(query.workspace, query.include_retired)? {
            // A family the caller may not read is simply not listed.
            if Self::permitted(&repository, principal, family.id, Access::Read).is_ok() {
                families.push(family);
            }
        }
        Ok(ListFamiliesResponse { families })
    }

    fn create_family(
        &self,
        context: RequestContext,
        request: CreateFamilyRequest,
    ) -> Result<ArtifactFamily> {
        self.admitted(&context, ArtifactScope::Write)?;
        // `ArtifactTitle` deserializes as it arrives, so a synced header a
        // wider build wrote still opens; a request body is the boundary
        // where the bound is enforced, and a title the store could not read
        // back must never be written.
        let title = ArtifactTitle::new(request.title.as_str())
            .map_err(|_| Failure::from(ArtifactError::InvalidRequest))?;
        let mut repository = self.lock()?;
        let family = repository.create_family(
            NewFamily {
                id: request.family_id,
                workspace: request.workspace,
                title,
                created_by_run: request.created_by_run,
                created_at_ms: (self.clock)(),
            },
            context.principal.principal,
        )?;
        Ok(family)
    }

    fn family(&self, context: RequestContext, family: ArtifactFamilyId) -> Result<FamilyResponse> {
        self.admitted(&context, ArtifactScope::Read)?;
        let repository = self.lock()?;
        let found = repository.family(family)?;
        Self::permitted(
            &repository,
            context.principal.principal,
            family,
            Access::Read,
        )?;
        let versions = repository.versions(family)?;
        Ok(FamilyResponse {
            family: found,
            versions,
        })
    }

    fn retire_family(
        &self,
        context: RequestContext,
        family: ArtifactFamilyId,
    ) -> Result<ArtifactFamily> {
        self.admitted(&context, ArtifactScope::Write)?;
        let mut repository = self.lock()?;
        repository.family(family)?;
        Self::permitted(
            &repository,
            context.principal.principal,
            family,
            Access::Owner,
        )?;
        Ok(repository.retire_family(family, (self.clock)())?)
    }

    fn registration_target(
        &self,
        context: RequestContext,
        family: ArtifactFamilyId,
    ) -> Result<WorkspaceId> {
        self.admitted(&context, ArtifactScope::Write)?;
        let repository = self.lock()?;
        let found = repository.family(family)?;
        Self::permitted(
            &repository,
            context.principal.principal,
            family,
            Access::Owner,
        )?;
        if found.is_retired() {
            return Err(ArtifactError::FamilyRetired.into());
        }
        Ok(found.workspace)
    }

    fn register_version(
        &self,
        context: RequestContext,
        family: ArtifactFamilyId,
        request: RegisterVersionRequest,
        blob: VerifiedBlob,
    ) -> Result<RegisterVersionResponse> {
        self.admitted(&context, ArtifactScope::Write)?;
        let mut repository = self.lock()?;
        repository.family(family)?;
        Self::permitted(
            &repository,
            context.principal.principal,
            family,
            Access::Owner,
        )?;
        let registered = repository.register_version(NewVersion {
            registration_id: request.registration_id,
            family,
            blob: request.blob,
            size_bytes: blob.size_bytes,
            mime: request.mime,
            filename: request.filename,
            produced_by: request.produced_by,
            derived_from: request.derived_from,
            created_at_ms: (self.clock)(),
        })?;
        Ok(RegisterVersionResponse {
            version: registered.version,
            retried: registered.retried,
        })
    }

    fn version(
        &self,
        context: RequestContext,
        version: ArtifactVersionId,
    ) -> Result<ArtifactVersion> {
        self.admitted(&context, ArtifactScope::Read)?;
        let repository = self.lock()?;
        let found = repository.version(version)?;
        Self::permitted(
            &repository,
            context.principal.principal,
            found.family,
            Access::Read,
        )?;
        Ok(found)
    }

    fn provenance(
        &self,
        context: RequestContext,
        version: ArtifactVersionId,
    ) -> Result<Provenance> {
        self.admitted(&context, ArtifactScope::Read)?;
        let repository = self.lock()?;
        let provenance = repository.provenance(version)?;
        // Every family the walk crossed, once.
        let mut families: Vec<_> = provenance.versions().map(|v| v.family).collect();
        families.sort();
        families.dedup();
        for family in families {
            Self::permitted(
                &repository,
                context.principal.principal,
                family,
                Access::Read,
            )?;
        }
        Ok(provenance)
    }

    fn link_run(
        &self,
        context: RequestContext,
        version: ArtifactVersionId,
        request: LinkRunRequest,
    ) -> Result<RunLink> {
        self.admitted(&context, ArtifactScope::Write)?;
        let mut repository = self.lock()?;
        let found = repository.version(version)?;
        Self::permitted(
            &repository,
            context.principal.principal,
            found.family,
            Access::Owner,
        )?;
        let link = RunLink {
            run: request.run,
            version,
            role: request.role,
            linked_at_ms: (self.clock)(),
        };
        repository.link_run(link.clone())?;
        Ok(link)
    }
}

#[cfg(test)]
mod tests;
