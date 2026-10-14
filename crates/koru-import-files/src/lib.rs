// SPDX-License-Identifier: MPL-2.0

//! Arbitrary file import through the same source-independent contract as note
//! importers. File bytes live in the blob store; the manifest entry holds a
//! tiny opaque document whose single root records name, MIME type and hash.
//!
//! **An import is a faithful restore, not a thing a person just did**
//! ([ADR-286], SPEC §5). `Kind::File` is task-capable, so this is where a task
//! could most plausibly be invented by accident — a file *can* be one, and a
//! dropped attachment is not. It arrives as a file and nothing else: no marker,
//! no lifecycle, no deadline, no tick, no command and no classification, so no
//! activity is owed. The exemption is not a flag this crate sets; it is that the
//! classification entry point and the six canonical commands are not on this
//! path at all, which `an_imported_file_is_a_file_and_never_a_task` reads off
//! the source itself. Anything that ever *does* need to write task state here
//! goes through `koru_core::tasks::faithful_import` and nowhere else.
//!
//! [ADR-286]: ../../../docs/decisions/ADR-276-300.md#adr-286-taskboundary--six-canonical-commands-own-task-state-and-every-host-is-a-thin-wrapper-around-them

use std::path::{Path, PathBuf};

use koru_core::NodeId;
use koru_core::node::{BlobHash, Node};
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_import::{
    ImportContext, ImportError, ImportOptions, ImportPreview, ImportProduct, ImportProgress,
    ImportSource, ImportStage, ImportWarning, ImportedContent, ImportedDocument, Importer,
    ImporterDescriptor, PreparedImport, PreviewNode, SourceRequest,
};
use koru_store_core::ids::DocumentId;

const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

pub static DESCRIPTOR: ImporterDescriptor = ImporterDescriptor {
    id: "files",
    name: "Files",
    source: SourceRequest::Files { extensions: &[] },
    // A service importer — `/attach`, image drops, the Files mode's Import
    // files… — not a menu format.
    menu_name: None,
    picker_title: "Add files",
    claims_zip: None,
    roots_import: false,
    dedups_known_files: true,
};

pub struct FileImporter;

struct PreparedFiles {
    paths: Vec<PathBuf>,
    preview: ImportPreview,
}

impl Importer for FileImporter {
    fn descriptor(&self) -> &'static ImporterDescriptor {
        &DESCRIPTOR
    }

    fn inspect(
        &self,
        source: ImportSource,
        progress: &dyn ImportProgress,
    ) -> Result<Box<dyn PreparedImport>, ImportError> {
        let ImportSource::Files(paths) = source else {
            return Err(ImportError::new("file import needs one or more files"));
        };
        if paths.is_empty() {
            return Err(ImportError::new("no files were selected"));
        }
        let mut accepted = Vec::new();
        let mut warnings = Vec::new();
        for (index, path) in paths.into_iter().enumerate() {
            progress.update(ImportStage::Inspecting, index, None);
            match path.metadata() {
                Ok(meta) if meta.is_file() && meta.len() <= MAX_FILE_BYTES => accepted.push(path),
                Ok(meta) if meta.len() > MAX_FILE_BYTES => warnings.push(format!(
                    "{} is larger than the 2 GiB file limit",
                    path.display()
                )),
                Ok(_) => warnings.push(format!("{} is not a file", path.display())),
                Err(error) => warnings.push(format!("{}: {error}", path.display())),
            }
        }
        if accepted.is_empty() {
            return Err(ImportError::new(warnings.join("\n")));
        }
        let documents = accepted
            .iter()
            .map(|path| PreviewNode {
                title: filename(path),
                children: Vec::new(),
                nodes: 1,
                trashed: false,
            })
            .collect();
        let preview = ImportPreview {
            native_workspace_id: None,
            workspace_name: "Files".into(),
            document_count: accepted.len(),
            node_count: accepted.len(),
            image_count: 0,
            documents,
            warnings,
            identity_modes: Vec::new(),
        };
        Ok(Box::new(PreparedFiles {
            paths: accepted,
            preview,
        }))
    }
}

impl PreparedImport for PreparedFiles {
    fn preview(&self) -> &ImportPreview {
        &self.preview
    }

    fn build(
        self: Box<Self>,
        _options: ImportOptions,
        context: &ImportContext<'_>,
        progress: &dyn ImportProgress,
    ) -> Result<ImportProduct, ImportError> {
        let store_workspace = context
            .target_workspace
            .ok_or_else(|| ImportError::new("file import needs a target workspace"))?;
        let workspace = koru_core::WorkspaceId::from_uuid(store_workspace.as_uuid());
        let total = self.paths.len();
        let mut documents = Vec::with_capacity(total);
        let mut warnings = Vec::new();
        for (index, path) in self.paths.into_iter().enumerate() {
            if progress.cancelled() {
                return Err(ImportError::cancelled());
            }
            progress.update(ImportStage::ReadingMedia, index, Some(total));
            let title = filename(&path);
            let result: Result<ImportedDocument, ImportError> = (|| {
                let bytes = std::fs::read(&path).map_err(import_failed)?;
                let mime = mime_for_name(&title);
                let stored = context.blobs.put(bytes, mime, Some(&title))?;
                let id = DocumentId::new();
                let mut root = Node::file(&title, BlobHash(stored.hash), stored.mime.clone())
                    .map_err(import_failed)?;
                root.id = NodeId::from_uuid(id.as_uuid());
                let mut doc = WorkspaceDoc::create(workspace, &title).map_err(import_failed)?;
                doc.insert(Parent::Root, root).map_err(import_failed)?;
                Ok(ImportedDocument {
                    id,
                    source: path.clone(),
                    title,
                    kind: koru_core::explore::KIND_FILE.into(),
                    mime: Some(stored.mime),
                    parent: None,
                    trashed: false,
                    trashed_at: None,
                    snapshot: doc.export_snapshot().map_err(import_failed)?,
                })
            })();
            match result {
                Ok(document) => documents.push(document),
                Err(error) => warnings.push(ImportWarning {
                    source: path,
                    code: None,
                    arguments: Vec::new(),
                    message: error.to_string(),
                }),
            }
        }
        if documents.is_empty() {
            return Err(ImportError::new(
                "none of the selected files could be imported",
            ));
        }
        Ok(ImportProduct::Documents(ImportedContent {
            documents,
            warnings,
        }))
    }
}

fn filename(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Untitled file")
        .to_owned()
}

fn mime_for_name(name: &str) -> &'static str {
    match Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("pdf") => "application/pdf",
        Some("txt") | Some("log") => "text/plain",
        // **`.url` is here because the share sheet invents it.** Shared *text*
        // is normalized before it is staged — a bare web URL becomes a `.url`
        // of `text/uri-list`, a shared page an `.html`, anything else a `.txt`
        // — by `SharedText.normalize`, written once in Swift and obeyed by
        // both phones and the Mac (ADR-137, ADR-416). Nothing else in the app
        // produces a `.url`, so a shared link arrived here and left as
        // `application/octet-stream`: a download rather than something a
        // reader will open. `.md` is not the share sheet's — a dropped one is
        // routed to the *markdown* importer, and only a shared or explicitly
        // added one reaches this table — but it was mislabelled the same way.
        Some("url") => "text/uri-list",
        Some("html" | "htm") => "text/html",
        Some("md" | "markdown") => "text/markdown",
        Some("csv") => "text/csv",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("zip") => "application/zip",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        Some("pptx") => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
}

fn import_failed(error: impl std::fmt::Display) -> ImportError {
    ImportError::new(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Sink(Mutex<Vec<u8>>);

    impl koru_import::BlobSink for Sink {
        fn put(
            &self,
            bytes: Vec<u8>,
            mime: &str,
            _: Option<&str>,
        ) -> Result<koru_import::StoredBlob, ImportError> {
            *self.0.lock().unwrap() = bytes;
            Ok(koru_import::StoredBlob {
                hash: "b3:abcd".into(),
                mime: mime.into(),
            })
        }
    }

    #[test]
    fn known_and_unknown_mime_types_are_stable() {
        assert_eq!(mime_for_name("notes.pdf"), "application/pdf");
        assert_eq!(mime_for_name("archive.unknown"), "application/octet-stream");
    }

    /// **What the share sheet stages, named the way it staged it.**
    /// `SharedText.normalize` turns a shared web URL into `Shared link.url` and
    /// a shared page into an `.html`; both used to arrive here as
    /// `application/octet-stream`, which is a download rather than something a
    /// reader opens. The extension is the only thing in the product that
    /// produces a `.url`, so this row is the whole of its handling.
    #[test]
    fn what_a_share_stages_keeps_its_type() {
        assert_eq!(mime_for_name("Shared link.url"), "text/uri-list");
        assert_eq!(mime_for_name("Shared page.html"), "text/html");
        assert_eq!(mime_for_name("A Page.htm"), "text/html");
        assert_eq!(mime_for_name("Shared text.txt"), "text/plain");
        // Case is the sharer's, not ours: `SharedText.normalize` keeps a
        // subject's own suffix rather than lowercasing it ("Roadmap.URL").
        assert_eq!(mime_for_name("Roadmap.URL"), "text/uri-list");
        assert_eq!(mime_for_name("notes.md"), "text/markdown");
    }

    #[test]
    fn import_stores_bytes_and_builds_one_root_only_file_entry() {
        use koru_import::Importer as _;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("report.pdf");
        std::fs::write(&path, b"pdf bytes").unwrap();
        let prepared = FileImporter
            .inspect(
                ImportSource::Files(vec![path.clone()]),
                &koru_import::NoProgress,
            )
            .unwrap();
        let sink = Sink(Mutex::new(Vec::new()));
        let workspace = koru_store_core::ids::WorkspaceId::new();
        let product = prepared
            .build(
                ImportOptions {
                    identity: koru_import::IdentityMode::Copy,
                    accept_warnings: true,
                },
                &ImportContext {
                    target_profile: None,
                    target_workspace: Some(workspace),
                    occupied_titles: &std::collections::BTreeSet::new(),
                    blobs: &sink,
                    tags: &koru_import::NoTagSink,
                },
                &koru_import::NoProgress,
            )
            .unwrap();
        let ImportProduct::Documents(content) = product else {
            panic!("file importer returned a workspace")
        };
        assert_eq!(&*sink.0.lock().unwrap(), b"pdf bytes");
        assert_eq!(content.documents.len(), 1);
        let imported = &content.documents[0];
        assert_eq!(imported.kind, koru_core::explore::KIND_FILE);
        let doc = WorkspaceDoc::from_snapshot(&imported.snapshot).unwrap();
        let roots = doc.children(Parent::Root).unwrap();
        assert_eq!(roots, [NodeId::from_uuid(imported.id.as_uuid())]);
        let root = doc.read(roots[0]).unwrap();
        assert_eq!(root.kind, koru_core::Kind::File);
        assert_eq!(root.meta.name.as_deref(), Some("report.pdf"));
    }

    /// **An import restores; it never commands** (SPEC §5, ADR-273).
    ///
    /// `Kind::File` is task-capable, so this is the importer where a task could
    /// most plausibly be invented by accident: a file *can* be one, and a
    /// dropped attachment is not. It arrives as a file and nothing else — no
    /// marker, no lifecycle, no deadline, no tick — because an importer's
    /// exemption is that the classification entry point and the six canonical
    /// commands are simply not on this path, which this test reads off the crate
    /// itself rather than taking on trust.
    #[test]
    fn an_imported_file_is_a_file_and_never_a_task() {
        use koru_import::Importer as _;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("report.pdf");
        std::fs::write(&path, b"pdf bytes").unwrap();
        let prepared = FileImporter
            .inspect(ImportSource::Files(vec![path]), &koru_import::NoProgress)
            .unwrap();
        let sink = Sink(Mutex::new(Vec::new()));
        let product = prepared
            .build(
                ImportOptions {
                    identity: koru_import::IdentityMode::Copy,
                    accept_warnings: true,
                },
                &ImportContext {
                    target_profile: None,
                    target_workspace: Some(koru_store_core::ids::WorkspaceId::new()),
                    occupied_titles: &std::collections::BTreeSet::new(),
                    blobs: &sink,
                    tags: &koru_import::NoTagSink,
                },
                &koru_import::NoProgress,
            )
            .unwrap();
        let ImportProduct::Documents(content) = product else {
            panic!("file importer returned a workspace")
        };
        let doc = WorkspaceDoc::from_snapshot(&content.documents[0].snapshot).unwrap();

        let nodes = doc.walk_all().unwrap();
        assert!(!nodes.is_empty(), "the file produced a document");
        for (node, _) in nodes {
            let read = doc.read(node).unwrap();
            assert!(!read.meta.done, "no tick was invented");
            assert!(
                !koru_core::tasks::is_task_node(&doc, node).unwrap(),
                "and a task-capable kind is still not a task until somebody says so"
            );
        }

        // The names are assembled here so the scan does not find them in the
        // very source it is reading.
        let source = include_str!("lib.rs");
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
                !source.contains(&name),
                "an importer is a faithful restore, so `{name}` is not on its path"
            );
        }
    }
}
