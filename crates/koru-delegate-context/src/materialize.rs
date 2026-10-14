// SPDX-License-Identifier: MPL-2.0

//! Materialize — the pinned bytes laid out for a provider (§7.2 step 3).
//!
//! Into `<run-workspace>/.koru/context/<RunId>/`, per run. Documents and
//! blocks are rendered to Markdown from the **captured version's** bytes;
//! blobs are copied by hash; `manifest.json` names every file by pin. The
//! layout is the same under both delivery modes — a provider that takes
//! attachments natively is handed the same paths a provider that reads
//! them out of a quoted prompt is — and the mode is recorded in the
//! manifest and on every file so the runtime hands them over the right
//! way. A provider that supports neither cannot receive context, and that
//! fails before it starts.
//!
//! **Only then may the provider start (§7.2 step 4)** — the runtime's
//! ordering, held by the fact that this function returns the layout the
//! launch needs and nothing else does.
//!
//! Re-materializing the same run (a resume) rebuilds its directory from
//! its pins; another run's directory beside it is never touched.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use koru_delegate_core::{RunContextItem, RunId};
use koru_export_markdown::Rendered;
use koru_session_protocol::DeliveryMode;
use koru_store_core::model::Blob;

use crate::{
    CapturedContext, ContextPin, ContextStore, Fault, Manifest, ManifestFile, StoreContext, layout,
    render,
};

/// Where materialization failed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MaterializeStage {
    /// The run directory could not be prepared, or the delivery mode
    /// cannot receive files.
    Layout,
    /// The pinned bytes could not be read back from the store.
    Read,
    /// The captured version could not be rendered.
    Render,
    /// A file could not be written.
    Write,
}

impl MaterializeStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Layout => "layout",
            Self::Read => "read",
            Self::Render => "render",
            Self::Write => "write",
        }
    }
}

/// Materialization did not complete. `item` is the pin that failed, or
/// `None` when the run directory itself could not be prepared.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MaterializeFailure {
    pub item: Option<usize>,
    pub stage: MaterializeStage,
    pub fault: Fault,
}

impl fmt::Display for MaterializeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.item {
            Some(item) => write!(
                f,
                "context item {item} could not be materialized at {}: {}",
                self.stage.as_str(),
                self.fault
            ),
            None => write!(
                f,
                "context could not be materialized at {}: {}",
                self.stage.as_str(),
                self.fault
            ),
        }
    }
}

impl std::error::Error for MaterializeFailure {}

/// One file a provider is given. `path` is absolute and `relative` is the
/// manifest's spelling; the `Debug` prints neither, because the run
/// workspace is a person's folder.
#[derive(Clone, PartialEq, Eq)]
pub struct MaterializedFile {
    /// The pin's index in the captured context.
    pub item: usize,
    pub path: PathBuf,
    pub relative: String,
    pub media_type: String,
    pub delivery: DeliveryMode,
}

impl fmt::Debug for MaterializedFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MaterializedFile")
            .field("item", &self.item)
            .field("media_type", &self.media_type)
            .field("delivery", &self.delivery)
            .finish_non_exhaustive()
    }
}

/// A run's context on disk. `root` is `<run-workspace>/.koru/context/<RunId>`.
#[derive(Clone, PartialEq, Eq)]
pub struct MaterializedContext {
    pub run: RunId,
    pub root: PathBuf,
    pub files: Vec<MaterializedFile>,
}

impl fmt::Debug for MaterializedContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MaterializedContext")
            .field("run", &self.run)
            .field("files", &self.files.len())
            .finish_non_exhaustive()
    }
}

impl<S: ContextStore> StoreContext<'_, S> {
    /// Lay the captured context out under `run_workspace` for a provider
    /// that receives attachments in `delivery` mode.
    pub fn materialize(
        &mut self,
        run: RunId,
        captured: &CapturedContext,
        delivery: DeliveryMode,
        run_workspace: &Path,
    ) -> Result<MaterializedContext, MaterializeFailure> {
        let layout_failure = |fault| MaterializeFailure {
            item: None,
            stage: MaterializeStage::Layout,
            fault,
        };
        if delivery == DeliveryMode::Unsupported {
            return Err(layout_failure(Fault::DeliveryUnsupported));
        }
        let root = layout::run_dir(run_workspace, run);
        // This run's own directory is rebuilt from its pins; a sibling
        // run's is not looked at.
        if root.exists() {
            fs::remove_dir_all(&root).map_err(|e| layout_failure(Fault::from_io(&e)))?;
        }
        fs::create_dir_all(&root).map_err(|e| layout_failure(Fault::from_io(&e)))?;

        let mut files = Vec::with_capacity(captured.pins.len());
        let mut entries = Vec::with_capacity(captured.pins.len());
        for (item, pin) in captured.pins.iter().enumerate() {
            let (file, entry) = self.materialize_one(item, pin, delivery, &root)?;
            files.push(file);
            entries.push(entry);
        }

        let manifest = Manifest {
            version: crate::MANIFEST_VERSION,
            run,
            delivery,
            files: entries,
        };
        let json = serde_json::to_vec_pretty(&manifest).map_err(|_| MaterializeFailure {
            item: None,
            stage: MaterializeStage::Write,
            fault: Fault::StoreFailed,
        })?;
        write_atomic(&root.join(layout::MANIFEST_FILE), &json).map_err(|e| MaterializeFailure {
            item: None,
            stage: MaterializeStage::Write,
            fault: Fault::from_io(&e),
        })?;

        Ok(MaterializedContext { run, root, files })
    }

    /// A Document or Block pin, read back from the store and rendered —
    /// every store read its materialization makes, and no file. A blob pin
    /// is copied rather than rendered and never reaches here.
    ///
    /// Split from the write so what an item *reads* is testable without a
    /// run directory: a Block reads its own version and the images of its own
    /// subtree (see [`render::block_blobs`]), and nothing another item holds.
    pub(crate) fn render_pinned(
        &mut self,
        pin: &ContextPin,
        assets_dir: &str,
    ) -> Result<Rendered, (MaterializeStage, Fault)> {
        let (workspace, document, version, node) = match &pin.item {
            RunContextItem::Document {
                workspace,
                document,
                version,
            } => (*workspace, *document, *version, None),
            RunContextItem::Block {
                workspace,
                document,
                version,
                node,
            } => (*workspace, *document, *version, Some(*node)),
            RunContextItem::Blob { .. } => {
                return Err((MaterializeStage::Render, Fault::RenderFailed));
            }
        };
        let stored = self
            .store
            .version(workspace, version)
            .map_err(|f| (MaterializeStage::Read, f))?
            .ok_or((MaterializeStage::Read, Fault::VersionUnavailable))?;
        if stored.document != document || stored.checksum != pin.checksum {
            return Err((MaterializeStage::Read, Fault::VersionUnavailable));
        }
        let doc = render::open(&stored.snapshot).map_err(|f| (MaterializeStage::Render, f))?;
        let wanted = match node {
            Some(node) => render::block_blobs(&doc, node),
            None => render::document_blobs(&doc),
        }
        .map_err(|f| (MaterializeStage::Render, f))?;
        let blobs = render::prefetch(self.store, workspace, wanted)
            .map_err(|f| (MaterializeStage::Read, f))?;
        match node {
            Some(node) => render::block(&doc, node, &blobs, layout::assets_rel(assets_dir)),
            None => render::document(&doc, &blobs, layout::assets_rel(assets_dir)),
        }
        .map_err(|f| (MaterializeStage::Render, f))
    }

    fn materialize_one(
        &mut self,
        item: usize,
        pin: &ContextPin,
        delivery: DeliveryMode,
        root: &Path,
    ) -> Result<(MaterializedFile, ManifestFile), MaterializeFailure> {
        let fail = |stage, fault| MaterializeFailure {
            item: Some(item),
            stage,
            fault,
        };
        match &pin.item {
            RunContextItem::Document {
                workspace,
                document,
                version,
            }
            | RunContextItem::Block {
                workspace,
                document,
                version,
                ..
            } => {
                let node = match &pin.item {
                    RunContextItem::Block { node, .. } => Some(*node),
                    _ => None,
                };
                let assets_dir = layout::assets_dir(*document, *version, node);
                let rendered = self
                    .render_pinned(pin, &assets_dir)
                    .map_err(|(stage, fault)| fail(stage, fault))?;

                let relative = match node {
                    Some(node) => layout::block_file(*document, *version, node),
                    None => layout::document_file(*document, *version),
                };
                let path = root.join(&relative);
                write_atomic(&path, rendered.markdown.as_bytes())
                    .map_err(|e| fail(MaterializeStage::Write, Fault::from_io(&e)))?;
                for asset in &rendered.assets {
                    let asset_path = root.join(&assets_dir).join(&asset.file_name);
                    write_atomic(&asset_path, &asset.bytes)
                        .map_err(|e| fail(MaterializeStage::Write, Fault::from_io(&e)))?;
                }

                let file = MaterializedFile {
                    item,
                    path,
                    relative: relative.clone(),
                    media_type: layout::MARKDOWN_MEDIA_TYPE.to_owned(),
                    delivery,
                };
                let entry = match node {
                    Some(node) => ManifestFile::Block {
                        item,
                        workspace: *workspace,
                        document: *document,
                        version: *version,
                        node,
                        path: relative,
                        checksum: pin.checksum.clone(),
                        media_type: layout::MARKDOWN_MEDIA_TYPE.to_owned(),
                    },
                    None => ManifestFile::Document {
                        item,
                        workspace: *workspace,
                        document: *document,
                        version: *version,
                        path: relative,
                        checksum: pin.checksum.clone(),
                        media_type: layout::MARKDOWN_MEDIA_TYPE.to_owned(),
                    },
                };
                Ok((file, entry))
            }
            RunContextItem::Blob {
                workspace,
                blob,
                blob_kind,
            } => {
                let record: Blob = self
                    .store
                    .blob(*workspace, blob)
                    .map_err(|f| fail(MaterializeStage::Read, f))?
                    .ok_or(fail(MaterializeStage::Read, Fault::BlobUnavailable))?;
                let bytes = self
                    .store
                    .blob_bytes(*workspace, blob)
                    .map_err(|f| fail(MaterializeStage::Read, f))?;
                let relative = layout::blob_file(blob, &record.mime, *blob_kind);
                let path = root.join(&relative);
                write_atomic(&path, &bytes)
                    .map_err(|e| fail(MaterializeStage::Write, Fault::from_io(&e)))?;
                let file = MaterializedFile {
                    item,
                    path,
                    relative: relative.clone(),
                    media_type: record.mime.clone(),
                    delivery,
                };
                let entry = ManifestFile::Blob {
                    item,
                    workspace: *workspace,
                    hash: blob.clone(),
                    blob_kind: *blob_kind,
                    path: relative,
                    size: bytes.len() as u64,
                    media_type: record.mime,
                    filename: record.filename,
                };
                Ok((file, entry))
            }
        }
    }
}

/// Write to a sibling and rename, so a provider never opens a half-written
/// file. The directory is created on demand.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let part = path.with_extension(match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{ext}.part"),
        None => "part".to_owned(),
    });
    fs::write(&part, bytes)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(&part, path)
}
