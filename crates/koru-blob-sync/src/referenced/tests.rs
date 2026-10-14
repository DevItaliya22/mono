// SPDX-License-Identifier: MPL-2.0

use super::*;

use crate::vectors;

fn node(blob: &str, mime: &str) -> WalkNode {
    WalkNode {
        blob: Some(blob.into()),
        mime: (!mime.is_empty()).then(|| mime.to_owned()),
    }
}

fn address(seed: &str) -> String {
    vectors::address_of(seed)
}

/// Two references to one picture are one object: identity is content.
#[test]
fn a_hash_appears_once_however_many_nodes_name_it() {
    let one = address("one");
    let walk = referenced_blobs(&[
        WalkDocument {
            durable: true,
            trashed: false,
            nodes: vec![node(&one, "image/png"), WalkNode::default()],
        },
        WalkDocument {
            durable: true,
            trashed: false,
            nodes: vec![node(&one, "image/png")],
        },
    ]);
    assert_eq!(walk.blobs.len(), 1);
    assert_eq!(walk.blobs[0].hash.as_str(), one);
    assert_eq!(walk.blobs[0].references, 2);
    assert_eq!(walk.blobs[0].mime.as_deref(), Some("image/png"));
}

/// **A reference in Trash still counts.** ADR-95 stamps the manifest entry and
/// leaves the content intact, so the bytes are still owed to a restore.
#[test]
fn a_binned_document_still_references_its_picture() {
    let one = address("one");
    let walk = referenced_blobs(&[WalkDocument {
        durable: true,
        trashed: true,
        nodes: vec![node(&one, "image/png")],
    }]);
    assert_eq!(walk.blobs.len(), 1, "a trashed reference was dropped");
}

/// **Permanently removed content does not.** The corpus has no blob under a
/// removed document, so this is the rule stated on its own; the gap is written
/// to the maintainers with a proposed case.
#[test]
fn a_permanently_removed_document_references_nothing() {
    let one = address("one");
    let walk = referenced_blobs(&[
        WalkDocument {
            durable: false,
            trashed: false,
            nodes: vec![node(&one, "image/png")],
        },
        WalkDocument {
            durable: false,
            trashed: true,
            nodes: vec![node(&one, "image/png")],
        },
    ]);
    assert!(
        walk.blobs.is_empty(),
        "a removed document kept its bytes alive"
    );
}

/// A reference that is not an address cannot be transferred to anything, and
/// saying nothing about it would claim a completeness the walk has not got.
#[test]
fn an_unreadable_reference_is_counted_rather_than_guessed_at() {
    let walk = referenced_blobs(&[WalkDocument {
        durable: true,
        trashed: false,
        nodes: vec![
            node("not-a-hash", "image/png"),
            node("b3:zzzz", "image/png"),
            node("   ", "image/png"),
            node(&address("one"), ""),
        ],
    }]);
    assert_eq!(walk.unreadable, 2);
    assert_eq!(walk.blobs.len(), 1);
    assert_eq!(walk.blobs[0].mime, None, "a blank mime is not a media type");
}

/// The same state twice is the same list twice, or a queue is unreadable.
#[test]
fn the_answer_is_ordered_by_address() {
    let mut nodes: Vec<WalkNode> = (0..8).map(|n| node(&address(&n.to_string()), "")).collect();
    let forward = referenced_blobs(&[WalkDocument {
        durable: true,
        trashed: false,
        nodes: nodes.clone(),
    }]);
    nodes.reverse();
    let backward = referenced_blobs(&[WalkDocument {
        durable: true,
        trashed: false,
        nodes,
    }]);
    assert_eq!(forward, backward);
    let mut sorted: Vec<&str> = forward.blobs.iter().map(|b| b.hash.as_str()).collect();
    let original = sorted.clone();
    sorted.sort_unstable();
    assert_eq!(original, sorted);
}

/// **The corpus guard.**
///
/// For every case that enumerates documents, the walk is run over the
/// workspace's state and the blobs it found are compared with the corpus's
/// own `files_bytes` — the number the billing contract publishes. A walk that
/// excluded the Trash (case 12), required a `file` listing kind (case 13),
/// deduplicated wrongly (case 10) or counted a reserved upload's bytes (case
/// 19) disagrees with a published number rather than with an opinion.
#[test]
fn the_walk_agrees_with_the_logical_usage_corpus() {
    let mut walked = 0usize;
    let mut with_blobs = 0usize;
    let mut checked_bytes = 0usize;
    for case in vectors::cases() {
        for workspace in &case.input.workspaces {
            let Some(documents) = workspace.walk_documents() else {
                // Case 21's documents are parametric and cases 17 and 22 are
                // about an account migration; none of the three describes a
                // set of references.
                continue;
            };
            walked += 1;
            let walk = referenced_blobs(&documents);
            let known = workspace.blobs_by_address();
            if !known.is_empty() {
                with_blobs += 1;
            }

            // Every blob the corpus lists for the workspace is one the walk
            // found: the corpus lists the workspace's files, and a file it
            // holds that no live document references would be one this list
            // could not explain.
            for address in known.keys() {
                assert!(
                    walk.blobs.iter().any(|blob| blob.hash.as_str() == address),
                    "{}/{}: the walk missed a blob the corpus lists",
                    case.id,
                    workspace.workspace
                );
            }
            // And nothing the walk found is unknown to the corpus.
            for blob in &walk.blobs {
                assert!(
                    known.contains_key(blob.hash.as_str()),
                    "{}/{}: the walk invented a blob",
                    case.id,
                    workspace.workspace
                );
            }
            assert_eq!(
                walk.unreadable, 0,
                "{}: the corpus has no bad address",
                case.id
            );

            let expected = case
                .expected
                .workspaces
                .as_ref()
                .and_then(|list| list.iter().find(|e| e.workspace == workspace.workspace))
                .and_then(|entry| entry.files_bytes);
            let Some(expected) = expected else {
                continue;
            };
            // The billing sum: the walk's live set, narrowed to what Cloud
            // actually holds bytes for. Case 19's reference is live and its
            // upload never finalized, so it is in the walk and not in the
            // sum — which is exactly why a transfer engine needs the walk and
            // a bill needs the intersection.
            let counted: u64 = walk
                .blobs
                .iter()
                .filter_map(|blob| known.get(blob.hash.as_str()))
                .filter(|blob| blob.durable)
                .filter_map(|blob| blob.verified_plaintext_length_bytes)
                .sum();
            assert_eq!(
                counted, expected,
                "{}/{}: the walk and the published files_bytes disagree",
                case.id, workspace.workspace
            );
            checked_bytes += 1;
        }
    }
    // A guard that silently stopped reading the corpus would pass. These are
    // the counts as of `logical-usage-v1` v4: twenty-three walkable
    // workspaces, seven of them holding a blob, twenty with a published sum.
    // A case added to the corpus moves one of these, which is the point: the
    // guard notices that it was extended rather than quietly ignoring it.
    assert_eq!(walked, 23, "the corpus's walkable workspaces moved");
    assert_eq!(with_blobs, 7, "the corpus's blob-bearing workspaces moved");
    assert_eq!(checked_bytes, 20, "the corpus's published sums moved");
}

/// **A file that is not a picture is one of the files** (D259).
///
/// The walk has never consulted the media type and this pins that it never
/// starts to: the count a person reads is *every blob this Workspace
/// references*, and the one they had just attached — a 1.9 GiB
/// `application/octet-stream` archive — was the one a surface saying
/// "Pictures" appeared to exclude. The plumbing was right and the sentence
/// was wrong; a test here is what keeps the plumbing right while the
/// sentence is corrected above it.
#[test]
fn a_non_image_attachment_is_counted_beside_the_pictures() {
    let photo = address("photo");
    let archive = address("archive");
    let walk = referenced_blobs(&[WalkDocument {
        durable: true,
        trashed: false,
        nodes: vec![
            node(&photo, "image/png"),
            node(&archive, "application/octet-stream"),
        ],
    }]);
    assert_eq!(walk.blobs.len(), 2, "the archive is a referenced blob");
    let archive_entry = walk
        .blobs
        .iter()
        .find(|blob| blob.hash.as_str() == archive)
        .expect("the archive is in the walk");
    assert_eq!(
        archive_entry.mime.as_deref(),
        Some("application/octet-stream"),
        "and it is carried with its own media type, not guessed at",
    );
}

/// **Per document, by the same rule.** A document naming one picture twice
/// and a malformed address references that picture once; a document no
/// longer durable references nothing, exactly as the Workspace walk says.
#[test]
fn a_documents_own_blobs_follow_the_workspace_walks_rule() {
    let one = address("one");
    let two = address("two");
    let document = WalkDocument {
        durable: true,
        trashed: true,
        nodes: vec![
            node(&one, "image/png"),
            node(&one, ""),
            node(" ", ""),
            node("not-a-hash", ""),
            node(&two, ""),
        ],
    };
    assert_eq!(
        document_blobs(&document),
        BTreeSet::from([one.clone(), two.clone()])
    );
    let removed = WalkDocument {
        durable: false,
        ..document
    };
    assert!(document_blobs(&removed).is_empty());
}

/// **The per-document walk with its media types**: an address once, carrying
/// the first type any node recorded for it (a blank one does not win), and a
/// malformed reference counted rather than dropped.
#[test]
fn a_documents_references_carry_the_first_recorded_type() {
    let one = address("one");
    let two = address("two");
    let document = WalkDocument {
        durable: true,
        trashed: false,
        nodes: vec![
            node(&one, ""),
            node(&one, "image/png"),
            node(&one, "application/pdf"),
            node(&two, ""),
            node("not-a-hash", "image/png"),
        ],
    };
    let references = document_references(&document);
    assert_eq!(
        references.blobs,
        BTreeMap::from([(one, Some("image/png".to_owned())), (two, None)])
    );
    assert_eq!(references.unreadable, 1);
    let removed = WalkDocument {
        durable: false,
        ..document
    };
    assert_eq!(document_references(&removed), DocumentReferences::default());
}
