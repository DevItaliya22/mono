// SPDX-License-Identifier: MPL-2.0

//! Legacy Koru workspace importer.
//!
//! **An import is a faithful restore, not a thing a person just did**
//! ([ADR-286], SPEC §5), and this is the one importer whose source can already
//! hold task state — so it is the one that could be caught rewriting it.
//! `plant_import` carries each node's whole `Content` and `NodeMeta` across, so
//! the marker, the canonical lifecycle, the due and the tick arrive exactly as
//! they were recorded. A completed task standing over an unchecked child is the
//! residue SPEC §5 allows, and it comes back completed: no lifecycle command
//! runs, nothing classifies, nothing reopens and no activity is owed. The
//! exemption is not a flag this crate sets; it is that the classification entry
//! point and the six canonical commands are not on this path at all, which
//! `a_completed_task_over_unfinished_work_is_restored_exactly_as_recorded`
//! reads off the source itself. Anything that ever *does* need to write task
//! state here goes through `koru_core::tasks::faithful_import` and nowhere
//! else.
//!
//! [ADR-286]: ../../../docs/decisions/ADR-276-300.md#adr-286-taskboundary--six-canonical-commands-own-task-state-and-every-host-is-a-thin-wrapper-around-them

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use koru_core::ids::{DocumentId, NodeId};
use koru_core::node::{AttrValue, Content, Kind, Node};
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_import::{
    IdentityMode, ImportContext, ImportError, ImportOptions, ImportPreview, ImportProduct,
    ImportProgress, ImportSource, ImportStage, Importer, ImporterDescriptor, PreparedImport,
    PreviewNode, SourceRequest,
};
use koru_store_core::bundle::{Bundle, BundleDocument, BundleManifest};
use koru_store_core::ids::{DocumentId as StoreDocumentId, WorkspaceId as StoreWorkspaceId};
use koru_store_core::manifest::Manifest;
use koru_store_core::model::{Blob, BlobHash as StoreBlobHash, DocumentKind, Timestamp};

pub struct LegacyWorkspaceImporter;

pub static DESCRIPTOR: ImporterDescriptor = ImporterDescriptor {
    id: "koru-legacy-workspace",
    name: "Older Koru workspace",
    source: SourceRequest::Directory,
    // Whole-workspace restore is not a content-import menu format.
    menu_name: None,
    picker_title: "Choose an older Koru workspace",
    claims_zip: None,
    roots_import: false,
    dedups_known_files: false,
};

impl Importer for LegacyWorkspaceImporter {
    fn descriptor(&self) -> &'static ImporterDescriptor {
        &DESCRIPTOR
    }

    fn inspect(
        &self,
        source: ImportSource,
        progress: &dyn ImportProgress,
    ) -> Result<Box<dyn PreparedImport>, ImportError> {
        progress.update(ImportStage::Inspecting, 0, None);
        if progress.cancelled() {
            return Err(ImportError::cancelled());
        }
        let ImportSource::Directory(folder) = source else {
            return Err(ImportError::new("an older Koru workspace is a directory"));
        };
        let loaded = koru_docstore::readonly::load_workspace(&folder)
            .map_err(|e| ImportError::new(format!("could not read the old workspace: {e}")))?;
        match loaded.doc.compatibility() {
            koru_core::schema::Compatibility::Writable => {}
            other => {
                return Err(ImportError::new(format!(
                    "unsupported legacy schema: {other}"
                )));
            }
        }
        let source = loaded
            .doc
            .normalized_for_legacy_import()
            .map_err(import_failed)?;
        let prepared = PreparedLegacy::new(folder, source, loaded.warnings)?;
        Ok(Box::new(prepared))
    }
}

struct DocumentSpec {
    node: NodeId,
    id: DocumentId,
    parent: Option<DocumentId>,
    title: String,
    nodes: usize,
    trashed_at: Option<i64>,
    trashed_by: Option<String>,
}

struct PreparedLegacy {
    folder: PathBuf,
    source: WorkspaceDoc,
    specs: Vec<DocumentSpec>,
    live_loose: Vec<NodeId>,
    trashed_loose: Vec<NodeId>,
    preview: ImportPreview,
}

impl PreparedLegacy {
    fn new(
        folder: PathBuf,
        source: WorkspaceDoc,
        mut warnings: Vec<String>,
    ) -> Result<Self, ImportError> {
        let mut specs = Vec::new();
        for (entry, node) in source.explore_bridged().map_err(import_failed)? {
            specs.push(DocumentSpec {
                node,
                id: entry.id,
                parent: entry.parent,
                title: entry.title,
                nodes: count_import_nodes(&source, node)?,
                trashed_at: None,
                trashed_by: None,
            });
        }

        let trash = source.trashed().map_err(import_failed)?;
        for item in &trash {
            collect_trashed_documents(
                &source,
                item.node,
                None,
                item.trashed_at,
                item.trashed_by.clone(),
                &mut specs,
            )?;
        }

        let live_loose = source
            .children(Parent::Root)
            .map_err(import_failed)?
            .into_iter()
            .filter(|id| source.kind_of(*id) != Some(Kind::Document))
            .collect::<Vec<_>>();
        let trashed_loose = trash
            .iter()
            .filter(|item| item.kind != Kind::Document)
            .map(|item| item.node)
            .collect::<Vec<_>>();

        let blob_store = koru_docstore::BlobStore::new(folder.join("blobs"), "import-readonly");
        for hash in source.blob_hashes().map_err(import_failed)? {
            let hash = koru_core::node::BlobHash(hash);
            if !blob_store.contains(&hash) {
                warnings.push(format!("Image data is missing for {}.", hash.0));
            }
        }

        let node_count = specs.iter().map(|s| s.nodes).sum::<usize>()
            + live_loose
                .iter()
                .map(|id| count_import_nodes(&source, *id).unwrap_or(0))
                .sum::<usize>()
            + trashed_loose
                .iter()
                .map(|id| count_import_nodes(&source, *id).unwrap_or(0))
                .sum::<usize>();
        let image_count = source.blob_hashes().map_err(import_failed)?.len();
        let has_loose = !live_loose.is_empty() || !trashed_loose.is_empty();
        let mut documents = preview_tree(&specs, None, false);
        if has_loose {
            let mut children = preview_loose_nodes(&source, &live_loose, false)?;
            children.extend(preview_loose_nodes(&source, &trashed_loose, true)?);
            documents.push(PreviewNode {
                title: "Imported items".into(),
                children,
                nodes: live_loose
                    .iter()
                    .chain(&trashed_loose)
                    .map(|id| count_import_nodes(&source, *id).unwrap_or(0))
                    .sum::<usize>()
                    + 1,
                trashed: false,
            });
        }
        let preview = ImportPreview {
            native_workspace_id: source
                .id()
                .map(|id| StoreWorkspaceId::from_uuid(id.as_uuid())),
            workspace_name: source
                .name()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "Imported workspace".into()),
            documents,
            document_count: specs.len() + usize::from(has_loose),
            node_count,
            image_count,
            warnings,
            identity_modes: vec![IdentityMode::Keep, IdentityMode::Copy],
        };
        Ok(Self {
            folder,
            source,
            specs,
            live_loose,
            trashed_loose,
            preview,
        })
    }
}

impl PreparedImport for PreparedLegacy {
    fn preview(&self) -> &ImportPreview {
        &self.preview
    }

    fn build(
        self: Box<Self>,
        options: ImportOptions,
        _: &ImportContext<'_>,
        progress: &dyn ImportProgress,
    ) -> Result<ImportProduct, ImportError> {
        if !options.accept_warnings && !self.preview.warnings.is_empty() {
            return Err(ImportError::new("the import warnings were not accepted"));
        }
        build_bundle(*self, options.identity, progress).map(ImportProduct::Workspace)
    }
}

fn build_bundle(
    prepared: PreparedLegacy,
    identity: IdentityMode,
    progress: &dyn ImportProgress,
) -> Result<Bundle, ImportError> {
    let old_workspace = prepared
        .source
        .id()
        .ok_or_else(|| ImportError::new("the old workspace has no identity"))?;
    let workspace = StoreWorkspaceId::from_uuid(old_workspace.as_uuid());
    let now = now();
    let mut manifest = Manifest::create(workspace, &prepared.preview.workspace_name, now)
        .map_err(import_failed)?;
    let manifest_id = StoreDocumentId::new();
    let total = prepared.specs.len()
        + usize::from(!prepared.live_loose.is_empty() || !prepared.trashed_loose.is_empty());
    let mut snapshots = Vec::with_capacity(total + 1);
    let mut documents = Vec::with_capacity(total + 1);
    let mut blobs_by_hash: HashMap<StoreBlobHash, (Blob, Vec<u8>)> = HashMap::new();

    for (index, spec) in prepared.specs.iter().enumerate() {
        if progress.cancelled() {
            return Err(ImportError::cancelled());
        }
        progress.update(ImportStage::Converting, index, Some(total));
        let mut target = WorkspaceDoc::create(old_workspace, &spec.title).map_err(import_failed)?;
        let cutting = prepared
            .source
            .cutting_for_import(spec.node)
            .map_err(import_failed)?;
        target
            .plant_import(&cutting, Parent::Root)
            .map_err(import_failed)?;
        let copied = import_subtree_ids(&prepared.source, spec.node)?;
        rewrite_imported_nodes(
            &prepared.source,
            &mut target,
            &copied,
            &prepared.folder,
            identity,
            now,
            &mut blobs_by_hash,
        )?;
        let id = store_document(spec.id);
        let parent = spec.parent.map(store_document);
        manifest
            .add_document(id, &spec.title, koru_core::explore::KIND_DOCUMENT, parent)
            .map_err(import_failed)?;
        if let Some(at) = spec.trashed_at {
            manifest
                .trash_document(id, Timestamp::from_millis(at), spec.trashed_by.as_deref())
                .map_err(import_failed)?;
        }
        let bytes = target.export_snapshot().map_err(import_failed)?;
        documents.push(bundle_document(id, DocumentKind::Document, &bytes));
        snapshots.push((id, bytes));
    }

    if !prepared.live_loose.is_empty() || !prepared.trashed_loose.is_empty() {
        let id = StoreDocumentId::new();
        let mut target =
            WorkspaceDoc::create(old_workspace, "Imported items").map_err(import_failed)?;
        let root = target
            .insert(
                Parent::Root,
                Node::document("Imported items").map_err(import_failed)?,
            )
            .map_err(import_failed)?;
        let mut copied = Vec::new();
        for source_root in &prepared.live_loose {
            let cutting = prepared
                .source
                .cutting_for_import(*source_root)
                .map_err(import_failed)?;
            target
                .plant_import(&cutting, Parent::Node(root))
                .map_err(import_failed)?;
            copied.extend(import_subtree_ids(&prepared.source, *source_root)?);
        }
        for source_root in &prepared.trashed_loose {
            let cutting = prepared
                .source
                .cutting_for_import(*source_root)
                .map_err(import_failed)?;
            target
                .plant_import_in_trash(&cutting)
                .map_err(import_failed)?;
            copied.extend(import_subtree_ids(&prepared.source, *source_root)?);
        }
        rewrite_imported_nodes(
            &prepared.source,
            &mut target,
            &copied,
            &prepared.folder,
            identity,
            now,
            &mut blobs_by_hash,
        )?;
        manifest
            .add_document(
                id,
                "Imported items",
                koru_core::explore::KIND_DOCUMENT,
                None,
            )
            .map_err(import_failed)?;
        let bytes = target.export_snapshot().map_err(import_failed)?;
        documents.push(bundle_document(id, DocumentKind::Document, &bytes));
        snapshots.push((id, bytes));
    }

    progress.update(ImportStage::BuildingBundle, total, Some(total));
    let manifest_bytes = manifest.export_snapshot().map_err(import_failed)?;
    documents.insert(
        0,
        bundle_document(manifest_id, DocumentKind::Manifest, &manifest_bytes),
    );
    snapshots.insert(0, (manifest_id, manifest_bytes));
    let mut blob_pairs = Vec::with_capacity(blobs_by_hash.len());
    let mut blob_meta = Vec::with_capacity(blobs_by_hash.len());
    for (hash, (meta, bytes)) in blobs_by_hash {
        blob_meta.push(meta);
        blob_pairs.push((hash, bytes));
    }

    let bundle = Bundle {
        manifest: BundleManifest {
            format_version: koru_store_core::version::BUNDLE_FORMAT_VERSION,
            workspace,
            name: prepared.preview.workspace_name,
            created_at: now,
            exported_at: now,
            lineage: Vec::new(),
            documents,
            history_versions: Vec::new(),
            history_entries: Vec::new(),
            blobs: blob_meta,
        },
        snapshots,
        history_snapshots: Vec::new(),
        blobs: blob_pairs,
    };
    bundle.validate().map_err(import_failed)?;
    Ok(bundle)
}

fn rewrite_imported_nodes(
    source: &WorkspaceDoc,
    target: &mut WorkspaceDoc,
    ids: &[NodeId],
    folder: &Path,
    identity: IdentityMode,
    at: Timestamp,
    blobs: &mut HashMap<StoreBlobHash, (Blob, Vec<u8>)>,
) -> Result<(), ImportError> {
    let old_blobs = koru_docstore::BlobStore::new(folder.join("blobs"), "import-readonly");
    for id in ids.iter().copied() {
        let node = source.read(id).map_err(import_failed)?;
        if let Content::Blob { hash, mime, .. } = &node.content
            && let Ok(bytes) = old_blobs.get(hash)
        {
            let digest = blake3::hash(&bytes).to_hex().to_string();
            let new_hash = StoreBlobHash::from_digest(StoreBlobHash::BLAKE3, &digest)
                .map_err(import_failed)?;
            target
                .rewrite_blob_hash(id, koru_core::node::BlobHash(new_hash.as_str().to_owned()))
                .map_err(import_failed)?;
            blobs.entry(new_hash.clone()).or_insert_with(|| {
                (
                    Blob {
                        hash: new_hash,
                        size: bytes.len() as u64,
                        mime: mime.clone(),
                        filename: node.meta.name.clone(),
                        created_at: at,
                        // A migrated legacy blob has no recorded origin — its
                        // first device predates the attribute.
                        attributes: Default::default(),
                    },
                    bytes,
                )
            });
        }
        if identity == IdentityMode::Copy {
            for attr in node.content.attrs() {
                if let AttrValue::Ref(mut reference) = attr.value {
                    reference.home = None;
                    let mut changed = attr.clone();
                    changed.value = AttrValue::Ref(reference);
                    target.set_attr(id, &changed).map_err(import_failed)?;
                }
            }
        }
    }
    Ok(())
}

fn collect_trashed_documents(
    source: &WorkspaceDoc,
    node: NodeId,
    parent_doc: Option<DocumentId>,
    inherited_at: Option<i64>,
    inherited_by: Option<String>,
    out: &mut Vec<DocumentSpec>,
) -> Result<(), ImportError> {
    let is_doc = source.kind_of(node) == Some(Kind::Document);
    let this_id = is_doc.then(|| source.document_id(node));
    if let Some(id) = this_id {
        let read = source.read(node).map_err(import_failed)?;
        out.push(DocumentSpec {
            node,
            id,
            parent: parent_doc,
            title: read.content.as_text().unwrap_or("").to_owned(),
            nodes: count_import_nodes(source, node)?,
            trashed_at: parent_doc.is_none().then_some(inherited_at).flatten(),
            trashed_by: parent_doc
                .is_none()
                .then_some(inherited_by.clone())
                .flatten(),
        });
    }
    let next_parent = this_id.or(parent_doc);
    for child in source.children(Parent::Node(node)).map_err(import_failed)? {
        collect_trashed_documents(
            source,
            child,
            next_parent,
            inherited_at,
            inherited_by.clone(),
            out,
        )?;
    }
    Ok(())
}

fn count_import_nodes(source: &WorkspaceDoc, root: NodeId) -> Result<usize, ImportError> {
    let mut count = 1;
    for child in source.children(Parent::Node(root)).map_err(import_failed)? {
        if source.kind_of(child) != Some(Kind::Document) {
            count += count_import_nodes(source, child)?;
        }
    }
    Ok(count)
}

fn import_subtree_ids(source: &WorkspaceDoc, root: NodeId) -> Result<Vec<NodeId>, ImportError> {
    let mut ids = vec![root];
    for child in source.children(Parent::Node(root)).map_err(import_failed)? {
        if source.kind_of(child) != Some(Kind::Document) {
            ids.extend(import_subtree_ids(source, child)?);
        }
    }
    Ok(ids)
}

fn preview_tree(
    specs: &[DocumentSpec],
    parent: Option<DocumentId>,
    inherited_trash: bool,
) -> Vec<PreviewNode> {
    specs
        .iter()
        .filter(|spec| spec.parent == parent)
        .map(|spec| {
            let trashed = inherited_trash || spec.trashed_at.is_some();
            PreviewNode {
                title: spec.title.clone(),
                children: preview_tree(specs, Some(spec.id), trashed),
                nodes: spec.nodes,
                trashed,
            }
        })
        .collect()
}

fn preview_loose_nodes(
    source: &WorkspaceDoc,
    roots: &[NodeId],
    trashed: bool,
) -> Result<Vec<PreviewNode>, ImportError> {
    roots
        .iter()
        .map(|root| {
            let node = source.read(*root).map_err(import_failed)?;
            let children = source
                .children(Parent::Node(*root))
                .map_err(import_failed)?
                .into_iter()
                .filter(|child| source.kind_of(*child) != Some(Kind::Document))
                .collect::<Vec<_>>();
            Ok(PreviewNode {
                title: node
                    .content
                    .as_text()
                    .filter(|text| !text.is_empty())
                    .map(str::to_owned)
                    .or(node.meta.name)
                    .unwrap_or_else(|| node.kind.as_str().to_owned()),
                children: preview_loose_nodes(source, &children, trashed)?,
                nodes: count_import_nodes(source, *root)?,
                trashed,
            })
        })
        .collect()
}

fn bundle_document(id: StoreDocumentId, kind: DocumentKind, bytes: &[u8]) -> BundleDocument {
    BundleDocument {
        id,
        kind,
        checksum: koru_store_core::lifecycle::checksum(bytes),
    }
}

fn store_document(id: DocumentId) -> StoreDocumentId {
    StoreDocumentId::from_uuid(id.as_uuid())
}

fn now() -> Timestamp {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64;
    Timestamp::from_millis(millis)
}

fn import_failed(error: impl std::fmt::Display) -> ImportError {
    ImportError::new(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use koru_core::Mark;
    use koru_import::{ImportOptions, NoBlobSink, NoProgress};

    fn build_bundle(prepared: Box<dyn PreparedImport>) -> Bundle {
        let product = prepared
            .build(
                ImportOptions {
                    identity: IdentityMode::Keep,
                    accept_warnings: true,
                },
                &ImportContext {
                    target_profile: None,
                    target_workspace: None,
                    occupied_titles: &std::collections::BTreeSet::new(),
                    blobs: &NoBlobSink,
                    tags: &koru_import::NoTagSink,
                },
                &NoProgress,
            )
            .unwrap();
        let ImportProduct::Workspace(bundle) = product else {
            panic!("the legacy importer produced documents")
        };
        bundle
    }

    #[test]
    fn splits_nested_documents_and_preserves_node_ids() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = koru_core::WorkspaceId::new();
        let mut source = WorkspaceDoc::create(workspace, "Old").unwrap();
        let parent = source
            .insert(Parent::Root, Node::document("Parent").unwrap())
            .unwrap();
        let paragraph = source
            .insert(Parent::Node(parent), Node::paragraph("Body").unwrap())
            .unwrap();
        let child = source
            .insert(Parent::Node(parent), Node::document("Child").unwrap())
            .unwrap();
        std::fs::write(
            temp.path().join("snapshot.loro"),
            source.export_snapshot().unwrap(),
        )
        .unwrap();

        let importer = LegacyWorkspaceImporter;
        let prepared = importer
            .inspect(
                ImportSource::Directory(temp.path().to_path_buf()),
                &NoProgress,
            )
            .unwrap();
        assert_eq!(prepared.preview().document_count, 2);
        let bundle = build_bundle(prepared);
        let pages: Vec<_> = bundle
            .manifest
            .documents
            .iter()
            .filter(|d| d.kind == DocumentKind::Document)
            .collect();
        assert_eq!(pages.len(), 2);
        let parent_snapshot = bundle
            .snapshot_of(store_document(source.document_id(parent)))
            .unwrap();
        let parent_doc = WorkspaceDoc::from_snapshot(parent_snapshot).unwrap();
        assert!(parent_doc.read(paragraph).is_ok());
        assert!(
            parent_doc.read(child).is_err(),
            "nested document lives only in its own snapshot"
        );
    }

    #[test]
    fn preserves_formatting_and_readdresses_legacy_blobs() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = koru_core::WorkspaceId::new();
        let mut source = WorkspaceDoc::create(workspace, "Old").unwrap();
        let page = source
            .insert(Parent::Root, Node::document("Page").unwrap())
            .unwrap();
        let paragraph = source
            .insert(
                Parent::Node(page),
                Node::paragraph("formatted text").unwrap(),
            )
            .unwrap();
        source.set_mark(paragraph, 0..9, Mark::Bold, true).unwrap();

        let bytes = b"legacy image bytes";
        let legacy = koru_docstore::BlobStore::new(temp.path().join("blobs"), "test")
            .put(bytes)
            .unwrap();
        let image = source
            .insert(
                Parent::Node(page),
                Node::image(legacy, "image/png", Some(8), Some(6)).unwrap(),
            )
            .unwrap();
        std::fs::write(
            temp.path().join("snapshot.loro"),
            source.export_snapshot().unwrap(),
        )
        .unwrap();

        let prepared = LegacyWorkspaceImporter
            .inspect(
                ImportSource::Directory(temp.path().to_path_buf()),
                &NoProgress,
            )
            .unwrap();
        assert_eq!(prepared.preview().image_count, 1);
        assert!(prepared.preview().warnings.is_empty());
        let bundle = build_bundle(prepared);

        let snapshot = bundle
            .snapshot_of(store_document(source.document_id(page)))
            .unwrap();
        let imported = WorkspaceDoc::from_snapshot(snapshot).unwrap();
        assert!(
            imported
                .runs(paragraph)
                .unwrap()
                .iter()
                .any(|run| run.bold && run.text == "formatted")
        );
        let Content::Blob { hash, .. } = imported.read(image).unwrap().content else {
            panic!("the imported image changed kind");
        };
        assert!(hash.0.starts_with("b3:"));
        assert_eq!(bundle.blobs.len(), 1);
        assert_eq!(bundle.blobs[0].0.as_str(), hash.0);
        assert_eq!(bundle.blobs[0].1, bytes);
    }

    #[test]
    fn keeps_deleted_documents_and_loose_nodes_in_their_respective_trash() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = koru_core::WorkspaceId::new();
        let mut source = WorkspaceDoc::create(workspace, "Old").unwrap();
        let page = source
            .insert(Parent::Root, Node::document("Deleted page").unwrap())
            .unwrap();
        source.trash(page, 1234, Some("Ana")).unwrap();
        let loose = source
            .insert(Parent::Root, Node::paragraph("Loose note").unwrap())
            .unwrap();
        source.trash(loose, 2345, Some("Ana")).unwrap();
        std::fs::write(
            temp.path().join("snapshot.loro"),
            source.export_snapshot().unwrap(),
        )
        .unwrap();

        let prepared = LegacyWorkspaceImporter
            .inspect(
                ImportSource::Directory(temp.path().to_path_buf()),
                &NoProgress,
            )
            .unwrap();
        assert_eq!(prepared.preview().document_count, 2);
        assert!(
            prepared
                .preview()
                .documents
                .iter()
                .any(|node| node.title == "Imported items")
        );
        let bundle = build_bundle(prepared);
        let manifest_snapshot = bundle
            .snapshot_of(bundle.manifest_document().unwrap())
            .unwrap();
        let manifest = Manifest::from_snapshot(manifest_snapshot).unwrap();
        let entries = manifest.entries();
        let deleted = entries
            .iter()
            .find(|entry| entry.id == store_document(source.document_id(page)))
            .unwrap();
        assert_eq!(deleted.trashed_at, Some(Timestamp::from_millis(1234)));
        assert_eq!(deleted.trashed_by.as_deref(), Some("Ana"));

        let imported_items = entries
            .iter()
            .find(|entry| entry.title == "Imported items")
            .unwrap();
        let snapshot = bundle.snapshot_of(imported_items.id).unwrap();
        let imported = WorkspaceDoc::from_snapshot(snapshot).unwrap();
        assert!(imported.is_trashed(loose));
        assert_eq!(imported.trashed_by(loose).as_deref(), Some("Ana"));
    }

    /// **An import restores; it never commands** (SPEC §5, ADR-273).
    ///
    /// This is the importer that can actually carry task state, so it is the one
    /// that can be caught rewriting it. A completed task standing over an
    /// unchecked checklist item is the residue SPEC §5 allows — and it is
    /// exactly the shape a classification pass would "repair" on the way in,
    /// reopening a task the archive recorded as done. It comes back untouched,
    /// because an importer's exemption is that the classification entry point
    /// and the six canonical commands are simply not on this path — which this
    /// test reads off the crate itself rather than taking on trust.
    #[test]
    fn a_completed_task_over_unfinished_work_is_restored_exactly_as_recorded() {
        use koru_core::attributes::{Lifecycle, TaskReason};
        use koru_core::node::ListStyle;
        use koru_core::tasks::{TaskFields, faithful_import};

        let temp = tempfile::tempdir().unwrap();
        let workspace = koru_core::WorkspaceId::new();
        let mut source = WorkspaceDoc::create(workspace, "Old").unwrap();
        let page = source
            .insert(Parent::Root, Node::document("Shipping").unwrap())
            .unwrap();
        let ship = source
            .insert(Parent::Node(page), Node::task("ship it").unwrap())
            .unwrap();
        let list = source
            .insert(Parent::Node(ship), Node::list_block().unwrap())
            .unwrap();
        source
            .set_list_style(list, Some(ListStyle::Checkbox))
            .unwrap();
        let item = source
            .insert(
                Parent::Node(list),
                Node::paragraph("draft the summary").unwrap(),
            )
            .unwrap();
        // Written through the named restore door: `koru.task` and
        // `koru.task_state` on a node whose child is unfinished is a state no
        // command would produce in one step, which is the point.
        faithful_import::restore_task(
            &mut source,
            ship,
            &TaskFields {
                marker: Some(TaskReason::Explicit),
                due: None,
                lifecycle: Lifecycle::Completed,
                done: true,
                completed_at: None,
                pinned_today: false,
            },
        )
        .unwrap();
        faithful_import::restore_done(&mut source, ship, true).unwrap();
        std::fs::write(
            temp.path().join("snapshot.loro"),
            source.export_snapshot().unwrap(),
        )
        .unwrap();

        let prepared = LegacyWorkspaceImporter
            .inspect(
                ImportSource::Directory(temp.path().to_path_buf()),
                &NoProgress,
            )
            .unwrap();
        let bundle = build_bundle(prepared);
        let snapshot = bundle
            .snapshot_of(store_document(source.document_id(page)))
            .unwrap();
        let restored = WorkspaceDoc::from_snapshot(snapshot).unwrap();

        let fields = TaskFields::of(&restored.read(ship).unwrap());
        assert_eq!(
            fields.marker,
            Some(TaskReason::Explicit),
            "the marker came back as recorded, not re-derived"
        );
        assert_eq!(
            fields.lifecycle,
            Lifecycle::Completed,
            "and it is still completed — nothing reopened it"
        );
        assert!(fields.done, "the tick came with it");
        assert!(
            !restored.read(item).unwrap().meta.done,
            "over an unchecked item, which no cascade ticked"
        );

        // The names are assembled here so the scan does not find them in the
        // very source it is reading.
        let text = include_str!("lib.rs");
        for halves in [
            ["reclas", "sify"],
            ["ensure", "_task"],
            ["complete", "_task"],
            ["reopen", "_task"],
            ["set", "_due"],
            ["clear", "_due"],
            ["remove_from", "_tasks"],
        ] {
            let name = halves.concat();
            assert!(
                !text.contains(&name),
                "an importer is a faithful restore, so `{name}` is not on its path"
            );
        }
    }
}
