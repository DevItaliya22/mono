// SPDX-License-Identifier: MPL-2.0

//! An on-demand manifest turn pulls what an import wrote through its own
//! store client — the listing must not wait for the loop's cadence.

use super::*;
use crate::{Workspace, explore};
use koru_store_core::{manifest::Manifest, model::Timestamp};

#[test]
fn an_imports_entries_are_listed_the_moment_it_completes() {
    let (manifest_session, manifest_link, _) = session();
    let mut workspace =
        Workspace::new(WorkspaceDoc::create(koru_core::WorkspaceId::new(), "T").expect("create"));
    workspace.manifest_session = Some(manifest_session);
    let manifest =
        Manifest::create(StoreWorkspaceId::new(), "Test", Timestamp::from_millis(1)).unwrap();
    let mut remote = Manifest::from_snapshot(&manifest.export_snapshot().unwrap()).unwrap();
    workspace.manifest = Some(manifest);

    // The import wrote through its own client: the new entry is only on the
    // wire, and a manifest read before the turn cannot see it.
    let id = koru_core::DocumentId::new();
    remote
        .add_document(
            explore::store_document(id),
            "budget.xlsx",
            koru_core::explore::KIND_FILE,
            None,
        )
        .unwrap();
    assert!(workspace.entries().is_empty(), "nothing drained yet");
    manifest_link.delivers(vec![remote.export_all_updates().unwrap()]);

    assert!(workspace.sync_manifest_now(), "the turn saw the entry");
    assert!(workspace.take_entries_changed(), "and registered it");
    assert_eq!(workspace.entries()[0].title, "budget.xlsx");
}
