// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn body_revision_changes_the_leaf_without_a_manifest_change() {
    let workspace = crate::WorkspaceId::from_uuid(uuid::Uuid::from_u128(1));
    let document_id = crate::DocumentId::from_uuid(uuid::Uuid::from_u128(2));
    let before = revision(b"durable position one");
    let after = revision(b"durable position two");
    assert_ne!(
        document(workspace, document_id, before),
        document(workspace, document_id, after)
    );
    assert_ne!(
        document(workspace, document_id, before),
        document(
            workspace,
            crate::DocumentId::from_uuid(uuid::Uuid::from_u128(3)),
            before
        )
    );
}
