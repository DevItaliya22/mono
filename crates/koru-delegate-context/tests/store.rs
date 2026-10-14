// SPDX-License-Identifier: MPL-2.0

//! **§7.2 through the real store.** The same `Store<SqliteCatalog,
//! SqliteBackend>` the desktop worker and the Android host open, under a
//! temporary root, with a temporary run workspace beside it — the one
//! process-and-disk scenario this crate owns (the harness is
//! `koru-store-sqlite/tests/run_document.rs`'s).
//!
//! What is proved, in the roadmap's words (§10, P1.7): UC2 — capture, edit
//! the source, the run delivers and names the captured bytes; a
//! materialization per delivery mode; capture during a concurrent edit;
//! reload after checkpointing; a trashed source rendering as trashed and a
//! purged one as unavailable **with no cascade and no change to Trash or
//! restore** (D47); a deletion during an active attempt letting it finish
//! while refusing the source afterwards (D48); a second firing while the
//! first run's context exists; and that no filename, title or body text
//! reaches anything this crate renders.
//!
//! Trash here is the store's own `trash_document_entry`; restore and
//! unregister are what a client does — a manifest edit applied as an
//! update, exactly as `koru-workspace::edit_manifest` does — and the
//! physical purge is the repository's `drop_document`, the one operation
//! that cascades `document_versions`. None of them is this crate's.

use std::path::{Path, PathBuf};

use koru_core::node::Node;
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_delegate_context::{
    CaptureStage, ContextSource, ContextState, DeliveryMode, Fault, ManifestFile, MaterializeStage,
    PinLiveFailure, RunId, SourceRefusal, StoreContext, cleanup, layout,
};
use koru_delegate_core::{BlobHash, BlobKind, BlockNodeId, DocumentId, RunContextItem};
use koru_store_core::CheckpointPolicy;
use koru_store_core::fixtures::{SAMPLES, UNICODE_FILENAME};
use koru_store_core::ids::WorkspaceId;
use koru_store_core::manifest::Manifest;
use koru_store_core::model::Timestamp;
use koru_store_core::ports::WorkspaceRepo;
use koru_store_core::service::{Backend, Store};
use koru_store_core::text::Locale;
use koru_store_sqlite::{SqliteBackend, SqliteCatalog};

type TestStore = Store<SqliteCatalog, SqliteBackend>;

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millis(1_770_000_000_000 + ms)
}

/// A store root and a run workspace, both temporary.
struct Root {
    dir: tempfile::TempDir,
    /// The run workspace; kept alive by the `TempDir`, read as a field so
    /// it can be borrowed beside a mutable borrow of the store.
    _work: tempfile::TempDir,
    work_path: PathBuf,
    store: TestStore,
    clock: i64,
}

impl Root {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        Self {
            store: Self::open_in(dir.path()),
            dir,
            work_path: work.path().to_path_buf(),
            _work: work,
            clock: 0,
        }
    }

    fn open_in(root: &Path) -> TestStore {
        let catalog = SqliteCatalog::open(&root.join("catalog.db")).unwrap();
        Store::open(catalog, SqliteBackend::new(root)).unwrap()
    }

    /// Close and reopen, as a restart would. Nothing is cached across it.
    fn restart(&mut self) {
        self.store = Self::open_in(self.dir.path());
    }

    fn tick(&mut self) -> Timestamp {
        self.clock += 1_000;
        at(self.clock)
    }

    fn a_workspace(&mut self) -> WorkspaceId {
        let now = self.tick();
        let profile = self
            .store
            .create_profile("Owner", None, None, Locale::parse("en").unwrap(), now)
            .unwrap();
        self.store
            .create_workspace(profile.id, "Workspace", now)
            .unwrap()
            .id
    }

    /// The repository beneath the store — the seam a physical purge goes
    /// through. Opened with the store closed, the way a maintenance
    /// operation would.
    fn repo(&self, workspace: WorkspaceId) -> <SqliteBackend as Backend>::Workspace {
        SqliteBackend::new(self.dir.path())
            .open_workspace(workspace)
            .unwrap()
    }

    /// A page with a title and one paragraph, imported the way a document
    /// reaches the store. Returns the document and its paragraph's node.
    fn a_page(&mut self, ws: WorkspaceId, title: &str, body: &str) -> (DocumentId, BlockNodeId) {
        let mut page = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "W").unwrap();
        let root = page
            .insert(Parent::Root, Node::document(title).unwrap())
            .unwrap();
        let line = page
            .insert(Parent::Node(root), Node::paragraph(body).unwrap())
            .unwrap();
        let now = self.tick();
        let (id, _) = self
            .store
            .import_document(
                ws,
                title,
                "page",
                None,
                page.export_snapshot().unwrap(),
                now,
            )
            .unwrap();
        (id, BlockNodeId::from_uuid(line.as_uuid()))
    }

    /// Edit the paragraph through the store, as an editor's replica would:
    /// fork, change, send the delta, flush.
    fn edit(&mut self, ws: WorkspaceId, document: DocumentId, node: BlockNodeId, text: &str) {
        let mut doc = WorkspaceDoc::adopt(self.store.fork_document(ws, document).unwrap()).unwrap();
        let before = doc.version();
        doc.set_text(koru_core::NodeId::from_uuid(node.as_uuid()), text)
            .unwrap();
        let delta = doc.export_updates_since(&before).unwrap();
        let now = self.tick();
        self.store
            .apply_update(ws, document, delta, now, None)
            .unwrap();
        self.store.flush(ws, document, now).unwrap();
    }

    fn delete_node(&mut self, ws: WorkspaceId, document: DocumentId, node: BlockNodeId) {
        let mut doc = WorkspaceDoc::adopt(self.store.fork_document(ws, document).unwrap()).unwrap();
        let before = doc.version();
        doc.delete(koru_core::NodeId::from_uuid(node.as_uuid()))
            .unwrap();
        let delta = doc.export_updates_since(&before).unwrap();
        let now = self.tick();
        self.store
            .apply_update(ws, document, delta, now, None)
            .unwrap();
        self.store.flush(ws, document, now).unwrap();
    }

    fn a_blob(&mut self, ws: WorkspaceId, bytes: &[u8], mime: &str, filename: &str) -> BlobHash {
        let now = self.tick();
        self.store
            .put_blob(ws, bytes, mime, Some(filename), now, None)
            .unwrap()
            .hash
    }

    fn trash(&mut self, ws: WorkspaceId, document: DocumentId) {
        let now = self.tick();
        self.store
            .trash_document_entry(ws, document, now, Some("Tester"))
            .unwrap();
    }

    /// What a client does to restore or unregister: edit its copy of the
    /// manifest and send the delta.
    fn edit_manifest(&mut self, ws: WorkspaceId, edit: impl FnOnce(&mut Manifest)) {
        let manifest_id = self.store.manifest_id(ws).unwrap();
        let mut remote =
            Manifest::from_snapshot(&self.store.snapshot(ws, manifest_id).unwrap()).unwrap();
        let before = remote.version();
        edit(&mut remote);
        let delta = remote.export_updates_since(&before).unwrap();
        let now = self.tick();
        self.store
            .apply_update(ws, manifest_id, delta, now, None)
            .unwrap();
        self.store.flush(ws, manifest_id, now).unwrap();
    }

    fn restore(&mut self, ws: WorkspaceId, document: DocumentId) {
        self.edit_manifest(ws, |m| m.restore_document(document).unwrap());
    }

    /// The store's own two-step physical purge: unregister in the
    /// manifest, then drop the rows — "always the later of the two".
    fn purge(&mut self, ws: WorkspaceId, document: DocumentId) {
        self.edit_manifest(ws, |m| {
            m.remove_document(document).unwrap();
        });
        // Drop with the store closed, as a maintenance sweep would.
        let closed = Self::open_in(self.dir.path());
        drop(std::mem::replace(&mut self.store, closed));
        self.repo(ws).drop_document(document).unwrap();
        self.restart();
    }

    fn trashed_at(&mut self, ws: WorkspaceId, document: DocumentId) -> Option<Option<Timestamp>> {
        self.store
            .documents(ws)
            .unwrap()
            .into_iter()
            .find(|entry| entry.id == document)
            .map(|entry| entry.trashed_at)
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn manifest_of(root: &Path) -> koru_delegate_context::Manifest {
    serde_json::from_slice(&std::fs::read(root.join(layout::MANIFEST_FILE)).unwrap()).unwrap()
}

// ------------------------------------------------------------------ UC2

/// Capture, edit the source, materialize: the run delivers the captured
/// bytes and names the version it was given — not what the document says
/// now.
#[test]
fn uc2_the_run_delivers_and_names_the_bytes_it_was_given_after_the_source_is_edited() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let (doc, line) = root.a_page(ws, "Quarterly plan", "The first draft.");
    let run = RunId::mint();

    let items = StoreContext::new(&mut root.store)
        .pin(&[ContextSource::Document {
            workspace: ws,
            document: doc,
        }])
        .unwrap();
    let RunContextItem::Document { version, .. } = &items[0] else {
        panic!("a document pins as a document")
    };
    let captured = StoreContext::new(&mut root.store)
        .capture(run, &items, ws)
        .unwrap();
    assert_eq!(captured.pins[0].item, items[0]);

    root.edit(ws, doc, line, "The second draft, after delegation.");

    let laid = StoreContext::new(&mut root.store)
        .materialize(run, &captured, DeliveryMode::QuotedPath, &root.work_path)
        .unwrap();
    assert_eq!(laid.root, layout::run_dir(&root.work_path, run));
    assert_eq!(laid.files.len(), 1);
    let markdown = read(&laid.files[0].path);
    assert!(markdown.contains("The first draft."), "{markdown}");
    assert!(!markdown.contains("second draft"), "{markdown}");
    assert!(markdown.contains("# Quarterly plan"), "{markdown}");

    let manifest = manifest_of(&laid.root);
    assert_eq!(manifest.run, run);
    match &manifest.files[0] {
        ManifestFile::Document {
            document: named,
            version: named_version,
            path,
            checksum,
            ..
        } => {
            assert_eq!(named, &doc);
            assert_eq!(named_version, version);
            assert_eq!(path, &laid.files[0].relative);
            assert_eq!(checksum, &captured.pins[0].checksum);
        }
        other => panic!("{other:?}"),
    }

    // The live document did change; the pin did not follow it.
    let live = WorkspaceDoc::adopt(root.store.fork_document(ws, doc).unwrap()).unwrap();
    let text = live
        .read(koru_core::NodeId::from_uuid(line.as_uuid()))
        .unwrap()
        .content
        .as_text()
        .unwrap()
        .to_owned();
    assert!(text.contains("second draft"));
}

/// An edit applied but not yet flushed, then the capture, then another
/// edit: the pin is one consistent version — the first edit and not the
/// second — and its checksum verifies when read back.
#[test]
fn a_capture_during_a_concurrent_edit_pins_one_consistent_version() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let (doc, line) = root.a_page(ws, "Plan", "Draft zero.");
    let node = koru_core::NodeId::from_uuid(line.as_uuid());

    // Edit A reaches the store and is not yet durable when the capture
    // is asked for.
    let mut replica = WorkspaceDoc::adopt(root.store.fork_document(ws, doc).unwrap()).unwrap();
    let before = replica.version();
    replica.set_text(node, "Draft A, in flight.").unwrap();
    let delta_a = replica.export_updates_since(&before).unwrap();
    let now = root.tick();
    root.store
        .apply_update(ws, doc, delta_a, now, None)
        .unwrap();

    let items = StoreContext::new(&mut root.store)
        .pin(&[ContextSource::Document {
            workspace: ws,
            document: doc,
        }])
        .unwrap();

    // Edit B lands after the capture, and everything is flushed.
    let before = replica.version();
    replica.set_text(node, "Draft B, after.").unwrap();
    let delta_b = replica.export_updates_since(&before).unwrap();
    let now = root.tick();
    root.store
        .apply_update(ws, doc, delta_b, now, None)
        .unwrap();
    root.store.flush(ws, doc, now).unwrap();

    let run = RunId::mint();
    let captured = StoreContext::new(&mut root.store)
        .capture(run, &items, ws)
        .unwrap();
    let laid = StoreContext::new(&mut root.store)
        .materialize(
            run,
            &captured,
            DeliveryMode::NativeStructured,
            &root.work_path,
        )
        .unwrap();
    let markdown = read(&laid.files[0].path);
    assert!(markdown.contains("Draft A, in flight."), "{markdown}");
    assert!(!markdown.contains("Draft B"), "{markdown}");
}

/// The checkpoint policy prunes the journal beneath a capture; after a
/// restart the capture still opens, from `document_versions`.
#[test]
fn a_capture_survives_checkpointing_and_a_reload() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let (doc, line) = root.a_page(ws, "Plan", "Before any checkpoint.");
    root.store.checkpoints = CheckpointPolicy {
        every_updates: 1,
        every_bytes: 1,
    };

    let items = StoreContext::new(&mut root.store)
        .pin(&[ContextSource::Document {
            workspace: ws,
            document: doc,
        }])
        .unwrap();
    for i in 0..5 {
        root.edit(ws, doc, line, &format!("Edit {i} after the capture."));
    }
    let report = root.store.document_report(ws, doc).unwrap();
    assert!(report.latest_checkpoint.is_some(), "a checkpoint was taken");
    assert!(
        report.pending_updates <= 1,
        "{} pending against a policy of 1",
        report.pending_updates
    );

    root.restart();
    let run = RunId::mint();
    let captured = StoreContext::new(&mut root.store)
        .capture(run, &items, ws)
        .unwrap();
    let laid = StoreContext::new(&mut root.store)
        .materialize(run, &captured, DeliveryMode::QuotedPath, &root.work_path)
        .unwrap();
    let markdown = read(&laid.files[0].path);
    assert!(markdown.contains("Before any checkpoint."), "{markdown}");
    assert!(!markdown.contains("Edit "), "{markdown}");
}

// ------------------------------------------------------- delivery modes

fn three_sources(
    root: &mut Root,
    ws: WorkspaceId,
) -> (Vec<ContextSource>, DocumentId, BlockNodeId, BlobHash) {
    let (doc, line) = root.a_page(ws, SAMPLES[7].title, SAMPLES[7].body);
    let blob = root.a_blob(ws, b"%PDF-1.4 fixture", "application/pdf", UNICODE_FILENAME);
    let sources = vec![
        ContextSource::Document {
            workspace: ws,
            document: doc,
        },
        ContextSource::Block {
            workspace: ws,
            document: doc,
            node: line,
        },
        ContextSource::Blob {
            workspace: ws,
            blob: blob.clone(),
            blob_kind: BlobKind::Pdf,
        },
    ];
    (sources, doc, line, blob)
}

/// A document, a block and a blob under each delivery mode: the same
/// id-derived layout, the mode recorded on every file and in the manifest,
/// and the person's filename in the manifest's metadata and in no path.
#[test]
fn materializes_a_document_a_block_and_a_blob_under_each_delivery_mode() {
    for mode in [DeliveryMode::NativeStructured, DeliveryMode::QuotedPath] {
        let mut root = Root::new();
        let ws = root.a_workspace();
        let (sources, doc, line, blob) = three_sources(&mut root, ws);
        let run = RunId::mint();
        let items = StoreContext::new(&mut root.store).pin(&sources).unwrap();
        let captured = StoreContext::new(&mut root.store)
            .capture(run, &items, ws)
            .unwrap();
        let laid = StoreContext::new(&mut root.store)
            .materialize(run, &captured, mode, &root.work_path)
            .unwrap();

        assert_eq!(laid.files.len(), 3, "{mode:?}");
        for (i, file) in laid.files.iter().enumerate() {
            assert_eq!(file.item, i);
            assert_eq!(file.delivery, mode);
            assert!(file.path.is_file(), "{mode:?} item {i}");
            assert!(file.relative.is_ascii(), "{}", file.relative);
            assert!(!file.relative.contains(UNICODE_FILENAME));
            assert!(!file.path.to_string_lossy().contains(UNICODE_FILENAME));
        }
        let RunContextItem::Document { version, .. } = &items[0] else {
            panic!()
        };
        assert_eq!(laid.files[0].relative, layout::document_file(doc, *version));
        assert_eq!(
            laid.files[1].relative,
            layout::block_file(doc, *version, line)
        );
        assert_eq!(
            laid.files[2].relative,
            layout::blob_file(&blob, "application/pdf", BlobKind::Pdf)
        );
        assert_eq!(laid.files[0].media_type, "text/markdown");
        assert_eq!(laid.files[2].media_type, "application/pdf");
        assert_eq!(
            std::fs::read(&laid.files[2].path).unwrap(),
            b"%PDF-1.4 fixture"
        );

        let whole = read(&laid.files[0].path);
        let block = read(&laid.files[1].path);
        assert!(whole.contains(SAMPLES[7].title), "{mode:?}: {whole}");
        assert!(whole.contains(SAMPLES[7].body), "{mode:?}: {whole}");
        assert!(block.contains(SAMPLES[7].body), "{mode:?}: {block}");
        assert!(
            !block.contains(SAMPLES[7].title),
            "a block has no title line: {block}"
        );

        let manifest = manifest_of(&laid.root);
        assert_eq!(manifest.delivery, mode);
        assert_eq!(manifest.files.len(), 3);
        for (i, entry) in manifest.files.iter().enumerate() {
            assert_eq!(entry.item(), i);
            assert_eq!(entry.path(), laid.files[i].relative);
            assert!(!entry.path().contains(UNICODE_FILENAME));
        }
        match &manifest.files[2] {
            ManifestFile::Blob {
                hash,
                filename,
                size,
                blob_kind,
                ..
            } => {
                assert_eq!(hash, &blob);
                assert_eq!(filename.as_deref(), Some(UNICODE_FILENAME));
                assert_eq!(*size, b"%PDF-1.4 fixture".len() as u64);
                assert_eq!(*blob_kind, BlobKind::Pdf);
            }
            other => panic!("{other:?}"),
        }
    }
}

/// A provider that receives attachments neither way cannot be given
/// context, and that fails before it starts.
#[test]
fn an_unsupported_delivery_mode_fails_before_anything_is_written() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let (doc, _) = root.a_page(ws, "Plan", "Body.");
    let run = RunId::mint();
    let items = StoreContext::new(&mut root.store)
        .pin(&[ContextSource::Document {
            workspace: ws,
            document: doc,
        }])
        .unwrap();
    let captured = StoreContext::new(&mut root.store)
        .capture(run, &items, ws)
        .unwrap();
    let failure = StoreContext::new(&mut root.store)
        .materialize(run, &captured, DeliveryMode::Unsupported, &root.work_path)
        .unwrap_err();
    assert_eq!(failure.item, None);
    assert_eq!(failure.stage, MaterializeStage::Layout);
    assert_eq!(failure.fault, Fault::DeliveryUnsupported);
    assert!(!layout::run_dir(&root.work_path, run).exists());
}

// ------------------------------------------------------------------ D47

/// Trash the source: the state reads in-trash and the capture still opens.
/// Restore it: live again. Purge it — the store's own two steps — and the
/// capture is gone because `document_versions` cascade with the document,
/// which is the store's rule; the state reads unavailable. Throughout, the
/// listing says exactly what the store's own trash suite says it does.
#[test]
fn d47_a_trashed_source_reads_in_trash_a_restored_one_live_and_a_purged_one_unavailable() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let (doc, _) = root.a_page(ws, "Plan", "Pinned body.");
    let run = RunId::mint();
    let items = StoreContext::new(&mut root.store)
        .pin(&[ContextSource::Document {
            workspace: ws,
            document: doc,
        }])
        .unwrap();
    let captured = StoreContext::new(&mut root.store)
        .capture(run, &items, ws)
        .unwrap();
    let RunContextItem::Document { version, .. } = items[0].clone() else {
        panic!()
    };
    assert_eq!(
        StoreContext::new(&mut root.store)
            .states(&captured.pins)
            .unwrap(),
        [ContextState::Live]
    );

    // --- trash: the store's own operation, unchanged -------------------
    root.trash(ws, doc);
    let stamped = root.trashed_at(ws, doc).unwrap();
    assert!(stamped.is_some(), "the entry is stamped, and still listed");
    assert_eq!(
        StoreContext::new(&mut root.store)
            .states(&captured.pins)
            .unwrap(),
        [ContextState::InTrash]
    );
    // The capture is untouched: no cascade, the version row is there.
    let stored = root.store.document_version(ws, version).unwrap().unwrap();
    assert_eq!(stored.checksum, captured.pins[0].checksum);
    let laid = StoreContext::new(&mut root.store)
        .materialize(run, &captured, DeliveryMode::QuotedPath, &root.work_path)
        .unwrap();
    assert!(read(&laid.files[0].path).contains("Pinned body."));

    // --- restore: a client's manifest edit, unchanged ------------------
    root.restore(ws, doc);
    assert_eq!(
        root.trashed_at(ws, doc),
        Some(None),
        "restore clears the stamp and nothing else"
    );
    assert_eq!(
        StoreContext::new(&mut root.store)
            .states(&captured.pins)
            .unwrap(),
        [ContextState::Live]
    );
    assert!(root.store.document_version(ws, version).unwrap().is_some());

    // --- restart: the states are read, not remembered -------------------
    root.restart();
    assert_eq!(
        StoreContext::new(&mut root.store)
            .states(&captured.pins)
            .unwrap(),
        [ContextState::Live]
    );

    // --- purge: unregister, then drop the rows — the store's rule -------
    root.purge(ws, doc);
    assert_eq!(root.trashed_at(ws, doc), None, "no longer listed");
    assert!(
        root.store.document_version(ws, version).unwrap().is_none(),
        "document_versions cascade with the document's rows"
    );
    assert_eq!(
        StoreContext::new(&mut root.store)
            .states(&captured.pins)
            .unwrap(),
        [ContextState::Unavailable]
    );
    let failure = StoreContext::new(&mut root.store)
        .capture(run, &items, ws)
        .unwrap_err();
    assert_eq!(failure.item, 0);
    assert_eq!(failure.stage, CaptureStage::Resolve);
    assert_eq!(failure.fault, Fault::DocumentUnavailable);
}

/// A document under a trashed parent is in the trash by inheritance — the
/// manifest's rule, read through the listing.
#[test]
fn a_source_under_a_trashed_parent_reads_in_trash() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let now = root.tick();
    let (parent, _) = root
        .store
        .create_document(ws, "Parent", "page", None, None, now)
        .unwrap();
    let mut page = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "W").unwrap();
    let doc_root = page
        .insert(Parent::Root, Node::document("Child").unwrap())
        .unwrap();
    page.insert(
        Parent::Node(doc_root),
        Node::paragraph("Child body.").unwrap(),
    )
    .unwrap();
    let now = root.tick();
    let (child, _) = root
        .store
        .import_document(
            ws,
            "Child",
            "page",
            Some(parent),
            page.export_snapshot().unwrap(),
            now,
        )
        .unwrap();
    let items = StoreContext::new(&mut root.store)
        .pin(&[ContextSource::Document {
            workspace: ws,
            document: child,
        }])
        .unwrap();
    let captured = StoreContext::new(&mut root.store)
        .capture(RunId::mint(), &items, ws)
        .unwrap();
    root.trash(ws, parent);
    assert_eq!(
        root.trashed_at(ws, child),
        Some(None),
        "the child carries no stamp"
    );
    assert_eq!(
        StoreContext::new(&mut root.store)
            .states(&captured.pins)
            .unwrap(),
        [ContextState::InTrash]
    );
}

/// A blob has no trash: it is retained or it is not.
#[test]
fn a_blob_pin_reads_live_or_unavailable() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let blob = root.a_blob(ws, b"png bytes", "image/png", "photo.png");
    let items = StoreContext::new(&mut root.store)
        .pin(&[ContextSource::Blob {
            workspace: ws,
            blob: blob.clone(),
            blob_kind: BlobKind::Image,
        }])
        .unwrap();
    let captured = StoreContext::new(&mut root.store)
        .capture(RunId::mint(), &items, ws)
        .unwrap();
    assert_eq!(captured.pins[0].checksum, blob.as_str());
    assert_eq!(
        StoreContext::new(&mut root.store)
            .states(&captured.pins)
            .unwrap(),
        [ContextState::Live]
    );
    let other = root.a_workspace();
    let elsewhere = koru_delegate_context::ContextPin {
        item: RunContextItem::Blob {
            workspace: other,
            blob,
            blob_kind: BlobKind::Image,
        },
        checksum: captured.pins[0].checksum.clone(),
    };
    assert_eq!(
        StoreContext::new(&mut root.store)
            .states(&[elsewhere])
            .unwrap(),
        [ContextState::Unavailable]
    );
}

// ------------------------------------------------------------------ D48

/// The source is trashed between capture and materialization: the attempt
/// finishes with the bytes it was given. Afterwards the source resolves for
/// nothing new — admission refuses it by index with a stable code — until
/// it is restored; a purge refuses it for good.
#[test]
fn d48_a_deletion_during_an_attempt_lets_it_finish_and_refuses_the_source_afterwards() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let (doc, _) = root.a_page(ws, "Plan", "Handed to the provider.");
    let image = root.a_blob(ws, b"png", "image/png", "photo.png");
    let run = RunId::mint();
    let items = StoreContext::new(&mut root.store)
        .pin(&[
            ContextSource::Blob {
                workspace: ws,
                blob: image,
                blob_kind: BlobKind::Image,
            },
            ContextSource::Document {
                workspace: ws,
                document: doc,
            },
        ])
        .unwrap();
    let captured = StoreContext::new(&mut root.store)
        .capture(run, &items, ws)
        .unwrap();
    StoreContext::new(&mut root.store)
        .admit(&captured.pins)
        .unwrap();

    // The deletion lands mid-attempt.
    root.trash(ws, doc);

    let laid = StoreContext::new(&mut root.store)
        .materialize(
            run,
            &captured,
            DeliveryMode::NativeStructured,
            &root.work_path,
        )
        .unwrap();
    assert!(read(&laid.files[1].path).contains("Handed to the provider."));
    assert_eq!(laid.files.len(), 2, "every pinned item is delivered");

    // Afterwards: no Resume against it, no second run using it.
    assert_eq!(
        StoreContext::new(&mut root.store)
            .admit(&captured.pins)
            .unwrap_err(),
        SourceRefusal::InTrash { item: 1 }
    );
    let resume = StoreContext::new(&mut root.store)
        .admit(&captured.pins)
        .unwrap_err();
    assert_eq!(resume.as_str(), "source_in_trash");
    assert_eq!(resume.item(), 1);

    root.restore(ws, doc);
    StoreContext::new(&mut root.store)
        .admit(&captured.pins)
        .unwrap();

    root.purge(ws, doc);
    assert_eq!(
        StoreContext::new(&mut root.store)
            .admit(&captured.pins)
            .unwrap_err(),
        SourceRefusal::Unavailable { item: 1 }
    );
}

// --------------------------------------------------- two runs, one folder

/// A second firing while the first run's context exists: two directories,
/// and the first's bytes are untouched by the second's materialization
/// and by the second's cleanup.
#[test]
fn a_second_run_on_the_same_folder_gets_its_own_directory_and_leaves_the_first_alone() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let (doc, line) = root.a_page(ws, "Plan", "First firing.");
    let first = RunId::mint();
    let items = StoreContext::new(&mut root.store)
        .pin(&[ContextSource::Document {
            workspace: ws,
            document: doc,
        }])
        .unwrap();
    let captured_first = StoreContext::new(&mut root.store)
        .capture(first, &items, ws)
        .unwrap();
    let laid_first = StoreContext::new(&mut root.store)
        .materialize(
            first,
            &captured_first,
            DeliveryMode::QuotedPath,
            &root.work_path,
        )
        .unwrap();
    let first_bytes = std::fs::read(&laid_first.files[0].path).unwrap();
    let first_manifest = std::fs::read(laid_first.root.join(layout::MANIFEST_FILE)).unwrap();

    root.edit(ws, doc, line, "Second firing.");
    let second = RunId::mint();
    let items = StoreContext::new(&mut root.store)
        .pin(&[ContextSource::Document {
            workspace: ws,
            document: doc,
        }])
        .unwrap();
    let captured_second = StoreContext::new(&mut root.store)
        .capture(second, &items, ws)
        .unwrap();
    let laid_second = StoreContext::new(&mut root.store)
        .materialize(
            second,
            &captured_second,
            DeliveryMode::QuotedPath,
            &root.work_path,
        )
        .unwrap();

    assert_ne!(laid_first.root, laid_second.root);
    assert_eq!(laid_first.root.parent(), laid_second.root.parent());
    assert!(read(&laid_second.files[0].path).contains("Second firing."));
    assert_eq!(
        std::fs::read(&laid_first.files[0].path).unwrap(),
        first_bytes
    );
    assert_eq!(
        std::fs::read(laid_first.root.join(layout::MANIFEST_FILE)).unwrap(),
        first_manifest
    );

    cleanup(second, &root.work_path).unwrap();
    assert!(!laid_second.root.exists());
    assert!(laid_first.root.is_dir());
    assert_eq!(
        std::fs::read(&laid_first.files[0].path).unwrap(),
        first_bytes
    );
}

/// Re-materializing the same run — a resume — rebuilds its own directory
/// from the same pins.
#[test]
fn a_resume_rebuilds_the_same_run_directory_from_the_same_pins() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let (doc, line) = root.a_page(ws, "Plan", "Original.");
    let run = RunId::mint();
    let items = StoreContext::new(&mut root.store)
        .pin(&[ContextSource::Document {
            workspace: ws,
            document: doc,
        }])
        .unwrap();
    let captured = StoreContext::new(&mut root.store)
        .capture(run, &items, ws)
        .unwrap();
    let first = StoreContext::new(&mut root.store)
        .materialize(run, &captured, DeliveryMode::QuotedPath, &root.work_path)
        .unwrap();
    std::fs::write(first.root.join("stray.txt"), b"left by a provider").unwrap();
    root.edit(ws, doc, line, "Edited between attempts.");

    let again = StoreContext::new(&mut root.store)
        .materialize(run, &captured, DeliveryMode::QuotedPath, &root.work_path)
        .unwrap();
    assert_eq!(again.root, first.root);
    assert!(
        !again.root.join("stray.txt").exists(),
        "rebuilt, not appended"
    );
    assert!(read(&again.files[0].path).contains("Original."));
}

// ------------------------------------------------------------- cleanup

/// Cleanup removes the copies and only the copies.
#[test]
fn cleanup_removes_materialized_copies_and_never_a_capture_or_a_blob() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let (sources, _, _, blob) = three_sources(&mut root, ws);
    let run = RunId::mint();
    let items = StoreContext::new(&mut root.store).pin(&sources).unwrap();
    let RunContextItem::Document { version, .. } = items[0].clone() else {
        panic!()
    };
    let captured = StoreContext::new(&mut root.store)
        .capture(run, &items, ws)
        .unwrap();
    let laid = StoreContext::new(&mut root.store)
        .materialize(run, &captured, DeliveryMode::QuotedPath, &root.work_path)
        .unwrap();
    assert!(laid.root.is_dir());

    cleanup(run, &root.work_path).unwrap();
    assert!(!laid.root.exists());
    assert!(
        layout::context_root(&root.work_path).is_dir(),
        "the sibling root stays"
    );
    assert!(root.store.document_version(ws, version).unwrap().is_some());
    assert!(root.store.get_blob(ws, &blob).is_ok());
    // Idempotent.
    cleanup(run, &root.work_path).unwrap();
    cleanup(RunId::mint(), Path::new("never-materialized-here")).unwrap();
}

// ------------------------------------------------------------ failures

/// A fresh scheduled firing crosses all three pre-claim gates as one
/// operation: pin the current frontier, verify the exact bytes, then admit
/// only live sources. Direct and inherited Trash are both refusals, and a
/// late refusal returns no partial vector to a caller that could claim it.
#[test]
fn fresh_schedule_pins_require_every_source_to_be_live() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let (live, _) = root.a_page(ws, "Live", "First item.");
    let (trashed, _) = root.a_page(ws, "Trashed", "Second item.");
    root.trash(ws, trashed);

    let failure = StoreContext::new(&mut root.store)
        .pin_live(
            RunId::mint(),
            &[ContextSource::Document {
                workspace: ws,
                document: trashed,
            }],
            ws,
        )
        .unwrap_err();
    assert_eq!(
        failure,
        PinLiveFailure::Refused(SourceRefusal::InTrash { item: 0 })
    );

    let failure = StoreContext::new(&mut root.store)
        .pin_live(
            RunId::mint(),
            &[
                ContextSource::Document {
                    workspace: ws,
                    document: live,
                },
                ContextSource::Document {
                    workspace: ws,
                    document: trashed,
                },
            ],
            ws,
        )
        .unwrap_err();
    assert_eq!(failure.item(), 1, "the first usable pin is not returned");

    let now = root.tick();
    let (parent, _) = root
        .store
        .create_document(ws, "Parent", "page", None, None, now)
        .unwrap();
    let mut page = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "W").unwrap();
    page.insert(Parent::Root, Node::document("Child").unwrap())
        .unwrap();
    let now = root.tick();
    let (child, _) = root
        .store
        .import_document(
            ws,
            "Child",
            "page",
            Some(parent),
            page.export_snapshot().unwrap(),
            now,
        )
        .unwrap();
    root.trash(ws, parent);
    let failure = StoreContext::new(&mut root.store)
        .pin_live(
            RunId::mint(),
            &[ContextSource::Document {
                workspace: ws,
                document: child,
            }],
            ws,
        )
        .unwrap_err();
    assert_eq!(
        failure,
        PinLiveFailure::Refused(SourceRefusal::InTrash { item: 0 })
    );
}

/// A block pointer is resolved against the just-captured snapshot, not
/// merely against the document listing. Deleting the block before a firing
/// therefore fails at Resolve and no pin can reach a claim.
#[test]
fn fresh_schedule_refuses_a_block_deleted_before_the_firing() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let (document, node) = root.a_page(ws, "Plan", "Delete this block.");
    root.delete_node(ws, document, node);

    let failure = StoreContext::new(&mut root.store)
        .pin_live(
            RunId::mint(),
            &[ContextSource::Block {
                workspace: ws,
                document,
                node,
            }],
            ws,
        )
        .unwrap_err();
    assert_eq!(
        failure,
        PinLiveFailure::Capture(koru_delegate_context::CaptureFailure {
            item: 0,
            stage: CaptureStage::Resolve,
            fault: Fault::NodeUnavailable,
        })
    );
}

/// Purged documents and unknown blobs are unavailable before a schedule
/// can claim a run. Neither failure returns a provisional pin vector.
#[test]
fn fresh_schedule_refuses_purged_documents_and_missing_blobs() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let (document, _) = root.a_page(ws, "Plan", "Gone.");
    root.purge(ws, document);

    let failure = StoreContext::new(&mut root.store)
        .pin_live(
            RunId::mint(),
            &[ContextSource::Document {
                workspace: ws,
                document,
            }],
            ws,
        )
        .unwrap_err();
    assert_eq!(failure.item(), 0);
    assert!(matches!(
        failure,
        PinLiveFailure::Capture(koru_delegate_context::CaptureFailure {
            stage: CaptureStage::Resolve,
            fault: Fault::DocumentUnavailable,
            ..
        })
    ));

    let missing = BlobHash::from_digest("b3", &"ab".repeat(32)).unwrap();
    let failure = StoreContext::new(&mut root.store)
        .pin_live(
            RunId::mint(),
            &[ContextSource::Blob {
                workspace: ws,
                blob: missing,
                blob_kind: BlobKind::File,
            }],
            ws,
        )
        .unwrap_err();
    assert!(matches!(
        failure,
        PinLiveFailure::Capture(koru_delegate_context::CaptureFailure {
            stage: CaptureStage::Resolve,
            fault: Fault::BlobUnavailable,
            ..
        })
    ));
}

/// Each occurrence resolves anew: an unchanged frontier deduplicates to
/// the same version, while an edit before the next occurrence produces a
/// different immutable pin and leaves the old pin untouched.
#[test]
fn fresh_schedule_reuses_an_unchanged_frontier_and_pins_an_edit_anew() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let (document, node) = root.a_page(ws, "Plan", "First occurrence.");
    let source = ContextSource::Document {
        workspace: ws,
        document,
    };
    let first = StoreContext::new(&mut root.store)
        .pin_live(RunId::mint(), std::slice::from_ref(&source), ws)
        .unwrap();
    let unchanged = StoreContext::new(&mut root.store)
        .pin_live(RunId::mint(), std::slice::from_ref(&source), ws)
        .unwrap();
    assert_eq!(first, unchanged);

    root.edit(ws, document, node, "Second occurrence.");
    let edited = StoreContext::new(&mut root.store)
        .pin_live(RunId::mint(), &[source], ws)
        .unwrap();
    assert_ne!(first, edited);
    assert!(
        StoreContext::new(&mut root.store)
            .capture(RunId::mint(), &first, ws)
            .is_ok()
    );
}

/// An input that cannot be captured names the item by index and stage.
#[test]
fn a_failure_names_the_item_index_and_the_stage() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let (doc, _) = root.a_page(ws, "Plan", "Body.");
    let missing_blob = BlobHash::from_digest("b3", &"ef".repeat(32)).unwrap();

    let failure = StoreContext::new(&mut root.store)
        .pin(&[
            ContextSource::Document {
                workspace: ws,
                document: doc,
            },
            ContextSource::Blob {
                workspace: ws,
                blob: missing_blob.clone(),
                blob_kind: BlobKind::File,
            },
        ])
        .unwrap_err();
    assert_eq!(failure.item, 1);
    assert_eq!(failure.stage, CaptureStage::Resolve);
    assert_eq!(failure.fault, Fault::BlobUnavailable);

    let failure = StoreContext::new(&mut root.store)
        .pin(&[ContextSource::Document {
            workspace: ws,
            document: DocumentId::new(),
        }])
        .unwrap_err();
    assert_eq!((failure.item, failure.stage), (0, CaptureStage::Resolve));
    assert_eq!(failure.fault, Fault::DocumentUnavailable);

    let failure = StoreContext::new(&mut root.store)
        .pin(&[ContextSource::Document {
            workspace: WorkspaceId::new(),
            document: doc,
        }])
        .unwrap_err();
    assert_eq!(failure.fault, Fault::WorkspaceUnavailable);

    // A pin whose version was never captured here fails retention.
    let forged = RunContextItem::Document {
        workspace: ws,
        document: doc,
        version: koru_delegate_core::DocumentVersionId::new(),
    };
    let failure = StoreContext::new(&mut root.store)
        .capture(RunId::mint(), &[forged], ws)
        .unwrap_err();
    assert_eq!((failure.item, failure.stage), (0, CaptureStage::Retention));
    assert_eq!(failure.fault, Fault::VersionUnavailable);

    // A version that names another document's bytes is not delivered
    // under this document's name.
    let (other, _) = root.a_page(ws, "Other", "Other body.");
    let items = StoreContext::new(&mut root.store)
        .pin(&[ContextSource::Document {
            workspace: ws,
            document: other,
        }])
        .unwrap();
    let RunContextItem::Document { version, .. } = items[0].clone() else {
        panic!()
    };
    let crossed = RunContextItem::Document {
        workspace: ws,
        document: doc,
        version,
    };
    let failure = StoreContext::new(&mut root.store)
        .capture(RunId::mint(), &[crossed], ws)
        .unwrap_err();
    assert_eq!(failure.fault, Fault::VersionUnavailable);
}

/// Capturing an unchanged document twice pins the same version — the
/// store deduplicates by frontier, and this crate does not second-guess it.
#[test]
fn an_unchanged_document_pins_the_same_version_twice() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let (doc, _) = root.a_page(ws, "Plan", "Body.");
    let source = ContextSource::Document {
        workspace: ws,
        document: doc,
    };
    let first = StoreContext::new(&mut root.store)
        .pin(std::slice::from_ref(&source))
        .unwrap();
    let second = StoreContext::new(&mut root.store).pin(&[source]).unwrap();
    assert_eq!(first, second);
}

/// A materialization that cannot write names the item, the stage and the
/// io kind — and nothing about where it was writing.
#[test]
fn a_write_failure_is_a_stage_and_an_io_kind() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let (doc, _) = root.a_page(ws, "Plan", "Body.");
    let run = RunId::mint();
    let items = StoreContext::new(&mut root.store)
        .pin(&[ContextSource::Document {
            workspace: ws,
            document: doc,
        }])
        .unwrap();
    let captured = StoreContext::new(&mut root.store)
        .capture(run, &items, ws)
        .unwrap();
    // A run workspace that is a file, not a directory.
    let not_a_dir: PathBuf = root.work_path.join("a-file");
    std::fs::write(&not_a_dir, b"x").unwrap();
    let failure = StoreContext::new(&mut root.store)
        .materialize(run, &captured, DeliveryMode::QuotedPath, &not_a_dir)
        .unwrap_err();
    assert_eq!(failure.item, None);
    assert_eq!(failure.stage, MaterializeStage::Layout);
    assert!(matches!(failure.fault, Fault::Io(_)), "{failure:?}");
    assert!(!failure.to_string().contains("a-file"));
}

// ------------------------------------------------------------- privacy

/// Every rendering this crate produces on the real store path — pins,
/// captures, layouts, manifests' `Debug`, every failure and refusal — with
/// the eight-language titles and bodies and the Unicode filename planted
/// in the store, contains none of them. The Markdown files and the
/// manifest's `filename` field are the delivery, and are asserted to hold
/// them; nothing else may.
#[test]
fn nothing_this_crate_renders_carries_a_title_a_filename_or_body_text() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let mut sources = Vec::new();
    for sample in SAMPLES {
        let (doc, line) = root.a_page(ws, sample.title, sample.body);
        sources.push(ContextSource::Document {
            workspace: ws,
            document: doc,
        });
        sources.push(ContextSource::Block {
            workspace: ws,
            document: doc,
            node: line,
        });
    }
    let blob = root.a_blob(
        ws,
        SAMPLES[5].body.as_bytes(),
        "text/plain",
        UNICODE_FILENAME,
    );
    sources.push(ContextSource::Blob {
        workspace: ws,
        blob: blob.clone(),
        blob_kind: BlobKind::File,
    });
    let sentinels: Vec<&str> = SAMPLES
        .iter()
        .flat_map(|s| [s.title, s.body])
        .chain([UNICODE_FILENAME])
        .collect();
    let assert_clean = |what: &str, rendered: &str| {
        for sentinel in &sentinels {
            assert!(
                !rendered.contains(sentinel),
                "{what} leaked {sentinel:?}: {rendered}"
            );
        }
    };

    let run = RunId::mint();
    let items = StoreContext::new(&mut root.store).pin(&sources).unwrap();
    assert_clean("items", &format!("{items:#?}"));
    let captured = StoreContext::new(&mut root.store)
        .capture(run, &items, ws)
        .unwrap();
    assert_clean("captured", &format!("{captured:#?}"));
    assert_clean("captured json", &serde_json::to_string(&captured).unwrap());

    for mode in [DeliveryMode::NativeStructured, DeliveryMode::QuotedPath] {
        let laid = StoreContext::new(&mut root.store)
            .materialize(run, &captured, mode, &root.work_path)
            .unwrap();
        assert_clean("laid", &format!("{laid:#?}"));
        for file in &laid.files {
            assert_clean("file", &format!("{file:#?}"));
            assert!(file.relative.is_ascii(), "{}", file.relative);
        }
        let manifest = manifest_of(&laid.root);
        assert_clean("manifest debug", &format!("{manifest:#?}"));
        for entry in &manifest.files {
            assert_clean("manifest path", entry.path());
        }
        // The delivery itself carries the content — that is its job.
        let last = manifest.files.last().unwrap();
        assert!(
            matches!(last, ManifestFile::Blob { filename: Some(name), .. } if name == UNICODE_FILENAME)
        );
        assert!(read(&laid.files[0].path).contains(SAMPLES[0].body));
        assert_eq!(
            std::fs::read(&laid.files.last().unwrap().path).unwrap(),
            SAMPLES[5].body.as_bytes()
        );
    }
    let states = StoreContext::new(&mut root.store)
        .states(&captured.pins)
        .unwrap();
    assert_clean("states", &format!("{states:?}"));

    // Every failure and refusal the real path can produce.
    let RunContextItem::Document { document, .. } = items[0].clone() else {
        panic!()
    };
    root.trash(ws, document);
    let refusal = StoreContext::new(&mut root.store)
        .admit(&captured.pins)
        .unwrap_err();
    assert_clean("refusal", &format!("{refusal:?} {refusal}"));
    root.purge(ws, document);
    let refusal = StoreContext::new(&mut root.store)
        .admit(&captured.pins)
        .unwrap_err();
    assert_clean("refusal", &format!("{refusal:?} {refusal}"));
    let failure = StoreContext::new(&mut root.store)
        .capture(run, &items, ws)
        .unwrap_err();
    assert_clean("capture failure", &format!("{failure:?} {failure}"));
    let failure = StoreContext::new(&mut root.store)
        .materialize(run, &captured, DeliveryMode::QuotedPath, &root.work_path)
        .unwrap_err();
    assert_clean("materialize failure", &format!("{failure:?} {failure}"));
    let failure = StoreContext::new(&mut root.store)
        .pin(&[ContextSource::Blob {
            workspace: ws,
            blob: BlobHash::from_digest("b3", &"01".repeat(32)).unwrap(),
            blob_kind: BlobKind::File,
        }])
        .unwrap_err();
    assert_clean("pin failure", &format!("{failure:?} {failure}"));
    let fault = cleanup(run, &root.work_path.join("a-file-not-a-dir"));
    assert_clean("cleanup", &format!("{fault:?}"));
}
