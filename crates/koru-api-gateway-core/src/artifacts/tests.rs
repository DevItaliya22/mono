// SPDX-License-Identifier: MPL-2.0

//! `/v1/artifacts` in process: a real Store worker and a real `artifacts.db`
//! under one temporary root, driven through the router the way a client
//! would. Bytes go in through the Store's blob route and come back through
//! the artifact content route by hash; metadata is refused where the rules
//! say so, with the stable codes.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use koru_api_wire::GatewayStatus;
use tower::ServiceExt as _;

use crate::{GatewayState, now_millis, router};

pub(super) fn state(with_store: bool) -> (tempfile::TempDir, Arc<GatewayState>, String) {
    let root = tempfile::tempdir().unwrap();
    let worker = koru_store_server::StoreWorker::new(koru_store_server::WorkerConfig::embedded(
        root.path().to_path_buf(),
        "internal-debug",
    ))
    .spawn()
    .unwrap();
    let state = GatewayState::new("internal-debug".into(), worker);
    if with_store {
        let repository = koru_artifact_sqlite::Repository::open(
            &koru_channel::artifacts_root_beside(root.path()),
        )
        .unwrap();
        assert!(state.set_artifact_service(Arc::new(koru_artifact_api::Service::new(repository))));
    }
    let (token, _) = state.sessions.mint("test", &[], now_millis());
    (root, state, token)
}

pub(super) async fn send(
    state: &Arc<GatewayState>,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Vec<u8>>,
    content_type: &str,
) -> (StatusCode, header::HeaderMap, Vec<u8>) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let body = match body {
        Some(bytes) => {
            builder = builder.header(header::CONTENT_TYPE, content_type);
            Body::from(bytes)
        }
        None => Body::empty(),
    };
    let response = router(Arc::clone(state))
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap();
    (status, headers, bytes.to_vec())
}

pub(super) async fn json(
    state: &Arc<GatewayState>,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let body = body.map(|value| serde_json::to_vec(&value).unwrap());
    let (status, _, bytes) = send(state, token, method, uri, body, "application/json").await;
    let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, value)
}

/// A profile, a workspace, and `bytes` filed in its blob store: the
/// registration's precondition, reached the way a product would reach it.
pub(super) async fn workspace_with_blob(
    state: &Arc<GatewayState>,
    token: &str,
    bytes: &[u8],
) -> (String, String) {
    let (status, profile) = json(
        state,
        token,
        "POST",
        "/v1/profiles",
        Some(serde_json::json!({ "name": "Ada", "locale": "en" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{profile}");
    let (status, workspace) = json(
        state,
        token,
        "POST",
        "/v1/workspaces",
        Some(serde_json::json!({ "owner": profile["id"], "name": "Notes" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{workspace}");
    let workspace = workspace["id"].as_str().unwrap().to_owned();
    let (status, _, blob) = send(
        state,
        token,
        "PUT",
        &format!("/v1/workspaces/{workspace}/blobs"),
        Some(bytes.to_vec()),
        "text/markdown",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let blob: serde_json::Value = serde_json::from_slice(&blob).unwrap();
    (workspace, blob["hash"].as_str().unwrap().to_owned())
}

fn family_body(workspace: &str, n: u8) -> serde_json::Value {
    serde_json::json!({
        "family_id": format!("00000000-0000-4000-8000-0000000000{n:02x}"),
        "workspace": workspace,
        "title": "Quarterly report",
    })
}

fn version_body(registration: u8, hash: &str, derived_from: Option<&str>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "registration_id": format!("00000000-0000-4000-8000-00000000f0{registration:02x}"),
        "blob": hash,
        "mime": "text/markdown",
        "filename": "report.md",
    });
    if let Some(from) = derived_from {
        body["derived_from"] = serde_json::json!(from);
    }
    body
}

async fn status_of(state: &Arc<GatewayState>, token: &str) -> GatewayStatus {
    let (status, value) = json(state, token, "GET", "/v1/status", None).await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn the_capability_and_the_routes_follow_the_store() {
    let (_root, without, token) = state(false);
    let status = status_of(&without, &token).await;
    assert!(!status.capabilities.iter().any(|c| c == "artifacts"));
    let (status, failure) = json(
        &without,
        &token,
        "GET",
        "/v1/artifacts/families?workspace=00000000-0000-4000-8000-0000000000aa",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(failure["code"], "artifacts_unavailable");
    assert_eq!(failure["retryable"], true);

    let (_root, with, token) = state(true);
    let status = status_of(&with, &token).await;
    assert!(status.capabilities.iter().any(|c| c == "artifacts"));
    assert!(
        !with.set_artifact_service(Arc::new(koru_artifact_api::Service::new(
            koru_artifact_sqlite::Repository::open_in_memory().unwrap()
        )))
    );
}

#[tokio::test]
async fn scope_is_checked_before_the_store_is_asked() {
    let (_root, state, _) = state(true);
    let (read_only, _) = state.sessions.mint(
        "reader",
        &["artifacts.read".into(), "store.read_documents".into()],
        now_millis(),
    );
    let (status, failure) = json(
        &state,
        &read_only,
        "POST",
        "/v1/artifacts/families",
        Some(family_body("00000000-0000-4000-8000-0000000000aa", 1)),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(failure["code"], "forbidden");
    let (status, _) = json(
        &state,
        &read_only,
        "GET",
        "/v1/artifacts/families?workspace=00000000-0000-4000-8000-0000000000aa",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, failure) = json(&state, &read_only, "GET", "/v1/artifacts/families", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(failure["code"], "invalid_request");
    let (status, failure) = json(
        &state,
        &read_only,
        "GET",
        "/v1/artifacts/families/not-an-id",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(failure["code"], "invalid_request");
}

/// Bytes in through the Store's object path, metadata registered against
/// the hash, bytes back out through the content route — and the size the
/// version records is the Store's, not the request's.
#[tokio::test]
async fn content_round_trips_through_the_blob_store_by_hash() {
    let (_root, state, token) = state(true);
    let bytes = b"# Quarterly report\n\nRevenue is up.\n".to_vec();
    let (workspace, hash) = workspace_with_blob(&state, &token, &bytes).await;

    let (status, family) = json(
        &state,
        &token,
        "POST",
        "/v1/artifacts/families",
        Some(family_body(&workspace, 1)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{family}");
    let family_id = family["id"].as_str().unwrap().to_owned();
    assert_eq!(family["workspace"], workspace);
    assert!(family.get("retired_at_ms").is_none());

    let (status, registered) = json(
        &state,
        &token,
        "POST",
        &format!("/v1/artifacts/families/{family_id}/versions"),
        Some(version_body(1, &hash, None)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{registered}");
    assert_eq!(registered["retried"], false);
    let version = &registered["version"];
    assert_eq!(version["blob"], hash);
    assert_eq!(version["size_bytes"], bytes.len());
    assert_eq!(version["ordinal"], 1);
    assert_eq!(version["mime"], "text/markdown");
    let version_id = version["id"].as_str().unwrap().to_owned();

    let (status, headers, body) = send(
        &state,
        &token,
        "GET",
        &format!("/v1/artifacts/versions/{version_id}/content"),
        None,
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, bytes);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "text/markdown",
        "the Store's own headers pass through"
    );

    let (status, read) = json(
        &state,
        &token,
        "GET",
        &format!("/v1/artifacts/versions/{version_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&read, version);

    let (status, listed) = json(
        &state,
        &token,
        "GET",
        &format!("/v1/artifacts/families?workspace={workspace}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["families"].as_array().unwrap().len(), 1);
    let (status, with_versions) = json(
        &state,
        &token,
        "GET",
        &format!("/v1/artifacts/families/{family_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(with_versions["versions"].as_array().unwrap().len(), 1);

    // The same registration id again is the same version, not a second one.
    let (status, retried) = json(
        &state,
        &token,
        "POST",
        &format!("/v1/artifacts/families/{family_id}/versions"),
        Some(version_body(1, &hash, None)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(retried["retried"], true);
    assert_eq!(retried["version"]["id"], version_id);

    // A hash the workspace does not hold registers nothing.
    let absent = format!("b3:{}", "ee".repeat(32));
    let (status, failure) = json(
        &state,
        &token,
        "POST",
        &format!("/v1/artifacts/families/{family_id}/versions"),
        Some(version_body(2, &absent, None)),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(failure["code"], "blob_not_found");
}

#[tokio::test]
async fn a_version_is_immutable_at_the_api_layer_with_the_stable_code() {
    let (_root, state, token) = state(true);
    let (workspace, hash) = workspace_with_blob(&state, &token, b"v1").await;
    let (_, family) = json(
        &state,
        &token,
        "POST",
        "/v1/artifacts/families",
        Some(family_body(&workspace, 1)),
    )
    .await;
    let family_id = family["id"].as_str().unwrap();
    let (_, registered) = json(
        &state,
        &token,
        "POST",
        &format!("/v1/artifacts/families/{family_id}/versions"),
        Some(version_body(1, &hash, None)),
    )
    .await;
    let version_id = registered["version"]["id"].as_str().unwrap();
    for method in ["PATCH", "PUT", "DELETE"] {
        let (status, failure) = json(
            &state,
            &token,
            method,
            &format!("/v1/artifacts/versions/{version_id}"),
            Some(serde_json::json!({ "mime": "text/plain" })),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{method}");
        assert_eq!(failure["code"], "artifact_version_immutable", "{method}");
        assert_eq!(failure["retryable"], false);
    }
    let (status, unchanged) = json(
        &state,
        &token,
        "GET",
        &format!("/v1/artifacts/versions/{version_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(unchanged["mime"], "text/markdown");
}

#[tokio::test]
async fn a_metadata_body_over_the_route_limit_is_refused_with_a_stable_code() {
    let (_root, state, token) = state(true);
    let mut body = family_body("00000000-0000-4000-8000-0000000000aa", 1);
    body["title"] = serde_json::json!("x".repeat(koru_artifact_wire::METADATA_BODY_LIMIT + 1));
    let (status, _, bytes) = send(
        &state,
        &token,
        "POST",
        "/v1/artifacts/families",
        Some(serde_json::to_vec(&body).unwrap()),
        "application/json",
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let failure: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(failure["code"], "payload_too_large");
    // And the same body, under the limit but over the title's own bound, is
    // a plain bad request: the two refusals are different advice.
    body["title"] = serde_json::json!("x".repeat(600));
    let (status, failure) =
        json(&state, &token, "POST", "/v1/artifacts/families", Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(failure["code"], "invalid_request");
}

#[tokio::test]
async fn a_retired_family_refuses_new_versions_and_keeps_the_old() {
    let (_root, state, token) = state(true);
    let (workspace, hash) = workspace_with_blob(&state, &token, b"kept").await;
    let (_, family) = json(
        &state,
        &token,
        "POST",
        "/v1/artifacts/families",
        Some(family_body(&workspace, 1)),
    )
    .await;
    let family_id = family["id"].as_str().unwrap().to_owned();
    let (status, _) = json(
        &state,
        &token,
        "POST",
        &format!("/v1/artifacts/families/{family_id}/versions"),
        Some(version_body(1, &hash, None)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, retired) = json(
        &state,
        &token,
        "POST",
        &format!("/v1/artifacts/families/{family_id}/retire"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(retired["retired_at_ms"].is_i64());
    let (status, failure) = json(
        &state,
        &token,
        "POST",
        &format!("/v1/artifacts/families/{family_id}/versions"),
        Some(version_body(2, &hash, None)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(failure["code"], "artifact_family_retired");
    let (_, listed) = json(
        &state,
        &token,
        "GET",
        &format!("/v1/artifacts/families?workspace={workspace}"),
        None,
    )
    .await;
    assert!(listed["families"].as_array().unwrap().is_empty());
    let (_, listed) = json(
        &state,
        &token,
        "GET",
        &format!("/v1/artifacts/families?workspace={workspace}&include_retired=true"),
        None,
    )
    .await;
    assert_eq!(listed["families"].as_array().unwrap().len(), 1);
    let (status, with_versions) = json(
        &state,
        &token,
        "GET",
        &format!("/v1/artifacts/families/{family_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(with_versions["versions"].as_array().unwrap().len(), 1);
    let (status, failure) = json(
        &state,
        &token,
        "POST",
        "/v1/artifacts/families",
        Some(family_body(&workspace, 1)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(failure["code"], "artifact_family_exists");
}

/// `v1` produced by run R1; `v2` derived from it; R2 consumed `v2` and
/// produced `v3`. The walk from `v3` reads all three generations.
#[tokio::test]
async fn provenance_walks_three_generations_through_the_api() {
    let (_root, state, token) = state(true);
    let (workspace, hash) = workspace_with_blob(&state, &token, b"lineage").await;
    let (_, family) = json(
        &state,
        &token,
        "POST",
        "/v1/artifacts/families",
        Some(family_body(&workspace, 1)),
    )
    .await;
    let family_id = family["id"].as_str().unwrap().to_owned();
    let r1 = "00000000-0000-4000-8000-0000000000e1";
    let r2 = "00000000-0000-4000-8000-0000000000e2";
    let register = |n: u8, from: Option<String>, produced_by: Option<&str>| {
        let state = Arc::clone(&state);
        let token = token.clone();
        let family_id = family_id.clone();
        let hash = hash.clone();
        let produced_by = produced_by.map(str::to_owned);
        async move {
            let mut body = version_body(n, &hash, from.as_deref());
            if let Some(run) = produced_by {
                body["produced_by"] = serde_json::json!({ "run": run, "generation": 1 });
            }
            let (status, registered) = json(
                &state,
                &token,
                "POST",
                &format!("/v1/artifacts/families/{family_id}/versions"),
                Some(body),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{registered}");
            registered["version"]["id"].as_str().unwrap().to_owned()
        }
    };
    let link = |version: String, run: &str, role: &str| {
        let state = Arc::clone(&state);
        let token = token.clone();
        let run = run.to_owned();
        let role = role.to_owned();
        async move {
            let (status, link) = json(
                &state,
                &token,
                "POST",
                &format!("/v1/artifacts/versions/{version}/runs"),
                Some(serde_json::json!({ "run": run, "role": role })),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{link}");
            assert_eq!(link["role"], role);
        }
    };
    let v1 = register(1, None, Some(r1)).await;
    link(v1.clone(), r1, "output").await;
    let v2 = register(2, Some(v1.clone()), None).await;
    link(v2.clone(), r2, "input").await;
    let v3 = register(3, Some(v2.clone()), Some(r2)).await;
    link(v3.clone(), r2, "output").await;

    let (status, provenance) = json(
        &state,
        &token,
        "GET",
        &format!("/v1/artifacts/versions/{v3}/provenance"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{provenance}");
    assert_eq!(provenance["subject"]["id"], v3);
    assert_eq!(provenance["subject"]["derived_from"], v2);
    assert_eq!(provenance["subject"]["produced_by"]["run"], r2);
    let ancestors: Vec<_> = provenance["ancestors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|version| version["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ancestors, [v2.clone(), v1.clone()]);
    let runs = provenance["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 3);
    let has = |version: &str, run: &str, role: &str| {
        runs.iter()
            .any(|link| link["version"] == version && link["run"] == run && link["role"] == role)
    };
    assert!(has(&v3, r2, "output"));
    assert!(has(&v2, r2, "input"));
    assert!(has(&v1, r1, "output"));

    let (status, failure) = json(
        &state,
        &token,
        "GET",
        "/v1/artifacts/versions/00000000-0000-4000-8000-0000000000ff/provenance",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(failure["code"], "artifact_not_found");
}

/// Two roots, two databases: what one gateway registered the other cannot
/// see, and neither created the other's `artifacts/` directory.
#[tokio::test]
async fn two_roots_hold_two_artifact_stores() {
    let (root_a, a, token_a) = state(true);
    let (root_b, b, token_b) = state(true);
    let (workspace, _) = workspace_with_blob(&a, &token_a, b"a").await;
    let (status, _) = json(
        &a,
        &token_a,
        "POST",
        "/v1/artifacts/families",
        Some(family_body(&workspace, 1)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, on_b) = json(
        &b,
        &token_b,
        "GET",
        &format!("/v1/artifacts/families?workspace={workspace}&include_retired=true"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(on_b["families"].as_array().unwrap().is_empty());
    assert!(
        root_a
            .path()
            .join("artifacts")
            .join("artifacts.db")
            .exists()
    );
    assert!(
        root_b
            .path()
            .join("artifacts")
            .join("artifacts.db")
            .exists()
    );
}
