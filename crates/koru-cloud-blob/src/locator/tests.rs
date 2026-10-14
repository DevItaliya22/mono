// SPDX-License-Identifier: MPL-2.0

use super::*;

fn id(byte: u8) -> CloudId {
    let mut bytes = [byte; 16];
    bytes[6] = 0x40 | (bytes[6] & 0x0f);
    bytes[8] = 0x80 | (bytes[8] & 0x3f);
    CloudId::from_bytes(bytes)
}

/// Every segment is a token this process minted or validated. Nothing a client
/// sent — a filename, a hash, a header — can reach a path component.
#[test]
fn an_object_key_is_four_validated_segments_and_nothing_else() {
    let key = object_key(
        Region::Us,
        id(0x11),
        id(0x22),
        ObjectLocator::from_uuid(id(0x33).as_uuid()),
    );
    let segments: Vec<_> = key.split('/').collect();
    assert_eq!(segments.len(), 4);
    assert_eq!(segments[0], "us");
    assert!(key.is_ascii());
    assert!(!key.contains("//"));
    assert!(!segments.iter().any(|part| *part == "." || *part == ".."));
    assert!(
        key.bytes().all(|byte| byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || b"-/".contains(&byte))
    );
    assert_eq!(
        object_key(
            Region::Eu,
            id(0x11),
            id(0x22),
            ObjectLocator::from_uuid(id(0x33).as_uuid())
        )
        .split('/')
        .next(),
        Some("eu")
    );
}

/// Two Workspaces holding identical bytes get different keys, because the
/// durable Workspace mapping is the object record and server-wide content
/// deduplication would be either temporary or a privacy leak.
#[test]
fn the_same_bytes_in_two_workspaces_never_share_a_key() {
    let locator = ObjectLocator::mint();
    assert_ne!(
        object_key(Region::Us, id(1), id(3), locator),
        object_key(Region::Us, id(2), id(3), locator)
    );
    // And a replaced incarnation is a different key, so a fenced dataset's
    // objects can never be read through the dataset that replaced it.
    assert_ne!(
        object_key(Region::Us, id(1), id(3), locator),
        object_key(Region::Us, id(1), id(4), locator)
    );
}

/// Staging is named by the session, because it exists before anybody knows
/// whether the bytes are the object they claim to be.
#[test]
fn staging_is_named_by_the_session_and_is_one_flat_component() {
    let name = staging_name(id(7));
    assert!(name.ends_with(".part"));
    assert!(!name.contains('/') && !name.contains('\\'));
    assert!(name.is_ascii());
    assert_ne!(name, staging_name(id(8)));
}

/// One spelling per locator. A locator that round-trips through two spellings
/// is two paths for one object.
#[test]
fn only_the_canonical_spelling_of_a_locator_parses() {
    let locator = ObjectLocator::mint();
    let text = locator.to_string();
    assert_eq!(ObjectLocator::parse(&text), Some(locator));
    assert_eq!(ObjectLocator::parse(&text.to_ascii_uppercase()), None);
    assert_eq!(ObjectLocator::parse(&text.replace('-', "")), None);
    assert_eq!(ObjectLocator::parse("not-a-locator"), None);
    assert_eq!(
        serde_json::to_value(locator).unwrap(),
        serde_json::json!(text)
    );
}
