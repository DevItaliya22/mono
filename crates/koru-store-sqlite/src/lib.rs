// SPDX-License-Identifier: MPL-2.0

//! **SQLite behind the store's ports.**
//!
//! `koru-store-core` says what must be stored and what must be atomic; this
//! crate says how. Nothing above it knows SQL exists, and no route, no bundle
//! and no error message names a table — third-party clients never need to learn
//! Koru's physical layout.
//!
//! ```text
//! catalog.db                     profiles, workspaces, lineage, credentials, state
//! workspaces/<id>/workspace.db   loro_documents, loro_checkpoints, loro_updates,
//!                                the derived document mirror, blob metadata
//! workspaces/<id>/blobs/<hash>   bytes, outside Loro and outside SQLite
//! ```
//!
//! ## What this crate is responsible for getting right
//!
//! - **`synchronous = FULL`.** See [`db`]. An acknowledgement means the bytes
//!   are on the disk, and under WAL that is not what `NORMAL` promises.
//! - **The two transactions.** [`WorkspaceRepo::commit_updates`] writes the
//!   journal and the document's position together;
//!   [`WorkspaceRepo::commit_checkpoint`] writes the snapshot and prunes what
//!   it covers together. An implementation that satisfies the signature but not
//!   the transaction is a data-loss bug that type-checks.
//! - **Forward-only migrations.** A database newer than this build is refused,
//!   never rewritten.
//!
//! [`WorkspaceRepo::commit_updates`]: koru_store_core::ports::WorkspaceRepo::commit_updates
//! [`WorkspaceRepo::commit_checkpoint`]: koru_store_core::ports::WorkspaceRepo::commit_checkpoint

pub mod backend;
pub mod backup;
pub mod blobs;
pub mod catalog;
pub mod db;
pub mod index;
pub mod workspace;

#[cfg(test)]
mod service_tests;

pub use backend::SqliteBackend;
pub use backup::{BACKUPS_KEPT, SqliteBackups};
pub use blobs::{PendingSweep, SqliteBlobs};
pub use catalog::SqliteCatalog;
pub use workspace::SqliteWorkspace;
