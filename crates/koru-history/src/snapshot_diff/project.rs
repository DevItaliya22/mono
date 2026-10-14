// SPDX-License-Identifier: MPL-2.0

//! Projection from a [`SemanticDocument`] to the visible blocks the snapshot
//! diff compares.

use crate::unified::BlockView;
use crate::{SemanticDocument, SemanticNode};

/// A visible block plus the [`SemanticNode`] it came from.
///
/// The [`BlockView`] is what the matcher sees; the [`SemanticNode`] is kept so
/// the returned [`DocumentDiff`](crate::DocumentDiff) can carry real before/after
/// nodes for the renderer.
#[derive(Clone, PartialEq, Debug)]
pub struct ProjectedBlock {
    pub view: BlockView,
    pub node: SemanticNode,
}

/// Walk `doc` in preorder and emit one [`ProjectedBlock`] per visible block.
pub fn project_blocks(doc: &SemanticDocument) -> Vec<ProjectedBlock> {
    doc.nodes
        .iter()
        .map(|node| ProjectedBlock {
            view: BlockView::of(node),
            node: node.clone(),
        })
        .collect()
}
