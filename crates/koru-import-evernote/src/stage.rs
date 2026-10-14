// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};

use koru_core::{BlobHash, NodeId, SystemDoc, TagId, WorkspaceDoc};
use koru_import::{BlobSink, ImportError, ImportProgress, ImportWarning, ImportedDocument};

use crate::resources::SpoolSet;

struct ResourceBinding {
    document: koru_store_core::ids::DocumentId,
    node: NodeId,
    spool: usize,
    mime: String,
    filename: Option<String>,
}

/// All effects accumulated while one ENEX source is being parsed.
///
/// Nothing outside the stage changes until it is consumed by [`commit`]. This
/// is the source-level transaction boundary: resource spools, documents and
/// tag mutations remain invisible until the complete source passes its
/// security checks.
///
/// [`commit`]: SourceStage::commit
pub struct SourceStage {
    source: PathBuf,
    documents: Vec<ImportedDocument>,
    warnings: Vec<ImportWarning>,
    system: SystemDoc,
    had_system: bool,
    tags_touched: bool,
    spools: SpoolSet,
    resource_bindings: Vec<ResourceBinding>,
}

impl SourceStage {
    /// Begin an isolated source stage from the current system snapshot, or
    /// create an in-memory system document when the workspace has none.
    ///
    /// This constructor performs no Store operation and registers no manifest
    /// row. Dropping the stage therefore drops every staged effect.
    pub fn new(source: PathBuf, system_snapshot: Option<&[u8]>) -> Result<Self, ImportError> {
        let system = SystemDoc::open_or_create(system_snapshot)
            .map_err(|error| ImportError::new(error.to_string()))?;
        Ok(Self {
            source,
            documents: Vec::new(),
            warnings: Vec::new(),
            system,
            had_system: system_snapshot.is_some(),
            tags_touched: false,
            spools: SpoolSet::default(),
            resource_bindings: Vec::new(),
        })
    }

    #[cfg(test)]
    pub(crate) fn new_with_spools(
        source: PathBuf,
        system_snapshot: Option<&[u8]>,
        spools: SpoolSet,
    ) -> Result<Self, ImportError> {
        let mut stage = Self::new(source, system_snapshot)?;
        stage.spools = spools;
        Ok(stage)
    }

    pub(crate) fn spools_mut(&mut self) -> &mut SpoolSet {
        &mut self.spools
    }

    pub(crate) fn bind_resource(
        &mut self,
        document: koru_store_core::ids::DocumentId,
        node: NodeId,
        spool: usize,
        mime: String,
        filename: Option<String>,
    ) {
        self.resource_bindings.push(ResourceBinding {
            document,
            node,
            spool,
            mime,
            filename,
        });
    }

    pub fn source(&self) -> &Path {
        &self.source
    }

    pub fn documents(&self) -> &[ImportedDocument] {
        &self.documents
    }

    pub fn warnings(&self) -> &[ImportWarning] {
        &self.warnings
    }

    pub fn create_tag(&mut self, name: &str, now_ms: i64) -> Result<TagId, ImportError> {
        let tag = self
            .system
            .create_tag(name, None, now_ms)
            .map_err(|error| ImportError::new(error.to_string()))?;
        self.tags_touched = true;
        Ok(tag)
    }

    pub fn push_document(&mut self, document: ImportedDocument) {
        self.documents.push(document);
    }

    pub fn push_warning(&mut self, warning: ImportWarning) {
        self.warnings.push(warning);
    }

    /// Seal the accepted source and hand its still-unpublished effects to the
    /// importer commit path. The caller remains responsible for publishing the
    /// system update and appending documents to the job product in that order.
    pub fn commit(self) -> Result<CommittedSource, ImportError> {
        self.commit_inner(None, None)
    }

    /// Upload and patch every referenced resource before any staged snapshot
    /// crosses the importer boundary. Factories reopen the owned spool at byte
    /// zero, so the Store client's one retry cannot resume in the middle.
    pub fn commit_with_blobs(self, blobs: &dyn BlobSink) -> Result<CommittedSource, ImportError> {
        self.commit_inner(Some(blobs), None)
    }

    pub(crate) fn commit_with_blobs_progress(
        self,
        blobs: &dyn BlobSink,
        progress: Option<&dyn ImportProgress>,
    ) -> Result<CommittedSource, ImportError> {
        self.commit_inner(Some(blobs), progress)
    }

    fn commit_inner(
        mut self,
        blobs: Option<&dyn BlobSink>,
        progress: Option<&dyn ImportProgress>,
    ) -> Result<CommittedSource, ImportError> {
        if !self.resource_bindings.is_empty() {
            let blobs =
                blobs.ok_or_else(|| ImportError::new("resource stage has no blob destination"))?;
            let mut stored = std::collections::BTreeMap::new();
            for binding in &self.resource_bindings {
                if stored.contains_key(&binding.spool) {
                    continue;
                }
                if progress.is_some_and(ImportProgress::cancelled) {
                    return Err(ImportError::cancelled());
                }
                let factory = self.spools.reader_factory(binding.spool);
                let result =
                    blobs.put_from(factory.as_ref(), &binding.mime, binding.filename.as_deref())?;
                crate::debug::resource(&result.hash);
                // A cancellation cannot interrupt an in-flight Store call. It
                // is observed immediately after that call returns, before a
                // second upload or any document/tag effect can escape.
                if progress.is_some_and(ImportProgress::cancelled) {
                    return Err(ImportError::cancelled());
                }
                stored.insert(binding.spool, result);
            }
            for document in &mut self.documents {
                let bindings = self
                    .resource_bindings
                    .iter()
                    .filter(|binding| binding.document == document.id)
                    .collect::<Vec<_>>();
                if bindings.is_empty() {
                    continue;
                }
                let mut workspace = WorkspaceDoc::from_snapshot(&document.snapshot)
                    .map_err(|error| ImportError::new(error.to_string()))?;
                for binding in bindings {
                    let hash = stored
                        .get(&binding.spool)
                        .expect("each bound spool was uploaded")
                        .hash
                        .clone();
                    workspace
                        .rewrite_blob_hash(binding.node, BlobHash(hash))
                        .map_err(|error| ImportError::new(error.to_string()))?;
                }
                document.snapshot = workspace
                    .export_snapshot()
                    .map_err(|error| ImportError::new(error.to_string()))?;
            }
        }
        let system_snapshot = self
            .tags_touched
            .then(|| self.system.export_snapshot())
            .transpose()
            .map_err(|error| ImportError::new(error.to_string()))?;
        Ok(CommittedSource {
            source: self.source,
            documents: self.documents,
            warnings: self.warnings,
            system_snapshot,
            register_system: self.tags_touched && !self.had_system,
        })
    }
}

/// Accepted effects from one source, still separate from Store and host state.
/// Later importer phases consume this value at the shell-owned commit seam.
pub struct CommittedSource {
    pub source: PathBuf,
    pub documents: Vec<ImportedDocument>,
    pub warnings: Vec<ImportWarning>,
    pub system_snapshot: Option<Vec<u8>>,
    pub register_system: bool,
}

#[cfg(test)]
mod tests;
