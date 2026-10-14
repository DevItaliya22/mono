// SPDX-License-Identifier: MPL-2.0

//! What one pin call captures and what one item reads back, over a store
//! that lives in memory and counts every question it is asked.
//!
//! The shape a canvas hands an agent: many cards, each a Block of one
//! document, some of them pictures. Nothing here opens a store or a disk —
//! [`StoreContext::render_pinned`] is the half of materialization that reads,
//! and the write it is split from is `tests/store.rs`'s.

use std::collections::HashMap;

use koru_core::node::{BlobHash as NodeBlobHash, Node};
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_delegate_core::{BlobHash, BlockNodeId, DocumentId, DocumentVersionId, WorkspaceId};
use koru_store_core::model::{Blob, DocumentEntry, Timestamp};
use koru_store_core::ports::DocumentVersion;

use super::*;

/// A store in memory: the documents it lists, the live snapshot a capture
/// pins, the blob bytes it can serve, and a tally of what it was asked.
#[derive(Default)]
struct Memory {
    live: HashMap<DocumentId, Vec<u8>>,
    versions: HashMap<DocumentVersionId, DocumentVersion>,
    blobs: HashMap<String, Vec<u8>>,
    captures: usize,
    listings: usize,
    fetched: Vec<String>,
}

impl ContextStore for Memory {
    fn capture_version(
        &mut self,
        _workspace: WorkspaceId,
        document: DocumentId,
        at: Timestamp,
    ) -> Result<DocumentVersionId, Fault> {
        self.captures += 1;
        let snapshot = self
            .live
            .get(&document)
            .cloned()
            .ok_or(Fault::DocumentUnavailable)?;
        let id = DocumentVersionId::new();
        self.versions.insert(
            id,
            DocumentVersion {
                id,
                document,
                snapshot,
                version_vector: Vec::new(),
                frontiers: Vec::new(),
                checksum: format!("b3:{}", self.captures),
                captured_at: at,
            },
        );
        Ok(id)
    }

    fn version(
        &mut self,
        _workspace: WorkspaceId,
        version: DocumentVersionId,
    ) -> Result<Option<DocumentVersion>, Fault> {
        Ok(self.versions.get(&version).cloned())
    }

    fn entries(&mut self, _workspace: WorkspaceId) -> Result<Vec<DocumentEntry>, Fault> {
        self.listings += 1;
        Ok(self
            .live
            .keys()
            .map(|id| DocumentEntry {
                id: *id,
                parent: None,
                ordinal: 0,
                title: String::new(),
                kind: "page".into(),
                mime: None,
                note: None,
                trashed_at: None,
                trashed_by: None,
            })
            .collect())
    }

    fn blob(&mut self, _workspace: WorkspaceId, _hash: &BlobHash) -> Result<Option<Blob>, Fault> {
        Ok(None)
    }

    fn blob_bytes(&mut self, _workspace: WorkspaceId, hash: &BlobHash) -> Result<Vec<u8>, Fault> {
        self.fetched.push(hash.as_str().to_owned());
        self.blobs
            .get(hash.as_str())
            .cloned()
            .ok_or(Fault::BlobUnavailable)
    }
}

/// A blob name that parses: `b3:` and sixty-four hex digits, the `n`th.
fn hash(n: u8) -> String {
    BlobHash::from_digest("b3", &format!("{n:02x}").repeat(32))
        .unwrap()
        .as_str()
        .to_owned()
}

/// A canvas-like page: `cards` cards under its root, each a paragraph with a
/// picture of its own beneath it. Returns the document's snapshot and each
/// card's node, with the hash of the picture under it.
fn a_page_of_cards(cards: u8) -> (Vec<u8>, Vec<(BlockNodeId, String)>) {
    let mut page = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "W").unwrap();
    let root = page
        .insert(Parent::Root, Node::document("Canvas").unwrap())
        .unwrap();
    let mut out = Vec::new();
    for n in 0..cards {
        let card = page
            .insert(
                Parent::Node(root),
                Node::paragraph(format!("Card {n}")).unwrap(),
            )
            .unwrap();
        let picture = hash(n + 1);
        page.insert(
            Parent::Node(card),
            Node::image(NodeBlobHash(picture.clone()), "image/png", None, None).unwrap(),
        )
        .unwrap();
        out.push((BlockNodeId::from_uuid(card.as_uuid()), picture));
    }
    (page.export_snapshot().unwrap(), out)
}

/// **Twenty cards from one canvas are one moment of it.** One capture, one
/// History marker, and every item carries the same version.
#[test]
fn twenty_blocks_of_one_document_capture_one_version() {
    let workspace = WorkspaceId::new();
    let document = DocumentId::new();
    let (snapshot, cards) = a_page_of_cards(20);
    let mut store = Memory::default();
    store.live.insert(document, snapshot);

    let sources: Vec<ContextSource> = cards
        .iter()
        .map(|(node, _)| ContextSource::Block {
            workspace,
            document,
            node: *node,
        })
        .collect();
    let items = StoreContext::new(&mut store).pin(&sources).unwrap();

    assert_eq!(store.captures, 1, "one capture_history commit, not twenty");
    assert_eq!(store.listings, 1, "the document is resolved once");
    assert_eq!(items.len(), 20, "every card is still its own item");
    let versions: Vec<DocumentVersionId> = items
        .iter()
        .map(|item| match item {
            RunContextItem::Block { version, .. } => *version,
            other => panic!("{other:?}"),
        })
        .collect();
    assert!(versions.iter().all(|version| *version == versions[0]));
    // And the order and the nodes are the ones given.
    for (item, (node, _)) in items.iter().zip(&cards) {
        let RunContextItem::Block { node: pinned, .. } = item else {
            panic!("{item:?}")
        };
        assert_eq!(pinned, node);
    }
}

/// A whole-document item and blocks of the same document share one version;
/// a second document gets its own. Sharing is per document, never global.
#[test]
fn a_document_and_its_blocks_share_one_version_and_another_document_gets_its_own() {
    let workspace = WorkspaceId::new();
    let (first, second) = (DocumentId::new(), DocumentId::new());
    let (snapshot, cards) = a_page_of_cards(2);
    let mut store = Memory::default();
    store.live.insert(first, snapshot.clone());
    store.live.insert(second, snapshot);

    let items = StoreContext::new(&mut store)
        .pin(&[
            ContextSource::Block {
                workspace,
                document: first,
                node: cards[0].0,
            },
            ContextSource::Document {
                workspace,
                document: second,
            },
            ContextSource::Document {
                workspace,
                document: first,
            },
            ContextSource::Block {
                workspace,
                document: first,
                node: cards[1].0,
            },
        ])
        .unwrap();

    assert_eq!(store.captures, 2);
    let version = |item: &RunContextItem| match item {
        RunContextItem::Document { version, .. } | RunContextItem::Block { version, .. } => {
            *version
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(version(&items[0]), version(&items[2]));
    assert_eq!(version(&items[0]), version(&items[3]));
    assert_ne!(version(&items[0]), version(&items[1]));
}

/// A later call captures again: sharing lasts one pin call, so a second Run
/// press pins the document as it is by then.
#[test]
fn each_pin_call_captures_its_own_moment() {
    let workspace = WorkspaceId::new();
    let document = DocumentId::new();
    let (snapshot, cards) = a_page_of_cards(1);
    let mut store = Memory::default();
    store.live.insert(document, snapshot);
    let source = ContextSource::Block {
        workspace,
        document,
        node: cards[0].0,
    };

    StoreContext::new(&mut store)
        .pin(std::slice::from_ref(&source))
        .unwrap();
    StoreContext::new(&mut store).pin(&[source]).unwrap();
    assert_eq!(store.captures, 2);
}

/// **A card reads its own pictures and nobody else's.** Rendering one Block
/// of a twenty-card page fetches exactly the image under that card.
#[test]
fn a_block_fetches_only_the_images_in_its_own_subtree() {
    let workspace = WorkspaceId::new();
    let document = DocumentId::new();
    let (snapshot, cards) = a_page_of_cards(20);
    let mut store = Memory::default();
    store.live.insert(document, snapshot);
    for (_, picture) in &cards {
        store.blobs.insert(picture.clone(), b"png".to_vec());
    }
    let sources: Vec<ContextSource> = cards
        .iter()
        .map(|(node, _)| ContextSource::Block {
            workspace,
            document,
            node: *node,
        })
        .collect();
    let items = StoreContext::new(&mut store).pin(&sources).unwrap();
    let checksum = store.versions.values().next().unwrap().checksum.clone();

    for (item, (_, picture)) in items.iter().zip(&cards) {
        store.fetched.clear();
        let pin = ContextPin {
            item: item.clone(),
            checksum: checksum.clone(),
        };
        let rendered = StoreContext::new(&mut store)
            .render_pinned(&pin, "assets/card")
            .unwrap();
        assert_eq!(store.fetched, vec![picture.clone()]);
        assert_eq!(rendered.assets.len(), 1, "its picture is delivered");
    }
}

/// **A picture that cannot be served fails the card it is in, and only that
/// card.** Before, every Block prefetched the whole document's images, so one
/// unavailable picture failed every card from the page.
#[test]
fn an_unavailable_image_in_another_card_does_not_fail_this_card() {
    let workspace = WorkspaceId::new();
    let document = DocumentId::new();
    let (snapshot, cards) = a_page_of_cards(2);
    let mut store = Memory::default();
    store.live.insert(document, snapshot);
    // The first card's picture is here; the second card's is not.
    store.blobs.insert(cards[0].1.clone(), b"png".to_vec());
    let items = StoreContext::new(&mut store)
        .pin(&[
            ContextSource::Block {
                workspace,
                document,
                node: cards[0].0,
            },
            ContextSource::Block {
                workspace,
                document,
                node: cards[1].0,
            },
        ])
        .unwrap();
    let checksum = store.versions.values().next().unwrap().checksum.clone();
    let pin = |item: &RunContextItem| ContextPin {
        item: item.clone(),
        checksum: checksum.clone(),
    };

    let first = StoreContext::new(&mut store).render_pinned(&pin(&items[0]), "assets/a");
    assert!(first.is_ok(), "the card with its picture renders");
    let second = StoreContext::new(&mut store).render_pinned(&pin(&items[1]), "assets/b");
    assert_eq!(
        second.err(),
        Some((MaterializeStage::Read, Fault::BlobUnavailable)),
        "the card whose picture is missing still fails, rather than arriving without it"
    );
}

/// A Document item is the whole page, so it still reads every picture in it.
#[test]
fn a_document_item_still_fetches_every_image_in_its_document() {
    let workspace = WorkspaceId::new();
    let document = DocumentId::new();
    let (snapshot, cards) = a_page_of_cards(3);
    let mut store = Memory::default();
    store.live.insert(document, snapshot);
    for (_, picture) in &cards {
        store.blobs.insert(picture.clone(), b"png".to_vec());
    }
    let items = StoreContext::new(&mut store)
        .pin(&[ContextSource::Document {
            workspace,
            document,
        }])
        .unwrap();
    let checksum = store.versions.values().next().unwrap().checksum.clone();

    store.fetched.clear();
    StoreContext::new(&mut store)
        .render_pinned(
            &ContextPin {
                item: items[0].clone(),
                checksum,
            },
            "assets/page",
        )
        .unwrap();
    let mut fetched = store.fetched.clone();
    fetched.sort();
    let mut every: Vec<String> = cards.iter().map(|(_, picture)| picture.clone()).collect();
    every.sort();
    assert_eq!(fetched, every);
}

/// A nested page under a card is its own document, not rendered with the
/// card — so its pictures are not fetched with it either.
#[test]
fn a_nested_page_under_a_block_is_not_fetched_with_it() {
    let mut page = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "W").unwrap();
    let root = page
        .insert(Parent::Root, Node::document("Canvas").unwrap())
        .unwrap();
    let card = page
        .insert(Parent::Node(root), Node::paragraph("Card").unwrap())
        .unwrap();
    let nested = page
        .insert(Parent::Node(card), Node::document("Nested").unwrap())
        .unwrap();
    page.insert(
        Parent::Node(nested),
        Node::image(NodeBlobHash(hash(9)), "image/png", None, None).unwrap(),
    )
    .unwrap();
    page.insert(
        Parent::Node(card),
        Node::image(NodeBlobHash(hash(8)), "image/png", None, None).unwrap(),
    )
    .unwrap();

    let wanted = crate::render::block_blobs(&page, BlockNodeId::from_uuid(card.as_uuid())).unwrap();
    assert_eq!(wanted, vec![hash(8)]);
}
