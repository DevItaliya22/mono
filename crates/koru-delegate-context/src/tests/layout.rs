// SPDX-License-Identifier: MPL-2.0

//! Every materialized name is an id or a hash, and every one is ASCII.

use std::path::Path;

use koru_delegate_core::{BlobHash, BlobKind, BlockNodeId, DocumentId, DocumentVersionId, RunId};

use crate::layout;

fn ids() -> (DocumentId, DocumentVersionId, BlockNodeId) {
    (
        DocumentId::new(),
        DocumentVersionId::new(),
        BlockNodeId::mint(),
    )
}

#[test]
fn a_run_directory_is_the_run_id_under_the_context_root() {
    let run = RunId::mint();
    let dir = layout::run_dir(Path::new("work"), run);
    assert_eq!(
        dir,
        Path::new("work")
            .join(".koru")
            .join("context")
            .join(run.to_storage())
    );
}

#[test]
fn document_and_block_files_are_named_by_their_ids_only() {
    let (document, version, node) = ids();
    let file = layout::document_file(document, version);
    assert_eq!(
        file,
        format!(
            "documents/{}.{}.md",
            document.to_storage(),
            version.to_storage()
        )
    );
    let block = layout::block_file(document, version, node);
    assert_eq!(
        block,
        format!(
            "documents/{}.{}.{}.md",
            document.to_storage(),
            version.to_storage(),
            node.to_storage()
        )
    );
    for name in [&file, &block] {
        assert!(name.is_ascii(), "{name}");
    }
}

#[test]
fn a_blob_file_is_its_hash_with_an_extension_from_its_media_type() {
    let hash = BlobHash::from_digest("b3", &"ab".repeat(32)).unwrap();
    assert_eq!(
        layout::blob_file(&hash, "image/png", BlobKind::Image),
        format!("blobs/b3-{}.png", "ab".repeat(32))
    );
    assert_eq!(
        layout::blob_file(&hash, "application/octet-stream", BlobKind::Pdf),
        format!("blobs/b3-{}.pdf", "ab".repeat(32))
    );
    assert_eq!(
        layout::blob_file(&hash, "x/unknown", BlobKind::File),
        format!("blobs/b3-{}.bin", "ab".repeat(32))
    );
    assert_eq!(layout::extension("IMAGE/JPEG", BlobKind::File), "jpg");
}

#[test]
fn the_assets_directory_sits_beside_its_markdown_and_links_by_name() {
    let (document, version, node) = ids();
    let dir = layout::assets_dir(document, version, None);
    assert!(dir.starts_with("documents/"));
    assert!(dir.ends_with(".assets"));
    assert_eq!(layout::assets_rel(&dir), &dir["documents/".len()..]);
    let block_dir = layout::assets_dir(document, version, Some(node));
    assert!(block_dir.contains(&node.to_storage()));
}
