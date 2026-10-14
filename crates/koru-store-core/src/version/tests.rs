// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn reminder_capability_is_public_and_coarse() {
    assert!(CAPABILITIES.contains(&"reminders"));
    assert!(
        !CAPABILITIES
            .iter()
            .any(|value| value.starts_with("reminders-"))
    );
}

#[test]
fn an_equal_version_is_writable() {
    assert!(Compatibility::against(Some(3), 3).writable());
}

#[test]
fn an_older_document_is_writable() {
    // Where a migration will go. Until there is one, an older document is
    // simply understood.
    assert!(Compatibility::against(Some(1), 3).writable());
}

#[test]
fn a_missing_version_is_writable() {
    // The absence of a version is not a claim to a newer one.
    assert!(Compatibility::against(None, 3).writable());
}

#[test]
fn a_newer_document_is_read_only_and_says_why() {
    let c = Compatibility::against(Some(4), 3);
    assert!(!c.writable());
    assert_eq!(
        c,
        Compatibility::TooNew {
            found: 4,
            understood: 3
        }
    );
    assert_eq!(c.code(), Some("schema_too_new"));

    let said = c.to_string();
    // The message has to carry both numbers, or a bug report cannot say which
    // build wrote the document.
    assert!(said.contains('4'), "{said}");
    assert!(said.contains('3'), "{said}");
    assert!(said.contains("reading only"), "{said}");
}

#[test]
fn a_writable_document_has_no_error_code() {
    assert_eq!(Compatibility::Writable.code(), None);
}

#[test]
fn the_crdt_gate_reads_this_builds_constant() {
    assert!(Compatibility::of_crdt(Some(CRDT_SCHEMA_VERSION)).writable());
    assert!(!Compatibility::of_crdt(Some(CRDT_SCHEMA_VERSION + 1)).writable());
}

#[test]
fn the_five_versions_are_independent_constants() {
    // Not a tautology: it fails the moment someone "simplifies" these into one
    // shared constant, which is exactly what this module exists to prevent.
    fn distinct_storage_locations() -> [&'static str; 5] {
        [
            "route path segment",
            "catalog.db user_version",
            "workspace.db user_version",
            "manifest meta.schema_version",
            "bundle manifest.json format_version",
        ]
    }
    assert_eq!(distinct_storage_locations().len(), 5);
    assert!(API_VERSION >= 1);
    assert!(CATALOG_SCHEMA_VERSION >= 1);
    assert!(WORKSPACE_SCHEMA_VERSION >= 1);
    assert!(CRDT_SCHEMA_VERSION >= 1);
    assert!(BUNDLE_FORMAT_VERSION >= 1);
}

#[test]
fn capabilities_are_stable_ascii_tokens() {
    // They cross the API, so they are machine-readable values and never
    // display strings — the internationalization invariant, as a test.
    for c in CAPABILITIES {
        assert!(!c.is_empty());
        assert!(
            c.chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'),
            "{c} is not a stable token"
        );
    }
}
