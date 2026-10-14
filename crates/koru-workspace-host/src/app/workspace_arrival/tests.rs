// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn first_arrival_selects_a_live_workspace_without_stealing_an_existing_selection() {
    let mut archived = WorkspaceEntry::new(WorkspaceId::new(), "Archived", 1);
    archived.archived = true;
    let mut deleted = WorkspaceEntry::new(WorkspaceId::new(), "Deleted", 2);
    deleted.deleted_at = Some(3);
    let first = WorkspaceEntry::new(WorkspaceId::new(), "First", 3);
    let second = WorkspaceEntry::new(WorkspaceId::new(), "Second", 4);
    assert_eq!(first_workspace_when_empty(false, &[]), None);
    assert_eq!(
        first_workspace_when_empty(false, &[archived.clone(), deleted.clone()]),
        None
    );
    let rows = [archived, deleted, first.clone(), second];
    assert_eq!(first_workspace_when_empty(false, &rows), Some(first.id));
    assert_eq!(first_workspace_when_empty(true, &rows), None);
}
