// SPDX-License-Identifier: MPL-2.0

//! Image nodes: the model side. Bytes are out of scope here (they live in the
//! blob store, never the CRDT — ADR-12); what these pin is that an image node
//! can be created, a text node can be *converted* into one in place, and that
//! the dimensions travel with it.

use koru_core::node::{BlobHash, Content, Node};
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_core::{Kind, NodeId, WorkspaceId};

fn ws() -> WorkspaceDoc {
    WorkspaceDoc::create(WorkspaceId::new(), "acme").unwrap()
}

/// Inserting a freshly-built image node reads back with its hash, mime and
/// dimensions intact.
#[test]
fn an_image_node_round_trips() {
    let mut ws = ws();
    let img = Node::image(BlobHash("abc123".into()), "image/png", Some(800), Some(600)).unwrap();
    let id = ws.insert(Parent::Root, img).unwrap();

    let read = ws.read(id).unwrap();
    assert_eq!(read.kind, Kind::Image);
    let Content::Blob {
        hash,
        mime,
        width,
        height,
        ..
    } = &read.content
    else {
        panic!("not a blob: {:?}", read.content);
    };
    assert_eq!(hash.0, "abc123");
    assert_eq!(mime, "image/png");
    assert_eq!((*width, *height), (Some(800), Some(600)));
}

/// The slash command converts the line it was typed on rather than inserting a
/// new node: `set_image` keeps the identity and the position, swaps the kind to
/// `Image`, and replaces the text with a blob.
#[test]
fn set_image_converts_a_text_node_in_place() {
    let mut ws = ws();
    let para = ws
        .insert(Parent::Root, Node::paragraph("a line").unwrap())
        .unwrap();

    ws.set_image(
        para,
        BlobHash("deadbeef".into()),
        "image/jpeg",
        Some(100),
        Some(100),
    )
    .unwrap();

    let read = ws.read(para).unwrap();
    assert_eq!(read.id, para, "the node keeps its identity");
    assert_eq!(read.kind, Kind::Image);
    assert_eq!(read.content.as_text(), None, "the text is gone");
    let Content::Blob { hash, mime, .. } = &read.content else {
        panic!("not a blob");
    };
    assert_eq!(hash.0, "deadbeef");
    assert_eq!(mime, "image/jpeg");
}

/// Dimensions are optional — a blob whose decoder could not find a size still
/// stores, and round-trips with `None`.
#[test]
fn an_image_without_known_dimensions_round_trips() {
    let mut ws = ws();
    let id = ws
        .insert(
            Parent::Root,
            Node::image(BlobHash("nohwh".into()), "image/png", None, None).unwrap(),
        )
        .unwrap();
    let read = ws.read(id).unwrap();
    let Content::Blob { width, height, .. } = &read.content else {
        panic!("not a blob");
    };
    assert_eq!((*width, *height), (None, None));
}

/// Converting carries no leftover text: after `set_image`, the node has one
/// content shape, not a blob *and* a stale text container.
#[test]
fn converting_drops_the_text_container() {
    let mut ws = ws();
    let para = ws
        .insert(Parent::Root, Node::paragraph("discard me").unwrap())
        .unwrap();
    ws.set_image(para, BlobHash("h".into()), "image/png", Some(1), Some(1))
        .unwrap();
    ws.set_image(para, BlobHash("h2".into()), "image/png", Some(2), Some(2))
        .unwrap();

    let read = ws.read(para).unwrap();
    let Content::Blob { hash, .. } = &read.content else {
        panic!("not a blob");
    };
    assert_eq!(
        hash.0, "h2",
        "a second convert overwrites the first, not appends"
    );
}

/// An image lives *inside* a document rather than at the top of the tree, and
/// the sweep that decodes blobs has to descend to find it. This is the bug that
/// made images decode on insert yet stay grey after a restart: the decode pass
/// walked one level.
///
/// It used to prove something sharper — that `walk` could not reach the image
/// because the document was in the library realm and `walk` only ever saw the
/// outline. There is one tree now, so the two walks agree; what is left to
/// assert is that both of them go *down*.
#[test]
fn the_walks_reach_images_nested_inside_documents() {
    let mut ws = ws();
    let doc = ws
        .insert(Parent::Root, Node::document("picture doc").unwrap())
        .unwrap();
    let img = ws
        .insert(
            Parent::Node(doc),
            Node::image(BlobHash("h".into()), "image/png", Some(10), Some(10)).unwrap(),
        )
        .unwrap();

    for (name, walked) in [
        ("walk", ws.walk().unwrap()),
        ("walk_all", ws.walk_all().unwrap()),
    ] {
        let ids: Vec<NodeId> = walked.into_iter().map(|(id, _)| id).collect();
        assert!(ids.contains(&doc), "{name} reaches the document");
        assert!(ids.contains(&img), "{name} reaches the image inside it");
    }
}

/// Resizing an image stores a display size in its meta, which reads back and
/// can be reset to the natural size with `None`.
#[test]
fn image_size_round_trips_and_resets() {
    let mut ws = ws();
    let id = ws
        .insert(
            Parent::Root,
            Node::image(BlobHash("h".into()), "image/png", Some(800), Some(600)).unwrap(),
        )
        .unwrap();

    assert_eq!(ws.read(id).unwrap().meta.image_size, None);
    ws.set_image_size(id, Some((400, 150))).unwrap();
    assert_eq!(ws.read(id).unwrap().meta.image_size, Some((400, 150)));
    ws.set_image_size(id, None).unwrap();
    assert_eq!(ws.read(id).unwrap().meta.image_size, None, "None resets it");
}

/// Renaming an image changes only its synced display metadata. Its blob remains
/// the same picture, and removing the name restores the view-level fallback.
#[test]
fn image_name_round_trips_without_changing_the_blob() {
    let mut ws = ws();
    let id = ws
        .insert(
            Parent::Root,
            Node::image(BlobHash("photo".into()), "image/png", Some(80), Some(60)).unwrap(),
        )
        .unwrap();

    ws.set_name(id, Some("Architecture diagram")).unwrap();
    let named = ws.read(id).unwrap();
    assert_eq!(named.meta.name.as_deref(), Some("Architecture diagram"));
    assert!(matches!(
        named.content,
        Content::Blob { ref hash, .. } if hash.0 == "photo"
    ));

    ws.set_name(id, None).unwrap();
    assert_eq!(ws.read(id).unwrap().meta.name, None);
}

/// `blob_hashes` finds every referenced picture, at whatever depth.
///
/// This is the query the shell runs whenever the document changes, to decide
/// which blobs still need decoding. It used to be spelled as a `walk_all` with a
/// full `read` per node, which materialised every paragraph's text into a
/// `String` to look at a field paragraphs do not have.
///
/// **It does not go through the walks at all**, and that is deliberate: it reads
/// the tree's nodes directly so a binned image's bytes stay decodable. A restore
/// that handed back a broken picture would be worse than the bin.
#[test]
fn blob_hashes_finds_pictures_at_every_depth() {
    let mut ws = ws();

    let top_level = Node::image(BlobHash("at-the-top".into()), "image/png", None, None).unwrap();
    ws.insert(Parent::Root, top_level).unwrap();

    let doc = ws
        .insert(Parent::Root, Node::document("Notes").unwrap())
        .unwrap();
    let nested = Node::image(BlobHash("in-a-document".into()), "image/png", None, None).unwrap();
    ws.insert(Parent::Node(doc), nested).unwrap();

    // Plenty of text around them, none of which should contribute anything.
    for i in 0..20 {
        ws.insert(
            Parent::Node(doc),
            Node::paragraph(&format!("line {i}")).unwrap(),
        )
        .unwrap();
    }

    let mut found = ws.blob_hashes().unwrap();
    found.sort();
    assert_eq!(found, ["at-the-top", "in-a-document"]);
}

/// A workspace with no pictures answers with nothing rather than with a row per
/// node. Cheap to assert, and it is the case that runs on every change in the
/// overwhelming majority of documents.
#[test]
fn blob_hashes_is_empty_when_there_are_no_pictures() {
    let mut ws = ws();
    for i in 0..50 {
        ws.insert(Parent::Root, Node::task(&format!("task {i}")).unwrap())
            .unwrap();
    }
    assert!(ws.blob_hashes().unwrap().is_empty());
}

/// A deleted image stops being referenced, so its bytes are no longer something
/// the shell tries to decode.
#[test]
fn a_deleted_image_drops_out_of_blob_hashes() {
    let mut ws = ws();
    let id = ws
        .insert(
            Parent::Root,
            Node::image(BlobHash("gone".into()), "image/png", None, None).unwrap(),
        )
        .unwrap();
    assert_eq!(ws.blob_hashes().unwrap(), ["gone"]);

    ws.delete(id).unwrap();
    assert!(ws.blob_hashes().unwrap().is_empty());
}

#[test]
fn duplicate_hashes_and_subtree_deletion_update_the_locator_cache() {
    let mut ws = ws();
    let doc = ws
        .insert(Parent::Root, Node::document("Pictures").unwrap())
        .unwrap();
    for _ in 0..2 {
        ws.insert(
            Parent::Node(doc),
            Node::image(BlobHash("same".into()), "image/png", None, None).unwrap(),
        )
        .unwrap();
    }
    assert_eq!(ws.blob_hashes().unwrap(), ["same", "same"]);

    ws.delete(doc).unwrap();
    assert!(
        ws.blob_hashes().unwrap().is_empty(),
        "deleting a container must evict images in its whole subtree"
    );
}

#[test]
fn conversion_undo_and_redo_reindex_blob_references() {
    let mut ws = ws();
    let para = ws
        .insert(Parent::Root, Node::paragraph("before").unwrap())
        .unwrap();
    ws.enable_undo();
    ws.set_image(
        para,
        BlobHash("converted".into()),
        "image/png",
        Some(10),
        Some(10),
    )
    .unwrap();
    ws.checkpoint();
    assert_eq!(ws.blob_hashes().unwrap(), ["converted"]);

    assert!(ws.undo().unwrap());
    assert!(ws.blob_hashes().unwrap().is_empty());
    assert!(ws.redo().unwrap());
    assert_eq!(ws.blob_hashes().unwrap(), ["converted"]);
}

#[test]
fn reload_and_remote_import_rebuild_blob_references() {
    let mut original = ws();
    original
        .insert(
            Parent::Root,
            Node::image(BlobHash("synced".into()), "image/png", None, None).unwrap(),
        )
        .unwrap();

    let loaded = WorkspaceDoc::from_snapshot(&original.export_snapshot().unwrap()).unwrap();
    assert_eq!(loaded.blob_hashes().unwrap(), ["synced"]);

    let mut remote = ws();
    remote
        .import(&original.export_all_updates().unwrap())
        .unwrap();
    assert_eq!(remote.blob_hashes().unwrap(), ["synced"]);
}

/// **`blob_hashes` must not cost a copy of the document.**
///
/// The shell asks this question every time the document changes, and once asked
/// it on every event-loop wake — which, with the caret blinking, is forever. Its
/// old spelling was `walk_all` plus a full `read` per node, and `read`
/// materialises each node's text into a `String` to look at a field that a
/// paragraph does not have. On a real workspace that was 26% of a CPU core,
/// permanently, for an app sitting idle.
///
/// The ratio is the assertion, not the microseconds: what must not come back is
/// the *shape*, where finding the pictures means reading the prose.
#[test]
fn finding_the_pictures_does_not_read_the_prose() {
    use std::time::Instant;

    let mut ws = ws();
    let doc = ws
        .insert(Parent::Root, Node::document("Big").unwrap())
        .unwrap();
    // Long enough to shape and to carry a few clusters — a paragraph that costs
    // something to materialise, rather than one that flatters.
    let line = "A paragraph with a reasonable amount of text on it, as a real one has";
    for _ in 0..2000 {
        ws.insert(Parent::Node(doc), Node::paragraph(line).unwrap())
            .unwrap();
    }
    ws.insert(
        Parent::Node(doc),
        Node::image(BlobHash("the-one-picture".into()), "image/png", None, None).unwrap(),
    )
    .unwrap();

    let at = Instant::now();
    let cheap = ws.blob_hashes().unwrap();
    let cheap_took = at.elapsed();

    // The spelling this replaced, kept here only to be compared against.
    let at = Instant::now();
    let dear: Vec<String> = ws
        .walk_all()
        .unwrap()
        .into_iter()
        .filter_map(|(id, _)| ws.read(id).ok())
        .filter_map(|n| match n.content {
            Content::Blob { hash, .. } => Some(hash.0),
            _ => None,
        })
        .collect();
    let dear_took = at.elapsed();

    assert_eq!(
        cheap, dear,
        "the cheap scan must find exactly the same blobs"
    );
    println!("blob_hashes {cheap_took:?}   walk_all + read {dear_took:?}");
    assert!(
        cheap_took * 4 < dear_took,
        "blob_hashes ({cheap_took:?}) should be far cheaper than walking and \
         reading every node ({dear_took:?}) — it is reading whole nodes again"
    );
}
