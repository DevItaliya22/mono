// SPDX-License-Identifier: MPL-2.0

//! A run's bytes through the Store: filed under the target workspace on
//! `create` and `create_version`, read back for `read`.
//!
//! The same door a person's client uses — `PUT` and `GET
//! /v1/workspaces/{id}/blobs` on the embedded Store worker — with a caller
//! that names the run and holds exactly the Store scope the one call
//! needs. The hash the Store answers is checked against the hash computed
//! from the staged bytes, so a file that changed under the read is refused
//! here too.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, header};
use koru_artifact_core::{BlobHash, MediaType, RunId, WorkspaceId};
use koru_artifact_wire::run_tools::{READ_BOUND_BYTES, ToolFailure};
use koru_store_core::model::Scope;

use super::staging::Staged;
use crate::GatewayState;

fn caller(run: RunId, scope: Scope) -> koru_store_api::auth::Caller {
    koru_store_api::auth::Caller {
        label: format!("run:{}", run.to_storage()),
        scopes: vec![scope],
    }
}

fn store_unavailable(error: koru_store_server::WorkerError) -> ToolFailure {
    ToolFailure::new(
        crate::store_worker_code(error),
        "The Store worker did not answer for the run's bytes.",
    )
}

/// `X-Koru-Filename` is percent-encoded: a filename is arbitrary Unicode
/// and a header value is not. Unreserved ASCII passes; everything else is
/// `%XX` per UTF-8 byte.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// File the staged bytes in `workspace` and answer the size the Store
/// recorded. The hash is the Store's word and must be the staged one.
pub(crate) async fn upload(
    state: &Arc<GatewayState>,
    run: RunId,
    workspace: WorkspaceId,
    staged: &Staged,
    mime: &MediaType,
) -> Result<u64, ToolFailure> {
    let mut builder = Request::builder()
        .method("PUT")
        .uri(format!("/v1/workspaces/{}/blobs", workspace.to_storage()))
        .header(header::CONTENT_TYPE, mime.as_str());
    if let Some(filename) = &staged.filename {
        builder = builder.header("x-koru-filename", percent_encode(filename.as_str()));
    }
    let request = builder
        .body(Body::from(staged.bytes.clone()))
        .expect("a literal internal Store request is well-formed");
    let response = state
        .store
        .request(request, caller(run, Scope::EditDocuments))
        .await
        .map_err(store_unavailable)?;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap_or_default();
    if !status.is_success() {
        let code = serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|value| value["code"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "store_unavailable".to_owned());
        return Err(ToolFailure::new(
            code,
            "The Store refused to file the run's bytes.",
        ));
    }
    let blob: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
        ToolFailure::new(
            "store_unavailable",
            "The Store's answer for the filed bytes was unreadable.",
        )
    })?;
    let filed = blob["hash"]
        .as_str()
        .and_then(|hash| BlobHash::parse(hash).ok());
    if filed.as_ref() != Some(&staged.hash) {
        return Err(ToolFailure::new(
            "file_changed",
            "The Store filed different bytes than were staged.",
        ));
    }
    blob["size"].as_u64().ok_or_else(|| {
        ToolFailure::new(
            "store_unavailable",
            "The Store's answer for the filed bytes carried no size.",
        )
    })
}

/// The version's bytes from `workspace`, bounded: the caller has already
/// checked the recorded size against [`READ_BOUND_BYTES`], and this reads
/// no more than that bound plus one so a Store that answers more than it
/// recorded is caught rather than buffered.
pub(crate) async fn download(
    state: &Arc<GatewayState>,
    run: RunId,
    workspace: WorkspaceId,
    blob: &BlobHash,
) -> Result<Vec<u8>, ToolFailure> {
    let request = Request::builder()
        .method("GET")
        .uri(format!(
            "/v1/workspaces/{}/blobs/{}",
            workspace.to_storage(),
            blob.as_str()
        ))
        .body(Body::empty())
        .expect("a literal internal Store request is well-formed");
    let response = state
        .store
        .request(request, caller(run, Scope::ReadDocuments))
        .await
        .map_err(store_unavailable)?;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), READ_BOUND_BYTES as usize + 1)
        .await
        .map_err(|_| {
            ToolFailure::new(
                "content_too_large",
                "The Store answered more bytes than the version records.",
            )
        })?;
    if !status.is_success() {
        let code = serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|value| value["code"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "blob_not_found".to_owned());
        return Err(ToolFailure::new(
            code,
            "The Store did not answer with the version's bytes.",
        ));
    }
    Ok(bytes.to_vec())
}
