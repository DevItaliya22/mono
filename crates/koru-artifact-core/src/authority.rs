// SPDX-License-Identifier: MPL-2.0

//! The run authority record — what a run is allowed to do, decided on the
//! server at every call (ROADMAP §7.3, D28).
//!
//! A delegated provider reaches Koru's artifact tools through one stable
//! endpoint per run, presenting a bearer token. Nothing about *who* is
//! calling travels in a request: the token is looked up in `artifacts.db`,
//! and the record it names says which run this is, which attempt is current,
//! which workspaces the run may reach and where its `.artifacts/<RunId>/`
//! folder is. So revocation is a state change on that row, a delegate worker
//! that crashed leaves a provider still able to publish, and a restarted
//! gateway reloads the same record.
//!
//! **Generation fencing is done by token, not by a number the caller
//! sends.** Every attempt gets a fresh token; advancing the generation
//! retires the previous token rather than erasing it, so a call carrying an
//! older attempt's token is refused with [`AuthorityRefusal::StaleGeneration`]
//! — distinguishable from a token that was never issued
//! ([`AuthorityRefusal::Unknown`]) precisely because the retired hash is
//! kept (PRINCIPLES, *retire state; do not erase it*).
//!
//! **The grant set follows Koru AI's model (D28).** It is a set of workspace
//! ids, replaced wholesale and frozen for an attempt: the delegating
//! workspace is the initial grant, the person may remove it, and an empty
//! set denies all workspace access. [`authorize_family`] is the one rule that
//! decides whether a run may touch a family, so the service and the tools
//! cannot drift into two.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{ArtifactError, ArtifactFamily, AttemptGeneration, RunId, WorkspaceId};

/// The folder under a run's working scope where an agent writes results
/// (D6). One subfolder per run; the preamble (P1.12) names it exactly.
pub const ARTIFACTS_DIR: &str = ".artifacts";

/// `<run-workspace>/.artifacts/<RunId>` — the only place a registration
/// accepts a path from.
pub fn artifacts_folder(run_workspace: &Path, run: RunId) -> PathBuf {
    run_workspace.join(ARTIFACTS_DIR).join(run.to_storage())
}

/// The run's working scope on this device. A path is content (standing
/// requirement 7): it is stored so the gateway can resolve `Register` paths
/// against it, and it is redacted from every `Debug` rendering.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct RunWorkspace(String);

impl RunWorkspace {
    /// Accept an absolute path, spelled in UTF-8 so it survives the wire
    /// and the database unchanged. A relative path is refused: the gateway
    /// has no working directory that would make it mean anything.
    pub fn new(path: &Path) -> Result<Self, ArtifactError> {
        if !path.is_absolute() {
            return Err(ArtifactError::InvalidRequest);
        }
        path.to_str()
            .map(|spelling| Self(spelling.to_owned()))
            .ok_or(ArtifactError::InvalidRequest)
    }

    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }

    /// The one place the spelling leaves the type: storage and the wire.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RunWorkspace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RunWorkspace(<redacted>)")
    }
}

/// The hash of an endpoint token — the only form a token exists in once it
/// has been handed out. Lowercase hex of a 32-byte digest, computed by the
/// service that minted the token; the store keys on it and never sees the
/// bearer itself.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TokenHash(String);

impl TokenHash {
    /// A digest already in hex. Refused unless it is 64 lowercase hex
    /// characters, so a token can never be stored in the clear by mistake.
    pub fn from_hex(hex: impl Into<String>) -> Result<Self, ArtifactError> {
        let hex = hex.into();
        let well_formed = hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        well_formed
            .then_some(Self(hex))
            .ok_or(ArtifactError::InvalidRequest)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A prefix only: enough to tell two hashes apart in a test failure, not
/// enough to search a database for one.
impl fmt::Debug for TokenHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "token({})", &self.0[..8])
    }
}

/// Whether the authority still stands. A closed domain, `CHECK`ed by the
/// database as well; a revoked record stays, marked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AuthorityState {
    Active,
    Revoked,
}

impl AuthorityState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Revoked => "revoked",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "revoked" => Some(Self::Revoked),
            _ => None,
        }
    }
}

/// One run's authority, as `artifacts.db` holds it.
///
/// `workspace` is the run's own — where a result it creates lands, whether
/// or not it is in `grants`. `generation` is the attempt whose token is
/// current. `grants` is D28's set, frozen for the attempt: a resume re-reads
/// it and never widens it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RunAuthorityRecord {
    pub run: RunId,
    pub workspace: WorkspaceId,
    pub generation: AttemptGeneration,
    pub grants: BTreeSet<WorkspaceId>,
    pub run_workspace: RunWorkspace,
    pub state: AuthorityState,
    pub created_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at_ms: Option<i64>,
}

impl RunAuthorityRecord {
    /// `<run-workspace>/.artifacts/<RunId>` for this run.
    pub fn artifacts_folder(&self) -> PathBuf {
        artifacts_folder(self.run_workspace.as_path(), self.run)
    }
}

/// What issuing an authority takes: the record's facts and the hash of
/// the token the caller minted for the first attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewAuthority {
    pub run: RunId,
    pub workspace: WorkspaceId,
    pub generation: AttemptGeneration,
    pub grants: BTreeSet<WorkspaceId>,
    pub run_workspace: RunWorkspace,
    pub token_hash: TokenHash,
    pub created_at_ms: i64,
}

/// What a token names, as the store answers it: the authority, the
/// generation the token was issued for, and whether a later attempt has
/// retired it. Turning that into a decision is [`AuthorityRefusal::decide`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenLookup {
    pub authority: RunAuthorityRecord,
    pub token_generation: AttemptGeneration,
    pub retired: bool,
}

/// Why a token was refused. Three facts a caller can tell apart, each with
/// its own stable code, and none of them names the run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AuthorityRefusal {
    /// The token belongs to an attempt a later one has retired (§7.3:
    /// "a call carrying an older attempt generation is refused with a
    /// stable code").
    StaleGeneration,
    /// The run's authority was revoked.
    Revoked,
    /// No token like it was ever issued.
    Unknown,
}

impl AuthorityRefusal {
    pub const fn code(self) -> &'static str {
        match self {
            Self::StaleGeneration => "stale_generation",
            Self::Revoked => "authority_revoked",
            Self::Unknown => "authority_unknown",
        }
    }

    /// The decision, from what the store found. Pure, so the resolution is
    /// one rule wherever it is asked. A retired token is stale whatever the
    /// record's state — the fence is checked before revocation so a stale
    /// caller learns nothing about the run's later life.
    pub fn decide(lookup: Option<TokenLookup>) -> Result<RunAuthorityRecord, Self> {
        let Some(lookup) = lookup else {
            return Err(Self::Unknown);
        };
        if lookup.retired || lookup.token_generation != lookup.authority.generation {
            return Err(Self::StaleGeneration);
        }
        match lookup.authority.state {
            AuthorityState::Active => Ok(lookup.authority),
            AuthorityState::Revoked => Err(Self::Revoked),
        }
    }
}

impl fmt::Display for AuthorityRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for AuthorityRefusal {}

/// The one decision D28 asks of every tool call: may this run touch this
/// family?
///
/// Yes when the family's workspace is in the run's grant set, and yes for a
/// family the run itself created — its own results are reachable whatever
/// the grants say, because it wrote the bytes. Everything else is
/// [`ArtifactError::PermissionDenied`], which never names the workspace.
/// An empty grant set therefore denies every family but the run's own.
pub fn authorize_family(
    authority: &RunAuthorityRecord,
    family: &ArtifactFamily,
) -> crate::Result<()> {
    if family.created_by_run == Some(authority.run) || authority.grants.contains(&family.workspace)
    {
        Ok(())
    } else {
        Err(ArtifactError::PermissionDenied)
    }
}
