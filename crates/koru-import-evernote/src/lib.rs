// SPDX-License-Identifier: MPL-2.0

//! Offline Evernote ENEX import.
//!
//! Inspection plans identities and source metadata without retaining ENML or
//! resource bytes. Build re-reads each source through an isolated
//! [`SourceStage`], so a source-level security rejection can discard every
//! document from that file while later selected files continue.

mod debug;
mod enex;
mod enml;
mod resources;
mod stage;

pub use enex::{ExternalEntityFetcher, NoExternalEntityFetcher, XmlSecurityError, validate_xml};
pub use stage::{CommittedSource, SourceStage};

use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use enex::{ParsedSource, SourceOutcome, parse_enex, parse_enex_content};
use enml::{EnmlError, LinkContext, NoteResource, convert_note_with_resources, validate_note};
use koru_core::TagId;
use koru_core::explore::KIND_DOCUMENT;
use koru_core::node::Node;
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_import::{
    IdentityMode, ImportContext, ImportError, ImportPreview, ImportProduct, ImportProgress,
    ImportSource, ImportStage, ImportedContent, ImportedDocument, Importer, ImporterDescriptor,
    PreparedImport, PreviewNode, SourceRequest,
};
use koru_store_core::ids::DocumentId;
use resources::spool_resource_data;

pub static DESCRIPTOR: ImporterDescriptor = ImporterDescriptor {
    id: "evernote",
    name: "Evernote",
    source: SourceRequest::Files {
        extensions: &["enex"],
    },
    menu_name: Some("Evernote"),
    picker_title: "Import an Evernote export (.enex)",
    claims_zip: None,
    roots_import: true,
    dedups_known_files: false,
};

pub struct EvernoteImporter;

#[derive(Clone, Debug)]
struct PlannedNote {
    id: DocumentId,
    ordinal: usize,
    title: String,
    #[allow(dead_code)]
    guid: Option<String>,
    trashed: bool,
    trashed_at: Option<koru_store_core::model::Timestamp>,
}

#[derive(Clone, Debug)]
struct PlannedSource {
    #[allow(dead_code)]
    file_index: usize,
    path: PathBuf,
    container_id: DocumentId,
    stem: String,
    notes: Vec<PlannedNote>,
    outcome: SourceOutcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GuidOccurrence {
    file_index: usize,
    document: DocumentId,
}

#[derive(Clone, Debug, Default)]
struct GuidIndex(HashMap<String, Vec<GuidOccurrence>>);

impl GuidIndex {
    fn add(&mut self, guid: &str, occurrence: GuidOccurrence) {
        self.0.entry(guid.to_owned()).or_default().push(occurrence);
    }

    fn resolve(&self, file_index: usize, guid: &str) -> Option<DocumentId> {
        let occurrences = self.0.get(guid)?;
        let same_source = occurrences
            .iter()
            .filter(|occurrence| occurrence.file_index == file_index)
            .collect::<Vec<_>>();
        match same_source.as_slice() {
            [occurrence] => Some(occurrence.document),
            [] if occurrences.len() == 1 => Some(occurrences[0].document),
            _ => None,
        }
    }

    #[cfg(test)]
    fn occurrences(&self, guid: &str) -> &[GuidOccurrence] {
        self.0.get(guid).map(Vec::as_slice).unwrap_or_default()
    }
}

struct PreparedEvernote {
    sources: Vec<PlannedSource>,
    #[allow(dead_code)]
    guids: GuidIndex,
    preview: ImportPreview,
}

impl Importer for EvernoteImporter {
    fn descriptor(&self) -> &'static ImporterDescriptor {
        &DESCRIPTOR
    }

    fn inspect(
        &self,
        source: ImportSource,
        progress: &dyn ImportProgress,
    ) -> Result<Box<dyn PreparedImport>, ImportError> {
        let ImportSource::Files(paths) = source else {
            return Err(ImportError::new(
                "Evernote import needs one or more .enex files",
            ));
        };
        if paths.is_empty() {
            return Err(ImportError::new(
                "Evernote import needs at least one .enex file",
            ));
        }

        let mut inspected = Vec::with_capacity(paths.len());
        for (file_index, path) in paths.iter().enumerate() {
            if progress.cancelled() {
                return Err(ImportError::cancelled());
            }
            progress.update(ImportStage::Inspecting, file_index, Some(paths.len()));
            let file = File::open(path).map_err(|error| {
                ImportError::new(format!("could not open {}: {error}", path.display()))
            })?;
            inspected.push((
                file_index,
                path.clone(),
                parse_enex(BufReader::new(file), path, &NoExternalEntityFetcher)?,
            ));
        }
        progress.update(ImportStage::Inspecting, paths.len(), Some(paths.len()));
        Ok(Box::new(plan_sources(inspected)))
    }
}

fn plan_sources(inspected: Vec<(usize, PathBuf, ParsedSource)>) -> PreparedEvernote {
    let mut sources = Vec::with_capacity(inspected.len());
    let mut guids = GuidIndex::default();
    let mut preview_documents = Vec::new();
    let mut warning_messages = Vec::new();

    for (file_index, path, parsed) in inspected {
        warning_messages.extend(
            parsed
                .warnings
                .iter()
                .map(|warning| warning.message.clone()),
        );
        let container_id = DocumentId::new();
        let notes = if parsed.outcome == SourceOutcome::SecurityRejected {
            Vec::new()
        } else {
            parsed
                .notes
                .into_iter()
                .map(|note| {
                    let id = DocumentId::new();
                    if let Some(guid) = &note.guid {
                        guids.add(
                            guid,
                            GuidOccurrence {
                                file_index,
                                document: id,
                            },
                        );
                    }
                    PlannedNote {
                        id,
                        ordinal: note.ordinal,
                        title: note.title,
                        guid: note.guid,
                        trashed: note.trashed,
                        trashed_at: note.trashed_at,
                    }
                })
                .collect::<Vec<_>>()
        };
        let stem = sanitized_stem(&path);
        if parsed.outcome != SourceOutcome::SecurityRejected {
            preview_documents.push(PreviewNode {
                title: stem.clone(),
                children: notes
                    .iter()
                    .map(|note| PreviewNode {
                        title: note.title.clone(),
                        children: Vec::new(),
                        nodes: 1,
                        trashed: note.trashed,
                    })
                    .collect(),
                nodes: notes.len() + 1,
                trashed: false,
            });
        }
        sources.push(PlannedSource {
            file_index,
            path,
            container_id,
            stem,
            notes,
            outcome: parsed.outcome,
        });
    }

    let document_count = preview_documents
        .iter()
        .map(|source| source.children.len() + 1)
        .sum();
    PreparedEvernote {
        sources,
        guids,
        preview: ImportPreview {
            native_workspace_id: None,
            workspace_name: "Evernote".into(),
            documents: preview_documents,
            document_count,
            node_count: document_count,
            image_count: 0,
            warnings: warning_messages,
            identity_modes: vec![IdentityMode::Copy],
        },
    }
}

impl PreparedImport for PreparedEvernote {
    fn preview(&self) -> &ImportPreview {
        &self.preview
    }

    fn build(
        self: Box<Self>,
        _options: koru_import::ImportOptions,
        context: &ImportContext<'_>,
        progress: &dyn ImportProgress,
    ) -> Result<ImportProduct, ImportError> {
        let target = context
            .target_workspace
            .ok_or_else(|| ImportError::new("Evernote import needs a target workspace"))?;
        let workspace = koru_core::WorkspaceId::from_uuid(target.as_uuid());
        let titles = allocate_container_titles(&self.sources, context.occupied_titles);
        let mut documents = Vec::new();
        let mut warnings = Vec::new();

        for (index, source) in self.sources.iter().enumerate() {
            if progress.cancelled() {
                return Err(ImportError::cancelled());
            }
            progress.update(ImportStage::Converting, index, Some(self.sources.len()));
            let file = File::open(&source.path).map_err(|error| {
                ImportError::new(format!("could not open {}: {error}", source.path.display()))
            })?;
            commit_body_source(
                source,
                &titles[index],
                BufReader::new(file),
                workspace,
                context.blobs,
                context.tags,
                &self.guids,
                Some(progress),
                &mut documents,
                &mut warnings,
            )?;
        }
        progress.update(
            ImportStage::BuildingBundle,
            self.sources.len(),
            Some(self.sources.len()),
        );
        Ok(ImportProduct::Documents(ImportedContent {
            documents,
            warnings,
        }))
    }
}

fn commit_body_source<R: std::io::BufRead + Seek>(
    plan: &PlannedSource,
    container_title: &str,
    input: R,
    workspace: koru_core::WorkspaceId,
    blobs: &dyn koru_import::BlobSink,
    tags: &dyn koru_import::TagSink,
    guids: &GuidIndex,
    progress: Option<&dyn ImportProgress>,
    documents: &mut Vec<ImportedDocument>,
    warnings: &mut Vec<koru_import::ImportWarning>,
) -> Result<(), ImportError> {
    if progress.is_some_and(ImportProgress::cancelled) {
        return Err(ImportError::cancelled());
    }
    let system_snapshot = tags.snapshot()?;
    let mut stage = SourceStage::new(plan.path.clone(), system_snapshot.as_deref())?;
    stage.push_document(ImportedDocument {
        id: plan.container_id,
        source: plan.path.clone(),
        title: container_title.to_owned(),
        kind: KIND_DOCUMENT.into(),
        mime: None,
        parent: None,
        trashed: false,
        trashed_at: None,
        snapshot: title_snapshot(workspace, container_title)?,
    });

    let mut input = input;
    let mut enml_security_rejected = false;
    let security_pass = parse_enex_content(
        &mut input,
        &plan.path,
        &NoExternalEntityFetcher,
        |parsed_note, content| {
            if progress.is_some_and(ImportProgress::cancelled) {
                return Err(ImportError::cancelled());
            }
            if enml_security_rejected {
                return Ok(());
            }
            match validate_note(content) {
                Ok(()) | Err(EnmlError::Malformed(_)) => {}
                Err(EnmlError::Security) => enml_security_rejected = true,
                Err(EnmlError::Build(error)) => return Err(error),
            }
            let _ = parsed_note;
            Ok(())
        },
    )?;

    if plan.outcome == SourceOutcome::SecurityRejected
        || security_pass.outcome == SourceOutcome::SecurityRejected
        || enml_security_rejected
    {
        warnings.extend(security_pass.warnings);
        if !warnings.iter().any(|warning| {
            warning.source == plan.path
                && warning.code == Some(koru_import::ImportWarningCode::EnexSecurityRejected)
        }) {
            warnings.push(koru_import::ImportWarning {
                source: plan.path.clone(),
                code: Some(koru_import::ImportWarningCode::EnexSecurityRejected),
                arguments: Vec::new(),
                message: "enex security rejected".into(),
            });
        }
        debug_source_warnings(warnings, &plan.path);
        return Ok(());
    }

    input.seek(SeekFrom::Start(0)).map_err(|error| {
        ImportError::new(format!("could not rewind {}: {error}", plan.path.display()))
    })?;
    let resource_data = spool_resource_data(&mut input, &plan.path, progress, stage.spools_mut())?;
    if progress.is_some_and(ImportProgress::cancelled) {
        return Err(ImportError::cancelled());
    }
    input.seek(SeekFrom::Start(0)).map_err(|error| {
        ImportError::new(format!("could not rewind {}: {error}", plan.path.display()))
    })?;

    enml_security_rejected = false;
    let parsed = parse_enex_content(
        &mut input,
        &plan.path,
        &NoExternalEntityFetcher,
        |parsed_note, content| {
            if let Some(progress) = progress {
                progress.update(
                    ImportStage::Converting,
                    parsed_note.ordinal,
                    Some(plan.notes.len()),
                );
            }
            if progress.is_some_and(ImportProgress::cancelled) {
                return Err(ImportError::cancelled());
            }
            if enml_security_rejected {
                return Ok(());
            }
            let Some(note) = plan
                .notes
                .iter()
                .find(|note| note.ordinal == parsed_note.ordinal)
            else {
                return Ok(());
            };
            debug::note(note.ordinal);
            let (note_resources, resource_warnings, invalid_resource) =
                resolved_resources(parsed_note, &resource_data, &plan.path);
            if invalid_resource {
                for warning in resource_warnings {
                    stage.push_warning(warning);
                }
                return Ok(());
            }
            match convert_note_with_resources(
                workspace,
                note.id,
                note.ordinal,
                &note.title,
                content,
                &plan.path,
                &note_resources,
                Some(LinkContext {
                    file_index: plan.file_index,
                    guids,
                }),
            ) {
                Ok(mut converted) => {
                    let has_modern_task = parsed_note.modern_task_payload
                        || converted.warnings.iter().any(|warning| {
                            warning.code
                                == Some(
                                    koru_import::ImportWarningCode::EvernoteTaskPayloadUnsupported,
                                )
                        });
                    if has_modern_task {
                        push_warning_once(
                            &mut converted.warnings,
                            &plan.path,
                            koru_import::ImportWarningCode::EvernoteTaskPayloadUnsupported,
                            vec![(note.ordinal + 1).to_string(), note.title.clone()],
                            "Evernote Task status, due date, reminder, recurrence, flag, and assignee were not imported; visible text was kept",
                        );
                    }
                    if parsed_note.reminder_present {
                        push_warning_once(
                            &mut converted.warnings,
                            &plan.path,
                            koru_import::ImportWarningCode::EvernoteReminderNotMapped,
                            Vec::new(),
                            "Evernote reminder metadata was not mapped",
                        );
                    }
                    let mut tag_ids = Vec::new();
                    for source_tag in &parsed_note.tags {
                        let Some(name) = normalize_source_tag(source_tag) else {
                            continue;
                        };
                        let tag = stage
                            .create_tag(&name, koru_store_core::model::Timestamp::now().millis())?;
                        if !tag_ids.contains(&tag) {
                            tag_ids.push(tag);
                        }
                    }
                    if !tag_ids.is_empty() {
                        converted.snapshot = tag_title_row(&converted.snapshot, &tag_ids)?;
                    }
                    for file in converted.files {
                        stage.push_document(file);
                    }
                    stage.push_document(ImportedDocument {
                        id: note.id,
                        source: plan.path.clone(),
                        title: note.title.clone(),
                        kind: KIND_DOCUMENT.into(),
                        mime: None,
                        parent: Some(plan.container_id),
                        trashed: note.trashed,
                        trashed_at: note.trashed_at,
                        snapshot: converted.snapshot,
                    });
                    for binding in converted.bindings {
                        stage.bind_resource(
                            binding.document,
                            binding.node,
                            binding.spool,
                            binding.mime,
                            binding.filename,
                        );
                    }
                    for warning in resource_warnings {
                        stage.push_warning(warning);
                    }
                    for warning in converted.warnings {
                        stage.push_warning(warning);
                    }
                }
                Err(EnmlError::Security) => enml_security_rejected = true,
                Err(EnmlError::Malformed(message)) => {
                    stage.push_warning(koru_import::ImportWarning {
                        source: plan.path.clone(),
                        code: Some(koru_import::ImportWarningCode::EvernoteUnknownElement),
                        arguments: Vec::new(),
                        message: format!(
                            "note {} had malformed ENML and was omitted: {message}",
                            note.ordinal + 1
                        ),
                    });
                }
                Err(EnmlError::Build(error)) => return Err(error),
            }
            Ok(())
        },
    )?;

    if plan.outcome == SourceOutcome::SecurityRejected
        || parsed.outcome == SourceOutcome::SecurityRejected
        || enml_security_rejected
    {
        warnings.extend(parsed.warnings);
        if !warnings.iter().any(|warning| {
            warning.source == plan.path
                && warning.code == Some(koru_import::ImportWarningCode::EnexSecurityRejected)
        }) {
            warnings.push(koru_import::ImportWarning {
                source: plan.path.clone(),
                code: Some(koru_import::ImportWarningCode::EnexSecurityRejected),
                arguments: Vec::new(),
                message: "enex security rejected".into(),
            });
        }
        debug_source_warnings(warnings, &plan.path);
        return Ok(());
    }

    for warning in parsed.warnings {
        stage.push_warning(warning);
    }
    if progress.is_some_and(ImportProgress::cancelled) {
        return Err(ImportError::cancelled());
    }
    let committed = stage.commit_with_blobs_progress(blobs, progress)?;
    if let Some(system_snapshot) = &committed.system_snapshot {
        tags.publish(system_snapshot, committed.register_system)?;
    }
    for warning in &committed.warnings {
        if let Some(code) = warning.code {
            debug::warning(code);
        }
    }
    documents.extend(committed.documents);
    warnings.extend(committed.warnings);
    Ok(())
}

fn push_warning_once(
    warnings: &mut Vec<koru_import::ImportWarning>,
    source: &Path,
    code: koru_import::ImportWarningCode,
    arguments: Vec<String>,
    message: &str,
) {
    if let Some(warning) = warnings
        .iter_mut()
        .find(|warning| warning.code == Some(code))
    {
        warning.arguments = arguments;
        return;
    }
    warnings.push(koru_import::ImportWarning {
        source: source.to_path_buf(),
        code: Some(code),
        arguments,
        message: message.to_owned(),
    });
}

fn debug_source_warnings(warnings: &[koru_import::ImportWarning], source: &Path) {
    for warning in warnings.iter().filter(|warning| warning.source == source) {
        if let Some(code) = warning.code {
            debug::warning(code);
        }
    }
}

/// Evernote exports may spell the surface `#` and a slash hierarchy. Koru's
/// stored tag name has no `#`; v1 maps a hierarchical source tag to its root,
/// so `#fun/home` creates and assigns `fun` only.
fn normalize_source_tag(raw: &str) -> Option<String> {
    let name = raw.trim().strip_prefix('#').unwrap_or(raw.trim()).trim();
    let root = name.split('/').next().unwrap_or_default().trim();
    (!root.is_empty()).then(|| root.to_owned())
}

fn resolved_resources(
    note: &enex::ParsedNote,
    data: &[resources::ResourceData],
    source: &Path,
) -> (Vec<NoteResource>, Vec<koru_import::ImportWarning>, bool) {
    let mut warnings = Vec::new();
    let mut invalid = false;
    let resources = note
        .resources
        .iter()
        .map(|metadata| {
            let decoded = data.iter().find(|decoded| {
                decoded.note == note.ordinal && decoded.resource == metadata.ordinal
            });
            let encoding_valid = metadata.encoding.eq_ignore_ascii_case("base64");
            if !encoding_valid
                || decoded.is_none_or(|decoded| decoded.invalid || decoded.spool.is_none())
            {
                invalid = true;
                warnings.push(resource_warning(
                    source,
                    koru_import::ImportWarningCode::EvernoteInvalidResource,
                    "Evernote resource data was invalid",
                ));
            }
            let filename = metadata
                .filename
                .as_deref()
                .and_then(safe_resource_filename);
            let declared_mime = if metadata.mime.is_empty() {
                mime_from_filename(filename.as_deref())
                    .unwrap_or("application/octet-stream")
                    .to_owned()
            } else {
                metadata.mime.clone()
            };
            if let Some(detected) = decoded.and_then(|decoded| decoded.detected_mime)
                && !declared_mime.eq_ignore_ascii_case(detected)
            {
                warnings.push(resource_warning(
                    source,
                    koru_import::ImportWarningCode::EvernoteMimeMismatch,
                    "Evernote resource MIME did not match its data",
                ));
            } else if decoded.is_some_and(|decoded| decoded.detected_mime.is_none())
                && mime_has_signature(&declared_mime)
            {
                warnings.push(resource_warning(
                    source,
                    koru_import::ImportWarningCode::EvernoteMimeMismatch,
                    "Evernote resource data did not match its declared MIME",
                ));
            }
            let mime = decoded
                .and_then(|decoded| decoded.detected_mime)
                .map_or(declared_mime, str::to_owned);
            if let Some(extension_mime) = mime_from_filename(filename.as_deref())
                && !mime.eq_ignore_ascii_case(extension_mime)
            {
                warnings.push(resource_warning(
                    source,
                    koru_import::ImportWarningCode::EvernoteMimeMismatch,
                    "Evernote resource MIME did not match its filename",
                ));
            }
            NoteResource {
                ordinal: metadata.ordinal,
                md5: encoding_valid
                    .then(|| decoded.and_then(|decoded| decoded.md5.clone()))
                    .flatten(),
                spool: encoding_valid
                    .then(|| decoded.and_then(|decoded| decoded.spool))
                    .flatten(),
                mime,
                filename,
                attachment: metadata.attachment,
                width: metadata.width,
                height: metadata.height,
            }
        })
        .collect();
    (resources, warnings, invalid)
}

fn resource_warning(
    source: &Path,
    code: koru_import::ImportWarningCode,
    message: &str,
) -> koru_import::ImportWarning {
    koru_import::ImportWarning {
        source: source.to_path_buf(),
        code: Some(code),
        arguments: Vec::new(),
        message: message.to_owned(),
    }
}

fn safe_resource_filename(raw: &str) -> Option<String> {
    let leaf = raw
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())?
        .trim();
    if leaf.is_empty() || matches!(leaf, "." | "..") {
        return None;
    }
    let sanitized = leaf
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
            {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    (!sanitized.trim_matches(['.', ' ']).is_empty()).then_some(sanitized)
}

fn mime_from_filename(filename: Option<&str>) -> Option<&'static str> {
    let extension = filename?.rsplit_once('.')?.1.to_ascii_lowercase();
    match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "pdf" => Some("application/pdf"),
        "txt" => Some("text/plain"),
        "html" | "htm" => Some("text/html"),
        "zip" => Some("application/zip"),
        _ => None,
    }
}

fn mime_has_signature(mime: &str) -> bool {
    matches!(
        mime.to_ascii_lowercase().as_str(),
        "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "application/pdf"
            | "application/zip"
    )
}

fn tag_title_row(snapshot: &[u8], tags: &[TagId]) -> Result<Vec<u8>, ImportError> {
    let mut document = WorkspaceDoc::from_snapshot(snapshot)
        .map_err(|error| ImportError::new(error.to_string()))?;
    let title = document
        .children(Parent::Root)
        .map_err(|error| ImportError::new(error.to_string()))?
        .into_iter()
        .next()
        .ok_or_else(|| ImportError::new("imported note has no title row"))?;
    for tag in tags {
        document
            .add_tag(title, *tag)
            .map_err(|error| ImportError::new(error.to_string()))?;
    }
    document
        .export_snapshot()
        .map_err(|error| ImportError::new(error.to_string()))
}

#[cfg(test)]
fn commit_title_source(
    plan: &PlannedSource,
    container_title: &str,
    parsed: ParsedSource,
    workspace: koru_core::WorkspaceId,
    documents: &mut Vec<ImportedDocument>,
    warnings: &mut Vec<koru_import::ImportWarning>,
) -> Result<(), ImportError> {
    // Start the source transaction before deciding its outcome. A security
    // rejection takes this branch without consuming the stage, so Drop is the
    // only thing that runs and no staged system/document effect can escape.
    let mut stage = SourceStage::new(plan.path.clone(), None)?;
    if plan.outcome == SourceOutcome::SecurityRejected
        || parsed.outcome == SourceOutcome::SecurityRejected
    {
        warnings.extend(parsed.warnings);
        if plan.outcome == SourceOutcome::SecurityRejected
            && !warnings.iter().any(|warning| {
                warning.source == plan.path
                    && warning.code == Some(koru_import::ImportWarningCode::EnexSecurityRejected)
            })
        {
            warnings.push(koru_import::ImportWarning {
                source: plan.path.clone(),
                code: Some(koru_import::ImportWarningCode::EnexSecurityRejected),
                arguments: Vec::new(),
                message: "enex security rejected".into(),
            });
        }
        return Ok(());
    }

    stage.push_document(ImportedDocument {
        id: plan.container_id,
        source: plan.path.clone(),
        title: container_title.to_owned(),
        kind: KIND_DOCUMENT.into(),
        mime: None,
        parent: None,
        trashed: false,
        trashed_at: None,
        snapshot: title_snapshot(workspace, container_title)?,
    });

    let completed: BTreeSet<usize> = parsed.notes.iter().map(|note| note.ordinal).collect();
    for note in &plan.notes {
        if !completed.contains(&note.ordinal) {
            continue;
        }
        stage.push_document(ImportedDocument {
            id: note.id,
            source: plan.path.clone(),
            title: note.title.clone(),
            kind: KIND_DOCUMENT.into(),
            mime: None,
            parent: Some(plan.container_id),
            trashed: note.trashed,
            trashed_at: note.trashed_at,
            snapshot: title_snapshot(workspace, &note.title)?,
        });
    }
    for warning in parsed.warnings {
        stage.push_warning(warning);
    }
    let committed = stage.commit()?;
    documents.extend(committed.documents);
    warnings.extend(committed.warnings);
    Ok(())
}

fn allocate_container_titles(
    sources: &[PlannedSource],
    occupied: &BTreeSet<String>,
) -> Vec<String> {
    let mut used = occupied.clone();
    sources
        .iter()
        .map(|source| {
            let base = source.stem.clone();
            if source.outcome == SourceOutcome::SecurityRejected {
                return base;
            }
            let mut candidate = base.clone();
            let mut suffix = 2usize;
            while used.contains(&candidate) {
                candidate = format!("{base} ({suffix})");
                suffix += 1;
            }
            used.insert(candidate.clone());
            candidate
        })
        .collect()
}

fn sanitized_stem(path: &Path) -> String {
    let raw = path
        .file_stem()
        .map(|stem| stem.to_string_lossy())
        .unwrap_or_default();
    let mut cleaned = String::new();
    let mut separating = false;
    for character in raw.chars() {
        if character.is_control()
            || matches!(
                character,
                '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*'
            )
        {
            separating = true;
        } else {
            if separating && !cleaned.is_empty() && !cleaned.ends_with(' ') {
                cleaned.push(' ');
            }
            separating = false;
            cleaned.push(character);
        }
        if cleaned.chars().count() >= 200 {
            break;
        }
    }
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "Evernote".into()
    } else {
        cleaned.to_owned()
    }
}

fn title_snapshot(workspace: koru_core::WorkspaceId, title: &str) -> Result<Vec<u8>, ImportError> {
    let mut document = WorkspaceDoc::create(workspace, title)
        .map_err(|error| ImportError::new(error.to_string()))?;
    document
        .insert(
            Parent::Root,
            Node::document(title).map_err(|error| ImportError::new(error.to_string()))?,
        )
        .map_err(|error| ImportError::new(error.to_string()))?;
    document
        .export_snapshot()
        .map_err(|error| ImportError::new(error.to_string()))
}

#[cfg(test)]
mod tests;
