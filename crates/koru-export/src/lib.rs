// SPDX-License-Identifier: MPL-2.0

//! Format-independent document export contract.
//!
//! Each format lives in its own `koru-export-*` crate. This crate owns only the
//! lifecycle the app can present consistently: registry, destination shape,
//! progress and errors. The shell picks a path, loads document snapshots, and
//! hands each exporter plain `WorkspaceDoc` values plus a blob source.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use koru_core::schema::WorkspaceDoc;
use koru_core::{DocumentId, node::BlobHash};

/// What every exporter advertises.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExporterDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    /// File extensions without the dot — `["md"]`, `["pdf"]`, …
    pub extensions: &'static [&'static str],
}

/// One document ready to export: identity, a title for naming, and its tree.
pub struct ExportDocument {
    pub id: DocumentId,
    pub title: String,
    pub doc: WorkspaceDoc,
}

/// Where the shell should send the bytes, given how many documents were chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DestinationKind {
    /// One document: a save-file dialog with this suggested name.
    File { suggested_name: String },
    /// Several documents: a folder picker; the exporter names files inside.
    Directory,
}

/// What the exporter wrote, for toasts and last-dir memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportResult {
    /// Paths of every primary file written (not sidecar assets).
    pub files: Vec<PathBuf>,
    /// Directory that should become `last_export_dir` — parent of a single file,
    /// or the chosen folder itself when multi.
    pub remembered_dir: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportStage {
    Loading,
    Converting,
    WritingMedia,
    Writing,
}

pub trait ExportProgress: Send + Sync {
    fn update(&self, stage: ExportStage, completed: usize, total: Option<usize>);
    fn cancelled(&self) -> bool;
}

pub struct NoProgress;

impl ExportProgress for NoProgress {
    fn update(&self, _: ExportStage, _: usize, _: Option<usize>) {}
    fn cancelled(&self) -> bool {
        false
    }
}

#[derive(Default)]
pub struct CancellationToken(AtomicBool);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Bytes for content-addressed media. Implemented by the shell over the Store.
pub trait BlobSource: Send + Sync {
    fn get(&self, hash: &BlobHash) -> Result<Vec<u8>, ExportError>;
}

/// A source that never has bytes — tests and formats that ignore images.
pub struct NoBlobs;

impl BlobSource for NoBlobs {
    fn get(&self, hash: &BlobHash) -> Result<Vec<u8>, ExportError> {
        Err(ExportError::new(format!("no blob source for {}", hash.0)))
    }
}

pub trait Exporter: Send + Sync {
    fn descriptor(&self) -> &'static ExporterDescriptor;

    /// How the shell should ask the OS for a destination, given selection size.
    fn destination_kind(&self, documents: &[ExportDocument]) -> DestinationKind {
        match documents {
            [one] => DestinationKind::File {
                suggested_name: suggested_file_name(&one.title, self.descriptor().extensions),
            },
            _ => DestinationKind::Directory,
        }
    }

    /// Write every document under `destination` (a file path or a directory).
    fn export(
        &self,
        documents: &[ExportDocument],
        destination: &Path,
        blobs: &dyn BlobSource,
        progress: &dyn ExportProgress,
    ) -> Result<ExportResult, ExportError>;
}

pub struct ExportRegistry {
    exporters: Vec<Box<dyn Exporter>>,
}

impl ExportRegistry {
    pub fn new(exporters: Vec<Box<dyn Exporter>>) -> Result<Self, ExportError> {
        let mut ids = std::collections::HashSet::new();
        for exporter in &exporters {
            if !ids.insert(exporter.descriptor().id) {
                return Err(ExportError::new(format!(
                    "duplicate exporter id: {}",
                    exporter.descriptor().id
                )));
            }
        }
        Ok(Self { exporters })
    }

    pub fn exporters(&self) -> &[Box<dyn Exporter>] {
        &self.exporters
    }

    pub fn get(&self, id: &str) -> Option<&dyn Exporter> {
        self.exporters
            .iter()
            .find(|e| e.descriptor().id == id)
            .map(|e| e.as_ref())
    }

    /// Take one exporter out of the registry, consuming it. The shell moves
    /// the exporter onto a worker thread, which a borrow cannot cross.
    pub fn into_exporter(mut self, id: &str) -> Option<Box<dyn Exporter>> {
        let index = self
            .exporters
            .iter()
            .position(|e| e.descriptor().id == id)?;
        Some(self.exporters.remove(index))
    }
}

/// A safe file stem from a document title, plus the first advertised extension.
pub fn suggested_file_name(title: &str, extensions: &[&str]) -> String {
    let stem = sanitize_stem(title);
    let ext = extensions.first().copied().unwrap_or("txt");
    format!("{stem}.{ext}")
}

/// Keep letters, digits, space, hyphen and underscore; collapse the rest.
pub fn sanitize_stem(title: &str) -> String {
    let safe: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let safe = safe.trim().trim_matches('-').trim();
    if safe.chars().any(|c| c.is_alphanumeric()) {
        safe.to_owned()
    } else {
        "document".into()
    }
}

/// Pick a path that does not yet exist: `name.ext`, then `name (2).ext`, …
pub fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    let candidate = dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");
    let ext = path.extension().and_then(|s| s.to_str());
    for n in 2..10_000 {
        let name = match ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = dir.join(&name);
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(file_name)
}

#[derive(Debug)]
pub struct ExportError {
    message: String,
}

impl ExportError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
    pub fn cancelled() -> Self {
        Self::new("export cancelled")
    }
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ExportError {}

#[cfg(test)]
mod tests {
    use super::*;

    struct Stub;
    static DESC: ExporterDescriptor = ExporterDescriptor {
        id: "stub",
        name: "Stub",
        extensions: &["stub"],
    };
    impl Exporter for Stub {
        fn descriptor(&self) -> &'static ExporterDescriptor {
            &DESC
        }
        fn export(
            &self,
            _: &[ExportDocument],
            _: &Path,
            _: &dyn BlobSource,
            _: &dyn ExportProgress,
        ) -> Result<ExportResult, ExportError> {
            Err(ExportError::new("unused"))
        }
    }

    #[test]
    fn registry_refuses_duplicate_stable_ids() {
        assert!(ExportRegistry::new(vec![Box::new(Stub), Box::new(Stub)]).is_err());
    }

    #[test]
    fn sanitize_falls_back_when_title_is_punctuation() {
        assert_eq!(sanitize_stem("???"), "document");
        assert_eq!(sanitize_stem("Hello / World"), "Hello - World");
    }

    #[test]
    fn suggested_name_uses_first_extension() {
        assert_eq!(
            suggested_file_name("Notes", &["md", "markdown"]),
            "Notes.md"
        );
    }
}
