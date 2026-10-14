// SPDX-License-Identifier: MPL-2.0

use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

const EXTERNAL_ENEX: &[u8] = include_bytes!("fixtures/external-doctype.enex");
const EXTERNAL_ENML: &[u8] = include_bytes!("fixtures/external-doctype.enml");
const INTERNAL_ENTITY: &[u8] = include_bytes!("fixtures/internal-entity.enex");

#[derive(Default)]
struct FetchSpy(AtomicUsize);

impl ExternalEntityFetcher for FetchSpy {
    fn fetch(&self, _: &str) -> Result<Vec<u8>, XmlSecurityError> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Err(XmlSecurityError::ExternalFetch("spy".into()))
    }
}

#[test]
fn normal_evernote_external_doctypes_are_inert() {
    let fetches = FetchSpy::default();

    validate_xml(Cursor::new(EXTERNAL_ENEX), &fetches).expect("external ENEX doctype");
    validate_xml(Cursor::new(EXTERNAL_ENML), &fetches).expect("external ENML doctype");

    assert_eq!(fetches.0.load(Ordering::Relaxed), 0);
}

#[test]
fn an_internal_entity_rejects_the_source_without_fetching() {
    let fetches = FetchSpy::default();

    assert_eq!(
        validate_xml(Cursor::new(INTERNAL_ENTITY), &fetches),
        Err(XmlSecurityError::ForbiddenDoctype)
    );
    assert_eq!(fetches.0.load(Ordering::Relaxed), 0);
}

#[test]
fn only_xml_builtin_and_character_references_are_accepted() {
    let fetches = FetchSpy::default();
    let allowed = br#"<en-note a="&quot;">&amp;&#10;&#xA;</en-note>"#;
    validate_xml(Cursor::new(allowed), &fetches).expect("built-in references");

    let custom = br#"<en-note a="&custom;"/>"#;
    assert_eq!(
        validate_xml(Cursor::new(custom), &fetches),
        Err(XmlSecurityError::ForbiddenEntity("custom".into()))
    );
}

fn parse_outcome(bytes: &[u8]) -> SourceOutcome {
    parse_enex(
        Cursor::new(bytes),
        Path::new("hostile.enex"),
        &NoExternalEntityFetcher,
    )
    .unwrap()
    .outcome
}

#[test]
fn malformed_outer_xml_exercises_the_parser_failure_diagnostic() {
    let malformed = br#"<en-export>
        <note><title>valid</title><content>&lt;en-note/&gt;</content></note>
        <note><title>broken & value</title><content>&lt;en-note/&gt;</content></note>
    </en-export>"#;

    let error = parse_enex(
        Cursor::new(malformed),
        Path::new("malformed.enex"),
        &NoExternalEntityFetcher,
    )
    .expect_err("the malformed entity must fail the outer parser");

    assert!(error.to_string().contains("entity or character reference"));
}

#[test]
fn resource_parser_failures_report_the_specific_metadata_field() {
    assert_eq!(
        parser_scope(None, Some(ResourceField::Filename), true, true, true, false,),
        ParseScope::ResourceFilename
    );
    assert_eq!(
        resource_field_for(b"source-url", true),
        Some(ResourceField::SourceUrl)
    );
    assert_eq!(
        parser_scope(
            None,
            Some(ResourceField::SourceUrl),
            true,
            true,
            true,
            false,
        ),
        ParseScope::ResourceSourceUrl
    );
    assert_eq!(
        resource_field_for(b"application-data", true),
        Some(ResourceField::ApplicationData)
    );
    assert_eq!(
        parser_scope(None, None, true, true, true, false),
        ParseScope::ResourceAttributes
    );
}

#[test]
fn escaped_references_are_preserved_in_resource_metadata() {
    let source = br#"<en-export><note>
        <title>valid</title><content>&lt;en-note/&gt;</content>
        <resource><data encoding="base64"></data><mime>application/pdf</mime>
        <resource-attributes><file-name>R&amp;D.pdf</file-name></resource-attributes>
        </resource>
    </note></en-export>"#;

    let parsed = parse_enex(
        Cursor::new(source),
        Path::new("resource-reference.enex"),
        &NoExternalEntityFetcher,
    )
    .expect("valid resource metadata");

    assert_eq!(
        parsed.notes[0].resources[0].filename.as_deref(),
        Some("R&D.pdf")
    );
}

#[test]
fn malformed_resource_application_data_is_ignored_without_hiding_note_tasks() {
    let source = br#"<en-export><note>
        <title>valid</title><content>&lt;en-note&gt;kept&lt;/en-note&gt;</content>
        <note-attributes><application-data>modern-task</application-data></note-attributes>
        <resource><data encoding="base64"></data><mime>application/pdf</mime>
        <resource-attributes>
            <file-name>kept.pdf</file-name>
            <application-data>ignored key=one&value=two</application-data>
        </resource-attributes></resource>
    </note></en-export>"#;

    let inspected = parse_enex(
        Cursor::new(source),
        Path::new("malformed-resource-application-data.enex"),
        &NoExternalEntityFetcher,
    )
    .expect("ignored malformed resource metadata must not reject the source");
    assert_eq!(inspected.outcome, SourceOutcome::Accepted);
    assert!(inspected.notes[0].modern_task_payload);
    assert_eq!(
        inspected.notes[0].resources[0].filename.as_deref(),
        Some("kept.pdf")
    );

    let mut body = None;
    let built = parse_enex_content(
        Cursor::new(source),
        Path::new("malformed-resource-application-data.enex"),
        &NoExternalEntityFetcher,
        |note, content| {
            assert!(note.modern_task_payload);
            body = Some(content.to_owned());
            Ok(())
        },
    )
    .expect("the content pass must apply the same narrow recovery");
    assert_eq!(built.outcome, SourceOutcome::Accepted);
    assert_eq!(body.as_deref(), Some("<en-note>kept</en-note>"));
}

#[test]
fn depth_attribute_and_metadata_field_limits_reject_the_source() {
    let mut deep = String::from("<en-export>");
    for _ in 0..MAX_XML_DEPTH {
        deep.push_str("<wrapper>");
    }
    for _ in 0..MAX_XML_DEPTH {
        deep.push_str("</wrapper>");
    }
    deep.push_str("</en-export>");
    assert_eq!(
        parse_outcome(deep.as_bytes()),
        SourceOutcome::SecurityRejected
    );

    let attributes = (0..=MAX_ATTRIBUTES)
        .map(|index| format!(" a{index}=\"\""))
        .collect::<String>();
    let many_attributes = format!("<en-export{attributes}/>");
    assert_eq!(
        parse_outcome(many_attributes.as_bytes()),
        SourceOutcome::SecurityRejected
    );

    let long_title = "x".repeat(MAX_METADATA_FIELD_BYTES + 1);
    let large_field = format!(
        "<en-export><note><title>{long_title}</title><content>&lt;en-note/&gt;</content></note></en-export>"
    );
    assert_eq!(
        parse_outcome(large_field.as_bytes()),
        SourceOutcome::SecurityRejected
    );
}

#[test]
fn internal_entity_declaration_reference_and_expansion_are_source_security() {
    let hostile = br#"<!DOCTYPE en-export [<!ENTITY repeated "expanded">]><en-export><note><title>&repeated;&repeated;</title><content>&lt;en-note/&gt;</content></note></en-export>"#;

    assert_eq!(parse_outcome(hostile), SourceOutcome::SecurityRejected);
}
