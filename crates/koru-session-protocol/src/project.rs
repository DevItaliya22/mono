// SPDX-License-Identifier: MPL-2.0

//! Projects: the durable grouping a terminal session may belong to.
//!
//! A project is a folder in the session domain and nothing more. It names no
//! process, holds no lifecycle state of its own, and carries no ordering key:
//! visibility is derived from the catalog its scope names, and the tree that
//! draws it sorts by name under the surface's own collation.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CatalogProfileId, CatalogWorkspaceId, ErrorCode, ProjectId};

/// Longest accepted project name, counted in Unicode scalar values.
///
/// Bounded rather than unbounded because the name is persisted, listed, and
/// drawn; counted in scalars rather than bytes so the limit does not depend on
/// which script somebody types in.
pub const MAX_PROJECT_NAME_CHARS: usize = 128;

/// Longest accepted project root, counted in Unicode scalar values. The host
/// still validates the path itself at launch; this only bounds the wire.
pub const MAX_PROJECT_ROOT_CHARS: usize = 4_096;

/// The Koru catalog place one project belongs to.
///
/// A typed pair, versioned with the protocol, rather than a compound string:
/// opaque to the session host does not mean unstructured on the wire. A
/// project with no scope belongs to no workspace and is visible everywhere.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct WorkspaceScope {
    pub profile: CatalogProfileId,
    pub workspace: CatalogWorkspaceId,
}

/// One project row. There is deliberately no `sort_key` and no lifecycle
/// field: archiving a workspace hides its projects without mutating any of
/// them, so there is nothing here for an `ArchiveProject` verb to write.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectSummary {
    pub id: ProjectId,
    pub name: String,
    /// An absolute local path chosen by the user, or none until "Set folder…"
    /// supplies one. A project with no root launches from today's default; a
    /// project whose root has gone missing refuses to launch instead of
    /// silently falling back.
    pub root_directory: Option<String>,
    pub workspace_scope: Option<WorkspaceScope>,
    pub created_at_ms: i64,
}

impl ProjectSummary {
    pub fn validate(&self) -> Result<(), ErrorCode> {
        validate_project_name(&self.name)?;
        validate_project_root(self.root_directory.as_deref())
    }
}

/// Names are Unicode and bounded, and duplicates are legal — the id is the
/// identity. Control characters are refused because a name is a display
/// string, not a document.
pub fn validate_project_name(name: &str) -> Result<(), ErrorCode> {
    let length = name.chars().count();
    if length == 0
        || length > MAX_PROJECT_NAME_CHARS
        || name.trim().is_empty()
        || name.chars().any(char::is_control)
    {
        return Err(ErrorCode::InvalidRequest);
    }
    Ok(())
}

/// A root is bounded and free of control characters. Whether the directory
/// exists is a launch-time question the host answers against the real
/// filesystem, never a wire-shape one.
pub fn validate_project_root(root: Option<&str>) -> Result<(), ErrorCode> {
    let Some(root) = root else {
        return Ok(());
    };
    let length = root.chars().count();
    if length == 0 || length > MAX_PROJECT_ROOT_CHARS || root.chars().any(char::is_control) {
        return Err(ErrorCode::InvalidRequest);
    }
    Ok(())
}

/// The named catalog subscription's payload.
///
/// This stream is invalidation, never content and never a command: a holder
/// re-reads `ListProjects` at its own pace. Because every notification says
/// the same thing, a queue that is already full may drop one without a reader
/// missing anything — which is what makes the stream bounded by construction.
///
/// It is an enum with one arm rather than a bare struct so the next catalog
/// fact is a new variant instead of a second subscription.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CatalogEvent {
    ProjectsChanged,
}
