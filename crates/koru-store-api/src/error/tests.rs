// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_store_core::errors::Code;

#[test]
fn every_class_maps_to_a_status() {
    // Exhaustive by construction — `status_for` matches without a wildcard, so
    // a new class fails to compile rather than defaulting to 500.
    for class in [
        Class::NotFound,
        Class::Invalid,
        Class::Unauthenticated,
        Class::Forbidden,
        Class::Conflict,
        Class::Unavailable,
        Class::Internal,
    ] {
        let status = status_for(class);
        assert!(
            status.is_client_error() || status.is_server_error(),
            "{class:?}"
        );
    }
}

#[test]
fn the_statuses_are_the_conventional_ones() {
    assert_eq!(status_for(Class::NotFound), StatusCode::NOT_FOUND);
    assert_eq!(status_for(Class::Invalid), StatusCode::BAD_REQUEST);
    assert_eq!(status_for(Class::Unauthenticated), StatusCode::UNAUTHORIZED);
    assert_eq!(status_for(Class::Forbidden), StatusCode::FORBIDDEN);
    assert_eq!(status_for(Class::Conflict), StatusCode::CONFLICT);
    assert_eq!(
        status_for(Class::Internal),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[test]
fn damaged_state_is_retryable_rather_than_a_server_bug() {
    // 503, not 500: corruption is a condition of the data, and a client may
    // usefully retry after a recovery.
    assert_eq!(
        status_for(Class::Unavailable),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[test]
fn a_missing_workspace_is_a_404_carrying_its_code() {
    let response = ApiError::bare(Code::WorkspaceNotFound).into_response();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[test]
fn a_document_from_a_newer_build_is_a_conflict_not_a_404() {
    // The store understood the request and declined it. A 404 would send a
    // client looking for a document that is right there.
    let response = ApiError::bare(Code::SchemaTooNew).into_response();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[test]
fn the_body_carries_a_stable_code_and_developer_text() {
    let failure = Failure {
        code: Code::WorkspaceNotFound.as_str().to_string(),
        message: "no row for the given id".into(),
        waiting_on: Vec::new(),
        hold: None,
    };
    let json = serde_json::to_string(&failure).unwrap();
    assert!(json.contains("\"code\":\"workspace_not_found\""), "{json}");
    assert!(json.contains("\"message\""), "{json}");
}

#[test]
fn every_code_serialises_as_its_stable_token() {
    // What a client branches on. A code that changed spelling would silently
    // stop matching in the field.
    for code in Code::ALL {
        let failure = Failure {
            code: code.as_str().to_string(),
            message: String::new(),
            waiting_on: Vec::new(),
            hold: None,
        };
        let json = serde_json::to_string(&failure).unwrap();
        assert!(json.contains(code.as_str()), "{json}");
    }
}

/// **The hold survives the response body** (D259).
///
/// `GET /blobs/{hash}` flattens every hydration failure into one of four
/// `Code`s, and the blob engine has six holds. The one a person most needs —
/// *this device is not using Koru Cloud for this Workspace* — landed as
/// `storage_failed` with its reason in developer text a client is told never
/// to show, so the missing-image placeholder had nothing true left to say.
#[tokio::test]
async fn a_blob_hold_rides_beside_the_code_it_could_not_become() {
    let response = ApiError::new(Code::StorageFailed, "blob hydration failed")
        .with_hold(Some("mode_refuses_transport".into()))
        .into_response();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body: Failure = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body.code, "storage_failed");
    assert_eq!(body.hold.as_deref(), Some("mode_refuses_transport"));

    // …and it is absent, not empty, on everything that is not a hold.
    let plain = ApiError::bare(Code::BlobNotFound).into_response();
    let bytes = axum::body::to_bytes(plain.into_body(), 64 * 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json.get("hold").is_none(), "{json}");
}
