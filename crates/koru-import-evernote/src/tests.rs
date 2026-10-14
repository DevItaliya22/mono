// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeSet;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use koru_import::{BlobSink, ImportWarningCode, StoredBlob, TagSink};

use super::*;

const FIRST: &[u8] = include_bytes!("fixtures/first.enex");
const SECOND: &[u8] = include_bytes!("fixtures/second.enex");
const TRUNCATED: &[u8] = include_bytes!("fixtures/truncated.enex");
const SECURITY_AFTER_NOTE: &[u8] = include_bytes!("fixtures/security-after-note.enex");
const DELETED: &[u8] = include_bytes!("fixtures/deleted.enex");
const BODY_RICH: &[u8] = include_bytes!("fixtures/body-rich.enex");
const BODY_INNER_SECURITY: &[u8] = include_bytes!("fixtures/body-inner-security.enex");
const BODY_MALFORMED_NOTE: &[u8] = include_bytes!("fixtures/body-malformed-note.enex");
const TAGS: &[u8] = include_bytes!("fixtures/tags.enex");
const TAGS_TRUNCATED: &[u8] = include_bytes!("fixtures/tags-truncated.enex");

#[derive(Default)]
struct RecordingTags {
    snapshot: Mutex<Option<Vec<u8>>>,
    publishes: Mutex<usize>,
    rows: Mutex<usize>,
}

#[derive(Default)]
struct RecordingBlobs {
    puts: Mutex<Vec<(Vec<u8>, String, Option<String>)>>,
    returned_hash: String,
}

impl RecordingBlobs {
    fn returning(hash: &str) -> Self {
        Self {
            puts: Mutex::new(Vec::new()),
            returned_hash: hash.to_owned(),
        }
    }
}

impl BlobSink for RecordingBlobs {
    fn put(
        &self,
        bytes: Vec<u8>,
        mime: &str,
        filename: Option<&str>,
    ) -> Result<StoredBlob, ImportError> {
        self.puts
            .lock()
            .unwrap()
            .push((bytes, mime.to_owned(), filename.map(str::to_owned)));
        Ok(StoredBlob {
            hash: self.returned_hash.clone(),
            mime: mime.to_owned(),
        })
    }
}

impl RecordingTags {
    fn with_system(system: &koru_core::SystemDoc) -> Self {
        Self {
            snapshot: Mutex::new(Some(system.export_snapshot().unwrap())),
            publishes: Mutex::new(0),
            rows: Mutex::new(1),
        }
    }

    fn system(&self) -> Option<koru_core::SystemDoc> {
        self.snapshot
            .lock()
            .unwrap()
            .as_deref()
            .map(koru_core::SystemDoc::from_snapshot)
            .transpose()
            .unwrap()
    }

    fn publish_count(&self) -> usize {
        *self.publishes.lock().unwrap()
    }

    fn row_count(&self) -> usize {
        *self.rows.lock().unwrap()
    }
}

impl TagSink for RecordingTags {
    fn snapshot(&self) -> Result<Option<Vec<u8>>, ImportError> {
        Ok(self.snapshot.lock().unwrap().clone())
    }

    fn publish(&self, staged: &[u8], register_system: bool) -> Result<(), ImportError> {
        let mut current = self.snapshot.lock().unwrap();
        let mut system = match current.as_deref() {
            Some(snapshot) => koru_core::SystemDoc::from_snapshot(snapshot),
            None if register_system => {
                *self.rows.lock().unwrap() += 1;
                koru_core::SystemDoc::create()
            }
            None => return Err(ImportError::new("publish without a system row")),
        }
        .map_err(|error| ImportError::new(error.to_string()))?;
        system
            .absorb(staged)
            .map_err(|error| ImportError::new(error.to_string()))?;
        *current = Some(
            system
                .export_snapshot()
                .map_err(|error| ImportError::new(error.to_string()))?,
        );
        *self.publishes.lock().unwrap() += 1;
        Ok(())
    }
}

fn parsed(path: &str, bytes: &[u8]) -> ParsedSource {
    parse_enex(
        Cursor::new(bytes),
        Path::new(path),
        &NoExternalEntityFetcher,
    )
    .expect("parse ENEX")
}

fn planned(inputs: &[(&str, &[u8])]) -> PreparedEvernote {
    plan_sources(
        inputs
            .iter()
            .enumerate()
            .map(|(index, (path, bytes))| (index, PathBuf::from(path), parsed(path, bytes)))
            .collect(),
    )
}

fn build_planned(
    prepared: &PreparedEvernote,
    bytes: &[&[u8]],
    occupied: &BTreeSet<String>,
) -> (Vec<ImportedDocument>, Vec<koru_import::ImportWarning>) {
    let workspace = koru_core::WorkspaceId::new();
    let titles = allocate_container_titles(&prepared.sources, occupied);
    let mut documents = Vec::new();
    let mut warnings = Vec::new();
    for (index, plan) in prepared.sources.iter().enumerate() {
        let current = parse_enex(
            Cursor::new(bytes[index]),
            &plan.path,
            &NoExternalEntityFetcher,
        )
        .unwrap();
        commit_title_source(
            plan,
            &titles[index],
            current,
            workspace,
            &mut documents,
            &mut warnings,
        )
        .unwrap();
    }
    (documents, warnings)
}

fn build_planned_body(
    prepared: &PreparedEvernote,
    bytes: &[&[u8]],
) -> (Vec<ImportedDocument>, Vec<koru_import::ImportWarning>) {
    let workspace = koru_core::WorkspaceId::new();
    let titles = allocate_container_titles(&prepared.sources, &BTreeSet::new());
    let mut documents = Vec::new();
    let mut warnings = Vec::new();
    for (index, plan) in prepared.sources.iter().enumerate() {
        commit_body_source(
            plan,
            &titles[index],
            Cursor::new(bytes[index]),
            workspace,
            &koru_import::NoBlobSink,
            &koru_import::NoTagSink,
            &prepared.guids,
            None,
            &mut documents,
            &mut warnings,
        )
        .unwrap();
    }
    (documents, warnings)
}

fn build_planned_body_with_tags(
    prepared: &PreparedEvernote,
    bytes: &[&[u8]],
    tags: &dyn TagSink,
) -> Result<(Vec<ImportedDocument>, Vec<koru_import::ImportWarning>), ImportError> {
    let workspace = koru_core::WorkspaceId::new();
    let titles = allocate_container_titles(&prepared.sources, &BTreeSet::new());
    let mut documents = Vec::new();
    let mut warnings = Vec::new();
    for (index, plan) in prepared.sources.iter().enumerate() {
        commit_body_source(
            plan,
            &titles[index],
            Cursor::new(bytes[index]),
            workspace,
            &koru_import::NoBlobSink,
            tags,
            &prepared.guids,
            None,
            &mut documents,
            &mut warnings,
        )?;
    }
    Ok((documents, warnings))
}

fn build_resource_enex(
    title: &str,
    enml: &str,
    resources: &[(&[u8], &str, Option<&str>, bool)],
) -> Vec<u8> {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;

    let mut xml =
        format!("<en-export><note><title>{title}</title><content><![CDATA[{enml}]]></content>");
    for (bytes, mime, filename, attachment) in resources {
        xml.push_str("<resource><data encoding=\"base64\">");
        xml.push_str(&STANDARD.encode(bytes));
        xml.push_str("</data><mime>");
        xml.push_str(mime);
        xml.push_str("</mime><resource-attributes>");
        if let Some(filename) = filename {
            xml.push_str("<file-name>");
            xml.push_str(filename);
            xml.push_str("</file-name>");
        }
        if *attachment {
            xml.push_str("<attachment>true</attachment>");
        }
        xml.push_str("</resource-attributes></resource>");
    }
    xml.push_str("</note></en-export>");
    xml.into_bytes()
}

fn build_resource_documents(
    source: &[u8],
    blobs: &dyn BlobSink,
) -> (Vec<ImportedDocument>, Vec<koru_import::ImportWarning>) {
    let prepared = planned(&[("resources.enex", source)]);
    let workspace = koru_core::WorkspaceId::new();
    let mut documents = Vec::new();
    let mut warnings = Vec::new();
    commit_body_source(
        &prepared.sources[0],
        "resources",
        Cursor::new(source),
        workspace,
        blobs,
        &koru_import::NoTagSink,
        &prepared.guids,
        None,
        &mut documents,
        &mut warnings,
    )
    .unwrap();
    (documents, warnings)
}

#[test]
fn two_sources_create_separate_named_containers_and_title_children() {
    let prepared = planned(&[("Work.enex", FIRST), ("Personal.enex", SECOND)]);
    let (documents, warnings) = build_planned(&prepared, &[FIRST, SECOND], &BTreeSet::new());

    assert!(warnings.is_empty());
    assert_eq!(documents.len(), 5);
    let roots: Vec<_> = documents
        .iter()
        .filter(|document| document.parent.is_none())
        .collect();
    assert_eq!(
        roots
            .iter()
            .map(|root| root.title.as_str())
            .collect::<Vec<_>>(),
        ["Work", "Personal"]
    );
    assert_eq!(
        documents
            .iter()
            .filter(|document| document.parent == Some(roots[0].id))
            .map(|document| document.title.as_str())
            .collect::<Vec<_>>(),
        ["First note"]
    );
    assert_eq!(
        documents
            .iter()
            .filter(|document| document.parent == Some(roots[1].id))
            .map(|document| document.title.as_str())
            .collect::<Vec<_>>(),
        ["Second note", "Third note"]
    );
    for document in documents {
        WorkspaceDoc::from_snapshot(&document.snapshot).expect("valid title-only snapshot");
    }
}

#[test]
fn container_titles_suffix_against_workspace_and_this_job() {
    let prepared = planned(&[("Notes.enex", FIRST), ("Notes.enex", SECOND)]);
    let occupied = BTreeSet::from(["Notes".to_owned(), "Notes (2)".to_owned()]);

    assert_eq!(
        allocate_container_titles(&prepared.sources, &occupied),
        ["Notes (3)", "Notes (4)"]
    );
}

#[test]
fn truncation_keeps_complete_notes_and_the_next_source_proceeds() {
    let prepared = planned(&[("Damaged.enex", TRUNCATED), ("Later.enex", FIRST)]);
    let (documents, warnings) = build_planned(&prepared, &[TRUNCATED, FIRST], &BTreeSet::new());

    assert_eq!(
        documents
            .iter()
            .map(|document| document.title.as_str())
            .collect::<Vec<_>>(),
        ["Damaged", "Complete note", "Later", "First note"]
    );
    assert_eq!(
        warnings
            .iter()
            .filter_map(|warning| warning.code)
            .collect::<Vec<_>>(),
        [ImportWarningCode::EnexEnvelopeTruncated]
    );
}

#[test]
fn security_after_a_complete_note_drops_the_whole_source_and_later_source_proceeds() {
    let prepared = planned(&[
        ("Rejected.enex", SECURITY_AFTER_NOTE),
        ("Later.enex", FIRST),
    ]);
    assert!(prepared.sources[0].notes.is_empty());
    assert_eq!(prepared.preview.document_count, 2);

    let (documents, warnings) =
        build_planned(&prepared, &[SECURITY_AFTER_NOTE, FIRST], &BTreeSet::new());

    assert_eq!(
        documents
            .iter()
            .map(|document| document.title.as_str())
            .collect::<Vec<_>>(),
        ["Later", "First note"]
    );
    assert_eq!(
        warnings
            .iter()
            .filter_map(|warning| warning.code)
            .collect::<Vec<_>>(),
        [ImportWarningCode::EnexSecurityRejected]
    );
    // No staged source was committed, so there is no document or system
    // registration effect to undo for the rejected source.
    assert!(
        documents
            .iter()
            .all(|document| document.source != Path::new("Rejected.enex"))
    );
}

#[test]
fn deleted_notes_keep_valid_source_time_and_fall_back_when_absent_or_invalid() {
    let prepared = planned(&[("Deleted.enex", DELETED)]);
    assert!(
        prepared.preview.documents[0]
            .children
            .iter()
            .all(|note| note.trashed)
    );
    let (documents, warnings) = build_planned(&prepared, &[DELETED], &BTreeSet::new());
    let notes: Vec<_> = documents
        .iter()
        .filter(|document| document.parent.is_some())
        .collect();

    assert_eq!(notes[0].trashed_at.unwrap().millis(), 1_704_164_645_000);
    assert_eq!(notes[1].trashed_at, None);
    assert_eq!(notes[2].trashed_at, None);
    assert_eq!(
        warnings
            .iter()
            .filter(|warning| warning.code == Some(ImportWarningCode::InvalidSourceTimestamp))
            .count(),
        1
    );
}

#[test]
fn guid_plan_preserves_every_occurrence_and_its_source() {
    let prepared = planned(&[("One.enex", FIRST), ("Two.enex", SECOND)]);
    let duplicate = prepared.guids.occurrences("same-guid");

    assert_eq!(duplicate.len(), 2);
    assert_eq!(duplicate[0].file_index, 0);
    assert_eq!(duplicate[1].file_index, 1);
    assert_ne!(duplicate[0].document, duplicate[1].document);
    assert_eq!(prepared.guids.occurrences("unique-guid").len(), 1);
}

fn one_document_attr(document: &ImportedDocument) -> koru_core::node::Attr {
    let snapshot = WorkspaceDoc::from_snapshot(&document.snapshot).unwrap();
    snapshot
        .walk_all()
        .unwrap()
        .into_iter()
        .flat_map(|(id, _)| snapshot.read(id).unwrap().content.attrs().to_vec())
        .next()
        .expect("document has a link pill")
}

#[test]
fn unique_same_source_guid_links_rewrite_both_directions_to_routed_documents() {
    let a = "01234567-89ab-cdef-0123-456789abcdef";
    let b = "fedcba98-7654-3210-fedc-ba9876543210";
    let source = format!(
        "<en-export><note><title>A</title><guid>{a}</guid><content><![CDATA[<en-note><p><a href=\"evernote:///view/1/s1/{b}/{b}/\">B</a></p></en-note>]]></content></note><note><title>B</title><guid>{b}</guid><content><![CDATA[<en-note><p><a href=\"evernote:///view/1/s1/{a}/{a}/\">A</a></p></en-note>]]></content></note></en-export>"
    )
    .into_bytes();
    let prepared = planned(&[("links.enex", &source)]);
    let (documents, warnings) = build_planned_body(&prepared, &[&source]);
    assert!(warnings.is_empty());
    let a_doc = documents
        .iter()
        .find(|document| document.title == "A")
        .unwrap();
    let b_doc = documents
        .iter()
        .find(|document| document.title == "B")
        .unwrap();

    let koru_core::AttrValue::Document(a_target) = one_document_attr(a_doc).value else {
        panic!("A link is routed")
    };
    let koru_core::AttrValue::Document(b_target) = one_document_attr(b_doc).value else {
        panic!("B link is routed")
    };
    assert_eq!(
        a_target.document,
        koru_core::DocumentId::from_uuid(b_doc.id.as_uuid())
    );
    assert_eq!(
        b_target.document,
        koru_core::DocumentId::from_uuid(a_doc.id.as_uuid())
    );
    assert_eq!(a_target.node, None);
    assert_eq!(b_target.node, None);
}

#[test]
fn guid_found_only_in_another_selected_file_still_rewrites() {
    let guid = "01234567-89ab-cdef-0123-456789abcdef";
    let linker = format!(
        "<en-export><note><title>Linker</title><content><![CDATA[<en-note><a href=\"evernote:///view/1/s1/{guid}/{guid}/\">Target</a></en-note>]]></content></note></en-export>"
    )
    .into_bytes();
    let target = format!(
        "<en-export><note><title>Target</title><guid>{guid}</guid><content><![CDATA[<en-note>target</en-note>]]></content></note></en-export>"
    )
    .into_bytes();
    let prepared = planned(&[("linker.enex", &linker), ("target.enex", &target)]);
    let (documents, warnings) = build_planned_body(&prepared, &[&linker, &target]);
    assert!(warnings.is_empty());
    let linker = documents
        .iter()
        .find(|document| document.title == "Linker")
        .unwrap();
    let target = documents
        .iter()
        .find(|document| document.title == "Target")
        .unwrap();
    let koru_core::AttrValue::Document(reference) = one_document_attr(linker).value else {
        panic!("cross-source link is routed")
    };
    assert_eq!(
        reference.document,
        koru_core::DocumentId::from_uuid(target.id.as_uuid())
    );
}

#[test]
fn duplicate_guid_keeps_the_evernote_url_and_warns() {
    let guid = "01234567-89ab-cdef-0123-456789abcdef";
    let linker = format!(
        "<en-export><note><title>Linker</title><content><![CDATA[<en-note><a href=\"evernote:///view/1/s1/{guid}/{guid}/\">Ambiguous</a></en-note>]]></content></note></en-export>"
    )
    .into_bytes();
    let target = |title: &str| {
        format!(
            "<en-export><note><title>{title}</title><guid>{guid}</guid><content><![CDATA[<en-note>{title}</en-note>]]></content></note></en-export>"
        )
        .into_bytes()
    };
    let first = target("First copy");
    let second = target("Second copy");
    let prepared = planned(&[
        ("linker.enex", &linker),
        ("first.enex", &first),
        ("second.enex", &second),
    ]);
    let (documents, warnings) = build_planned_body(&prepared, &[&linker, &first, &second]);
    assert_eq!(
        warnings
            .iter()
            .filter(|warning| warning.code == Some(ImportWarningCode::EvernoteLinkAmbiguous))
            .count(),
        1
    );
    let linker = documents
        .iter()
        .find(|document| document.title == "Linker")
        .unwrap();
    let koru_core::AttrValue::Url(url) = one_document_attr(linker).value else {
        panic!("ambiguous link stays an Evernote URL")
    };
    assert!(url.starts_with("evernote://"));
}

#[test]
fn task_application_data_warns_without_creating_a_task_node() {
    let source = br#"<en-export><note><title>Modern task</title><content><![CDATA[<en-note><p>Readable task payload</p></en-note>]]></content><note-attributes><application-data><key>com.evernote.tasks.v1</key><value>opaque</value></application-data></note-attributes></note></en-export>"#;
    let prepared = planned(&[("task.enex", source)]);
    let (documents, warnings) = build_planned_body(&prepared, &[source]);
    assert_eq!(
        warnings
            .iter()
            .filter(|warning| {
                warning.code == Some(ImportWarningCode::EvernoteTaskPayloadUnsupported)
            })
            .count(),
        1
    );
    let warning = warnings
        .iter()
        .find(|warning| warning.code == Some(ImportWarningCode::EvernoteTaskPayloadUnsupported))
        .unwrap();
    assert_eq!(warning.arguments, ["1", "Modern task"]);
    let note = documents
        .iter()
        .find(|document| document.title == "Modern task")
        .unwrap();
    let snapshot = WorkspaceDoc::from_snapshot(&note.snapshot).unwrap();
    assert!(
        snapshot
            .walk_all()
            .unwrap()
            .into_iter()
            .all(|(id, _)| { snapshot.read(id).unwrap().kind != koru_core::Kind::Task })
    );
    assert!(snapshot.walk_all().unwrap().into_iter().any(|(id, _)| {
        snapshot.read(id).unwrap().content.as_text() == Some("Readable task payload")
    }));
}

#[test]
fn reminder_metadata_warns_without_writing_due_or_plan() {
    let source = br#"<en-export><note><title>Reminder</title><content><![CDATA[<en-note><p>Remember this</p></en-note>]]></content><note-attributes><reminder-time>1700000000000</reminder-time></note-attributes></note></en-export>"#;
    let prepared = planned(&[("reminder.enex", source)]);
    let (documents, warnings) = build_planned_body(&prepared, &[source]);
    assert_eq!(
        warnings
            .iter()
            .filter(|warning| warning.code == Some(ImportWarningCode::EvernoteReminderNotMapped))
            .count(),
        1
    );
    let note = documents
        .iter()
        .find(|document| document.title == "Reminder")
        .unwrap();
    let snapshot = WorkspaceDoc::from_snapshot(&note.snapshot).unwrap();
    assert!(snapshot.walk_all().unwrap().into_iter().all(|(id, _)| {
        snapshot
            .read(id)
            .unwrap()
            .content
            .attrs()
            .iter()
            .all(|attr| !matches!(attr.role.as_str(), "due" | "plan"))
    }));
}

#[test]
fn inactive_without_a_source_instant_is_born_trashed_with_host_fallback() {
    let source = br#"<en-export><note><title>x</title><content>&lt;en-note/&gt;</content><active>0</active></note><note><title>y</title><content>&lt;en-note/&gt;</content><deleted/></note></en-export>"#;
    let parsed = parsed("inactive.enex", source);

    assert!(parsed.notes[0].trashed);
    assert_eq!(parsed.notes[0].trashed_at, None);
    assert!(parsed.notes[1].trashed);
    assert_eq!(parsed.notes[1].trashed_at, None);
    assert!(parsed.warnings.is_empty());
}

#[test]
fn inspection_elides_a_large_enml_body_instead_of_returning_it() {
    let mut source = b"<en-export><note><title>bounded</title><content>".to_vec();
    source.extend(std::iter::repeat_n(b'x', 4 * 1024 * 1024));
    source.extend_from_slice(b"</content></note></en-export>");

    let parsed = parsed("large.enex", &source);

    assert_eq!(parsed.outcome, SourceOutcome::Accepted);
    assert_eq!(parsed.notes.len(), 1);
    assert_eq!(parsed.notes[0].title, "bounded");
}

#[test]
fn body_build_elides_large_resource_data_before_xml_events_are_created() {
    let mut source = b"<en-export><note><title>bounded</title><content>&lt;en-note&gt;body&lt;/en-note&gt;</content></note><resource><data encoding=\"base64\">".to_vec();
    source.extend(std::iter::repeat_n(b'A', 4 * 1024 * 1024));
    source
        .extend_from_slice(b"</data><mime>application/octet-stream</mime></resource></en-export>");
    let inspected = parse_enex(
        Cursor::new(source.as_slice()),
        Path::new("resource.enex"),
        &NoExternalEntityFetcher,
    )
    .unwrap();
    assert_eq!(inspected.outcome, SourceOutcome::Accepted);
    assert_eq!(inspected.notes.len(), 1);

    let mut body = None;

    let parsed = parse_enex_content(
        Cursor::new(source),
        Path::new("resource.enex"),
        &NoExternalEntityFetcher,
        |_, content| {
            body = Some(content.to_owned());
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(parsed.outcome, SourceOutcome::Accepted);
    assert_eq!(body.as_deref(), Some("<en-note>body</en-note>"));
}

#[test]
fn descriptor_claims_enex_files_and_one_container_root_per_source() {
    let SourceRequest::Files { extensions } = DESCRIPTOR.source else {
        panic!("Evernote should accept one or more files");
    };
    assert_eq!(extensions, ["enex"]);
    assert!(DESCRIPTOR.roots_import);
}

#[test]
fn build_pass_commits_the_converted_body_snapshot() {
    let prepared = planned(&[("Body.enex", BODY_RICH)]);
    let (documents, warnings) = build_planned_body(&prepared, &[BODY_RICH]);

    assert!(warnings.is_empty());
    assert_eq!(documents.len(), 2);
    let note = WorkspaceDoc::from_snapshot(&documents[1].snapshot).unwrap();
    let title = note.children(Parent::Root).unwrap()[0];
    assert!(
        note.children(Parent::Node(title))
            .unwrap()
            .into_iter()
            .any(|row| note.read(row).unwrap().content.as_text() == Some("Heading one"))
    );
}

#[test]
fn hostile_inner_enml_drops_every_document_already_staged_for_that_source() {
    let prepared = planned(&[("Hostile.enex", BODY_INNER_SECURITY)]);
    let (documents, warnings) = build_planned_body(&prepared, &[BODY_INNER_SECURITY]);

    assert!(documents.is_empty());
    assert_eq!(
        warnings
            .iter()
            .filter(|warning| warning.code == Some(ImportWarningCode::EnexSecurityRejected))
            .count(),
        1
    );
}

#[test]
fn malformed_enml_is_note_local_and_a_sibling_still_commits() {
    let prepared = planned(&[("Malformed.enex", BODY_MALFORMED_NOTE)]);
    let (documents, warnings) = build_planned_body(&prepared, &[BODY_MALFORMED_NOTE]);

    assert_eq!(
        documents
            .iter()
            .map(|document| document.title.as_str())
            .collect::<Vec<_>>(),
        ["Malformed", "Good sibling"]
    );
    assert_eq!(
        warnings
            .iter()
            .filter(|warning| warning.code == Some(ImportWarningCode::EvernoteUnknownElement))
            .count(),
        1
    );
}

#[test]
fn shared_folded_tags_reuse_one_id_and_hierarchy_assigns_only_the_root() {
    let prepared = planned(&[("Tags.enex", TAGS)]);
    let tags = RecordingTags::default();
    let (documents, warnings) = build_planned_body_with_tags(&prepared, &[TAGS], &tags).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(tags.publish_count(), 1);
    assert_eq!(tags.row_count(), 1);

    let system = tags.system().expect("published system");
    assert_eq!(
        system
            .tags()
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        ["Café", "fun"]
    );
    assert!(system.tags().iter().all(|tag| tag.name != "fun/home"));

    let first = WorkspaceDoc::from_snapshot(&documents[1].snapshot).unwrap();
    let second = WorkspaceDoc::from_snapshot(&documents[2].snapshot).unwrap();
    let first_title = first.children(Parent::Root).unwrap()[0];
    let second_title = second.children(Parent::Root).unwrap()[0];
    let first_ids: Vec<_> = first.read(first_title).unwrap().tag_ids().collect();
    let second_ids: Vec<_> = second.read(second_title).unwrap().tag_ids().collect();
    assert_eq!(first_ids.len(), 2);
    assert_eq!(second_ids.len(), 1);
    assert!(first_ids.contains(&second_ids[0]), "#Café and #cafe split");
}

#[test]
fn accepted_source_registers_once_and_existing_system_is_updated_not_duplicated() {
    let prepared = planned(&[("Tags.enex", TAGS)]);
    let fresh = RecordingTags::default();
    build_planned_body_with_tags(&prepared, &[TAGS], &fresh).unwrap();
    assert_eq!(fresh.row_count(), 1);
    assert_eq!(fresh.publish_count(), 1);

    let mut existing = koru_core::SystemDoc::create().unwrap();
    existing.create_tag("existing", None, 1).unwrap();
    let existing_sink = RecordingTags::with_system(&existing);
    build_planned_body_with_tags(&prepared, &[TAGS], &existing_sink).unwrap();
    assert_eq!(existing_sink.row_count(), 1);
    assert_eq!(existing_sink.publish_count(), 1);
    let names: Vec<_> = existing_sink
        .system()
        .unwrap()
        .tags()
        .into_iter()
        .map(|tag| tag.name)
        .collect();
    assert_eq!(names, ["Café", "existing", "fun"]);
}

#[test]
fn security_rejected_tagged_source_has_no_row_publish_or_documents() {
    let prepared = planned(&[("Rejected.enex", SECURITY_AFTER_NOTE)]);
    let tags = RecordingTags::default();
    let (documents, warnings) =
        build_planned_body_with_tags(&prepared, &[SECURITY_AFTER_NOTE], &tags).unwrap();

    assert!(documents.is_empty());
    assert_eq!(tags.row_count(), 0);
    assert_eq!(tags.publish_count(), 0);
    assert!(tags.system().is_none());
    assert!(
        warnings
            .iter()
            .any(|warning| { warning.code == Some(ImportWarningCode::EnexSecurityRejected) })
    );
}

#[test]
fn ordinary_truncation_publishes_tags_only_for_complete_notes() {
    let prepared = planned(&[("Truncated.enex", TAGS_TRUNCATED)]);
    let tags = RecordingTags::default();
    let (documents, warnings) =
        build_planned_body_with_tags(&prepared, &[TAGS_TRUNCATED], &tags).unwrap();

    assert_eq!(
        documents
            .iter()
            .map(|document| document.title.as_str())
            .collect::<Vec<_>>(),
        ["Truncated", "Complete tagged"]
    );
    assert_eq!(
        tags.system()
            .unwrap()
            .tags()
            .into_iter()
            .map(|tag| tag.name)
            .collect::<Vec<_>>(),
        ["kept"]
    );
    assert!(
        warnings
            .iter()
            .any(|warning| { warning.code == Some(ImportWarningCode::EnexEnvelopeTruncated) })
    );
}

struct CancelDuringSource;

impl ImportProgress for CancelDuringSource {
    fn update(&self, _: ImportStage, _: usize, _: Option<usize>) {}

    fn cancelled(&self) -> bool {
        true
    }
}

#[test]
fn cancellation_drops_a_tagged_stage_before_any_effect() {
    let prepared = planned(&[("Tags.enex", TAGS)]);
    let tags = RecordingTags::default();
    let mut documents = Vec::new();
    let mut warnings = Vec::new();
    let result = commit_body_source(
        &prepared.sources[0],
        "Tags",
        Cursor::new(TAGS),
        koru_core::WorkspaceId::new(),
        &koru_import::NoBlobSink,
        &tags,
        &prepared.guids,
        Some(&CancelDuringSource),
        &mut documents,
        &mut warnings,
    );

    assert!(result.is_err());
    assert!(documents.is_empty());
    assert_eq!(tags.row_count(), 0);
    assert_eq!(tags.publish_count(), 0);
}

struct CancelAtSecondNote(AtomicUsize);

impl ImportProgress for CancelAtSecondNote {
    fn update(&self, stage: ImportStage, completed: usize, _: Option<usize>) {
        if stage == ImportStage::Converting && completed >= 1 {
            self.0.store(1, Ordering::SeqCst);
        }
    }

    fn cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst) != 0
    }
}

#[test]
fn cancellation_between_notes_preserves_a_prior_source_and_drops_current_effects() {
    let payload = b"%PDF-1.7\ncancel-between-notes";
    let md5 = format!("{:x}", md5::compute(payload));
    let current = build_resource_enex(
        "First staged",
        &format!("<en-note><en-media type=\"application/pdf\" hash=\"{md5}\"/></en-note>"),
        &[(payload, "application/pdf", Some("staged.pdf"), true)],
    );
    let mut current = String::from_utf8(current).unwrap();
    current = current.replace(
        "</en-export>",
        "<note><title>Second staged</title><content><![CDATA[<en-note>second</en-note>]]></content><tag>must-not-publish</tag></note></en-export>",
    );
    let current = current.into_bytes();
    let prepared = planned(&[("prior.enex", TAGS), ("current.enex", &current)]);
    let workspace = koru_core::WorkspaceId::new();
    let tags = RecordingTags::default();
    let blobs = RecordingBlobs::returning("must-not-store");
    let mut documents = Vec::new();
    let mut warnings = Vec::new();

    commit_body_source(
        &prepared.sources[0],
        "prior",
        Cursor::new(TAGS),
        workspace,
        &koru_import::NoBlobSink,
        &tags,
        &prepared.guids,
        None,
        &mut documents,
        &mut warnings,
    )
    .unwrap();
    let prior_titles = documents
        .iter()
        .map(|document| document.title.clone())
        .collect::<Vec<_>>();
    let progress = CancelAtSecondNote(AtomicUsize::new(0));

    let error = commit_body_source(
        &prepared.sources[1],
        "current",
        Cursor::new(current),
        workspace,
        &blobs,
        &tags,
        &prepared.guids,
        Some(&progress),
        &mut documents,
        &mut warnings,
    )
    .unwrap_err();

    assert_eq!(error.to_string(), "import cancelled");
    assert_eq!(
        documents
            .iter()
            .map(|document| document.title.clone())
            .collect::<Vec<_>>(),
        prior_titles
    );
    assert!(blobs.puts.lock().unwrap().is_empty());
    assert_eq!(tags.publish_count(), 1, "only the prior source published");
    assert_eq!(
        tags.row_count(),
        1,
        "the cancelled source added no system row"
    );
}

#[test]
fn security_after_resource_and_tag_rolls_back_that_source_and_next_source_commits() {
    let payload = b"%PDF-1.7\nsecurity-boundary";
    let md5 = format!("{:x}", md5::compute(payload));
    let accepted = build_resource_enex(
        "Sensitive",
        &format!("<en-note><en-media type=\"application/pdf\" hash=\"{md5}\"/></en-note>"),
        &[(payload, "application/pdf", Some("sensitive.pdf"), true)],
    );
    let mut accepted = String::from_utf8(accepted).unwrap();
    accepted = accepted.replace(
        "</note></en-export>",
        "<tag>must-not-publish</tag></note></en-export>",
    );
    let accepted = accepted.into_bytes();
    let mut hostile = String::from_utf8(accepted.clone()).unwrap();
    hostile = hostile.replace(
        "</en-export>",
        "<!DOCTYPE en-export [<!ENTITY leak SYSTEM \"file:///secret\">]></en-export>",
    );
    let hostile = hostile.into_bytes();
    let prepared = planned(&[("hostile.enex", &accepted), ("later.enex", FIRST)]);
    let workspace = koru_core::WorkspaceId::new();
    let tags = RecordingTags::default();
    let blobs = RecordingBlobs::returning("must-not-store");
    let mut documents = Vec::new();
    let mut warnings = Vec::new();

    commit_body_source(
        &prepared.sources[0],
        "hostile",
        Cursor::new(hostile),
        workspace,
        &blobs,
        &tags,
        &prepared.guids,
        None,
        &mut documents,
        &mut warnings,
    )
    .unwrap();
    commit_body_source(
        &prepared.sources[1],
        "later",
        Cursor::new(FIRST),
        workspace,
        &blobs,
        &tags,
        &prepared.guids,
        None,
        &mut documents,
        &mut warnings,
    )
    .unwrap();

    assert_eq!(
        documents
            .iter()
            .map(|document| document.title.as_str())
            .collect::<Vec<_>>(),
        ["later", "First note"]
    );
    assert!(blobs.puts.lock().unwrap().is_empty());
    assert_eq!(tags.publish_count(), 0);
    assert_eq!(tags.row_count(), 0);
    assert!(warnings.iter().any(|warning| {
        warning.source == Path::new("hostile.enex")
            && warning.code == Some(ImportWarningCode::EnexSecurityRejected)
    }));
}

#[test]
fn png_media_streams_exact_bytes_and_uses_the_sink_returned_hash() {
    let png = b"\x89PNG\r\n\x1a\nphase-seven";
    let source_md5 = format!("{:x}", md5::compute(png));
    let source = build_resource_enex(
        "Picture",
        &format!(
            "<en-note><p>before<en-media type=\"image/png\" hash=\"{source_md5}\"/>after</p></en-note>"
        ),
        &[(png, "image/png", Some("picture.png"), false)],
    );
    let blobs = RecordingBlobs::returning("store-returned-hash");

    let (documents, warnings) = build_resource_documents(&source, &blobs);

    assert!(warnings.is_empty());
    let puts = blobs.puts.lock().unwrap();
    assert_eq!(
        puts.as_slice(),
        &[(png.to_vec(), "image/png".into(), Some("picture.png".into()))]
    );
    let note = documents
        .iter()
        .find(|document| document.title == "Picture")
        .unwrap();
    let snapshot = WorkspaceDoc::from_snapshot(&note.snapshot).unwrap();
    let image = snapshot
        .walk_all()
        .unwrap()
        .into_iter()
        .find_map(|(id, _)| {
            (snapshot.read(id).unwrap().kind == koru_core::Kind::Image).then_some(id)
        })
        .unwrap();
    let koru_core::node::Content::Blob { hash, .. } = snapshot.read(image).unwrap().content else {
        panic!("image has blob content")
    };
    assert_eq!(hash.0, "store-returned-hash");
    assert_ne!(hash.0, source_md5);
}

#[test]
fn pdf_resource_creates_a_file_entry_and_routed_attachment_pill() {
    let pdf = b"%PDF-1.7\nphase seven";
    let source_md5 = format!("{:x}", md5::compute(pdf));
    let source = build_resource_enex(
        "Paper",
        &format!("<en-note><en-media type=\"application/pdf\" hash=\"{source_md5}\"/></en-note>"),
        &[(pdf, "application/pdf", Some("paper.pdf"), true)],
    );
    let blobs = RecordingBlobs::returning("pdf-store-hash");

    let (documents, warnings) = build_resource_documents(&source, &blobs);

    assert!(warnings.is_empty());
    let file = documents
        .iter()
        .find(|document| document.kind == koru_core::explore::KIND_FILE)
        .unwrap();
    assert_eq!(file.title, "paper.pdf");
    let file_doc = WorkspaceDoc::from_snapshot(&file.snapshot).unwrap();
    assert_eq!(file_doc.blob_hashes().unwrap(), ["pdf-store-hash"]);

    let note = documents
        .iter()
        .find(|document| document.title == "Paper")
        .unwrap();
    let note_doc = WorkspaceDoc::from_snapshot(&note.snapshot).unwrap();
    let attachment = note_doc
        .walk_all()
        .unwrap()
        .into_iter()
        .flat_map(|(id, _)| note_doc.read(id).unwrap().content.attrs().to_vec())
        .find(|attr| attr.role == "attachment")
        .unwrap();
    let koru_core::node::AttrValue::Ref(reference) = attachment.value else {
        panic!("attachment uses routed ref")
    };
    assert_eq!(
        reference.node,
        koru_core::NodeId::from_uuid(file.id.as_uuid())
    );
}

#[test]
fn unreferenced_resource_is_kept_under_the_attachments_heading() {
    let pdf = b"%PDF-1.7\nunreferenced";
    let source = build_resource_enex(
        "Loose",
        "<en-note><p>body</p></en-note>",
        &[(pdf, "application/pdf", None, false)],
    );
    let blobs = RecordingBlobs::returning("loose-hash");

    let (documents, warnings) = build_resource_documents(&source, &blobs);

    assert!(warnings.is_empty());
    let note = documents
        .iter()
        .find(|document| document.title == "Loose")
        .unwrap();
    let note_doc = WorkspaceDoc::from_snapshot(&note.snapshot).unwrap();
    assert!(
        note_doc
            .walk_all()
            .unwrap()
            .into_iter()
            .any(|(id, _)| { note_doc.read(id).unwrap().content.as_text() == Some("Attachments") })
    );
    assert!(
        documents
            .iter()
            .any(|document| document.title == "attachment-1-1.pdf")
    );
}

#[test]
fn missing_media_reference_becomes_the_stable_placeholder_and_warning() {
    let source = build_resource_enex(
        "Missing",
        "<en-note><en-media type=\"application/pdf\" hash=\"deadbeef\"/></en-note>",
        &[],
    );
    let blobs = RecordingBlobs::returning("unused");

    let (documents, warnings) = build_resource_documents(&source, &blobs);

    assert!(blobs.puts.lock().unwrap().is_empty());
    assert!(
        warnings
            .iter()
            .any(|warning| { warning.code == Some(ImportWarningCode::EvernoteMissingResource) })
    );
    let note = documents
        .iter()
        .find(|document| document.title == "Missing")
        .unwrap();
    let note_doc = WorkspaceDoc::from_snapshot(&note.snapshot).unwrap();
    assert!(note_doc.walk_all().unwrap().into_iter().any(|(id, _)| {
        note_doc.read(id).unwrap().content.as_text() == Some("Missing attachment")
    }));
}

#[test]
fn traversal_filename_is_reduced_to_a_safe_leaf() {
    assert_eq!(
        safe_resource_filename("../../private/report.pdf"),
        Some("report.pdf".into())
    );
    assert_eq!(
        safe_resource_filename("..\\..\\secret.pdf"),
        Some("secret.pdf".into())
    );
    assert_eq!(safe_resource_filename(".."), None);
}

#[test]
fn data_signature_mismatch_warns_and_uses_the_detected_mime() {
    let pdf = b"%PDF-1.7\nmislabeled";
    let source_md5 = format!("{:x}", md5::compute(pdf));
    let source = build_resource_enex(
        "MIME",
        &format!("<en-note><en-media type=\"image/png\" hash=\"{source_md5}\"/></en-note>"),
        &[(pdf, "image/png", Some("wrong.png"), false)],
    );
    let blobs = RecordingBlobs::returning("mime-hash");

    let (_, warnings) = build_resource_documents(&source, &blobs);

    assert!(
        warnings
            .iter()
            .any(|warning| { warning.code == Some(ImportWarningCode::EvernoteMimeMismatch) })
    );
    assert_eq!(blobs.puts.lock().unwrap()[0].1, "application/pdf");
}

#[test]
fn security_after_a_resourced_note_performs_zero_blob_puts() {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;

    let bytes = b"%PDF-1.7\nprivate";
    let hash = format!("{:x}", md5::compute(bytes));
    let source = format!(
        "<en-export><note><title>private</title><content><![CDATA[<en-note><en-media type=\"application/pdf\" hash=\"{hash}\"/></en-note>]]></content><resource><data encoding=\"base64\">{}</data><mime>application/pdf</mime></resource></note><!DOCTYPE en-export [<!ENTITY x \"bad\">]></en-export>",
        STANDARD.encode(bytes)
    )
    .into_bytes();
    let blobs = RecordingBlobs::returning("must-not-upload");

    let (documents, warnings) = build_resource_documents(&source, &blobs);

    assert!(documents.is_empty());
    assert!(blobs.puts.lock().unwrap().is_empty());
    assert!(
        warnings
            .iter()
            .any(|warning| { warning.code == Some(ImportWarningCode::EnexSecurityRejected) })
    );
}

#[test]
fn bad_base64_is_note_local_and_a_sibling_still_imports() {
    let source = br#"<en-export>
      <note><title>Bad resource</title><content><![CDATA[<en-note><en-media type="application/pdf" hash="00"/></en-note>]]></content><resource><data encoding="base64">%%%bad%%%</data><mime>application/pdf</mime></resource></note>
      <note><title>Good sibling</title><content><![CDATA[<en-note><p>survives</p></en-note>]]></content></note>
    </en-export>"#;
    let blobs = RecordingBlobs::returning("unused");

    let (documents, warnings) = build_resource_documents(source, &blobs);

    assert!(
        !documents
            .iter()
            .any(|document| document.title == "Bad resource")
    );
    assert!(
        documents
            .iter()
            .any(|document| document.title == "Good sibling")
    );
    assert!(
        warnings
            .iter()
            .any(|warning| { warning.code == Some(ImportWarningCode::EvernoteInvalidResource) })
    );
    assert!(blobs.puts.lock().unwrap().is_empty());
}

struct RetryFromZeroBlob {
    expected: Vec<u8>,
    attempts: Mutex<Vec<Vec<u8>>>,
}

impl BlobSink for RetryFromZeroBlob {
    fn put(&self, _: Vec<u8>, _: &str, _: Option<&str>) -> Result<StoredBlob, ImportError> {
        unreachable!("resource commit uses put_from")
    }

    fn put_from(
        &self,
        factory: &(dyn Fn() -> Box<dyn std::io::Read + Send> + Send + Sync),
        mime: &str,
        _: Option<&str>,
    ) -> Result<StoredBlob, ImportError> {
        use std::io::Read as _;

        let mut first = factory();
        let mut prefix = vec![0; 5];
        first.read_exact(&mut prefix).unwrap();
        self.attempts.lock().unwrap().push(prefix);

        let mut retry = factory();
        let mut all = Vec::new();
        retry.read_to_end(&mut all).unwrap();
        assert_eq!(all, self.expected);
        self.attempts.lock().unwrap().push(all);
        Ok(StoredBlob {
            hash: "retry-hash".into(),
            mime: mime.into(),
        })
    }
}

#[test]
fn upload_retry_factory_reopens_the_spool_at_byte_zero() {
    let pdf = b"%PDF-1.7\nretry-from-zero";
    let source_md5 = format!("{:x}", md5::compute(pdf));
    let source = build_resource_enex(
        "Retry",
        &format!("<en-note><en-media type=\"application/pdf\" hash=\"{source_md5}\"/></en-note>"),
        &[(pdf, "application/pdf", Some("retry.pdf"), true)],
    );
    let blobs = RetryFromZeroBlob {
        expected: pdf.to_vec(),
        attempts: Mutex::new(Vec::new()),
    };

    let (documents, warnings) = build_resource_documents(&source, &blobs);

    assert!(warnings.is_empty());
    assert_eq!(blobs.attempts.lock().unwrap()[0], pdf[..5]);
    let file = documents
        .iter()
        .find(|document| document.kind == koru_core::explore::KIND_FILE)
        .unwrap();
    let file_doc = WorkspaceDoc::from_snapshot(&file.snapshot).unwrap();
    assert_eq!(file_doc.blob_hashes().unwrap(), ["retry-hash"]);
}

struct CancelAfterPolls(AtomicUsize);

impl ImportProgress for CancelAfterPolls {
    fn update(&self, _: ImportStage, _: usize, _: Option<usize>) {}

    fn cancelled(&self) -> bool {
        self.0.fetch_add(1, Ordering::SeqCst) >= 15
    }
}

#[test]
fn cancellation_while_resource_chunks_are_decoded_commits_nothing() {
    let payload = [&b"%PDF-1.7\n"[..], &vec![9_u8; 2 * 1024 * 1024]].concat();
    let md5 = format!("{:x}", md5::compute(&payload));
    let source = build_resource_enex(
        "Cancel resource",
        &format!("<en-note><en-media type=\"application/pdf\" hash=\"{md5}\"/></en-note>"),
        &[(&payload, "application/pdf", Some("large.pdf"), true)],
    );
    let prepared = planned(&[("cancel.enex", &source)]);
    let blobs = RecordingBlobs::returning("must-not-store");
    let tags = RecordingTags::default();
    let progress = CancelAfterPolls(AtomicUsize::new(0));
    let mut documents = Vec::new();
    let mut warnings = Vec::new();

    let result = commit_body_source(
        &prepared.sources[0],
        "cancel",
        Cursor::new(source),
        koru_core::WorkspaceId::new(),
        &blobs,
        &tags,
        &prepared.guids,
        Some(&progress),
        &mut documents,
        &mut warnings,
    );

    assert_eq!(result.unwrap_err().to_string(), "import cancelled");
    assert!(documents.is_empty());
    assert!(blobs.puts.lock().unwrap().is_empty());
    assert_eq!(tags.publish_count(), 0);
}
