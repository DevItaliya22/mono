// SPDX-License-Identifier: MPL-2.0

//! The workspace's list of documents, held as the store's own `Manifest`.
//!
//! ## Why the store's type rather than one of our own
//!
//! A workspace needs somewhere to record what documents it has, their order and
//! their nesting. `koru_store_core::manifest::Manifest` is exactly that, and it
//! is reachable: that crate is synchronous and links no HTTP *by design*,
//! because a mobile build embeds it — so using it here costs `loro`, `uuid` and
//! `blake3` and nothing else.
//!
//! Writing a local twin was the alternative and it loses twice. The manifest's
//! hard parts are already solved and already tested over there — concurrent
//! creates, rename-versus-rename converging on one whole title rather than a
//! character merge, edit-versus-delete not resurrecting a deleted document, a
//! duplicate id resolving the same way on every replica. A second
//! implementation would have to earn all of that again, and then agree with the
//! first forever. And when the list moves onto the store there is no conversion
//! at the seam, because both sides are the same type.
//!
//! `koru-core` deliberately does **not** depend on the store: it is the node
//! model, and a node knows nothing about how a workspace is filed. The
//! dependency belongs one layer up, here, where persistence already lives.
//!
//! ## The two id types
//!
//! `koru_core::WorkspaceId` and `koru_store_core::ids::WorkspaceId` are
//! different types that mean the same thing, and the same is true of
//! `DocumentId`. Both are v4-UUID newtypes, so the bridge is total and lossless
//! in both directions — [`store_workspace`] and the `Doc` pair below. They are
//! not unified because that would make `koru-core` depend on the store to name
//! a workspace, which is the coupling this module exists to keep out of it.

use koru_core::{DocumentId, WorkspaceId};
use koru_store_core::ids::{DocumentId as StoreDocumentId, WorkspaceId as StoreWorkspaceId};

/// A Koru workspace id as the store spells it.
pub fn store_workspace(id: WorkspaceId) -> StoreWorkspaceId {
    StoreWorkspaceId::from_uuid(id.as_uuid())
}

/// A Koru document id as the store spells it.
pub fn store_document(id: DocumentId) -> StoreDocumentId {
    StoreDocumentId::from_uuid(id.as_uuid())
}

/// A store document id as Koru spells it.
pub fn koru_document(id: StoreDocumentId) -> DocumentId {
    DocumentId::from_uuid(id.as_uuid())
}

#[cfg(test)]
mod tests;
