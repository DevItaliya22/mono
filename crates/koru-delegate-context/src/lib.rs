// SPDX-License-Identifier: MPL-2.0

//! What a delegated run was given — captured bytes, not a remembered
//! number (ROADMAP §7.2).
//!
//! A run must deliver the bytes it was handed, whatever the document says
//! later (UC2). This crate is the four steps that make that true, over the
//! store's own seams and nothing invented beside them:
//!
//! 1. **Resolve** each item to a concrete identity in its workspace.
//! 2. **Capture**: a document or block becomes a `capture_history`
//!    version — the same immutable, checksummed state the History panel and
//!    the MCP tools read — pinned by its `DocumentVersionId`; a file, image
//!    or PDF is pinned by its `BlobHash` with retention asserted.
//! 3. **Materialize** into `<run-workspace>/.koru/context/<RunId>/`, per
//!    run: Markdown rendered from the captured bytes through the one
//!    exporter Koru has, blobs copied by hash, and a `manifest.json` that
//!    names every file by pin.
//! 4. Only then may the provider start — the runtime's ordering, which this
//!    crate serves by returning the layout a launch needs.
//!
//! An input that cannot be captured or materialized fails before the run
//! starts, naming the item **by index and stage** — never by title or
//! filename. Source edits and store checkpoints do not touch a pin.
//!
//! **Deletion changes nothing here (D47).** Trash and purge behave exactly
//! as they do for any document; this crate adds no cascade, purges no
//! version and redefines nothing. What it does instead is *read* the state
//! of what a pin points at — [`ContextState::Live`], [`InTrash`] or
//! [`Unavailable`] — from what the store reports at that moment, beside the
//! exact document and version the run received (D46). **D48**: an attempt
//! under way is never interrupted by a deletion, because nothing in
//! capture or materialization consults the trash; what the runtime asks
//! *before* a resume or a new run is [`StoreContext::admit`], which refuses
//! a source that is now trashed or gone. Cleanup removes materialized
//! copies, never captures.
//!
//! No line of this crate logs. Every failure is a value of tokens and
//! indices, and every `Debug` that could hold a path or a filename is
//! written by hand not to.
//!
//! [`InTrash`]: ContextState::InTrash
//! [`Unavailable`]: ContextState::Unavailable

mod admission;
mod capture;
mod cleanup;
mod fault;
pub mod layout;
mod manifest;
mod materialize;
mod render;
mod state;
mod store;

pub use admission::SourceRefusal;
pub use capture::{
    CaptureFailure, CaptureStage, CapturedContext, ContextPin, ContextSource, PinLiveFailure,
    StoreContext,
};
pub use cleanup::cleanup;
pub use fault::Fault;
pub use manifest::{MANIFEST_VERSION, Manifest, ManifestFile};
pub use materialize::{
    MaterializeFailure, MaterializeStage, MaterializedContext, MaterializedFile,
};
pub use state::{blob_state, document_state};
// The vocabulary crate owns the closed value; this crate computes it and
// re-exports it so a caller names one crate for a pin and its state.
pub use koru_delegate_core::ContextState;
pub use store::ContextStore;

// The vocabulary this crate is spoken in, so a caller names one crate.
pub use koru_delegate_core::{BlobKind, RunContextItem, RunId};
pub use koru_session_protocol::DeliveryMode;

#[cfg(test)]
mod tests;
