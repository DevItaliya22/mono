// SPDX-License-Identifier: MPL-2.0

//! The two run-folder routes in process, against a real `artifacts.db`, a
//! real Store worker and a real folder on disk under a temporary root.
//!
//! What is asserted here and nowhere else: that a person's Register goes
//! through **P1.11's** path rule rather than a second copy of it — every
//! refusal below is a code `run_mcp::staging` raises, reached through the
//! person's route with the person's bearer.

use std::fs;
use std::path::Path;

use axum::http::StatusCode;
use koru_artifact_core::ARTIFACTS_DIR;

use super::super::tests::{json, state, workspace_with_blob};

const RUN: &str = "00000000-0000-4000-8000-0000000000a1";

/// A run's working scope on disk, with its `.artifacts/<RunId>/` folder.
fn run_scope(root: &Path) -> std::path::PathBuf {
    let scope = root.join("scope");
    fs::create_dir_all(scope.join(ARTIFACTS_DIR).join(RUN)).unwrap();
    scope
}

fn write(scope: &Path, relative: &str, bytes: &[u8]) {
    let path = scope.join(ARTIFACTS_DIR).join(RUN).join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, bytes).unwrap();
}

/// A workspace and a run authority over `scope`, the way the orchestrator
/// would make one.
async fn workspace_and_authority(
    state: &std::sync::Arc<crate::GatewayState>,
    token: &str,
    scope: &Path,
) -> String {
    let (workspace, _) = workspace_with_blob(state, token, b"seed").await;
    let (status, issued) = json(
        state,
        token,
        "POST",
        "/v1/artifacts/authorities",
        Some(serde_json::json!({
            "run": RUN,
            "workspace": workspace,
            "generation": 1,
            "grants": [workspace],
            "run_workspace": scope.to_str().unwrap(),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{issued}");
    workspace
}

async fn unregistered(
    state: &std::sync::Arc<crate::GatewayState>,
    token: &str,
    workspace: &str,
) -> serde_json::Value {
    let (status, body) = json(
        state,
        token,
        "GET",
        &format!("/v1/artifacts/unregistered?workspace={workspace}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn register(
    state: &std::sync::Arc<crate::GatewayState>,
    token: &str,
    path: &str,
) -> (StatusCode, serde_json::Value) {
    json(
        state,
        token,
        "POST",
        &format!("/v1/artifacts/runs/{RUN}/register"),
        Some(serde_json::json!({ "path": path })),
    )
    .await
}

fn paths_of(folder: &serde_json::Value) -> Vec<String> {
    folder["unregistered"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap().to_owned())
        .collect()
}

/// The whole gesture, end to end: a run leaves files, the surface lists
/// them as *not results yet*, one click adopts one, and it moves.
#[tokio::test]
async fn a_person_registers_a_file_the_run_left_and_it_stops_being_unregistered() {
    let (root, state, token) = state(true);
    let scope = run_scope(root.path());
    write(&scope, "findings.md", b"# Findings\n");
    write(&scope, "out/data.csv", b"a,b\n1,2\n");
    let workspace = workspace_and_authority(&state, &token, &scope).await;

    let listing = unregistered(&state, &token, &workspace).await;
    let runs = listing["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    let folder = &runs[0];
    assert_eq!(folder["run"], RUN);
    assert_eq!(folder["folder_present"], true);
    // D29's fact, stated rather than inferred from an empty list.
    assert_eq!(folder["registered"], 0);
    assert_eq!(folder["truncated"], false);
    assert_eq!(paths_of(folder), vec!["findings.md", "out/data.csv"]);
    assert_eq!(folder["unregistered"][0]["size_bytes"], 11);

    let (status, registered) = register(&state, &token, "out/data.csv").await;
    assert_eq!(status, StatusCode::OK, "{registered}");
    assert_eq!(registered["retried"], false);
    // No title crossed, so the file's own last segment named the result.
    assert_eq!(registered["family"]["title"], "data.csv");
    assert_eq!(registered["family"]["created_by_run"], RUN);
    assert_eq!(registered["family"]["workspace"], workspace);
    assert_eq!(registered["version"]["ordinal"], 1);
    // Provenance came from the authority record, not from the request.
    assert_eq!(registered["version"]["produced_by"]["run"], RUN);
    assert_eq!(registered["version"]["size_bytes"], 8);

    let listing = unregistered(&state, &token, &workspace).await;
    let folder = &listing["runs"][0];
    assert_eq!(paths_of(folder), vec!["findings.md"]);
    assert_eq!(folder["registered"], 1);

    // And it is a result now: the family route answers with it.
    let family = registered["family"]["id"].as_str().unwrap();
    let (status, shown) = json(
        &state,
        &token,
        "GET",
        &format!("/v1/artifacts/families/{family}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["versions"].as_array().unwrap().len(), 1);
}

/// **The same path rule, reached through the person's door.** Every one of
/// these codes is `run_mcp::staging`'s, which is the point: there is one
/// rule about what may be registered, not one per caller.
#[tokio::test]
async fn every_register_refusal_is_the_run_endpoints_own_path_rule() {
    let (root, state, token) = state(true);
    let scope = run_scope(root.path());
    write(&scope, "findings.md", b"# Findings\n");
    fs::create_dir_all(scope.join(ARTIFACTS_DIR).join(RUN).join("nested")).unwrap();
    // A file the run's folder does not contain, to escape towards.
    fs::write(root.path().join("secret.txt"), b"not yours").unwrap();
    workspace_and_authority(&state, &token, &scope).await;

    for (path, code) in [
        ("../secret.txt", "invalid_request"),
        ("missing.md", "file_not_found"),
        ("nested", "not_a_file"),
    ] {
        let (status, body) = register(&state, &token, path).await;
        assert_eq!(body["code"], code, "{path}: {body}");
        assert!(!status.is_success(), "{path}");
        // A refusal never echoes the path back.
        assert!(
            !body["message"].as_str().unwrap().contains("secret"),
            "{body}"
        );
    }

    // An absolute path outside the folder resolves outside it and is
    // refused as such — the containment check, not the lexical one.
    let outside = root.path().join("secret.txt");
    let (status, body) = register(&state, &token, outside.to_str().unwrap()).await;
    assert!(!status.is_success());
    assert_eq!(body["code"], "invalid_request", "{body}");

    // The folder itself going away is its own code, because "your results
    // folder is gone" and "that file is gone" are different problems.
    fs::remove_dir_all(scope.join(ARTIFACTS_DIR)).unwrap();
    let (status, body) = register(&state, &token, "findings.md").await;
    assert!(!status.is_success());
    assert_eq!(body["code"], "run_folder_missing", "{body}");
}

/// A retired result takes no new version, and the person's route says so
/// with the database's own code (the third refusal the roadmap names).
#[tokio::test]
async fn registering_into_a_retired_result_answers_the_retired_code() {
    let (root, state, token) = state(true);
    let scope = run_scope(root.path());
    write(&scope, "findings.md", b"one\n");
    workspace_and_authority(&state, &token, &scope).await;

    let (status, first) = register(&state, &token, "findings.md").await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let family = first["family"]["id"].as_str().unwrap().to_owned();
    let (status, _) = json(
        &state,
        &token,
        "POST",
        &format!("/v1/artifacts/families/{family}/retire"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // New bytes at the same path would be the next version, and the family
    // will not take one.
    write(&scope, "findings.md", b"two\n");
    let (status, body) = register(&state, &token, "findings.md").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "artifact_family_retired");
}

/// D37: the folder outlives the run's workspace, and a folder that has gone
/// anyway is a state the listing names rather than an emptiness it draws.
#[tokio::test]
async fn a_run_whose_folder_is_gone_is_listed_as_absent_not_as_empty() {
    let (root, state, token) = state(true);
    let scope = run_scope(root.path());
    let workspace = workspace_and_authority(&state, &token, &scope).await;

    let listing = unregistered(&state, &token, &workspace).await;
    assert_eq!(listing["runs"][0]["folder_present"], true);
    assert!(
        listing["runs"][0]["unregistered"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    fs::remove_dir_all(scope.join(ARTIFACTS_DIR)).unwrap();
    let listing = unregistered(&state, &token, &workspace).await;
    assert_eq!(listing["runs"][0]["folder_present"], false);
    assert_eq!(listing["runs"][0]["registered"], 0);
}

/// A workspace whose runs are somebody else's is not this workspace's
/// business, and a store that never opened answers 503 rather than an
/// empty list — the same rule every other artifact route keeps.
#[tokio::test]
async fn the_listing_is_workspace_scoped_and_needs_the_store() {
    let (root, opened, token) = state(true);
    let scope = run_scope(root.path());
    write(&scope, "findings.md", b"one\n");
    workspace_and_authority(&opened, &token, &scope).await;
    let (other, _) = workspace_with_blob(&opened, &token, b"other").await;
    let listing = unregistered(&opened, &token, &other).await;
    assert!(listing["runs"].as_array().unwrap().is_empty(), "{listing}");

    let (_root, without, token) = state(false);
    let (status, body) = json(
        &without,
        &token,
        "GET",
        &format!("/v1/artifacts/unregistered?workspace={other}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["code"], "artifacts_unavailable");
    let (status, body) = json(
        &without,
        &token,
        "POST",
        &format!("/v1/artifacts/runs/{RUN}/register"),
        Some(serde_json::json!({ "path": "a.md" })),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
}

/// A second press is the same version, not a second one — the run's own
/// idempotency, reached through the person's door.
#[tokio::test]
async fn pressing_register_twice_answers_the_same_version() {
    let (root, state, token) = state(true);
    let scope = run_scope(root.path());
    write(&scope, "findings.md", b"one\n");
    workspace_and_authority(&state, &token, &scope).await;

    let (_, first) = register(&state, &token, "findings.md").await;
    let (status, second) = register(&state, &token, "findings.md").await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["retried"], true);
    assert_eq!(second["version"]["id"], first["version"]["id"]);
}
