// SPDX-License-Identifier: MPL-2.0

//! A Profile opened before sync has no selected Workspace until one arrives.
use koru_core::{WorkspaceId, profile::WorkspaceEntry};

pub(super) fn first_workspace_when_empty(
    has_workspace: bool,
    workspaces: &[WorkspaceEntry],
) -> Option<WorkspaceId> {
    if has_workspace {
        return None;
    }
    workspaces
        .iter()
        .find(|row| !row.archived && row.deleted_at.is_none())
        .map(|row| row.id)
}

#[cfg(test)]
mod tests;
