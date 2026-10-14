// SPDX-License-Identifier: MPL-2.0

//! Snapshot-based visible-content diff for the visual compare path.
//!
//! This module compares two [`SemanticDocument`]s by their visible text, not by
//! stable [`NodeId`]. It reuses the existing [`DocumentDiff`] model and feeds
//! the existing [`unified`](crate::unified::unified) projector (via
//! [`unified_with_correspondence`](crate::unified::unified_with_correspondence)).
//!
//! Semantic history — attribution, moves, audit — stays in
//! [`compare`](crate::compare) and is not replaced.

use std::collections::HashMap;
use std::time::Instant;

use koru_core::NodeId;

use crate::unified::DiffOptions;
use crate::{DocumentDiff, SemanticDocument};

mod engine;
mod project;
mod refine;

#[cfg(test)]
mod tests;

pub use project::{ProjectedBlock, project_blocks};

/// Per-stage timing for one snapshot compare, measured on the worker.
#[derive(Clone, Copy, Debug, Default)]
pub struct SnapshotDiffTiming {
    pub project_us: u64,
    pub outer_us: u64,
    pub refine_us: u64,
}

/// Compare two semantic snapshots and return a [`DocumentDiff`] plus the
/// before→after correspondence map required by
/// [`unified_with_correspondence`](crate::unified::unified_with_correspondence).
pub fn snapshot_compare(
    before: &SemanticDocument,
    after: &SemanticDocument,
    options: DiffOptions,
) -> (DocumentDiff, HashMap<NodeId, NodeId>) {
    let (diff, correspondence, _) = snapshot_compare_timed(before, after, options);
    (diff, correspondence)
}

/// [`snapshot_compare`] with per-stage timing for `KORU_HISTORY_DEBUG`.
pub fn snapshot_compare_timed(
    before: &SemanticDocument,
    after: &SemanticDocument,
    options: DiffOptions,
) -> (DocumentDiff, HashMap<NodeId, NodeId>, SnapshotDiffTiming) {
    let project_start = Instant::now();
    let before_blocks = project_blocks(before);
    let after_blocks = project_blocks(after);
    let project_us = project_start.elapsed().as_micros() as u64;

    let (diff, correspondence, engine_timing) =
        engine::compare_blocks(&before_blocks, &after_blocks, options);

    (
        diff,
        correspondence,
        SnapshotDiffTiming {
            project_us,
            outer_us: engine_timing.outer_us,
            refine_us: engine_timing.refine_us,
        },
    )
}
