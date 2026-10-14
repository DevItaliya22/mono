// SPDX-License-Identifier: MPL-2.0

//! Where a workspace's **system document** is found and opened — once.
//!
//! Two things in this daemon need the tag table: the attribute query service
//! answers `koru.tag` values with names and colors, and the search indexer
//! turns a stored `TagId` into a matchable word (ADR-182). Both used to carry a
//! verbatim copy of the same two functions, and `tagnames.rs` said in its own
//! header that they owed this fold.
//!
//! **The fork discipline is the reason this is one function and not two.**
//! The table lives in root containers the projection deliberately does not
//! walk, so reaching it means the Store lock and the node model. In, fork, out:
//! the lock is held for the O(1) forks and nothing else, and snapshot encoding
//! and the CRDT union happen outside it. A second copy of that is a second
//! chance to hold the lock across a decode.
//!
//! What the two callers actually differ on is the question they ask afterwards
//! — [`SystemDoc::tags`] against [`SystemDoc::tag_names`] — so the
//! `SystemDoc` is what comes back and each caller asks its own.

use std::sync::Arc;

use koru_core::explore::KIND_SYSTEM;
use koru_core::system::SystemDoc;
use koru_projection::Projections;
use koru_store_api::state::AppState;
use koru_store_core::ids::{DocumentId, WorkspaceId};

/// Which documents hold one workspace's definitions and tag table, **canonical
/// first**.
///
/// From the projection's listing rather than by opening every workspace's
/// manifest: it converges with the same sweep the rows do, and a workspace the
/// listing has not mirrored yet answers empty — which a caller reads as *not
/// now* rather than as *no tags*.
///
/// The lowest `DocumentId` is every peer's pick — `SystemDoc::canonical_document`'s
/// rule, at the document level — and the rest are absorbed into it.
pub fn system_documents(projections: &Projections, workspace: WorkspaceId) -> Vec<DocumentId> {
    let Ok(Some(listing)) = projections.listing(workspace) else {
        return Vec::new();
    };
    let mut ids: Vec<DocumentId> = listing
        .documents
        .iter()
        .filter(|d| d.kind == KIND_SYSTEM)
        .map(|d| d.document)
        .collect();
    ids.sort_unstable();
    ids
}

/// One workspace's system document, unioned across duplicates.
///
/// In, fork, out. `None` — logged, never fatal — degrades that workspace to
/// system definitions and raw `TagId`s, which is what a chip renderer has to
/// tolerate anyway: a peer can merge or lose a tag record concurrently.
pub fn open_system_document(
    state: &Arc<AppState>,
    workspace: WorkspaceId,
    ids: &[DocumentId],
) -> Option<SystemDoc> {
    let mut forks = Vec::with_capacity(ids.len());
    {
        let mut store = state.store.lock().ok()?;
        for id in ids {
            match store.fork_document(workspace, *id) {
                Ok(fork) => forks.push(fork),
                Err(error) => {
                    tracing::debug!(%workspace, document = %id,
                        "a system document did not fork: {error}");
                }
            }
        }
    }
    let mut snapshots = forks
        .into_iter()
        .filter_map(|fork| fork.export(loro::ExportMode::snapshot()).ok());
    let mut system = SystemDoc::from_snapshot(&snapshots.next()?).ok()?;
    for snapshot in snapshots {
        if let Err(error) = system.absorb(&snapshot) {
            tracing::debug!(%workspace, "a duplicate system document did not absorb: {error}");
        }
    }
    Some(system)
}

/// Every stored `TagId` that a merge has re-pointed, paired with the id it now
/// means — `(from, into)`, in storage form.
///
/// **`merge_tags` re-points no node**, so nodes go on carrying merged-away ids
/// forever and a reader has to follow the alias itself. This is that walk,
/// flattened once per query so a `GROUP BY` can fold on it; `SystemDoc::resolve`
/// is where the walk lives, and it is bounded, so a cycle stops rather than
/// spinning.
///
/// Ids the table names but has not merged are absent — folding them would be a
/// no-op with a parameter attached.
pub fn tag_aliases(system: &SystemDoc) -> Vec<(String, String)> {
    system
        .tag_names()
        .into_iter()
        .filter_map(|(id, _)| {
            let into = system.resolve(id);
            (into != id).then(|| (id.to_storage(), into.to_storage()))
        })
        .collect()
}
