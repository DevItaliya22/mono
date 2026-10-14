// SPDX-License-Identifier: MPL-2.0

//! Cleanup removes materialized copies — never a capture, never a blob.
//!
//! `<run-workspace>/.koru/context/<RunId>/` and nothing beside it: a
//! sibling run's directory stays, the store's `document_versions` row
//! stays, the workspace's blob stays. Idempotent — a directory that is
//! already gone is a success, because the question was "is it gone".

use std::fs;
use std::path::Path;

use koru_delegate_core::RunId;

use crate::{Fault, layout};

/// Remove one run's context directory. `Ok` when it is gone, whether or
/// not it was there.
pub fn cleanup(run: RunId, run_workspace: &Path) -> Result<(), Fault> {
    let root = layout::run_dir(run_workspace, run);
    match fs::remove_dir_all(&root) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Fault::from_io(&e)),
    }
}
