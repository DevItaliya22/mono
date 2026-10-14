// SPDX-License-Identifier: MPL-2.0

use koru_contract::{
    proto::{DocumentEventEnvelope, DocumentProjection},
    validate_document,
};
use prost::Message;

const VALID: &[(&str, &[u8])] = &[
    (
        "simple",
        include_bytes!("../../../contracts/testdata/valid/simple_document.pb"),
    ),
    (
        "multilingual",
        include_bytes!("../../../contracts/testdata/valid/multilingual_document.pb"),
    ),
    (
        "rtl",
        include_bytes!("../../../contracts/testdata/valid/rtl_document.pb"),
    ),
    (
        "image",
        include_bytes!("../../../contracts/testdata/valid/image_document.pb"),
    ),
];

const INVALID: &[(&str, &[u8])] = &[
    (
        "uuid",
        include_bytes!("../../../contracts/testdata/invalid/invalid_uuid.pb"),
    ),
    (
        "range",
        include_bytes!("../../../contracts/testdata/invalid/invalid_range.pb"),
    ),
    (
        "hierarchy",
        include_bytes!("../../../contracts/testdata/invalid/invalid_hierarchy.pb"),
    ),
    (
        "required",
        include_bytes!("../../../contracts/testdata/invalid/invalid_required_fields.pb"),
    ),
];

#[test]
fn valid_shared_fixtures_decode_and_validate() {
    for (name, bytes) in VALID {
        let decoded =
            DocumentProjection::decode(*bytes).unwrap_or_else(|error| panic!("{name}: {error}"));
        validate_document(&decoded).unwrap_or_else(|error| panic!("{name}: {error:?}"));
    }
}

#[test]
fn invalid_shared_fixtures_decode_but_fail_semantic_validation() {
    for (name, bytes) in INVALID {
        let decoded =
            DocumentProjection::decode(*bytes).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(
            validate_document(&decoded).is_err(),
            "{name} unexpectedly passed"
        );
    }
}

#[test]
fn forward_document_fixtures_round_trip_without_loss() {
    for (name, bytes) in [
        (
            "block",
            include_bytes!("../../../contracts/testdata/forward_compat/unknown_block_kind.pb")
                .as_slice(),
        ),
        (
            "mark",
            include_bytes!("../../../contracts/testdata/forward_compat/unknown_mark.pb").as_slice(),
        ),
    ] {
        let decoded = DocumentProjection::decode(bytes).unwrap();
        validate_document(&decoded).unwrap_or_else(|error| panic!("{name}: {error:?}"));
        assert_eq!(
            decoded.encode_to_vec(),
            bytes,
            "{name} fixture changed on round trip"
        );
    }
}

#[test]
fn unknown_event_round_trips_without_loss() {
    let bytes = include_bytes!("../../../contracts/testdata/forward_compat/unknown_event.pb");
    let decoded = DocumentEventEnvelope::decode(bytes.as_slice()).unwrap();
    assert_eq!(decoded.encode_to_vec(), bytes);
}
