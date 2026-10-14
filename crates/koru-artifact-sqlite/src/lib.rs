// SPDX-License-Identifier: MPL-2.0

//! Sole owner of `artifacts.db` access and schema migration.
//!
//! The database holds identity, metadata and provenance — never a byte of
//! content, which lives in the workspace's blob store under the hash a
//! version names. It is device-local (D11), opened by the gateway worker
//! (D13) at `<channel-root>/artifacts/artifacts.db` beside `ai/ai.db`, and it
//! follows the convention `koru-ai-sqlite` and the events journal already
//! follow: `PRAGMA user_version` is read **before any other pragma**, a
//! database stamped by a newer build is refused with
//! [`OpenError::SchemaTooNew`] and left exactly as it was, and each
//! forward-only step runs in its own transaction and stamps its number.
//!
//! `synchronous = FULL`, because a registered version is a fact a person is
//! entitled to see (ROADMAP §7.1), not an observation.
//!
//! [`run_workspaces`] is the crate's one **read-only** view: it opens the
//! same file without the gateway, sets no pragma and writes nothing, so a
//! diagnostic can ask which folders D37's retained results are sitting in
//! while the worker that owns the database is stopped.

mod db;
mod inspect;
mod repository;

pub use db::{ARTIFACTS_SCHEMA_VERSION, OpenError};
pub use inspect::{InspectError, RunWorkspaceRow, run_workspaces};
pub use repository::Repository;

pub const DATABASE_FILE: &str = "artifacts.db";
