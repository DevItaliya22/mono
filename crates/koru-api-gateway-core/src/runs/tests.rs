// SPDX-License-Identifier: MPL-2.0

//! The route table, its OpenAPI projection and the code-to-status map —
//! what can be proved without a worker. The routes against a real worker
//! are `apps/koru-agent/tests/delegate_worker_process.rs`.

use std::collections::BTreeSet;

use super::*;

#[test]
fn every_route_is_catalogued_once_and_reaches_the_openapi_document() {
    let mut seen = BTreeSet::new();
    for route in ROUTES {
        assert!(seen.insert((route.method, route.path)), "{}", route.path);
        assert!(route.path.starts_with("/v1/runs") || route.path.starts_with("/v2/runs"));
    }
    let mut document = serde_json::json!({"paths": {}, "components": {"schemas": {}}});
    extend_openapi(&mut document);
    for route in ROUTES {
        let operation = &document["paths"][route.path][route.method.to_lowercase()];
        assert_eq!(operation["operationId"], route.operation_id);
        assert_eq!(
            operation["security"][0]["bearerAuth"][0],
            route.scope.as_str()
        );
    }
    assert!(document["paths"]["/v1/runs"]["post"]["responses"]["202"].is_object());
    assert!(document["paths"]["/v1/runs"]["post"]["responses"]["200"].is_null());
    assert!(document["paths"]["/v1/runs/{run}/stream"]["get"]["responses"]["200"]["content"]["text/event-stream"].is_object());
    assert!(document["components"]["schemas"]["DelegateFailure"].is_object());
    assert!(document["components"]["schemas"]["CreateRunRequest"].is_object());
}

#[test]
fn the_router_mounts_every_path_without_panicking() {
    let _ = router();
}

#[test]
fn stable_codes_map_to_one_status_each() {
    let cases = [
        ("forbidden", StatusCode::FORBIDDEN),
        ("unknown_run", StatusCode::NOT_FOUND),
        ("wrong_status", StatusCode::CONFLICT),
        ("attempt_live", StatusCode::CONFLICT),
        ("context_capture", StatusCode::UNPROCESSABLE_ENTITY),
        ("invalid_request", StatusCode::BAD_REQUEST),
        ("delegate_unavailable", StatusCode::SERVICE_UNAVAILABLE),
        ("not_delegatable", StatusCode::NOT_IMPLEMENTED),
    ];
    for (code, status) in cases {
        assert_eq!(status_for(&Failure::new(code, "", false)), status, "{code}");
    }
    assert_eq!(
        status_for(&Failure::new("port", "", true)),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        status_for(&Failure::new("port", "", false)),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[test]
fn the_scopes_are_in_the_gateways_default_set() {
    let scopes = crate::effective_scopes(&[], crate::Admission::Ordinary);
    assert!(
        scopes
            .iter()
            .any(|scope| scope == DelegateScope::Read.as_str())
    );
    assert!(
        scopes
            .iter()
            .any(|scope| scope == DelegateScope::Write.as_str())
    );
}

#[tokio::test]
async fn a_gateway_with_only_legacy_run_routes_refuses_protected_mutations() {
    use axum::body::Body;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt as _;
    let mutations = Arc::new(AtomicUsize::new(0));
    let mut legacy = Router::new();
    // An older gateway: v1 create, and the v1 resume this one no longer has.
    let create = ROUTES
        .iter()
        .find(|route| route.operation_id == "runs_create")
        .unwrap();
    for path in [create.path, "/v1/runs/{run}/resume"] {
        let mutations = Arc::clone(&mutations);
        legacy = legacy.route(
            path,
            post(move || async move {
                mutations.fetch_add(1, Ordering::Relaxed);
                StatusCode::ACCEPTED
            }),
        );
    }
    for path in [
        "/v2/runs",
        "/v2/runs/00000000-0000-4000-8000-000000000001/resume",
    ] {
        let response = legacy
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    assert_eq!(mutations.load(Ordering::Relaxed), 0);
}
