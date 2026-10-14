// SPDX-License-Identifier: MPL-2.0

//! The stable error codes of the artifact service.
//!
//! Every variant has one wire code, spelled once in [`ArtifactError::code`],
//! and the same code is what the database's own refusal maps to: the
//! `artifact_versions_are_immutable` trigger raises
//! `artifact_version_immutable`, and the store turns that message into
//! [`ArtifactError::VersionImmutable`], so a caller at the SQL layer and a
//! caller at the API layer read the same word. No message carries a title, a
//! filename or a principal.

use std::fmt;

/// Why an artifact operation was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArtifactError {
    /// No family under that id.
    FamilyNotFound,
    /// No version under that id — including a `derived_from` that names
    /// nothing.
    VersionNotFound,
    /// A family under that id already exists.
    FamilyExists,
    /// The family was retired; it accepts no new version.
    FamilyRetired,
    /// A version cannot be changed. Register a new one.
    VersionImmutable,
    /// The registration id was seen before, under a different family or
    /// with different bytes: a retry that is not a retry.
    RegistrationConflict,
    /// The principal holds no access that permits the operation.
    PermissionDenied,
    /// The blob is not in the workspace's store, so there is nothing to
    /// register.
    BlobNotFound,
    /// The request was malformed in a way a code names better than a
    /// message.
    InvalidRequest,
    /// An authority for that run was already issued. Advance its
    /// generation or revoke it; a second issue would mint a second token
    /// for the same attempt.
    AuthorityExists,
    /// No authority was ever issued for that run.
    AuthorityNotFound,
    /// The generation offered is not newer than the one that is current —
    /// the same code a stale token is refused with (§7.3).
    StaleGeneration,
    /// The run's authority was revoked; nothing under it proceeds.
    AuthorityRevoked,
    /// The store failed. `operation` is from a closed vocabulary; `detail`
    /// is the driver's message, which names no content.
    Storage {
        operation: &'static str,
        detail: String,
    },
}

impl ArtifactError {
    /// The wire code. Stable: a client matches on it, and a test pins it.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::FamilyNotFound => "artifact_family_not_found",
            Self::VersionNotFound => "artifact_not_found",
            Self::FamilyExists => "artifact_family_exists",
            Self::FamilyRetired => "artifact_family_retired",
            Self::VersionImmutable => "artifact_version_immutable",
            Self::RegistrationConflict => "registration_conflict",
            Self::PermissionDenied => "permission_denied",
            Self::BlobNotFound => "blob_not_found",
            Self::InvalidRequest => "invalid_request",
            Self::AuthorityExists => "authority_exists",
            Self::AuthorityNotFound => "authority_not_found",
            Self::StaleGeneration => "stale_generation",
            Self::AuthorityRevoked => "authority_revoked",
            Self::Storage { .. } => "artifact_storage_failed",
        }
    }

    /// Whether the same call may succeed later without a change from the
    /// caller. Only a storage fault is.
    pub const fn retryable(&self) -> bool {
        matches!(self, Self::Storage { .. })
    }
}

impl fmt::Display for ArtifactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage { operation, detail } => {
                write!(f, "could not {operation}: {detail}")
            }
            other => f.write_str(other.code()),
        }
    }
}

impl std::error::Error for ArtifactError {}
