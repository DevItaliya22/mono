// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::{locator::ObjectLocator, vectors};

/// §3.1's three status shapes, spelled exactly as the contract prints them.
#[test]
fn the_status_shapes_are_the_ones_the_contract_prints() {
    let missing = serde_json::to_value(BlobStatusV1::Missing { failure: None }).unwrap();
    assert_eq!(missing, serde_json::json!({"state": "missing"}));
    // A missing blob whose last attempt failed says why, which is where §4.3's
    // `hash_mismatch` is actually visible: the session is terminal, so there is
    // no "uploading" left to hang the reason on.
    assert_eq!(
        serde_json::to_value(BlobStatusV1::Missing {
            failure: Some(crate::BlobCode::HashMismatch)
        })
        .unwrap(),
        serde_json::json!({"state": "missing", "failure": "hash_mismatch"})
    );

    let upload = vectors::fixture_id("upload-1");
    let uploading = serde_json::to_value(BlobStatusV1::Uploading {
        upload_id: upload,
        committed_offset: 6_291_456,
        chunk_size: crate::CHUNK,
        expires_at_millis: 1_789_000_000_000,
        byte_url: "https://us.data.koru.do/v1/objects/uploads/x".into(),
    })
    .unwrap();
    assert_eq!(uploading["state"], "uploading");
    assert_eq!(uploading["committed_offset"], 6_291_456);
    assert_eq!(uploading["chunk_size"], 2_097_152);
    assert_eq!(uploading["upload_id"], upload.to_string());

    let durable = serde_json::to_value(BlobStatusV1::Durable {
        size: 4_194_304,
        mime: "image/png".into(),
        filename: Some("shot.png".into()),
        durable_at_millis: 1_789_000_000_000,
    })
    .unwrap();
    assert_eq!(durable["state"], "durable");
    assert_eq!(durable["filename"], "shot.png");
}

/// A filename is optional metadata and is omitted rather than sent as null, in
/// every shape that carries one.
#[test]
fn an_absent_filename_is_omitted_rather_than_null() {
    let status = serde_json::to_value(BlobStatusV1::Durable {
        size: 1,
        mime: "application/octet-stream".into(),
        filename: None,
        durable_at_millis: 1,
    })
    .unwrap();
    assert!(status.get("filename").is_none());
    let read = serde_json::to_value(ReadCapabilityV1 {
        capability: "x".into(),
        byte_url: "https://us.data.koru.do/v1/objects/x".into(),
        size: 1,
        mime: "application/octet-stream".into(),
        filename: None,
    })
    .unwrap();
    assert!(read.get("filename").is_none());
}

/// Every shape refuses an unknown field rather than ignoring it, in both
/// directions: a field a reader silently drops is a field a writer thinks it
/// sent.
#[test]
fn every_shape_refuses_an_unknown_field() {
    let mut allocate = serde_json::json!({"size": 1, "mime": "image/png"});
    assert!(serde_json::from_value::<AllocateRequestV1>(allocate.clone()).is_ok());
    allocate["extra"] = serde_json::json!(true);
    assert!(serde_json::from_value::<AllocateRequestV1>(allocate).is_err());

    let mut report = serde_json::to_value(ObjectReportV1 {
        upload_id: vectors::fixture_id("upload-1"),
        object_locator: ObjectLocator::mint(),
        region: koru_account_core::workspace_authorization::Region::Us,
        size: 4_194_304,
        blob_hash: vectors::fixture_hash("b3:aa"),
        verified_at_millis: 1_789_000_000_000,
    })
    .unwrap();
    assert!(serde_json::from_value::<ObjectReportV1>(report.clone()).is_ok());
    report["extra"] = serde_json::json!(1);
    assert!(serde_json::from_value::<ObjectReportV1>(report).is_err());

    let mut offset = serde_json::json!({"committed_offset": 1});
    assert!(serde_json::from_value::<CommittedOffsetV1>(offset.clone()).is_ok());
    offset["extra"] = serde_json::json!(1);
    assert!(serde_json::from_value::<CommittedOffsetV1>(offset).is_err());
}

/// The failure body is the same shape the account routes already use, so a
/// person meets one vocabulary and one envelope.
#[test]
fn a_failure_body_carries_the_stable_code_and_untranslated_text() {
    for code in crate::BlobCode::ALL {
        let body = serde_json::to_value(BlobErrorV1::from(*code)).unwrap();
        assert_eq!(body["code"], code.as_str());
        assert_eq!(body["message"], code.message());
        let decoded: BlobErrorV1 = serde_json::from_value(body).unwrap();
        assert_eq!(decoded.code, *code);
    }
}

/// `complete` answers `verifying`, and a fenced report says which fence moved
/// rather than only that something did.
#[test]
fn the_two_state_answers_say_what_happened() {
    assert_eq!(
        serde_json::to_value(VerifyingV1::default()).unwrap(),
        serde_json::json!({"state": "verifying"})
    );
    assert_eq!(
        serde_json::to_value(AbortedV1::default()).unwrap(),
        serde_json::json!({"state": "aborted"})
    );
    assert_eq!(
        serde_json::to_value(ReportOutcomeV1::Durable).unwrap(),
        serde_json::json!({"state": "durable"})
    );
    assert_eq!(
        serde_json::to_value(ReportOutcomeV1::Fenced {
            reason: crate::BlobCode::StaleEpoch
        })
        .unwrap(),
        serde_json::json!({"state": "fenced", "reason": "stale_epoch"})
    );
}
