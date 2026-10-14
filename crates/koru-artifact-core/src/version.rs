// SPDX-License-Identifier: MPL-2.0

//! The version: one immutable set of bytes registered under a family.

use serde::{Deserialize, Serialize};

use crate::{
    ArtifactFamilyId, ArtifactVersionId, AttemptGeneration, BlobHash, Filename, MediaType,
    RegistrationId, RunId,
};

/// A version's place in its family, starting at [`Ordinal::FIRST`]. Assigned
/// by the store when a version is registered — the next free number under
/// the family — and unique there (`UNIQUE (family_id, ordinal)`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct Ordinal(u32);

impl Ordinal {
    pub const FIRST: Self = Self(1);

    /// Saturates rather than wrapping, as `AttemptGeneration::next` does.
    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    pub fn get(self) -> u32 {
        self.0
    }

    /// An ordinal read back from storage. Zero was never assigned.
    pub fn from_raw(value: u32) -> Option<Self> {
        (value > 0).then_some(Self(value))
    }
}

/// Which attempt of which run produced a version. Both halves, because a
/// run's generation is the fence §7.5 and §7.6 rely on: a late registration
/// from an older attempt is recorded as that attempt's, not the current one's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProducedBy {
    pub run: RunId,
    pub generation: AttemptGeneration,
}

/// One immutable artifact version.
///
/// Identity is `id`; content is `blob`, an address in the workspace's
/// content-addressed store. The two are separate on purpose: the same bytes
/// registered twice are two versions with two histories, and a reference to
/// a version keeps meaning the same thing after the family moves on.
/// `derived_from` names the version this one was made from, which is how a
/// provenance walk crosses generations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ArtifactVersion {
    pub id: ArtifactVersionId,
    pub family: ArtifactFamilyId,
    pub ordinal: Ordinal,
    pub blob: BlobHash,
    pub size_bytes: u64,
    pub mime: MediaType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<Filename>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub produced_by: Option<ProducedBy>,
    pub registration_id: RegistrationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_from: Option<ArtifactVersionId>,
    pub created_at_ms: i64,
}

/// What registering a version takes. The version id and the ordinal are
/// minted by the store; the registration id is the caller's, and it is the
/// idempotency key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewVersion {
    pub registration_id: RegistrationId,
    pub family: ArtifactFamilyId,
    pub blob: BlobHash,
    /// As the blob store reports it — never as a caller states it.
    pub size_bytes: u64,
    pub mime: MediaType,
    pub filename: Option<Filename>,
    pub produced_by: Option<ProducedBy>,
    pub derived_from: Option<ArtifactVersionId>,
    pub created_at_ms: i64,
}

/// The answer to a registration: the version, and whether this call found
/// one an earlier attempt had already made under the same registration id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registered {
    pub version: ArtifactVersion,
    pub retried: bool,
}
