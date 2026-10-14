// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn durable_signals_wake_exact_documents_and_their_workspace() {
    use koru_store_core::commit_signal::CommitSignal;
    let hub = Changes::default();
    let workspace = WorkspaceId::from_uuid(uuid::Uuid::from_u128(1));
    let document = DocumentId::from_uuid(uuid::Uuid::from_u128(2));
    let other = DocumentId::from_uuid(uuid::Uuid::from_u128(3));
    let parent = hub.subscribe(Dataset::Workspace(workspace));
    let exact = hub.subscribe(Dataset::Document(workspace, document));
    let viewer = hub.subscribe(Dataset::DocumentView(document));
    let unrelated = hub.subscribe(Dataset::Document(workspace, other));
    hub.clone().committed(workspace, document);
    assert!(parent.pending());
    assert!(exact.pending());
    assert!(viewer.pending());
    assert!(!unrelated.pending());
}

#[test]
fn wakes_are_scoped_coalesced_and_do_not_lose_changes_during_a_pass() {
    let hub = Changes::default();
    let a = Dataset::Workspace(WorkspaceId::from_uuid(uuid::Uuid::from_u128(1)));
    let b = Dataset::Workspace(WorkspaceId::from_uuid(uuid::Uuid::from_u128(2)));
    let mut watch = hub.subscribe(a);
    hub.changed(b);
    assert!(!watch.pending());
    for _ in 0..1000 {
        hub.changed(a);
    }
    assert!(watch.pending());
    watch.begin_pass();
    assert!(!watch.pending());
    hub.changed(a);
    assert!(watch.pending());
    drop(watch);
    let _other = hub.subscribe(b);
    assert_eq!(hub.0.lock().unwrap().len(), 1);
}
