// SPDX-License-Identifier: MPL-2.0

//! The family: the thing a person names, under which versions accumulate.

use serde::{Deserialize, Serialize};

use crate::{ArtifactFamilyId, ArtifactTitle, RunId, WorkspaceId};

/// One artifact family. Workspace-scoped (D11), titled by a person or by the
/// run that made it, and **retired rather than deleted**: `retired_at_ms`
/// is a mark a reader can see and compare, and every version under a retired
/// family stays readable (PRINCIPLES, *retire state*).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ArtifactFamily {
    pub id: ArtifactFamilyId,
    pub workspace: WorkspaceId,
    pub title: ArtifactTitle,
    /// The run whose registration created the family, when an agent did
    /// (D6); `None` when a person created it by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by_run: Option<RunId>,
    pub created_at_ms: i64,
    /// Set once, by [`crate::ArtifactRepository::retire_family`]. A retired
    /// family accepts no new version and is left out of the ordinary listing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at_ms: Option<i64>,
}

impl ArtifactFamily {
    pub fn is_retired(&self) -> bool {
        self.retired_at_ms.is_some()
    }
}

/// What creating a family takes. The id is minted by the caller, so a retry
/// carries the same one and is refused as a duplicate rather than making a
/// second family with the same title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewFamily {
    pub id: ArtifactFamilyId,
    pub workspace: WorkspaceId,
    pub title: ArtifactTitle,
    pub created_by_run: Option<RunId>,
    pub created_at_ms: i64,
}
