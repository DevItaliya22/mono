// SPDX-License-Identifier: MPL-2.0

use std::fmt;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use chrono::NaiveDateTime;
use koru_import::{ImportError, ImportWarning, ImportWarningCode};
use koru_store_core::model::Timestamp;
use quick_xml::Reader;
use quick_xml::errors::IllFormedError;
use quick_xml::events::{BytesStart, Event};

use crate::debug::{ParsePass, ParseScope};

const MAX_XML_DEPTH: usize = 128;
const MAX_ATTRIBUTES: usize = 64;
const MAX_METADATA_FIELD_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceOutcome {
    Accepted,
    Truncated,
    SecurityRejected,
}

#[derive(Clone, Debug)]
pub(crate) struct ParsedNote {
    pub ordinal: usize,
    pub title: String,
    pub guid: Option<String>,
    pub trashed: bool,
    pub trashed_at: Option<Timestamp>,
    #[allow(dead_code)] // planned in inspection now; consumed by the Phase 6 tag mapper
    pub tags: Vec<String>,
    pub resources: Vec<ResourceMetadata>,
    pub reminder_present: bool,
    pub modern_task_payload: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ResourceMetadata {
    pub ordinal: usize,
    pub mime: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub filename: Option<String>,
    pub attachment: bool,
    pub encoding: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ParsedSource {
    pub notes: Vec<ParsedNote>,
    pub warnings: Vec<ImportWarning>,
    pub outcome: SourceOutcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NoteField {
    Title,
    Guid,
    Deleted,
    Active,
    Tag,
    Content,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResourceField {
    Mime,
    Width,
    Height,
    Filename,
    Attachment,
    SourceUrl,
    Timestamp,
    Latitude,
    Longitude,
    Altitude,
    CameraMake,
    CameraModel,
    ClientWillIndex,
    RecoType,
    ApplicationData,
}

#[derive(Default)]
struct ResourceBuilder {
    mime: String,
    width: String,
    height: String,
    filename: String,
    attachment: String,
    encoding: String,
}

impl ResourceBuilder {
    fn field_mut(&mut self, field: ResourceField) -> Option<&mut String> {
        match field {
            ResourceField::Mime => Some(&mut self.mime),
            ResourceField::Width => Some(&mut self.width),
            ResourceField::Height => Some(&mut self.height),
            ResourceField::Filename => Some(&mut self.filename),
            ResourceField::Attachment => Some(&mut self.attachment),
            ResourceField::SourceUrl
            | ResourceField::Timestamp
            | ResourceField::Latitude
            | ResourceField::Longitude
            | ResourceField::Altitude
            | ResourceField::CameraMake
            | ResourceField::CameraModel
            | ResourceField::ClientWillIndex
            | ResourceField::RecoType
            | ResourceField::ApplicationData => None,
        }
    }

    fn finish(self, ordinal: usize) -> ResourceMetadata {
        ResourceMetadata {
            ordinal,
            mime: self.mime.trim().to_ascii_lowercase(),
            width: self.width.trim().parse().ok(),
            height: self.height.trim().parse().ok(),
            filename: (!self.filename.trim().is_empty()).then(|| self.filename.trim().to_owned()),
            attachment: matches!(
                self.attachment.trim().to_ascii_lowercase().as_str(),
                "true" | "1" | "yes"
            ),
            encoding: self.encoding.trim().to_ascii_lowercase(),
        }
    }
}

#[derive(Default)]
struct NoteBuilder {
    title: String,
    guid: String,
    deleted: String,
    deleted_present: bool,
    active: String,
    tags: Vec<String>,
    content: String,
    resources: Vec<ResourceMetadata>,
    reminder_present: bool,
    modern_task_payload: bool,
    application_data: String,
}

impl NoteBuilder {
    fn field_mut(&mut self, field: NoteField) -> &mut String {
        match field {
            NoteField::Title => &mut self.title,
            NoteField::Guid => &mut self.guid,
            NoteField::Deleted => &mut self.deleted,
            NoteField::Active => &mut self.active,
            NoteField::Tag => self.tags.last_mut().expect("tag field was opened"),
            NoteField::Content => &mut self.content,
        }
    }

    fn finish(self, ordinal: usize, source: &Path) -> (ParsedNote, String, Option<ImportWarning>) {
        let deleted = self.deleted.trim();
        let inactive =
            self.active.trim().eq_ignore_ascii_case("false") || self.active.trim() == "0";
        let trashed = self.deleted_present || inactive;
        let (trashed_at, invalid_timestamp) = if deleted.is_empty() {
            (None, false)
        } else {
            match parse_evernote_time(deleted) {
                Some(value) => (Some(value), false),
                None => (None, true),
            }
        };
        let warning = invalid_timestamp.then(|| ImportWarning {
            source: source.to_path_buf(),
            code: Some(ImportWarningCode::InvalidSourceTimestamp),
            arguments: Vec::new(),
            message: "Evernote deleted timestamp is invalid; import time will be used".into(),
        });
        let guid = self.guid.trim().to_ascii_lowercase();
        let content = self.content;
        (
            ParsedNote {
                ordinal,
                title: self.title,
                guid: (!guid.is_empty()).then_some(guid),
                trashed,
                trashed_at,
                tags: self
                    .tags
                    .into_iter()
                    .map(|tag| tag.trim().to_owned())
                    .filter(|tag| !tag.is_empty())
                    .collect(),
                resources: self.resources,
                reminder_present: self.reminder_present,
                modern_task_payload: self.modern_task_payload
                    || contains_task_marker(&self.application_data),
            },
            content,
            warning,
        )
    }
}

pub(crate) fn parse_enex<R: BufRead>(
    input: R,
    source: &Path,
    external_entities: &dyn ExternalEntityFetcher,
) -> Result<ParsedSource, ImportError> {
    // The capability lives at this seam so tests prove it remains unused.
    let _external_entities = external_entities;
    // ENML is XML escaped inside `<content>`. Inspection needs none of it, so
    // remove that field before quick-xml sees it. Otherwise one Text event
    // grows quick-xml's scratch buffer to the complete note body even though
    // this pass retains no body data.
    let input = BufReader::with_capacity(64 * 1024, ElementElider::new(input, b"content"));
    let input = BufReader::with_capacity(64 * 1024, ElementElider::new(input, b"data"));
    parse_enex_reader(
        input,
        source,
        external_entities,
        ParsePass::Inspection,
        &mut |_, _| Ok(()),
    )
}

/// Build-pass parser. Unlike inspection this preserves one note's ENML long
/// enough for the caller to convert it, while the outer ENEX still streams.
pub(crate) fn parse_enex_content<R: BufRead, F>(
    input: R,
    source: &Path,
    external_entities: &dyn ExternalEntityFetcher,
    mut on_note: F,
) -> Result<ParsedSource, ImportError>
where
    F: FnMut(&ParsedNote, &str) -> Result<(), ImportError>,
{
    // Resource payloads are a later phase and can be gigabytes. Keep this
    // body-conversion pass bounded by removing `<data>` before quick-xml can
    // grow one event buffer to the whole Base64 field.
    let input = BufReader::with_capacity(64 * 1024, ElementElider::new(input, b"data"));
    parse_enex_reader(
        input,
        source,
        external_entities,
        ParsePass::Content,
        &mut on_note,
    )
}

fn parse_enex_reader<R: BufRead, F>(
    input: R,
    source: &Path,
    external_entities: &dyn ExternalEntityFetcher,
    pass: ParsePass,
    on_note: &mut F,
) -> Result<ParsedSource, ImportError>
where
    F: FnMut(&ParsedNote, &str) -> Result<(), ImportError>,
{
    let _external_entities = external_entities;
    let mut reader = Reader::from_reader(input);
    reader.config_mut().check_end_names = true;
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut note_depth = None;
    let mut field: Option<(NoteField, usize)> = None;
    let mut resource_depth = None;
    let mut resource_attributes_depth = None;
    let mut resource_field: Option<(ResourceField, usize)> = None;
    let mut resource = None::<ResourceBuilder>;
    let mut note = None::<NoteBuilder>;
    let mut note_attributes_depth = None;
    let mut application_data_depth = None;
    let mut notes = Vec::new();
    let mut warnings = Vec::new();

    loop {
        let event = match reader.read_event_into(&mut buffer) {
            Ok(event) => event,
            Err(error) => {
                let scope = parser_scope(
                    field.map(|(field, _)| field),
                    resource_field.map(|(field, _)| field),
                    note_depth.is_some(),
                    resource_depth.is_some(),
                    resource_attributes_depth.is_some(),
                    application_data_depth.is_some(),
                );
                let note_ordinal = note_depth.map(|_| notes.len());
                let stream_offset = reader.error_position();
                if scope == ParseScope::ResourceApplicationData
                    && matches!(
                        &error,
                        quick_xml::Error::IllFormed(IllFormedError::UnclosedReference)
                    )
                {
                    crate::debug::parser_recovery(pass, scope, note_ordinal, stream_offset);
                    buffer.clear();
                    continue;
                }
                crate::debug::parser_failure(pass, scope, note_ordinal, stream_offset);
                if is_truncated_error(&error) {
                    warnings.push(warning(source, ImportWarningCode::EnexEnvelopeTruncated));
                    return Ok(ParsedSource {
                        notes,
                        warnings,
                        outcome: SourceOutcome::Truncated,
                    });
                }
                if error.to_string().contains("import security limit") {
                    return Ok(security_rejected(source));
                }
                return Err(ImportError::new(format!(
                    "could not parse {}: {error}",
                    source.display()
                )));
            }
        };

        let security = match &event {
            Event::Start(element) | Event::Empty(element) => validate_attributes(element),
            Event::DocType(doctype) => validate_doctype(doctype.as_ref()),
            Event::GeneralRef(reference) => validate_reference(reference.as_ref()),
            _ => Ok(()),
        };
        if security.is_err() {
            return Ok(security_rejected(source));
        }

        match event {
            Event::Start(element) => {
                depth += 1;
                if depth > MAX_XML_DEPTH {
                    return Ok(security_rejected(source));
                }
                let name = element.local_name();
                if name.as_ref() == b"note" && note_depth.is_none() {
                    note_depth = Some(depth);
                    note = Some(NoteBuilder::default());
                } else if note_depth.is_some()
                    && name.as_ref() == b"note-attributes"
                    && note_attributes_depth.is_none()
                {
                    note_attributes_depth = Some(depth);
                } else if note_attributes_depth.is_some() && is_reminder_field(name.as_ref()) {
                    note.as_mut()
                        .expect("note attributes are inside a note")
                        .reminder_present = true;
                } else if note_attributes_depth.is_some()
                    && name.as_ref() == b"application-data"
                    && application_data_depth.is_none()
                {
                    application_data_depth = Some(depth);
                } else if application_data_depth.is_some()
                    && contains_task_marker(&String::from_utf8_lossy(name.as_ref()))
                {
                    note.as_mut()
                        .expect("application data is inside a note")
                        .modern_task_payload = true;
                } else if note_depth.is_some_and(|at| depth == at + 1)
                    && let Some(next) = note_field(name.as_ref())
                {
                    if next == NoteField::Tag {
                        note.as_mut()
                            .expect("note exists while a note field is open")
                            .tags
                            .push(String::new());
                    } else if next == NoteField::Deleted {
                        note.as_mut()
                            .expect("note exists while a note field is open")
                            .deleted_present = true;
                    }
                    field = Some((next, depth));
                } else if note_depth.is_some()
                    && name.as_ref() == b"resource"
                    && resource_depth.is_none()
                {
                    resource_depth = Some(depth);
                    resource = Some(ResourceBuilder::default());
                } else if resource_depth.is_some_and(|at| depth == at + 1) {
                    if name.as_ref() == b"resource-attributes" {
                        resource_attributes_depth = Some(depth);
                    } else if name.as_ref() == b"data" {
                        if let Some(value) = element
                            .attributes()
                            .with_checks(false)
                            .filter_map(Result::ok)
                            .find(|attribute| attribute.key.local_name().as_ref() == b"encoding")
                        {
                            resource
                                .as_mut()
                                .expect("data is inside a resource")
                                .encoding =
                                String::from_utf8_lossy(value.value.as_ref()).into_owned();
                        }
                    } else if let Some(next) = resource_field_for(name.as_ref(), false) {
                        resource_field = Some((next, depth));
                    }
                } else if resource_attributes_depth.is_some_and(|at| depth == at + 1)
                    && let Some(next) = resource_field_for(name.as_ref(), true)
                {
                    resource_field = Some((next, depth));
                }
            }
            Event::Empty(element) => {
                if depth + 1 > MAX_XML_DEPTH {
                    return Ok(security_rejected(source));
                }
                if note_depth.is_some_and(|at| depth + 1 == at + 1) {
                    match note_field(element.local_name().as_ref()) {
                        Some(NoteField::Tag) => note
                            .as_mut()
                            .expect("note exists while a note field is open")
                            .tags
                            .push(String::new()),
                        Some(NoteField::Deleted) => {
                            note.as_mut()
                                .expect("note exists while a note field is open")
                                .deleted_present = true;
                        }
                        _ => {}
                    }
                }
                if note_attributes_depth.is_some() {
                    let name = element.local_name();
                    if is_reminder_field(name.as_ref()) {
                        note.as_mut()
                            .expect("note attributes are inside a note")
                            .reminder_present = true;
                    }
                    if application_data_depth.is_some()
                        && contains_task_marker(&String::from_utf8_lossy(name.as_ref()))
                    {
                        note.as_mut()
                            .expect("application data is inside a note")
                            .modern_task_payload = true;
                    }
                }
            }
            Event::Text(text) => {
                if let Some((active, _)) = resource_field {
                    let decoded = text
                        .xml10_content()
                        .map_err(|error| ImportError::new(error.to_string()))?;
                    if !append_resource_field(resource.as_mut(), active, decoded.as_ref()) {
                        return Ok(security_rejected(source));
                    }
                    buffer.clear();
                    continue;
                }
                if let Some((active, _)) = field {
                    let decoded = text
                        .xml10_content()
                        .map_err(|error| ImportError::new(error.to_string()))?;
                    if !append_field(note.as_mut(), active, decoded.as_ref()) {
                        return Ok(security_rejected(source));
                    }
                }
                if application_data_depth.is_some() {
                    let decoded = text
                        .xml10_content()
                        .map_err(|error| ImportError::new(error.to_string()))?;
                    let note = note.as_mut().expect("application data is inside a note");
                    if note.application_data.len() + decoded.len() > MAX_METADATA_FIELD_BYTES {
                        return Ok(security_rejected(source));
                    }
                    note.application_data.push_str(decoded.as_ref());
                }
            }
            Event::CData(text) => {
                if let Some((active, _)) = resource_field {
                    let decoded = text
                        .xml10_content()
                        .map_err(|error| ImportError::new(error.to_string()))?;
                    if !append_resource_field(resource.as_mut(), active, decoded.as_ref()) {
                        return Ok(security_rejected(source));
                    }
                    buffer.clear();
                    continue;
                }
                if let Some((active, _)) = field {
                    let decoded = text
                        .xml10_content()
                        .map_err(|error| ImportError::new(error.to_string()))?;
                    if !append_field(note.as_mut(), active, decoded.as_ref()) {
                        return Ok(security_rejected(source));
                    }
                }
                if application_data_depth.is_some() {
                    let decoded = text
                        .xml10_content()
                        .map_err(|error| ImportError::new(error.to_string()))?;
                    let note = note.as_mut().expect("application data is inside a note");
                    if note.application_data.len() + decoded.len() > MAX_METADATA_FIELD_BYTES {
                        return Ok(security_rejected(source));
                    }
                    note.application_data.push_str(decoded.as_ref());
                }
            }
            Event::GeneralRef(reference) => {
                let value = reference_text(reference.as_ref())
                    .ok_or_else(|| ImportError::new("invalid XML character reference"))?;
                if let Some((active, _)) = resource_field {
                    if !append_resource_field(resource.as_mut(), active, &value) {
                        return Ok(security_rejected(source));
                    }
                } else if let Some((active, _)) = field
                    && !append_field(note.as_mut(), active, &value)
                {
                    return Ok(security_rejected(source));
                }
            }
            Event::End(element) => {
                if application_data_depth == Some(depth) {
                    application_data_depth = None;
                }
                if note_attributes_depth == Some(depth) {
                    note_attributes_depth = None;
                }
                if resource_field.is_some_and(|(_, at)| at == depth) {
                    resource_field = None;
                }
                if resource_attributes_depth == Some(depth) {
                    resource_attributes_depth = None;
                }
                if element.local_name().as_ref() == b"resource" && resource_depth == Some(depth) {
                    let builder = resource.take().expect("open resource has a builder");
                    let note = note.as_mut().expect("resource is inside a note");
                    let ordinal = note.resources.len();
                    note.resources.push(builder.finish(ordinal));
                    resource_depth = None;
                    resource_attributes_depth = None;
                    resource_field = None;
                }
                if field.is_some_and(|(_, at)| at == depth) {
                    field = None;
                }
                if element.local_name().as_ref() == b"note" && note_depth == Some(depth) {
                    let builder = note.take().expect("open note has a builder");
                    let (parsed, content, timestamp_warning) = builder.finish(notes.len(), source);
                    on_note(&parsed, &content)?;
                    notes.push(parsed);
                    warnings.extend(timestamp_warning);
                    note_depth = None;
                    note_attributes_depth = None;
                    application_data_depth = None;
                    field = None;
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => {
                if depth != 0 || note_depth.is_some() {
                    warnings.push(warning(source, ImportWarningCode::EnexEnvelopeTruncated));
                    return Ok(ParsedSource {
                        notes,
                        warnings,
                        outcome: SourceOutcome::Truncated,
                    });
                }
                return Ok(ParsedSource {
                    notes,
                    warnings,
                    outcome: SourceOutcome::Accepted,
                });
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn parser_scope(
    field: Option<NoteField>,
    resource_field: Option<ResourceField>,
    in_note: bool,
    in_resource: bool,
    in_resource_attributes: bool,
    in_application_data: bool,
) -> ParseScope {
    if let Some(field) = resource_field {
        return match field {
            ResourceField::Mime => ParseScope::ResourceMime,
            ResourceField::Width => ParseScope::ResourceWidth,
            ResourceField::Height => ParseScope::ResourceHeight,
            ResourceField::Filename => ParseScope::ResourceFilename,
            ResourceField::Attachment => ParseScope::ResourceAttachment,
            ResourceField::SourceUrl => ParseScope::ResourceSourceUrl,
            ResourceField::Timestamp => ParseScope::ResourceTimestamp,
            ResourceField::Latitude => ParseScope::ResourceLatitude,
            ResourceField::Longitude => ParseScope::ResourceLongitude,
            ResourceField::Altitude => ParseScope::ResourceAltitude,
            ResourceField::CameraMake => ParseScope::ResourceCameraMake,
            ResourceField::CameraModel => ParseScope::ResourceCameraModel,
            ResourceField::ClientWillIndex => ParseScope::ResourceClientWillIndex,
            ResourceField::RecoType => ParseScope::ResourceRecoType,
            ResourceField::ApplicationData => ParseScope::ResourceApplicationData,
        };
    }
    if in_resource_attributes {
        return ParseScope::ResourceAttributes;
    }
    if in_resource {
        return ParseScope::Resource;
    }
    if let Some(field) = field {
        return match field {
            NoteField::Title => ParseScope::Title,
            NoteField::Guid => ParseScope::Guid,
            NoteField::Deleted => ParseScope::Deleted,
            NoteField::Active => ParseScope::Active,
            NoteField::Tag => ParseScope::Tag,
            NoteField::Content => ParseScope::Content,
        };
    }
    if in_application_data {
        ParseScope::ApplicationData
    } else if in_note {
        ParseScope::Note
    } else {
        ParseScope::Envelope
    }
}

fn is_reminder_field(name: &[u8]) -> bool {
    name.eq_ignore_ascii_case(b"reminder-time")
        || name.eq_ignore_ascii_case(b"remindertime")
        || name.eq_ignore_ascii_case(b"reminder-order")
        || name.eq_ignore_ascii_case(b"reminderorder")
}

fn contains_task_marker(value: &str) -> bool {
    value.to_ascii_lowercase().contains("task")
}

enum ElideState {
    Text,
    Tag,
    Content,
    ContentTag,
}

/// A bounded adapter that passes the ENEX envelope through while replacing
/// each selected field body with an empty field.
struct ElementElider<R> {
    inner: R,
    element: &'static [u8],
    state: ElideState,
    pending: Vec<u8>,
    pending_at: usize,
    tag: Vec<u8>,
}

impl<R: BufRead> ElementElider<R> {
    fn new(inner: R, element: &'static [u8]) -> Self {
        Self {
            inner,
            element,
            state: ElideState::Text,
            pending: Vec::new(),
            pending_at: 0,
            tag: Vec::new(),
        }
    }

    fn copy_pending(&mut self, output: &mut [u8]) -> usize {
        let available = &self.pending[self.pending_at..];
        let count = available.len().min(output.len());
        output[..count].copy_from_slice(&available[..count]);
        self.pending_at += count;
        if self.pending_at == self.pending.len() {
            self.pending.clear();
            self.pending_at = 0;
        }
        count
    }

    fn finish_tag(&mut self, from_content: bool) {
        let tag = std::mem::take(&mut self.tag);
        if from_content {
            if is_element_end(&tag, self.element) {
                self.pending = tag;
                self.state = ElideState::Text;
            } else {
                self.state = ElideState::Content;
            }
        } else {
            let content_start = is_element_start(&tag, self.element);
            self.pending = tag;
            self.state = if content_start {
                ElideState::Content
            } else {
                ElideState::Text
            };
        }
    }
}

impl<R: BufRead> Read for ElementElider<R> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        loop {
            if !self.pending.is_empty() {
                return Ok(self.copy_pending(output));
            }
            let available = self.inner.fill_buf()?;
            if available.is_empty() {
                return Ok(0);
            }
            match self.state {
                ElideState::Text => {
                    if let Some(position) = available.iter().position(|byte| *byte == b'<') {
                        if position != 0 {
                            let count = position.min(output.len());
                            output[..count].copy_from_slice(&available[..count]);
                            self.inner.consume(count);
                            return Ok(count);
                        }
                        self.tag.clear();
                        self.state = ElideState::Tag;
                    } else {
                        let count = available.len().min(output.len());
                        output[..count].copy_from_slice(&available[..count]);
                        self.inner.consume(count);
                        return Ok(count);
                    }
                }
                ElideState::Content => {
                    if let Some(position) = available.iter().position(|byte| *byte == b'<') {
                        self.inner.consume(position);
                        if position == 0 {
                            self.tag.clear();
                            self.state = ElideState::ContentTag;
                        }
                    } else {
                        let count = available.len();
                        self.inner.consume(count);
                    }
                }
                ElideState::Tag | ElideState::ContentTag => {
                    let from_content = matches!(self.state, ElideState::ContentTag);
                    let count = available
                        .iter()
                        .position(|byte| *byte == b'>')
                        .map(|position| position + 1)
                        .unwrap_or(available.len());
                    self.tag.extend_from_slice(&available[..count]);
                    self.inner.consume(count);
                    if self.tag.len() > MAX_METADATA_FIELD_BYTES {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "XML tag exceeds import security limit",
                        ));
                    }
                    if self.tag.last() == Some(&b'>') {
                        self.finish_tag(from_content);
                    }
                }
            }
        }
    }
}

fn is_element_start(tag: &[u8], element: &[u8]) -> bool {
    let Some(rest) = tag
        .strip_prefix(b"<")
        .and_then(|tag| tag.strip_prefix(element))
    else {
        return false;
    };
    rest.first()
        .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'>')
}

fn is_element_end(tag: &[u8], element: &[u8]) -> bool {
    let Some(rest) = tag
        .strip_prefix(b"</")
        .and_then(|tag| tag.strip_prefix(element))
    else {
        return false;
    };
    rest.iter()
        .all(|byte| byte.is_ascii_whitespace() || *byte == b'>')
        && rest.last() == Some(&b'>')
}

fn note_field(name: &[u8]) -> Option<NoteField> {
    match name {
        b"title" => Some(NoteField::Title),
        b"guid" => Some(NoteField::Guid),
        b"deleted" => Some(NoteField::Deleted),
        b"active" => Some(NoteField::Active),
        b"tag" => Some(NoteField::Tag),
        b"content" => Some(NoteField::Content),
        _ => None,
    }
}

fn resource_field_for(name: &[u8], attributes: bool) -> Option<ResourceField> {
    match (attributes, name) {
        (false, b"mime") => Some(ResourceField::Mime),
        (false, b"width") => Some(ResourceField::Width),
        (false, b"height") => Some(ResourceField::Height),
        (true, b"file-name") => Some(ResourceField::Filename),
        (true, b"attachment") => Some(ResourceField::Attachment),
        (true, b"source-url") => Some(ResourceField::SourceUrl),
        (true, b"timestamp") => Some(ResourceField::Timestamp),
        (true, b"latitude") => Some(ResourceField::Latitude),
        (true, b"longitude") => Some(ResourceField::Longitude),
        (true, b"altitude") => Some(ResourceField::Altitude),
        (true, b"camera-make") => Some(ResourceField::CameraMake),
        (true, b"camera-model") => Some(ResourceField::CameraModel),
        (true, b"client-will-index") => Some(ResourceField::ClientWillIndex),
        (true, b"reco-type") => Some(ResourceField::RecoType),
        (true, b"application-data") => Some(ResourceField::ApplicationData),
        _ => None,
    }
}

fn append_field(note: Option<&mut NoteBuilder>, field: NoteField, text: &str) -> bool {
    let value = note
        .expect("note exists while a note field is open")
        .field_mut(field);
    if field != NoteField::Content
        && value.len().saturating_add(text.len()) > MAX_METADATA_FIELD_BYTES
    {
        return false;
    }
    value.push_str(text);
    true
}

fn append_resource_field(
    resource: Option<&mut ResourceBuilder>,
    field: ResourceField,
    text: &str,
) -> bool {
    let Some(value) = resource
        .expect("resource exists while a resource field is open")
        .field_mut(field)
    else {
        return true;
    };
    if value.len().saturating_add(text.len()) > MAX_METADATA_FIELD_BYTES {
        return false;
    }
    value.push_str(text);
    true
}

fn parse_evernote_time(raw: &str) -> Option<Timestamp> {
    ["%Y%m%dT%H%M%SZ", "%Y%m%dT%H%M%S%.fZ"]
        .into_iter()
        .find_map(|format| NaiveDateTime::parse_from_str(raw, format).ok())
        .map(|time| Timestamp::from_millis(time.and_utc().timestamp_millis()))
}

fn is_truncated_error(error: &quick_xml::Error) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("unexpected eof")
        || message.contains("missing end tag")
        || message.contains("unexpected end of stream")
}

fn warning(source: &Path, code: ImportWarningCode) -> ImportWarning {
    ImportWarning {
        source: source.to_path_buf(),
        code: Some(code),
        arguments: Vec::new(),
        message: code.as_str().replace('_', " "),
    }
}

fn security_rejected(source: &Path) -> ParsedSource {
    ParsedSource {
        notes: Vec::new(),
        warnings: vec![warning(source, ImportWarningCode::EnexSecurityRejected)],
        outcome: SourceOutcome::SecurityRejected,
    }
}

/// A capability an XML parser could use to retrieve an external entity.
///
/// The ENEX parser accepts Evernote's external `SYSTEM` identifiers as inert
/// metadata and never calls this capability. Keeping it explicit makes the
/// no-network boundary observable in tests rather than an assumption about an
/// XML library's defaults.
pub trait ExternalEntityFetcher {
    fn fetch(&self, system_id: &str) -> Result<Vec<u8>, XmlSecurityError>;
}

/// Production policy: an attempted external fetch is a source-security error.
pub struct NoExternalEntityFetcher;

impl ExternalEntityFetcher for NoExternalEntityFetcher {
    fn fetch(&self, system_id: &str) -> Result<Vec<u8>, XmlSecurityError> {
        Err(XmlSecurityError::ExternalFetch(system_id.to_owned()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XmlSecurityError {
    Malformed(String),
    ForbiddenDoctype,
    ForbiddenEntity(String),
    ExternalFetch(String),
}

impl fmt::Display for XmlSecurityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(error) => write!(f, "malformed XML: {error}"),
            Self::ForbiddenDoctype => f.write_str("XML contains a forbidden document type"),
            Self::ForbiddenEntity(name) => write!(f, "XML contains a forbidden entity: {name}"),
            Self::ExternalFetch(system_id) => {
                write!(f, "XML attempted to fetch an external entity: {system_id}")
            }
        }
    }
}

impl std::error::Error for XmlSecurityError {}

/// Validate the ENEX or ENML XML security envelope without resolving a DTD.
///
/// Evernote writes an external `SYSTEM` doctype into normal exports. It is
/// accepted for the `en-export` and `en-note` roots but treated only as an XML
/// event: `external_entities` is intentionally never invoked. Internal DTD
/// subsets and non-built-in entity references reject the complete source.
pub fn validate_xml<R: BufRead>(
    input: R,
    external_entities: &dyn ExternalEntityFetcher,
) -> Result<(), XmlSecurityError> {
    // Hold the capability at the parsing seam so tests can prove that normal
    // Evernote input never exercises it. quick-xml's event reader itself has
    // no external resolver and performs no I/O.
    let _external_entities = external_entities;
    let mut reader = Reader::from_reader(input);
    let mut buffer = Vec::new();

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element) | Event::Empty(element)) => {
                validate_attributes(&element)?;
            }
            Ok(Event::DocType(doctype)) => validate_doctype(doctype.as_ref())?,
            Ok(Event::GeneralRef(reference)) => validate_reference(reference.as_ref())?,
            Ok(Event::Eof) => return Ok(()),
            Ok(_) => {}
            Err(error) => return Err(XmlSecurityError::Malformed(error.to_string())),
        }
        buffer.clear();
    }
}

fn validate_attributes(element: &BytesStart<'_>) -> Result<(), XmlSecurityError> {
    for (index, attribute) in element.attributes().enumerate() {
        if index >= MAX_ATTRIBUTES {
            return Err(XmlSecurityError::ForbiddenDoctype);
        }
        let attribute =
            attribute.map_err(|error| XmlSecurityError::Malformed(error.to_string()))?;
        if attribute.key.as_ref().len() > MAX_METADATA_FIELD_BYTES
            || attribute.value.len() > MAX_METADATA_FIELD_BYTES
        {
            return Err(XmlSecurityError::ForbiddenDoctype);
        }
        validate_references(attribute.value.as_ref())?;
    }
    Ok(())
}

fn validate_doctype(bytes: &[u8]) -> Result<(), XmlSecurityError> {
    if bytes.contains(&b'[') || bytes.contains(&b']') {
        return Err(XmlSecurityError::ForbiddenDoctype);
    }

    let declaration = std::str::from_utf8(bytes)
        .map_err(|error| XmlSecurityError::Malformed(error.to_string()))?;
    let mut parts = declaration.split_ascii_whitespace();
    let Some(root) = parts.next() else {
        return Err(XmlSecurityError::ForbiddenDoctype);
    };
    if root != "en-export" && root != "en-note" {
        return Err(XmlSecurityError::ForbiddenDoctype);
    }
    let Some(kind) = parts.next() else {
        return Err(XmlSecurityError::ForbiddenDoctype);
    };
    if !kind.eq_ignore_ascii_case("SYSTEM") {
        return Err(XmlSecurityError::ForbiddenDoctype);
    }
    let Some(system_id) = parts.next() else {
        return Err(XmlSecurityError::ForbiddenDoctype);
    };
    let quoted = (system_id.starts_with('"') && system_id.ends_with('"'))
        || (system_id.starts_with('\'') && system_id.ends_with('\''));
    if !quoted || system_id.len() < 2 || parts.next().is_some() {
        return Err(XmlSecurityError::ForbiddenDoctype);
    }
    Ok(())
}

fn validate_references(bytes: &[u8]) -> Result<(), XmlSecurityError> {
    let mut remaining = bytes;
    while let Some(start) = remaining.iter().position(|byte| *byte == b'&') {
        remaining = &remaining[start + 1..];
        let Some(end) = remaining.iter().position(|byte| *byte == b';') else {
            return Err(XmlSecurityError::ForbiddenEntity("unterminated".into()));
        };
        validate_reference(&remaining[..end])?;
        remaining = &remaining[end + 1..];
    }
    Ok(())
}

fn validate_reference(reference: &[u8]) -> Result<(), XmlSecurityError> {
    let allowed = matches!(reference, b"amp" | b"lt" | b"gt" | b"apos" | b"quot")
        || valid_character_reference(reference);
    if allowed {
        return Ok(());
    }

    Err(XmlSecurityError::ForbiddenEntity(
        String::from_utf8_lossy(reference).into_owned(),
    ))
}

fn valid_character_reference(reference: &[u8]) -> bool {
    match reference {
        [b'#', b'x' | b'X', digits @ ..] => {
            !digits.is_empty() && digits.iter().all(u8::is_ascii_hexdigit)
        }
        [b'#', digits @ ..] => !digits.is_empty() && digits.iter().all(u8::is_ascii_digit),
        _ => false,
    }
}

fn reference_text(reference: &[u8]) -> Option<String> {
    let builtin = match reference {
        b"amp" => Some('&'),
        b"lt" => Some('<'),
        b"gt" => Some('>'),
        b"apos" => Some('\''),
        b"quot" => Some('"'),
        _ => None,
    };
    if let Some(character) = builtin {
        return Some(character.to_string());
    }
    let number = if let Some(hex) = reference
        .strip_prefix(b"#x")
        .or_else(|| reference.strip_prefix(b"#X"))
    {
        u32::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?
    } else if let Some(decimal) = reference.strip_prefix(b"#") {
        std::str::from_utf8(decimal).ok()?.parse().ok()?
    } else {
        return None;
    };
    char::from_u32(number).map(|character| character.to_string())
}

#[cfg(test)]
mod tests;
