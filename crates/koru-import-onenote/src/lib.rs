// SPDX-License-Identifier: MPL-2.0

//! Offline Microsoft OneNote import.
//!
//! The format crate owns parsing and conversion only. Blob bytes cross the
//! `BlobSink` seam and complete Koru snapshots cross the importer seam, so no
//! Store transport or application UI leaks into this crate.
//!
//! **An import is a faithful restore, not a thing a person just did**
//! ([ADR-286], SPEC §5). OneNote's model has no completion and no deadline of
//! its own, so a page arrives carrying no task state at all: no marker is
//! fabricated, no tick is invented, no lifecycle command runs and nothing
//! classifies, so no activity is owed. The exemption is not a flag this crate
//! sets; it is that the classification entry point and the six canonical
//! commands are not on this path at all, which
//! `an_imported_page_carries_no_task_state_and_invents_none` reads off the
//! source itself. Anything that ever *does* need to write task state here goes
//! through `koru_core::tasks::faithful_import` and nowhere else.
//!
//! [ADR-286]: ../../../docs/decisions/ADR-276-300.md#adr-286-taskboundary--six-canonical-commands-own-task-state-and-every-host-is-a-thin-wrapper-around-them

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Error as IoError, ErrorKind as IoErrorKind, Read};
use std::path::{Path, PathBuf};

use koru_core::ids::{DocRef, DocumentId, DocumentReference, NodeId, Ref};
use koru_core::node::{Attr, AttrValue, BlobHash, Kind, Node};
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_import::{
    ImportContext, ImportError, ImportOptions, ImportPreview, ImportProduct, ImportProgress,
    ImportSource, ImportStage, ImportWarning, ImportedContent, ImportedDocument, Importer,
    ImporterDescriptor, PreparedImport, PreviewNode, SourceRequest,
};
use koru_store_core::ids::DocumentId as StoreDocumentId;
use onenote_parser::contents::{Content, EmbeddedFile, Outline, OutlineElement, OutlineItem};
use onenote_parser::page::{Page, PageContent};
use onenote_parser::section::{Section, SectionEntry};
use onenote_parser::{FileSystem, Parser, notebook::Notebook};
use tempfile::TempDir;
use typed_path::{TypedComponent, TypedPath, TypedPathBuf, UnixComponent, WindowsComponent};

const MAX_SOURCE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 20_000;
const MAX_ARCHIVE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_ATTACHMENT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

pub static DESCRIPTOR: ImporterDescriptor = ImporterDescriptor {
    id: "onenote",
    name: "Microsoft OneNote",
    source: SourceRequest::Files {
        extensions: &["one", "onetoc2", "onepkg", "zip"],
    },
    menu_name: Some("Microsoft OneNote"),
    picker_title: "Import Microsoft OneNote content",
    claims_zip: Some(claims_onenote_zip),
    roots_import: true,
    dedups_known_files: true,
};

/// A dropped `.zip` belongs to OneNote when it contains `.one`/`.onetoc2`
/// members. Anything else stays with the formats that treat zip as a
/// container convention.
fn claims_onenote_zip(names: &[String]) -> bool {
    names.iter().any(|name| {
        let name = name.to_ascii_lowercase();
        name.ends_with(".one") || name.ends_with(".onetoc2")
    })
}

pub struct OneNoteImporter;

enum ParsedSource {
    Notebook(Notebook),
    Section(Section),
}

struct Source {
    path: PathBuf,
    title: String,
    parsed: ParsedSource,
}

struct PreparedOneNote {
    sources: Vec<Source>,
    /// Parsed ZIP members are lazy file sources; keep their extraction roots
    /// alive through `build`.
    _temporary: Vec<TempDir>,
    preview: ImportPreview,
}

impl Importer for OneNoteImporter {
    fn descriptor(&self) -> &'static ImporterDescriptor {
        &DESCRIPTOR
    }

    fn inspect(
        &self,
        source: ImportSource,
        progress: &dyn ImportProgress,
    ) -> Result<Box<dyn PreparedImport>, ImportError> {
        let ImportSource::Files(paths) = source else {
            return Err(ImportError::new("OneNote import needs one or more files"));
        };
        if paths.is_empty() {
            return Err(ImportError::new("no OneNote files were selected"));
        }

        let mut sources = Vec::new();
        let mut temporary = Vec::new();
        let mut warnings = Vec::new();
        for (index, path) in paths.iter().enumerate() {
            if progress.cancelled() {
                return Err(ImportError::cancelled());
            }
            // A single package can spend most of a minute inside the parser,
            // and there is no byte-level callback to make a percentage honest.
            // Keep that case moving; multiple independent files can use their
            // completed-file count as determinate progress.
            progress.update(
                ImportStage::Inspecting,
                index,
                (paths.len() > 1).then_some(paths.len()),
            );
            match inspect_path(path, &mut temporary) {
                Ok(mut found) => sources.append(&mut found),
                Err(error) => warnings.push(format!("{}: {error}", path.display())),
            }
        }
        if sources.is_empty() {
            return Err(ImportError::new(if warnings.is_empty() {
                "none of the selected files contained a OneNote notebook".into()
            } else {
                warnings.join("\n")
            }));
        }
        progress.update(ImportStage::Inspecting, paths.len(), Some(paths.len()));

        let documents: Vec<PreviewNode> = sources.iter().map(preview_source).collect();
        let document_count = documents.iter().map(count_preview).sum();
        let node_count = sources.iter().map(estimate_nodes).sum();
        let image_count = sources.iter().map(count_images).sum();
        let preview = ImportPreview {
            native_workspace_id: None,
            workspace_name: sources
                .first()
                .map(|s| s.title.clone())
                .unwrap_or_else(|| "OneNote import".into()),
            documents,
            document_count,
            node_count,
            image_count,
            warnings,
            identity_modes: vec![koru_import::IdentityMode::Copy],
        };
        Ok(Box::new(PreparedOneNote {
            sources,
            _temporary: temporary,
            preview,
        }))
    }
}

impl PreparedImport for PreparedOneNote {
    fn preview(&self) -> &ImportPreview {
        &self.preview
    }

    fn build(
        self: Box<Self>,
        _options: ImportOptions,
        context: &ImportContext<'_>,
        progress: &dyn ImportProgress,
    ) -> Result<ImportProduct, ImportError> {
        let target = context
            .target_workspace
            .ok_or_else(|| ImportError::new("OneNote import needs an open workspace"))?;
        let workspace = koru_core::WorkspaceId::from_uuid(target.as_uuid());
        let plan = ImportPlan::new(&self.sources);
        let mut build = Build {
            workspace,
            context,
            plan,
            entries: Vec::new(),
            files: Vec::new(),
            file_by_hash: HashMap::new(),
            unresolved: HashSet::new(),
            warnings: self
                .preview
                .warnings
                .iter()
                .map(|message| ImportWarning {
                    source: PathBuf::new(),
                    code: None,
                    arguments: Vec::new(),
                    message: message.clone(),
                })
                .collect(),
        };

        for (index, source) in self.sources.iter().enumerate() {
            if progress.cancelled() {
                return Err(ImportError::cancelled());
            }
            progress.update(ImportStage::Converting, index, Some(self.sources.len()));
            build.source(index, source)?;
        }
        progress.update(
            ImportStage::BuildingBundle,
            self.sources.len(),
            Some(self.sources.len()),
        );

        // Files must be durable before any attachment reference is published.
        build.files.extend(build.entries);
        Ok(ImportProduct::Documents(ImportedContent {
            documents: build.files,
            warnings: build.warnings,
        }))
    }
}

fn inspect_path(path: &Path, temporary: &mut Vec<TempDir>) -> Result<Vec<Source>, ImportError> {
    let size = std::fs::metadata(path)
        .map_err(|e| ImportError::new(format!("could not inspect source: {e}")))?
        .len();
    if size > MAX_SOURCE_BYTES {
        return Err(ImportError::new("the OneNote source is larger than 4 GiB"));
    }
    let extension = extension(path);
    if extension == "zip" {
        let temp = tempfile::tempdir()
            .map_err(|e| ImportError::new(format!("could not prepare ZIP import: {e}")))?;
        extract_zip(path, temp.path())?;
        let candidates = find_sources(temp.path())?;
        let mut parsed = Vec::new();
        for candidate in candidates {
            parsed.extend(inspect_path(&candidate, temporary)?);
        }
        temporary.push(temp);
        return Ok(parsed);
    }

    // onenote_parser 2.0's NativeFs rejects absolute Windows drive paths while
    // converting them to its verbatim namespace. Give the parser a path rooted
    // at the selected file's directory instead. Besides avoiding that library
    // bug, this keeps every TOC-derived lookup inside one audited filesystem
    // boundary rather than handing attacker-controlled child names to the OS.
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ImportError::new("the OneNote filename is not valid Unicode"))?;
    let parser = Parser::new_with_fs(RootedFs { root });
    let typed = TypedPath::derive(filename);
    match extension.as_str() {
        "one" => {
            let section = parser
                .parse_section(typed)
                .map_err(|e| ImportError::new(format!("could not parse OneNote section: {e}")))?;
            Ok(vec![Source {
                path: path.to_path_buf(),
                title: section.display_name().to_owned(),
                parsed: ParsedSource::Section(section),
            }])
        }
        "onetoc2" => {
            let notebook = parser
                .parse_notebook(typed)
                .map_err(|e| ImportError::new(format!("could not parse OneNote notebook: {e}")))?;
            Ok(vec![Source {
                path: path.to_path_buf(),
                title: notebook_title(path),
                parsed: ParsedSource::Notebook(notebook),
            }])
        }
        "onepkg" => {
            let notebook = parser
                .parse_package(typed)
                .map_err(|e| ImportError::new(format!("could not parse OneNote package: {e}")))?;
            Ok(vec![Source {
                path: path.to_path_buf(),
                title: file_stem(path),
                parsed: ParsedSource::Notebook(notebook),
            }])
        }
        _ => Err(ImportError::new("unsupported OneNote source extension")),
    }
}

/// Read-only native filesystem rooted at the selected source directory.
///
/// Paths emitted by a `.onetoc2` are untrusted. The parser already removes
/// traversal, prefixes and roots; this boundary checks them again before any
/// host filesystem call and canonicalizes them relative to `root`.
#[derive(Clone, Copy)]
struct RootedFs<'a> {
    root: &'a Path,
}

impl RootedFs<'_> {
    fn resolve(self, path: TypedPath<'_>) -> Result<PathBuf, IoError> {
        let mut resolved = self.root.to_path_buf();
        for component in path.components() {
            let name = match component {
                TypedComponent::Windows(WindowsComponent::Normal(name))
                | TypedComponent::Unix(UnixComponent::Normal(name)) => name,
                TypedComponent::Windows(WindowsComponent::CurDir)
                | TypedComponent::Unix(UnixComponent::CurDir) => continue,
                _ => {
                    return Err(IoError::new(
                        IoErrorKind::InvalidInput,
                        "OneNote path escaped its source directory",
                    ));
                }
            };
            push_native_component(&mut resolved, name)?;
        }
        Ok(resolved)
    }

    fn relative(self, path: &Path, base: &Path) -> Result<TypedPathBuf, IoError> {
        let relative = path.strip_prefix(base).map_err(|_| {
            IoError::new(
                IoErrorKind::PermissionDenied,
                "OneNote path escaped its source directory",
            )
        })?;
        native_typed_path(relative)
    }
}

impl FileSystem for RootedFs<'_> {
    fn is_directory(&self, path: TypedPath<'_>) -> Result<bool, IoError> {
        match std::fs::metadata(self.resolve(path)?) {
            Ok(metadata) => Ok(metadata.is_dir()),
            Err(error) if error.kind() == IoErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn read_dir(&self, path: TypedPath<'_>) -> Result<Vec<TypedPathBuf>, IoError> {
        std::fs::read_dir(self.resolve(path)?)?
            .map(|entry| {
                let path = entry?.path();
                self.relative(&path, self.root)
            })
            .collect()
    }

    fn read_file(&self, path: TypedPath<'_>) -> Result<Vec<u8>, IoError> {
        std::fs::read(self.resolve(path)?)
    }

    fn write_file(&self, _path: TypedPath<'_>, _data: &[u8]) -> Result<(), IoError> {
        Err(IoError::new(
            IoErrorKind::Unsupported,
            "OneNote import filesystem is read-only",
        ))
    }

    fn stream_to_file(&self, _path: TypedPath<'_>, _reader: &mut dyn Read) -> Result<(), IoError> {
        Err(IoError::new(
            IoErrorKind::Unsupported,
            "OneNote import filesystem is read-only",
        ))
    }

    fn make_dir(&self, _path: TypedPath<'_>) -> Result<(), IoError> {
        Err(IoError::new(
            IoErrorKind::Unsupported,
            "OneNote import filesystem is read-only",
        ))
    }

    fn canonicalize(&self, path: TypedPath<'_>) -> Result<TypedPathBuf, IoError> {
        let root = std::fs::canonicalize(self.root)?;
        let path = std::fs::canonicalize(self.resolve(path)?)?;
        self.relative(&path, &root)
    }

    fn exists(&self, path: TypedPath<'_>) -> Result<bool, IoError> {
        match std::fs::metadata(self.resolve(path)?) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == IoErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }
}

fn push_native_component(path: &mut PathBuf, name: &[u8]) -> Result<(), IoError> {
    #[cfg(unix)]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        path.push(OsStr::from_bytes(name));
    }
    #[cfg(windows)]
    {
        let name = std::str::from_utf8(name)
            .map_err(|_| IoError::new(IoErrorKind::InvalidData, "OneNote path is not UTF-8"))?;
        if is_windows_device_name(name) {
            return Err(IoError::new(
                IoErrorKind::InvalidInput,
                "OneNote path names a reserved Windows device",
            ));
        }
        path.push(name);
    }
    Ok(())
}

fn native_typed_path(path: &Path) -> Result<TypedPathBuf, IoError> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(TypedPath::unix(path.as_os_str().as_bytes()).to_path_buf())
    }
    #[cfg(windows)]
    {
        let path = path
            .to_str()
            .ok_or_else(|| IoError::new(IoErrorKind::InvalidData, "OneNote path is not UTF-8"))?;
        Ok(TypedPath::windows(path).to_path_buf())
    }
}

#[cfg(windows)]
fn is_windows_device_name(name: &str) -> bool {
    let stem = name
        .trim_end_matches(['.', ' '])
        .split('.')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|n| matches!(n, "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
}

fn extract_zip(path: &Path, destination: &Path) -> Result<(), ImportError> {
    let file = File::open(path)
        .map_err(|e| ImportError::new(format!("could not open ZIP package: {e}")))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| ImportError::new(format!("invalid ZIP package: {e}")))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(ImportError::new(
            "the ZIP package contains too many entries",
        ));
    }
    let mut expanded = 0u64;
    for index in 0..archive.len() {
        let mut member = archive
            .by_index(index)
            .map_err(|e| ImportError::new(format!("could not read ZIP entry: {e}")))?;
        expanded = expanded.saturating_add(member.size());
        if expanded > MAX_ARCHIVE_BYTES {
            return Err(ImportError::new(
                "the expanded ZIP package is larger than 8 GiB",
            ));
        }
        let Some(relative) = member.enclosed_name() else {
            return Err(ImportError::new("the ZIP package contains an unsafe path"));
        };
        let output = destination.join(relative);
        if member.is_dir() {
            std::fs::create_dir_all(&output)
                .map_err(|e| ImportError::new(format!("could not create ZIP directory: {e}")))?;
            continue;
        }
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ImportError::new(format!("could not create ZIP directory: {e}")))?;
        }
        let mut out = File::create(&output)
            .map_err(|e| ImportError::new(format!("could not extract ZIP entry: {e}")))?;
        std::io::copy(&mut member, &mut out)
            .map_err(|e| ImportError::new(format!("could not extract ZIP entry: {e}")))?;
    }
    Ok(())
}

fn find_sources(root: &Path) -> Result<Vec<PathBuf>, ImportError> {
    let mut toc = Vec::new();
    let mut sections = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 16 {
            return Err(ImportError::new("the ZIP package is nested too deeply"));
        }
        for entry in std::fs::read_dir(&dir)
            .map_err(|e| ImportError::new(format!("could not inspect ZIP contents: {e}")))?
        {
            let path = entry
                .map_err(|e| ImportError::new(format!("could not inspect ZIP contents: {e}")))?
                .path();
            if path.is_dir() {
                stack.push((path, depth + 1));
            } else if extension(&path) == "onetoc2" {
                toc.push(path);
            } else if extension(&path) == "one" {
                sections.push(path);
            }
        }
    }
    toc.sort();
    sections.sort();
    if !toc.is_empty() {
        Ok(toc)
    } else if !sections.is_empty() {
        Ok(sections)
    } else {
        Err(ImportError::new(
            "the ZIP package contains no .onetoc2 or .one files",
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PlannedTarget {
    document: StoreDocumentId,
    node: NodeId,
}

impl PlannedTarget {
    fn new() -> Self {
        Self {
            document: StoreDocumentId::new(),
            node: NodeId::new(),
        }
    }
}

/// A normalized link target is either unique or deliberately ambiguous. Never
/// let a duplicate OneNote id silently select whichever page happened to be
/// traversed last.
#[derive(Default)]
struct TargetIndex {
    targets: HashMap<String, Option<PlannedTarget>>,
}

impl TargetIndex {
    fn insert(&mut self, key: String, target: PlannedTarget) {
        if key.is_empty() {
            return;
        }
        self.targets
            .entry(key)
            .and_modify(|found| {
                if *found != Some(target) {
                    *found = None;
                }
            })
            .or_insert(Some(target));
    }

    fn get(&self, key: &str) -> Option<PlannedTarget> {
        self.targets.get(key).copied().flatten()
    }
}

#[derive(Clone)]
struct PlannedElement {
    content_nodes: Vec<NodeId>,
}

/// First pass: mint every Koru identity before any rich text is converted.
/// Pointer keys are process-local traversal handles only; nothing derived from
/// them enters a snapshot.
struct ImportPlan {
    source_roots: Vec<PlannedTarget>,
    sections: HashMap<usize, PlannedTarget>,
    groups: HashMap<usize, PlannedTarget>,
    pages: HashMap<usize, PlannedTarget>,
    elements: HashMap<usize, PlannedElement>,
    section_links: TargetIndex,
    page_links: TargetIndex,
    object_links: TargetIndex,
}

impl ImportPlan {
    fn new(sources: &[Source]) -> Self {
        let mut plan = Self {
            source_roots: Vec::with_capacity(sources.len()),
            sections: HashMap::new(),
            groups: HashMap::new(),
            pages: HashMap::new(),
            elements: HashMap::new(),
            section_links: TargetIndex::default(),
            page_links: TargetIndex::default(),
            object_links: TargetIndex::default(),
        };
        for source in sources {
            let root = PlannedTarget::new();
            plan.source_roots.push(root);
            match &source.parsed {
                ParsedSource::Notebook(notebook) => {
                    plan.plan_section_entries(notebook.entries());
                }
                ParsedSource::Section(section) => {
                    plan.sections.insert(address(section), root);
                    plan.index_section(section, root);
                    plan.plan_pages(section);
                }
            }
        }
        plan
    }

    fn plan_section_entries(&mut self, entries: &[SectionEntry]) {
        for entry in entries {
            match entry {
                SectionEntry::Section(section) => {
                    let target = PlannedTarget::new();
                    self.sections.insert(address(section), target);
                    self.index_section(section, target);
                    self.plan_pages(section);
                }
                SectionEntry::SectionGroup(group) => {
                    self.groups.insert(address(group), PlannedTarget::new());
                    self.plan_section_entries(group.entries());
                }
            }
        }
    }

    fn index_section(&mut self, section: &Section, target: PlannedTarget) {
        for key in [section.source_file_name(), section.display_name()] {
            let normalized = normalize_section_key(key);
            self.section_links.insert(normalized.clone(), target);
            self.section_links
                .insert(normalized.trim_end_matches(".one").to_owned(), target);
        }
    }

    fn plan_pages(&mut self, section: &Section) {
        for page in section
            .page_series()
            .iter()
            .flat_map(|series| series.pages())
        {
            let target = PlannedTarget::new();
            self.pages.insert(address(page), target);
            let page_key = normalize_link_id(page.link_target_id());
            self.page_links.insert(page_key.clone(), target);
            for content in page.contents() {
                if let PageContent::Outline(outline) = content {
                    self.plan_outline(&page_key, outline, target);
                }
            }
        }
    }

    fn plan_outline(&mut self, page_key: &str, outline: &Outline, page: PlannedTarget) -> NodeId {
        let mut first = None;
        for item in outline.items() {
            let node = self.plan_outline_item(page_key, item, page);
            first.get_or_insert(node);
        }
        let first = first.unwrap_or(page.node);
        self.index_object(page_key, outline.link_target_id(), page, first);
        first
    }

    fn plan_outline_item(
        &mut self,
        page_key: &str,
        item: &OutlineItem,
        page: PlannedTarget,
    ) -> NodeId {
        match item {
            OutlineItem::Group(group) => {
                let mut first = None;
                for item in group.outlines() {
                    let node = self.plan_outline_item(page_key, item, page);
                    first.get_or_insert(node);
                }
                let first = first.unwrap_or(page.node);
                self.index_object(page_key, group.link_target_id(), page, first);
                first
            }
            OutlineItem::Element(element) => {
                let count = element.contents().len().max(1);
                let content_nodes = (0..count).map(|_| NodeId::new()).collect::<Vec<_>>();
                let first = content_nodes[0];
                self.elements.insert(
                    address(element),
                    PlannedElement {
                        content_nodes: content_nodes.clone(),
                    },
                );
                self.index_object(page_key, element.link_target_id(), page, first);
                for (content, node) in element.contents().iter().zip(content_nodes) {
                    if let Content::Table(table) = content {
                        for nested in table
                            .contents()
                            .iter()
                            .flat_map(|row| row.contents())
                            .flat_map(|cell| cell.contents())
                        {
                            self.index_flattened_element(page_key, nested, page, node);
                        }
                    }
                }
                for child in element.children() {
                    self.plan_outline_item(page_key, child, page);
                }
                first
            }
        }
    }

    fn index_flattened_element(
        &mut self,
        page_key: &str,
        element: &OutlineElement,
        page: PlannedTarget,
        table_node: NodeId,
    ) {
        self.index_object(page_key, element.link_target_id(), page, table_node);
        for child in element.children() {
            self.index_flattened_item(page_key, child, page, table_node);
        }
    }

    fn index_flattened_item(
        &mut self,
        page_key: &str,
        item: &OutlineItem,
        page: PlannedTarget,
        table_node: NodeId,
    ) {
        match item {
            OutlineItem::Group(group) => {
                self.index_object(page_key, group.link_target_id(), page, table_node);
                for child in group.outlines() {
                    self.index_flattened_item(page_key, child, page, table_node);
                }
            }
            OutlineItem::Element(element) => {
                self.index_flattened_element(page_key, element, page, table_node)
            }
        }
    }

    fn index_object(&mut self, page_key: &str, object_id: &str, page: PlannedTarget, node: NodeId) {
        self.object_links.insert(
            object_key(page_key, object_id),
            PlannedTarget {
                document: page.document,
                node,
            },
        );
    }
}

fn address<T>(value: &T) -> usize {
    value as *const T as usize
}

struct Build<'a> {
    workspace: koru_core::WorkspaceId,
    context: &'a ImportContext<'a>,
    plan: ImportPlan,
    entries: Vec<ImportedDocument>,
    files: Vec<ImportedDocument>,
    file_by_hash: HashMap<String, (StoreDocumentId, NodeId)>,
    unresolved: HashSet<String>,
    warnings: Vec<ImportWarning>,
}

impl Build<'_> {
    fn source(&mut self, index: usize, source: &Source) -> Result<(), ImportError> {
        let target = self.plan.source_roots[index];
        let result = (|| match &source.parsed {
            ParsedSource::Notebook(notebook) => {
                self.empty_document(target, &source.path, &source.title, None)?;
                self.section_entries(&source.path, notebook.entries(), Some(target.document))
            }
            ParsedSource::Section(section) => {
                self.empty_document(target, &source.path, section.display_name(), None)?;
                self.section_pages(&source.path, section, target.document)
            }
        })();
        result.map_err(|error| ImportError::new(format!("{}: {error}", source.path.display())))
    }

    fn section_entries(
        &mut self,
        source: &Path,
        entries: &[SectionEntry],
        parent: Option<StoreDocumentId>,
    ) -> Result<(), ImportError> {
        for entry in entries {
            match entry {
                SectionEntry::Section(section) => {
                    let target = self.plan.sections[&address(section)];
                    self.empty_document(target, source, section.display_name(), parent)?;
                    self.section_pages(source, section, target.document)?;
                }
                SectionEntry::SectionGroup(group) => {
                    let target = self.plan.groups[&address(group)];
                    self.empty_document(target, source, group.display_name(), parent)?;
                    self.section_entries(source, group.entries(), Some(target.document))?;
                }
            }
        }
        Ok(())
    }

    fn section_pages(
        &mut self,
        source: &Path,
        section: &Section,
        section_id: StoreDocumentId,
    ) -> Result<(), ImportError> {
        for series in section.page_series() {
            let mut levels: Vec<(i32, StoreDocumentId)> = Vec::new();
            for page in series.pages() {
                while levels
                    .last()
                    .is_some_and(|(level, _)| *level >= page.level())
                {
                    levels.pop();
                }
                let parent = levels.last().map(|(_, id)| *id).unwrap_or(section_id);
                let title = page_title(page);
                let target = self.plan.pages[&address(page)];
                self.page(source, page, target, parent).map_err(|error| {
                    ImportError::new(format!(
                        "section \"{}\", page \"{title}\": {error}",
                        section.display_name()
                    ))
                })?;
                levels.push((page.level(), target.document));
            }
        }
        Ok(())
    }

    fn empty_document(
        &mut self,
        target: PlannedTarget,
        source: &Path,
        title: &str,
        parent: Option<StoreDocumentId>,
    ) -> Result<(), ImportError> {
        let mut doc = WorkspaceDoc::create(self.workspace, title).map_err(import_failed)?;
        doc.insert(
            Parent::Root,
            Node::document(title)
                .map_err(import_failed)?
                .with_id(target.node),
        )
        .map_err(import_failed)?;
        self.entries.push(ImportedDocument {
            id: target.document,
            source: source.to_path_buf(),
            title: title.to_owned(),
            kind: koru_core::explore::KIND_DOCUMENT.to_owned(),
            mime: None,
            parent,
            trashed: false,
            trashed_at: None,
            snapshot: doc.export_snapshot().map_err(import_failed)?,
        });
        Ok(())
    }

    fn page(
        &mut self,
        source: &Path,
        page: &Page,
        target: PlannedTarget,
        parent: StoreDocumentId,
    ) -> Result<(), ImportError> {
        let title = page_title(page);
        let mut doc = WorkspaceDoc::create(self.workspace, title).map_err(import_failed)?;
        let root = doc
            .insert(
                Parent::Root,
                Node::document(title)
                    .map_err(import_failed)?
                    .with_id(target.node),
            )
            .map_err(import_failed)?;
        let page_key = normalize_link_id(page.link_target_id());
        let mut ordered: Vec<_> = page.contents().iter().enumerate().collect();
        ordered.sort_by(|(ai, a), (bi, b)| {
            let (ay, ax) = content_position(a);
            let (by, bx) = content_position(b);
            ay.total_cmp(&by).then(ax.total_cmp(&bx)).then(ai.cmp(bi))
        });
        for (_, content) in ordered {
            self.page_content(source, &mut doc, root, &page_key, title, content)?;
        }
        self.entries.push(ImportedDocument {
            id: target.document,
            source: source.to_path_buf(),
            title: title.to_owned(),
            kind: koru_core::explore::KIND_DOCUMENT.to_owned(),
            mime: None,
            parent: Some(parent),
            trashed: false,
            trashed_at: None,
            snapshot: doc.export_snapshot().map_err(import_failed)?,
        });
        Ok(())
    }

    fn page_content(
        &mut self,
        source: &Path,
        doc: &mut WorkspaceDoc,
        root: NodeId,
        page_key: &str,
        page_title: &str,
        content: &PageContent,
    ) -> Result<(), ImportError> {
        match content {
            PageContent::Outline(outline) => {
                self.outline(source, doc, root, page_key, page_title, outline)
            }
            PageContent::Image(image) => {
                self.image(source, doc, root, image, None)?;
                Ok(())
            }
            PageContent::EmbeddedFile(file) => {
                self.attachment(source, doc, root, file, None)?;
                Ok(())
            }
            PageContent::Ink(_) => {
                doc.insert(
                    Parent::Node(root),
                    Node::paragraph("Handwritten content").map_err(import_failed)?,
                )
                .map_err(import_failed)?;
                self.warn(
                    source,
                    "handwriting was preserved as a placeholder; page OCR was unavailable",
                );
                Ok(())
            }
            PageContent::Unknown => {
                self.warn(source, "OneNote contained an unsupported page object");
                Ok(())
            }
        }
    }

    fn outline(
        &mut self,
        source: &Path,
        doc: &mut WorkspaceDoc,
        parent: NodeId,
        page_key: &str,
        page_title: &str,
        outline: &Outline,
    ) -> Result<(), ImportError> {
        for item in outline.items() {
            self.outline_item(source, doc, parent, page_key, page_title, item)?;
        }
        Ok(())
    }

    fn outline_item(
        &mut self,
        source: &Path,
        doc: &mut WorkspaceDoc,
        parent: NodeId,
        page_key: &str,
        page_title: &str,
        item: &OutlineItem,
    ) -> Result<(), ImportError> {
        match item {
            OutlineItem::Group(group) => {
                for item in group.outlines() {
                    self.outline_item(source, doc, parent, page_key, page_title, item)?;
                }
            }
            OutlineItem::Element(element) => {
                let planned = self.plan.elements[&address(element)].clone();
                let mut structural_parent = parent;
                if element.contents().is_empty() {
                    doc.insert(
                        Parent::Node(parent),
                        Node::paragraph("")
                            .map_err(import_failed)?
                            .with_id(planned.content_nodes[0]),
                    )
                    .map_err(import_failed)?;
                }
                for (content, node_id) in element
                    .contents()
                    .iter()
                    .zip(planned.content_nodes.iter().copied())
                {
                    match content {
                        Content::RichText(text) => {
                            let rich = converted_rich_text(text);
                            let inserted = doc
                                .insert(
                                    Parent::Node(parent),
                                    Node::paragraph(&rich.text)
                                        .map_err(import_failed)?
                                        .with_id(node_id),
                                )
                                .map_err(import_failed)?;
                            self.decorate_links(
                                source, doc, inserted, page_key, page_title, &rich,
                            )?;
                            structural_parent =
                                nested_content_parent(structural_parent, inserted, Kind::Paragraph);
                            if !text.math_inline_objects().is_empty() {
                                let attr = Attr::new(
                                    "equation-source",
                                    AttrValue::Text(text.text().to_owned()),
                                );
                                doc.set_attr(inserted, &attr).map_err(import_failed)?;
                                self.warn(source, "an equation was preserved as source text; rendered equation import is unavailable");
                            }
                        }
                        Content::Table(table) => {
                            let markdown = table_markdown(table);
                            let inserted = doc
                                .insert(
                                    Parent::Node(parent),
                                    Node::table(markdown)
                                        .map_err(import_failed)?
                                        .with_id(node_id),
                                )
                                .map_err(import_failed)?;
                            // A OneNote outline element may contain both a
                            // table and nested outline items. Koru tables are
                            // leaf blocks, so those items stay under the last
                            // structural text row (or the surrounding page)
                            // instead of becoming illegal table children.
                            structural_parent =
                                nested_content_parent(structural_parent, inserted, Kind::Table);
                        }
                        Content::Image(image) => {
                            if self
                                .image(source, doc, parent, image, Some(node_id))?
                                .is_none()
                            {
                                doc.insert(
                                    Parent::Node(parent),
                                    Node::paragraph("Image unavailable")
                                        .map_err(import_failed)?
                                        .with_id(node_id),
                                )
                                .map_err(import_failed)?;
                            }
                        }
                        Content::EmbeddedFile(file) => {
                            self.attachment(source, doc, parent, file, Some(node_id))?;
                        }
                        Content::Ink(_) => {
                            doc.insert(
                                Parent::Node(parent),
                                Node::paragraph("Handwritten content")
                                    .map_err(import_failed)?
                                    .with_id(node_id),
                            )
                            .map_err(import_failed)?;
                            self.warn(source, "embedded handwriting could not be converted");
                        }
                        Content::Unknown => {
                            doc.insert(
                                Parent::Node(parent),
                                Node::paragraph("Unsupported OneNote content")
                                    .map_err(import_failed)?
                                    .with_id(node_id),
                            )
                            .map_err(import_failed)?;
                            self.warn(source, "OneNote contained unsupported outline content");
                        }
                    }
                }
                for child in element.children() {
                    self.outline_item(source, doc, structural_parent, page_key, page_title, child)?;
                }
            }
        }
        Ok(())
    }

    fn image(
        &mut self,
        source: &Path,
        doc: &mut WorkspaceDoc,
        parent: NodeId,
        image: &onenote_parser::contents::Image,
        forced_id: Option<NodeId>,
    ) -> Result<Option<NodeId>, ImportError> {
        let Some(mut reader) = image.read() else {
            self.warn(source, "an image had no available binary data");
            return Ok(None);
        };
        let bytes = read_limited(&mut reader, MAX_ATTACHMENT_BYTES)?;
        let extension = image.extension().unwrap_or("bin");
        let mime = image_mime(extension);
        let name = image.image_filename().map(safe_filename);
        let stored = self.context.blobs.put(bytes, mime, name.as_deref())?;
        let width = image.picture_width().map(half_inches_to_pixels);
        let height = image.picture_height().map(half_inches_to_pixels);
        let mut node = Node::image(BlobHash(stored.hash), stored.mime, width, height)
            .map_err(import_failed)?;
        if let Some(id) = forced_id {
            node = node.with_id(id);
        }
        let node_id = doc
            .insert(Parent::Node(parent), node)
            .map_err(import_failed)?;
        let label = image.alt_text().or(name.as_deref());
        let _ = doc.set_name(node_id, label);
        Ok(Some(node_id))
    }

    fn attachment(
        &mut self,
        source: &Path,
        doc: &mut WorkspaceDoc,
        parent: NodeId,
        file: &EmbeddedFile,
        forced_id: Option<NodeId>,
    ) -> Result<NodeId, ImportError> {
        if file.size() > MAX_ATTACHMENT_BYTES {
            self.warn(
                source,
                &format!(
                    "{} is larger than the 2 GiB attachment limit",
                    file.filename()
                ),
            );
            let mut node = Node::paragraph(format!("Attachment: {}", file.filename()))
                .map_err(import_failed)?;
            if let Some(id) = forced_id {
                node = node.with_id(id);
            }
            return doc
                .insert(Parent::Node(parent), node)
                .map_err(import_failed);
        }
        let mut reader = file.read();
        let bytes = read_limited(&mut reader, MAX_ATTACHMENT_BYTES)?;
        let filename = safe_filename(file.filename());
        let mime = mime_for_name(&filename);
        let stored = self.context.blobs.put(bytes, mime, Some(&filename))?;
        let (file_id, root_id) = if let Some(existing) = self.file_by_hash.get(&stored.hash) {
            *existing
        } else {
            let file_id = StoreDocumentId::new();
            let root_id = NodeId::from_uuid(file_id.as_uuid());
            let mut file_doc =
                WorkspaceDoc::create(self.workspace, &filename).map_err(import_failed)?;
            let mut root = Node::file(
                &filename,
                BlobHash(stored.hash.clone()),
                stored.mime.clone(),
            )
            .map_err(import_failed)?;
            root.id = root_id;
            file_doc.insert(Parent::Root, root).map_err(import_failed)?;
            self.files.push(ImportedDocument {
                id: file_id,
                source: source.to_path_buf(),
                title: filename.clone(),
                kind: koru_core::explore::KIND_FILE.to_owned(),
                mime: Some(stored.mime.clone()),
                parent: None,
                trashed: false,
                trashed_at: None,
                snapshot: file_doc.export_snapshot().map_err(import_failed)?,
            });
            self.file_by_hash.insert(stored.hash, (file_id, root_id));
            (file_id, root_id)
        };
        let mut paragraph_node = Node::paragraph("").map_err(import_failed)?;
        if let Some(id) = forced_id {
            paragraph_node = paragraph_node.with_id(id);
        }
        let paragraph = doc
            .insert(Parent::Node(parent), paragraph_node)
            .map_err(import_failed)?;
        let target = Ref::in_doc(
            root_id,
            DocRef::Document(DocumentId::from_uuid(file_id.as_uuid())),
        );
        let attr = Attr::labelled("attachment", AttrValue::Ref(target), &filename);
        doc.insert_pill(paragraph, 0, &attr)
            .map_err(import_failed)?;
        Ok(paragraph)
    }

    fn decorate_links(
        &mut self,
        source: &Path,
        doc: &mut WorkspaceDoc,
        node: NodeId,
        current_page: &str,
        page_title: &str,
        rich: &ConvertedRichText,
    ) -> Result<(), ImportError> {
        for link in rich.links.iter().rev() {
            let Some(label) = rich.text.get(link.start..link.end) else {
                continue;
            };
            let attr = self.link_attr(source, current_page, page_title, &link.target, label);
            doc.delete_text(node, link.start..link.end)
                .map_err(import_failed)?;
            doc.insert_pill(node, link.start, &attr)
                .map_err(import_failed)?;
        }
        Ok(())
    }

    fn link_attr(
        &mut self,
        source: &Path,
        current_page: &str,
        page_title: &str,
        raw: &str,
        label: &str,
    ) -> Attr {
        let Some(link) = OneNoteLink::parse(raw) else {
            return Attr::labelled("link", AttrValue::Url(raw.to_owned()), label);
        };
        let target = if let Some(object) = link.object.as_deref() {
            let page = link.page.as_deref().unwrap_or(current_page);
            self.plan.object_links.get(&object_key(page, object))
        } else if let Some(page) = link.page.as_deref() {
            self.plan.page_links.get(page)
        } else {
            link.section_candidates()
                .find_map(|key| self.plan.section_links.get(&key))
        };
        if let Some(target) = target {
            return Attr::labelled(
                "document",
                AttrValue::Document(DocumentReference::at(
                    DocumentId::from_uuid(target.document.as_uuid()),
                    target.node,
                )),
                label,
            );
        }

        let key = format!("{}\0{}\0{raw}", source.display(), page_title);
        if self.unresolved.insert(key) {
            self.warn(
                source,
                &format!(
                    "page \"{page_title}\": OneNote link \"{label}\" could not be matched; its original link was preserved ({raw})"
                ),
            );
        }
        Attr::labelled("link", AttrValue::Url(raw.to_owned()), label)
    }

    fn warn(&mut self, source: &Path, message: &str) {
        self.warnings.push(ImportWarning {
            source: source.to_path_buf(),
            code: None,
            arguments: Vec::new(),
            message: message.to_owned(),
        });
    }
}

fn page_title(page: &Page) -> &str {
    page.title_text()
        .filter(|title| !title.trim().is_empty())
        .unwrap_or("Untitled page")
}

/// Choose where OneNote's nested outline items continue. Leaf content such as
/// a table is a sibling in Koru and must never become their parent.
fn nested_content_parent(current: NodeId, inserted: NodeId, kind: Kind) -> NodeId {
    if kind.accepts_children() {
        inserted
    } else {
        current
    }
}

fn preview_source(source: &Source) -> PreviewNode {
    let children = match &source.parsed {
        ParsedSource::Notebook(notebook) => preview_sections(notebook.entries()),
        ParsedSource::Section(section) => vec![preview_section(section)],
    };
    PreviewNode {
        title: source.title.clone(),
        nodes: children.iter().map(|n| n.nodes).sum(),
        children,
        trashed: false,
    }
}

fn preview_sections(entries: &[SectionEntry]) -> Vec<PreviewNode> {
    entries
        .iter()
        .map(|entry| match entry {
            SectionEntry::Section(section) => preview_section(section),
            SectionEntry::SectionGroup(group) => {
                let children = preview_sections(group.entries());
                PreviewNode {
                    title: group.display_name().into(),
                    nodes: children.iter().map(|n| n.nodes).sum(),
                    children,
                    trashed: false,
                }
            }
        })
        .collect()
}

fn preview_section(section: &Section) -> PreviewNode {
    let children: Vec<_> = section
        .page_series()
        .iter()
        .flat_map(|s| s.pages())
        .map(|p| PreviewNode {
            title: p.title_text().unwrap_or("Untitled page").into(),
            children: Vec::new(),
            nodes: p.contents().len() + 1,
            trashed: false,
        })
        .collect();
    PreviewNode {
        title: section.display_name().into(),
        nodes: children.iter().map(|n| n.nodes).sum(),
        children,
        trashed: false,
    }
}

fn count_preview(node: &PreviewNode) -> usize {
    1 + node.children.iter().map(count_preview).sum::<usize>()
}
fn estimate_nodes(source: &Source) -> usize {
    count_preview(&preview_source(source))
}
fn count_images(source: &Source) -> usize {
    fn section(s: &Section) -> usize {
        s.page_series()
            .iter()
            .flat_map(|p| p.pages())
            .map(|p| {
                p.contents()
                    .iter()
                    .filter(|c| matches!(c, PageContent::Image(_)))
                    .count()
            })
            .sum()
    }
    fn entries(e: &[SectionEntry]) -> usize {
        e.iter()
            .map(|e| match e {
                SectionEntry::Section(s) => section(s),
                SectionEntry::SectionGroup(g) => entries(g.entries()),
            })
            .sum()
    }
    match &source.parsed {
        ParsedSource::Notebook(n) => entries(n.entries()),
        ParsedSource::Section(s) => section(s),
    }
}

fn content_position(content: &PageContent) -> (f32, f32) {
    match content {
        PageContent::Outline(v) => (
            v.offset_vertical().unwrap_or(f32::MAX),
            v.offset_horizontal().unwrap_or(f32::MAX),
        ),
        PageContent::Image(v) => (
            v.offset_vertical().unwrap_or(f32::MAX),
            v.offset_horizontal().unwrap_or(f32::MAX),
        ),
        PageContent::EmbeddedFile(v) => (
            v.offset_vertical().unwrap_or(f32::MAX),
            v.offset_horizontal().unwrap_or(f32::MAX),
        ),
        _ => (f32::MAX, f32::MAX),
    }
}

fn table_markdown(table: &onenote_parser::contents::Table) -> String {
    let mut rows: Vec<Vec<String>> = table
        .contents()
        .iter()
        .map(|row| {
            row.contents()
                .iter()
                .map(|cell| {
                    cell.contents()
                        .iter()
                        .flat_map(|e| e.contents())
                        .filter_map(Content::rich_text)
                        .map(|r| visible_text(r.text()))
                        .collect::<Vec<_>>()
                        .join(" ")
                        .replace('|', "\\|")
                })
                .collect()
        })
        .collect();
    let cols = rows.iter().map(Vec::len).max().unwrap_or(1);
    for row in &mut rows {
        row.resize(cols, String::new());
    }
    if rows.is_empty() {
        rows.push(vec![String::new(); cols]);
    }
    let mut out = String::new();
    out.push_str(&format!("| {} |\n", rows[0].join(" | ")));
    out.push_str(&format!("| {} |\n", vec!["---"; cols].join(" | ")));
    for row in rows.iter().skip(1) {
        out.push_str(&format!("| {} |\n", row.join(" | ")));
    }
    out
}

const HYPERLINK_MARKER: &str = "\u{fddf}HYPERLINK \"";

#[derive(Clone, Debug, PartialEq, Eq)]
struct ConvertedLink {
    start: usize,
    end: usize,
    target: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ConvertedRichText {
    text: String,
    links: Vec<ConvertedLink>,
}

#[derive(Clone, Debug)]
struct RawLink {
    start: u32,
    end: u32,
    target: String,
}

fn converted_rich_text(text: &onenote_parser::contents::RichText) -> ConvertedRichText {
    let links = text
        .hyperlinks()
        .into_iter()
        .map(|link| RawLink {
            start: link.start(),
            end: link.end(),
            target: link.target().to_owned(),
        })
        .collect();
    convert_rich_text(text.text(), links)
}

/// Remove OneNote's hidden hyperlink fields while translating their UTF-16
/// offsets to the UTF-8 byte offsets used by Koru.
fn convert_rich_text(text: &str, links: Vec<RawLink>) -> ConvertedRichText {
    let utf16_len = text.encode_utf16().count();
    let mut boundaries = vec![0usize; utf16_len + 1];
    let mut out = String::new();
    let mut byte = 0usize;
    let mut utf16 = 0usize;

    while byte < text.len() {
        if text[byte..].starts_with(HYPERLINK_MARKER) {
            let target_start = byte + HYPERLINK_MARKER.len();
            if let Some(relative_end) = text[target_start..].find('"') {
                let marker_end = target_start + relative_end + 1;
                for ch in text[byte..marker_end].chars() {
                    boundaries[utf16] = out.len();
                    utf16 += ch.len_utf16();
                    boundaries[utf16] = out.len();
                }
                byte = marker_end;
                continue;
            }
        }

        let ch = text[byte..]
            .chars()
            .next()
            .expect("byte is a char boundary");
        let units = ch.len_utf16();
        let output_start = out.len();
        boundaries[utf16] = output_start;
        match ch {
            '\0' => {}
            '\r' | '\n' => out.push(' '),
            _ => out.push(ch),
        }
        for offset in 1..=units {
            boundaries[utf16 + offset] = if offset == units {
                out.len()
            } else {
                output_start
            };
        }
        utf16 += units;
        byte += ch.len_utf8();
    }

    let leading = out.len() - out.trim_start().len();
    let text = out.trim().to_owned();
    let mut converted = links
        .into_iter()
        .filter_map(|link| {
            let start = *boundaries.get(link.start as usize)?;
            let end = *boundaries.get(link.end as usize)?;
            let start = start.saturating_sub(leading).min(text.len());
            let end = end.saturating_sub(leading).min(text.len());
            (start < end && text.is_char_boundary(start) && text.is_char_boundary(end)).then_some(
                ConvertedLink {
                    start,
                    end,
                    target: link.target,
                },
            )
        })
        .collect::<Vec<_>>();
    converted.sort_by_key(|link| link.start);
    ConvertedRichText {
        text,
        links: converted,
    }
}

fn visible_text(text: &str) -> String {
    convert_rich_text(text, Vec::new()).text
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OneNoteLink {
    section: Option<String>,
    page: Option<String>,
    object: Option<String>,
}

impl OneNoteLink {
    fn parse(raw: &str) -> Option<Self> {
        if raw.len() < "onenote:".len() || !raw[.."onenote:".len()].eq_ignore_ascii_case("onenote:")
        {
            return None;
        }
        let remainder = &raw["onenote:".len()..];
        let (path, fragment) = remainder.split_once('#').unwrap_or((remainder, ""));
        let section = (!path.is_empty()).then(|| normalize_section_key(path));
        let fields = fragment.split('&').collect::<Vec<_>>();
        let mut page = None;
        let mut object = None;
        for (index, field) in fields.iter().enumerate() {
            let Some((name, field_value)) = field.split_once('=') else {
                continue;
            };
            if name.eq_ignore_ascii_case("page-id") {
                page = Some(normalize_link_id(field_value));
            } else if name.eq_ignore_ascii_case("object-id") {
                let mut value = field_value.to_owned();
                if let Some(suffix) = fields.get(index + 1)
                    && !suffix.is_empty()
                    && !suffix.contains('=')
                    && !suffix.eq_ignore_ascii_case("end")
                {
                    value.push('&');
                    value.push_str(suffix);
                }
                object = Some(normalize_link_id(&value));
            }
        }
        Some(Self {
            section,
            page,
            object,
        })
    }

    fn section_candidates(&self) -> impl Iterator<Item = String> + '_ {
        let mut candidates = Vec::new();
        if let Some(section) = &self.section {
            candidates.push(section.clone());
            let stem = section.trim_end_matches(".one").to_owned();
            if stem != *section {
                candidates.push(stem);
            }
        }
        candidates.into_iter()
    }
}

fn object_key(page: &str, object: &str) -> String {
    format!("{}\0{}", normalize_link_id(page), normalize_link_id(object))
}

fn normalize_link_id(raw: &str) -> String {
    raw.chars()
        .filter(|ch| !matches!(ch, '{' | '}') && !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn normalize_section_key(raw: &str) -> String {
    let decoded = percent_decode(raw);
    decoded
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(&decoded)
        .trim()
        .to_lowercase()
}

fn percent_decode(raw: &str) -> String {
    fn hex(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2]))
        {
            out.push(high << 4 | low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn read_limited(reader: &mut dyn Read, limit: u64) -> Result<Vec<u8>, ImportError> {
    let mut bytes = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| ImportError::new(format!("could not read embedded data: {e}")))?;
    if bytes.len() as u64 > limit {
        return Err(ImportError::new("embedded data exceeds the import limit"));
    }
    Ok(bytes)
}

fn safe_filename(raw: &str) -> String {
    let leaf = raw
        .rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or("Attachment");
    let cleaned: String = leaf
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        .take(240)
        .collect();
    if cleaned.trim().is_empty() {
        "Attachment".into()
    } else {
        cleaned
    }
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}
fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("OneNote import")
        .to_owned()
}
fn notebook_title(path: &Path) -> String {
    path.parent()
        .and_then(Path::file_name)
        .and_then(|s| s.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| file_stem(path))
}
fn half_inches_to_pixels(value: f32) -> u32 {
    (value.max(0.0) * 96.0).round().clamp(1.0, u32::MAX as f32) as u32
}
fn image_mime(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        _ => "application/octet-stream",
    }
}
fn mime_for_name(name: &str) -> &'static str {
    match Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "zip" => "application/zip",
        ext => image_mime(ext),
    }
}
fn import_failed(error: impl std::fmt::Display) -> ImportError {
    ImportError::new(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_plan() -> ImportPlan {
        ImportPlan {
            source_roots: Vec::new(),
            sections: HashMap::new(),
            groups: HashMap::new(),
            pages: HashMap::new(),
            elements: HashMap::new(),
            section_links: TargetIndex::default(),
            page_links: TargetIndex::default(),
            object_links: TargetIndex::default(),
        }
    }

    #[test]
    fn untrusted_attachment_names_are_reduced_to_safe_leaf_names() {
        assert_eq!(safe_filename("..\\..\\CON:<report>?.pdf"), "CONreport.pdf");
        assert_eq!(safe_filename(""), "Attachment");
    }

    #[test]
    fn descriptor_accepts_every_supported_container() {
        let SourceRequest::Files { extensions } = DESCRIPTOR.source else {
            panic!("files")
        };
        assert_eq!(extensions, ["one", "onetoc2", "onepkg", "zip"]);
    }

    #[test]
    fn a_table_never_becomes_the_parent_of_nested_outline_items() {
        let surrounding = NodeId::new();
        let paragraph = NodeId::new();
        let table = NodeId::new();

        let structural = nested_content_parent(surrounding, paragraph, Kind::Paragraph);
        assert_eq!(structural, paragraph);
        assert_eq!(
            nested_content_parent(structural, table, Kind::Table),
            paragraph
        );
    }

    #[test]
    fn hidden_hyperlink_fields_are_removed_and_unicode_ranges_become_utf8() {
        let marker = format!("{HYPERLINK_MARKER}onenote:#page-id={{ABC}}&object-id={{DEF}}&E\"");
        let raw = format!("Before {marker}é風景 after");
        let start = raw[..raw.find("é").unwrap()].encode_utf16().count() as u32;
        let end = start + "é風景".encode_utf16().count() as u32;

        let converted = convert_rich_text(
            &raw,
            vec![RawLink {
                start,
                end,
                target: "onenote:#page-id={ABC}&object-id={DEF}&E".into(),
            }],
        );

        assert_eq!(converted.text, "Before é風景 after");
        assert_eq!(
            &converted.text[converted.links[0].start..converted.links[0].end],
            "é風景"
        );
        assert!(!converted.text.contains("HYPERLINK"));
    }

    #[test]
    fn a_paragraph_link_keeps_its_extended_object_id() {
        let parsed = OneNoteLink::parse(
            "onenote:Story%202.one#Title&section-id={AAA}&page-id={BBB}&object-id={CCC}&21&base-path=https://example.test/Notes",
        )
        .unwrap();

        assert_eq!(parsed.section.as_deref(), Some("story 2.one"));
        assert_eq!(parsed.page.as_deref(), Some("bbb"));
        assert_eq!(parsed.object.as_deref(), Some("ccc&21"));
    }

    #[test]
    fn duplicate_source_ids_are_ambiguous_instead_of_last_one_winning() {
        let mut index = TargetIndex::default();
        let first = PlannedTarget::new();
        let second = PlannedTarget::new();
        index.insert("same".into(), first);
        index.insert("same".into(), second);

        assert_eq!(index.get("same"), None);
    }

    #[test]
    fn a_resolved_object_link_becomes_a_routed_koru_block_reference() {
        let store_profile = koru_store_core::ids::ProfileId::new();
        let store_workspace = koru_store_core::ids::WorkspaceId::new();
        let target = PlannedTarget::new();
        let mut plan = empty_plan();
        plan.object_links
            .insert(object_key("page", "object&E"), target);
        let context = ImportContext {
            target_profile: Some(store_profile),
            target_workspace: Some(store_workspace),
            occupied_titles: &std::collections::BTreeSet::new(),
            blobs: &koru_import::NoBlobSink,
            tags: &koru_import::NoTagSink,
        };
        let mut build = Build {
            workspace: koru_core::WorkspaceId::from_uuid(store_workspace.as_uuid()),
            context: &context,
            plan,
            entries: Vec::new(),
            files: Vec::new(),
            file_by_hash: HashMap::new(),
            unresolved: HashSet::new(),
            warnings: Vec::new(),
        };

        let attr = build.link_attr(
            Path::new("Notebook.onepkg"),
            "source-page",
            "Source page",
            "onenote:#page-id={PAGE}&object-id={OBJECT}&E",
            "Target paragraph",
        );
        let AttrValue::Document(reference) = attr.value else {
            panic!("native reference")
        };
        assert_eq!(reference.node, Some(target.node));
        assert_eq!(
            reference.document,
            DocumentId::from_uuid(target.document.as_uuid())
        );
        assert!(build.warnings.is_empty());
    }

    #[test]
    fn an_unresolved_onenote_link_stays_clickable_and_is_reported() {
        let store_profile = koru_store_core::ids::ProfileId::new();
        let store_workspace = koru_store_core::ids::WorkspaceId::new();
        let context = ImportContext {
            target_profile: Some(store_profile),
            target_workspace: Some(store_workspace),
            occupied_titles: &std::collections::BTreeSet::new(),
            blobs: &koru_import::NoBlobSink,
            tags: &koru_import::NoTagSink,
        };
        let mut build = Build {
            workspace: koru_core::WorkspaceId::from_uuid(store_workspace.as_uuid()),
            context: &context,
            plan: empty_plan(),
            entries: Vec::new(),
            files: Vec::new(),
            file_by_hash: HashMap::new(),
            unresolved: HashSet::new(),
            warnings: Vec::new(),
        };
        let raw = "onenote:Missing.one#page-id={NOPE}";

        let attr = build.link_attr(
            Path::new("Notebook.onepkg"),
            "source-page",
            "Source page",
            raw,
            "Missing note",
        );

        assert_eq!(attr.value, AttrValue::Url(raw.into()));
        assert_eq!(build.warnings.len(), 1);
    }

    #[test]
    fn a_native_onepkg_path_reaches_the_cab_parser() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("probe.onepkg");
        std::fs::write(&path, b"not a cabinet").unwrap();

        let mut temporary = Vec::new();
        let error = match inspect_path(&path, &mut temporary) {
            Ok(_) => panic!("invalid package unexpectedly parsed"),
            Err(error) => error.to_string(),
        };
        assert!(
            error.contains("failed to read cabinet"),
            "the native path should be opened before CAB validation: {error}"
        );
    }

    /// **An import restores; it never commands** (SPEC §5, ADR-273).
    ///
    /// OneNote's model has no completion and no deadline of its own, so a page
    /// arriving here must carry no task state at all: a marker written on the
    /// way in would invent a creation reason the notebook never recorded, and a
    /// tick would invent a completion. Nothing above the tree can reopen either,
    /// because an importer's exemption is that the classification entry point
    /// and the six canonical commands are simply not on this path — which this
    /// test reads off the crate itself rather than taking on trust.
    #[test]
    fn an_imported_page_carries_no_task_state_and_invents_none() {
        let store_workspace = koru_store_core::ids::WorkspaceId::new();
        let context = ImportContext {
            target_profile: None,
            target_workspace: Some(store_workspace),
            occupied_titles: &std::collections::BTreeSet::new(),
            blobs: &koru_import::NoBlobSink,
            tags: &koru_import::NoTagSink,
        };
        let mut build = Build {
            workspace: koru_core::WorkspaceId::from_uuid(store_workspace.as_uuid()),
            context: &context,
            plan: empty_plan(),
            entries: Vec::new(),
            files: Vec::new(),
            file_by_hash: HashMap::new(),
            unresolved: HashSet::new(),
            warnings: Vec::new(),
        };

        build
            .empty_document(
                PlannedTarget::new(),
                Path::new("Notebook.onepkg"),
                "Shipping",
                None,
            )
            .unwrap();

        let doc = WorkspaceDoc::from_snapshot(&build.entries[0].snapshot).unwrap();
        let nodes = doc.walk_all().unwrap();
        assert!(!nodes.is_empty(), "the page produced a document");
        for (node, _) in nodes {
            let read = doc.read(node).unwrap();
            assert!(!read.meta.done, "no tick was invented");
            assert!(
                !koru_core::tasks::is_task_node(&doc, node).unwrap(),
                "and no marker and no due either"
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
