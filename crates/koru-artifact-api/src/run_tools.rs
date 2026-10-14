// SPDX-License-Identifier: MPL-2.0

//! What the six run tools decide (§7.3, D6, D28), over any repository.
//!
//! The gateway owns the transport and the bytes: it resolves the bearer,
//! stages a file from `<run-workspace>/.artifacts/<RunId>/` under the
//! register path rules, files the bytes in the workspace's blob store and
//! reads them back for `read`. Everything that is a *decision* is here —
//! may this run touch this family ([`koru_artifact_core::authorize_family`]),
//! which workspace the bytes go to, which id makes a retry the same
//! version, what provenance a registration records — so the transport
//! cannot drift from the rule and a test can drive the rule without HTTP.
//!
//! **Idempotency is derived, not minted.** A registration is identified by
//! `(run, attempt, canonical relative path, content hash)` and a family a
//! run creates by `(run, canonical relative path)`, both folded into
//! v4-shaped UUIDs, so an agent that retries `create` after a lost reply
//! finds its version and its family instead of making a second of each,
//! and the same path written again with new bytes is the next version of
//! the same family. The path itself is never stored: only its digest is.

use koru_artifact_core::{
    ArtifactError, ArtifactFamily, ArtifactFamilyId, ArtifactRepository, ArtifactTitle,
    ArtifactVersion, ArtifactVersionId, BlobHash, Filename, LinkRole, MediaType, NewFamily,
    NewVersion, Principal, ProducedBy, RegistrationId, RunAuthorityRecord, RunAuthorityRepository,
    RunLink, WorkspaceId, authorize_family,
};
use koru_artifact_wire::run_tools::{
    AttachOutput, MetadataOutput, RegisteredOutput, SEARCH_LIMIT, SearchHit, SearchOutput,
    ToolFailure,
};
use uuid::Uuid;

use crate::Service;

/// What the gateway staged: a file under the run's folder, read under the
/// register path rules and already filed in the target workspace's blob
/// store. `relative_path` is the canonical path inside
/// `.artifacts/<RunId>/`, `/`-separated — the registration's identity, and
/// never stored.
#[derive(Clone, PartialEq, Eq)]
pub struct StagedFile {
    pub relative_path: String,
    pub filename: Option<Filename>,
    pub blob: BlobHash,
    /// As the blob store reported it.
    pub size_bytes: u64,
}

impl std::fmt::Debug for StagedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StagedFile")
            .field("blob", &self.blob)
            .field("size_bytes", &self.size_bytes)
            .finish_non_exhaustive()
    }
}

/// Where the bytes of a registration go, and the media type they take
/// when the call named none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistrationTarget {
    pub workspace: WorkspaceId,
    pub mime: MediaType,
}

pub type ToolResult<T> = std::result::Result<T, ToolFailure>;

/// The tools' decisions. Every method takes the resolved authority — the
/// only place a run id or a workspace comes from.
pub trait RunToolPort: Send + Sync {
    /// Before staging for `create`: the title and media type are valid,
    /// and the bytes go to the run's own workspace — always, whatever the
    /// grants say (D28: an empty set denies workspace *access*, and a
    /// result the run wrote is not access to anything).
    fn create_target(
        &self,
        authority: &RunAuthorityRecord,
        title: &str,
        mime: Option<&str>,
    ) -> ToolResult<RegistrationTarget>;

    /// `create`, after the bytes are filed.
    fn create(
        &self,
        authority: &RunAuthorityRecord,
        staged: StagedFile,
        title: &str,
        mime: Option<&str>,
    ) -> ToolResult<RegisteredOutput>;

    /// Before staging for `create_version`: the family exists, the run may
    /// reach it, it is not retired, and this is the workspace whose blob
    /// store takes the bytes.
    fn create_version_target(
        &self,
        authority: &RunAuthorityRecord,
        family: ArtifactFamilyId,
        mime: Option<&str>,
    ) -> ToolResult<RegistrationTarget>;

    /// `create_version`, after the bytes are filed.
    fn create_version(
        &self,
        authority: &RunAuthorityRecord,
        family: ArtifactFamilyId,
        staged: StagedFile,
        mime: Option<&str>,
    ) -> ToolResult<RegisteredOutput>;

    /// `get`.
    fn get(
        &self,
        authority: &RunAuthorityRecord,
        version: ArtifactVersionId,
    ) -> ToolResult<MetadataOutput>;

    /// For `read`: the version and the workspace its blob is in, after
    /// the grant decision. The gateway fetches the bytes.
    fn content_target(
        &self,
        authority: &RunAuthorityRecord,
        version: ArtifactVersionId,
    ) -> ToolResult<(ArtifactVersion, WorkspaceId)>;

    /// `search`.
    fn search(&self, authority: &RunAuthorityRecord, query: &str) -> ToolResult<SearchOutput>;

    /// `attach_to_run`.
    fn attach_to_run(
        &self,
        authority: &RunAuthorityRecord,
        version: ArtifactVersionId,
    ) -> ToolResult<AttachOutput>;
}

/// A v4-shaped UUID folded from a domain and its parts, so a strict id
/// parser accepts it and two calls with the same inputs get the same id.
fn derived_uuid(domain: &str, parts: &[&str]) -> Uuid {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain.as_bytes());
    hasher.update(&[0]);
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update(&[0]);
    }
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

/// `(run, attempt, canonical relative path, hash)`: a retry is the same
/// version; a fresh attempt registering the same bytes is a new one.
pub fn registration_id(authority: &RunAuthorityRecord, staged: &StagedFile) -> RegistrationId {
    RegistrationId::from_uuid(derived_uuid(
        "koru.artifact.registration",
        &[
            &authority.run.to_storage(),
            &authority.generation.get().to_string(),
            &staged.relative_path,
            staged.blob.as_str(),
        ],
    ))
}

/// `(run, canonical relative path)`: the same path written again by the
/// same run is the next version of the same family.
pub fn family_id(authority: &RunAuthorityRecord, relative_path: &str) -> ArtifactFamilyId {
    ArtifactFamilyId::from_uuid(derived_uuid(
        "koru.artifact.family",
        &[&authority.run.to_storage(), relative_path],
    ))
}

fn media_type(mime: Option<&str>, fallback: impl FnOnce() -> MediaType) -> ToolResult<MediaType> {
    match mime {
        Some(mime) => MediaType::new(mime).map_err(|_| ArtifactError::InvalidRequest.into()),
        None => Ok(fallback()),
    }
}

fn octet_stream() -> MediaType {
    MediaType::new(MediaType::OCTET_STREAM).expect("the default media type is well-formed")
}

impl<R: ArtifactRepository + RunAuthorityRepository + Send> Service<R> {
    fn locked(&self) -> ToolResult<std::sync::MutexGuard<'_, R>> {
        self.lock()
            .map_err(|failure| ToolFailure::new(failure.code, failure.message))
    }

    /// The version and its family, after the grant decision.
    fn reachable_version(
        repository: &R,
        authority: &RunAuthorityRecord,
        version: ArtifactVersionId,
    ) -> ToolResult<(ArtifactVersion, ArtifactFamily)> {
        let version = repository.version(version)?;
        let family = repository.family(version.family)?;
        authorize_family(authority, &family)?;
        Ok((version, family))
    }

    /// Register `staged` under `family` as the run's output, or find the
    /// version an identical earlier call made.
    fn register(
        repository: &mut R,
        authority: &RunAuthorityRecord,
        family: ArtifactFamily,
        staged: StagedFile,
        mime: MediaType,
        now: i64,
    ) -> ToolResult<RegisteredOutput> {
        let registration = registration_id(authority, &staged);
        let latest = repository.versions(family.id)?.into_iter().last();
        let registered = repository.register_version(NewVersion {
            registration_id: registration,
            family: family.id,
            blob: staged.blob,
            size_bytes: staged.size_bytes,
            mime,
            filename: staged.filename,
            produced_by: Some(ProducedBy {
                run: authority.run,
                generation: authority.generation,
            }),
            derived_from: latest.map(|version| version.id),
            created_at_ms: now,
        })?;
        if !registered.retried {
            repository.link_run(RunLink {
                run: authority.run,
                version: registered.version.id,
                role: LinkRole::Output,
                linked_at_ms: now,
            })?;
        }
        Ok(RegisteredOutput {
            version: registered.version,
            family,
            retried: registered.retried,
        })
    }
}

impl<R: ArtifactRepository + RunAuthorityRepository + Send> RunToolPort for Service<R> {
    fn create_target(
        &self,
        authority: &RunAuthorityRecord,
        title: &str,
        mime: Option<&str>,
    ) -> ToolResult<RegistrationTarget> {
        ArtifactTitle::new(title).map_err(|_| ToolFailure::from(ArtifactError::InvalidRequest))?;
        Ok(RegistrationTarget {
            workspace: authority.workspace,
            mime: media_type(mime, octet_stream)?,
        })
    }

    fn create(
        &self,
        authority: &RunAuthorityRecord,
        staged: StagedFile,
        title: &str,
        mime: Option<&str>,
    ) -> ToolResult<RegisteredOutput> {
        let target = self.create_target(authority, title, mime)?;
        let title = ArtifactTitle::new(title).map_err(|_| ArtifactError::InvalidRequest)?;
        let now = (self.clock)();
        let mut repository = self.locked()?;
        // A retry after a lost reply: the same registration finds the same
        // version, whatever family it landed in.
        if let Some(existing) =
            repository.version_by_registration(registration_id(authority, &staged))?
        {
            if existing.blob != staged.blob {
                return Err(ArtifactError::RegistrationConflict.into());
            }
            let family = repository.family(existing.family)?;
            return Ok(RegisteredOutput {
                version: existing,
                family,
                retried: true,
            });
        }
        let id = family_id(authority, &staged.relative_path);
        let family = match repository.family(id) {
            Ok(family) if family.created_by_run == Some(authority.run) => family,
            // A family id that collides with one another run made cannot
            // happen — the run is part of the derivation — but the rule is
            // stated rather than assumed.
            Ok(_) => return Err(ArtifactError::FamilyExists.into()),
            Err(ArtifactError::FamilyNotFound) => repository.create_family(
                NewFamily {
                    id,
                    workspace: target.workspace,
                    title,
                    created_by_run: Some(authority.run),
                    created_at_ms: now,
                },
                // The person owns what their run made (D11).
                Principal::LocalOwner,
            )?,
            Err(error) => return Err(error.into()),
        };
        Self::register(&mut repository, authority, family, staged, target.mime, now)
    }

    fn create_version_target(
        &self,
        authority: &RunAuthorityRecord,
        family: ArtifactFamilyId,
        mime: Option<&str>,
    ) -> ToolResult<RegistrationTarget> {
        let repository = self.locked()?;
        let family = repository.family(family)?;
        authorize_family(authority, &family)?;
        if family.is_retired() {
            return Err(ArtifactError::FamilyRetired.into());
        }
        let latest = repository.versions(family.id)?.into_iter().last();
        Ok(RegistrationTarget {
            workspace: family.workspace,
            mime: media_type(mime, || {
                latest.map_or_else(octet_stream, |version| version.mime)
            })?,
        })
    }

    fn create_version(
        &self,
        authority: &RunAuthorityRecord,
        family: ArtifactFamilyId,
        staged: StagedFile,
        mime: Option<&str>,
    ) -> ToolResult<RegisteredOutput> {
        let target = self.create_version_target(authority, family, mime)?;
        let now = (self.clock)();
        let mut repository = self.locked()?;
        let family = repository.family(family)?;
        Self::register(&mut repository, authority, family, staged, target.mime, now)
    }

    fn get(
        &self,
        authority: &RunAuthorityRecord,
        version: ArtifactVersionId,
    ) -> ToolResult<MetadataOutput> {
        let repository = self.locked()?;
        let (version, family) = Self::reachable_version(&repository, authority, version)?;
        Ok(MetadataOutput { version, family })
    }

    fn content_target(
        &self,
        authority: &RunAuthorityRecord,
        version: ArtifactVersionId,
    ) -> ToolResult<(ArtifactVersion, WorkspaceId)> {
        let repository = self.locked()?;
        let (version, family) = Self::reachable_version(&repository, authority, version)?;
        Ok((version, family.workspace))
    }

    fn search(&self, authority: &RunAuthorityRecord, query: &str) -> ToolResult<SearchOutput> {
        // D28: an empty grant set denies all workspace access, and a search
        // is a workspace call.
        if authority.grants.is_empty() {
            return Err(ArtifactError::PermissionDenied.into());
        }
        let needle = query.trim().to_lowercase();
        let repository = self.locked()?;
        let mut hits = Vec::new();
        let mut truncated = false;
        for workspace in &authority.grants {
            for family in repository.list_families(*workspace, false)? {
                if !family.title.as_str().to_lowercase().contains(&needle) {
                    continue;
                }
                if hits.len() == SEARCH_LIMIT {
                    truncated = true;
                    break;
                }
                let versions = repository.versions(family.id)?;
                hits.push(SearchHit { family, versions });
            }
            if truncated {
                break;
            }
        }
        Ok(SearchOutput { hits, truncated })
    }

    fn attach_to_run(
        &self,
        authority: &RunAuthorityRecord,
        version: ArtifactVersionId,
    ) -> ToolResult<AttachOutput> {
        let mut repository = self.locked()?;
        let (version, _) = Self::reachable_version(&repository, authority, version)?;
        let link = RunLink {
            run: authority.run,
            version: version.id,
            role: LinkRole::Input,
            linked_at_ms: (self.clock)(),
        };
        repository.link_run(link.clone())?;
        Ok(AttachOutput { link })
    }
}

#[cfg(test)]
mod tests;
