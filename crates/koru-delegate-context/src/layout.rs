// SPDX-License-Identifier: MPL-2.0

//! Where materialized context lives, and what each file is called.
//!
//! **Every name is an id or a hash.** A run's directory is its `RunId`; a
//! document is `<document>.<version>.md`; a block adds the node; a blob is
//! its hash with an extension chosen from its media type. The person's
//! filename is arbitrary Unicode and never a path component
//! (PRINCIPLES, *a filename is metadata; identity is content*) — it rides
//! `manifest.json` as metadata instead. All names are ASCII, so a
//! `QuotedPath` provider can be handed any of them under any shell
//! quoting, and no platform branch is needed to spell them.

use std::path::{Path, PathBuf};

use koru_delegate_core::{BlobHash, BlobKind, BlockNodeId, DocumentId, DocumentVersionId, RunId};

/// The directory under a run workspace that holds every run's context.
pub const CONTEXT_DIR: &str = ".koru/context";
/// The manifest, at the root of one run's context directory.
pub const MANIFEST_FILE: &str = "manifest.json";
/// Rendered documents and blocks, and their image assets.
pub const DOCUMENTS_DIR: &str = "documents";
/// Copied blobs.
pub const BLOBS_DIR: &str = "blobs";

/// `<run-workspace>/.koru/context`.
pub fn context_root(run_workspace: &Path) -> PathBuf {
    run_workspace.join(CONTEXT_DIR)
}

/// `<run-workspace>/.koru/context/<RunId>` — one directory per run, so two
/// runs on one folder never see each other's copies.
pub fn run_dir(run_workspace: &Path, run: RunId) -> PathBuf {
    context_root(run_workspace).join(run.to_storage())
}

/// A document's rendered file, relative to the run directory.
pub fn document_file(document: DocumentId, version: DocumentVersionId) -> String {
    format!(
        "{DOCUMENTS_DIR}/{}.{}.md",
        document.to_storage(),
        version.to_storage()
    )
}

/// A block's rendered file, relative to the run directory.
pub fn block_file(document: DocumentId, version: DocumentVersionId, node: BlockNodeId) -> String {
    format!(
        "{DOCUMENTS_DIR}/{}.{}.{}.md",
        document.to_storage(),
        version.to_storage(),
        node.to_storage()
    )
}

/// The directory a rendered document's inline images are written to,
/// relative to the run directory — and, as the exporter needs it, relative
/// to the Markdown file itself.
pub fn assets_dir(
    document: DocumentId,
    version: DocumentVersionId,
    node: Option<BlockNodeId>,
) -> String {
    match node {
        Some(node) => format!(
            "{DOCUMENTS_DIR}/{}.{}.{}.assets",
            document.to_storage(),
            version.to_storage(),
            node.to_storage()
        ),
        None => format!(
            "{DOCUMENTS_DIR}/{}.{}.assets",
            document.to_storage(),
            version.to_storage()
        ),
    }
}

/// The assets directory as a link target *from the Markdown file*, which
/// sits in the same directory.
pub fn assets_rel(assets_dir: &str) -> &str {
    assets_dir
        .rsplit_once('/')
        .map_or(assets_dir, |(_, name)| name)
}

/// A blob's copied file, relative to the run directory: the hash, with an
/// extension a provider can recognise the bytes by.
pub fn blob_file(hash: &BlobHash, media_type: &str, kind: BlobKind) -> String {
    format!(
        "{BLOBS_DIR}/{}.{}",
        hash.file_name(),
        extension(media_type, kind)
    )
}

/// The extension for a media type, falling back to the item's declared
/// kind. A closed table: an unknown type is `bin`, never a guess parsed out
/// of the person's filename.
pub fn extension(media_type: &str, kind: BlobKind) -> &'static str {
    match media_type.trim().to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "image/bmp" => "bmp",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        "text/markdown" => "md",
        "text/csv" => "csv",
        "application/json" => "json",
        _ => match kind {
            BlobKind::Pdf => "pdf",
            BlobKind::Image | BlobKind::File => "bin",
        },
    }
}

/// The media type the Markdown a document renders to is delivered as.
pub const MARKDOWN_MEDIA_TYPE: &str = "text/markdown";
