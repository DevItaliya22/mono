// SPDX-License-Identifier: MPL-2.0

//! Diffing costs work proportional to the document, not to its square.
//!
//! A ratio, not a wall clock (PRINCIPLES): a millisecond budget passes on a
//! fast machine and relaxes until it asserts nothing. The trap this pins is
//! the anchor computation and the reorder analysis — both were written O(n)
//! and O(n log n), and an accidental O(n²) (a per-removal survivor scan, say)
//! turns a wholesale deletion of a large document into seconds.

use std::time::Instant;

use koru_core::{Content, Kind, Node, NodeId, NodeMeta, Parent};
use koru_history::unified::{DiffOptions, unified};
use koru_history::{SemanticDocument, SemanticNode, compare};

fn doc(count: usize, edit_every: usize, tag: &str) -> SemanticDocument {
    let nodes = (0..count)
        .map(|i| {
            let text = if edit_every > 0 && i % edit_every == 0 {
                format!("block {i} {tag} edited")
            } else {
                format!("block {i} body text that is a little longer than a word")
            };
            SemanticNode {
                node: Node {
                    id: NodeId::new(),
                    kind: Kind::Paragraph,
                    meta: NodeMeta::default(),
                    content: Content::text(text),
                },
                parent: Parent::Root,
                ordinal: i,
                depth: 1,
                runs: Vec::new(),
            }
        })
        .collect();
    SemanticDocument {
        schema_version: None,
        nodes,
    }
}

fn paired(count: usize) -> (SemanticDocument, SemanticDocument) {
    let before = doc(count, 0, "a");
    // Same ids, every 10th block edited, one middle block removed — the
    // realistic shape: mostly unchanged, position bookkeeping shifted.
    let mut after = before.clone();
    for (i, node) in after.nodes.iter_mut().enumerate() {
        if i % 10 == 0 {
            node.node.content = Content::text(format!("block {i} b edited"));
        }
    }
    after.nodes.remove(count / 2);
    for (i, node) in after.nodes.iter_mut().enumerate() {
        node.ordinal = i;
    }
    (before, after)
}

fn measure(count: usize) -> f64 {
    let (before, after) = paired(count);
    let started = Instant::now();
    let diff = compare(&before, &after);
    let out = unified(&before, &after, &diff, DiffOptions::default());
    let elapsed = started.elapsed().as_secs_f64();
    assert!(out.stats.removed >= 1);
    elapsed
}

#[test]
fn a_double_size_document_costs_about_double() {
    // Warm up allocators and caches so the first measurement is not paying
    // one-time costs the second does not.
    measure(500);
    let small = measure(4_000).max(1e-4);
    let large = measure(8_000).max(1e-4);
    let ratio = large / small;
    // Linear-ish work doubles; quadratic quadruples. The bar sits well above
    // noise and well below the failure it exists to catch.
    assert!(
        ratio < 3.2,
        "8k blocks cost {ratio:.1}× 4k blocks — diffing has gone superlinear"
    );
}
