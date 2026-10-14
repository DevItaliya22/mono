// SPDX-License-Identifier: MPL-2.0

//! The two routes a **person's** Artifacts surface needs over a run's
//! `.artifacts/<RunId>/` folder: what is in it that is not a result, and
//! D19's one-click Register that makes one.
//!
//! **The register path rule is not written twice.** Both handlers resolve
//! paths through [`crate::run_mcp::staging`] — the same function the run's
//! own `create` calls, with the same order of checks, the same stable codes
//! and the same "changed between listing and read" fingerprint. The only
//! difference is who is asking: the run endpoint resolves a run bearer to
//! its authority record, and these resolve the **session's own** gateway
//! bearer and then *look the record up* by run id. Nothing the call says
//! decides where bytes land.
//!
//! **Why that is not a widening** (ADR-355). A session holding
//! `artifacts.write` can already `POST /v1/artifacts/families` and
//! `.../{family}/versions` over any workspace it reaches, having put
//! arbitrary bytes in that workspace's blob store: the reach exists. What
//! Register adds is *narrower* than that door — the bytes must be a file
//! already inside one run's own folder, read under the path rule, with the
//! workspace, the family identity and the provenance all taken from the
//! durable authority record. The record's working-scope path is read here
//! and returned to nobody.
//!
//! **The scan is bounded and does no work proportional to a checkout.**
//! [`MAX_RUN_FOLDER_DEPTH`] levels, [`MAX_RUN_FOLDER_FILES`] files, and a
//! reparse point is skipped rather than followed — the same refusal the
//! path rule makes, made early so a junction cannot turn a listing into a
//! walk of somebody's home directory.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Extension, Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::{Json, extract::DefaultBodyLimit};
use koru_artifact_api::{ArtifactScope, RunFolderTarget, StagedFile};
use koru_artifact_core::{RelativePath, RunAuthorityRecord, RunId};
use koru_artifact_wire::Failure;
use koru_artifact_wire::run_folder::{
    MAX_RUN_FOLDER_DEPTH, MAX_RUN_FOLDER_FILES, RegisterFileRequest, RunFolder, UnregisteredFile,
    UnregisteredQuery, UnregisteredResponse,
};

use super::{admitted, answer, call, json_request, parse_id, refuse, status_for};
use crate::{GatewayPrincipal, GatewayState};

/// One file the scan found, before the store has been asked whether it is
/// already a result.
struct Found {
    path: RelativePath,
    size_bytes: u64,
    modified_at_ms: Option<i64>,
}

fn modified_ms(metadata: &fs::Metadata) -> Option<i64> {
    metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_millis()).ok())
}

/// Walk `folder`, breadth-first so the shallow files a person is looking
/// for come first, and stop at the bounds.
///
/// A directory entry that is a reparse point is skipped: the path rule
/// refuses a link anyway, and following one here would make a listing walk
/// wherever it pointed.
fn scan(folder: &Path) -> (Vec<Found>, bool) {
    let mut found = Vec::new();
    let mut truncated = false;
    // `(directory, depth)`, depth 1 being the folder itself.
    let mut queue = std::collections::VecDeque::from([(folder.to_path_buf(), 1usize)]);
    while let Some((directory, depth)) = queue.pop_front() {
        let Ok(entries) = fs::read_dir(&directory) else {
            truncated = true;
            continue;
        };
        let mut children: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
        children.sort();
        for child in children {
            if found.len() >= MAX_RUN_FOLDER_FILES {
                return (found, true);
            }
            let Ok(metadata) = fs::symlink_metadata(&child) else {
                truncated = true;
                continue;
            };
            let kind = metadata.file_type();
            if kind.is_symlink() {
                // A link is never followed, and never offered.
                continue;
            }
            if kind.is_dir() {
                if depth >= MAX_RUN_FOLDER_DEPTH {
                    truncated = true;
                } else {
                    queue.push_back((child, depth + 1));
                }
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let Ok(relative) = child.strip_prefix(folder) else {
                continue;
            };
            let spelling = relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            match RelativePath::new(spelling) {
                Ok(path) => found.push(Found {
                    path,
                    size_bytes: metadata.len(),
                    modified_at_ms: modified_ms(&metadata),
                }),
                // A name this device can spell but the wire's path type
                // cannot is left out rather than sent half-formed; the
                // listing says it was truncated so the surface does not
                // claim the folder is empty.
                Err(_) => truncated = true,
            }
        }
    }
    (found, truncated)
}

/// One run's folder as the surface reads it: the scan, then the store's
/// answer about which of the paths are already results.
async fn folder_for(
    service: &Arc<dyn koru_artifact_api::ArtifactDomain>,
    context: &koru_artifact_api::RequestContext,
    target: RunFolderTarget,
) -> Result<RunFolder, Failure> {
    let record = target.record.clone();
    let scanned = tokio::task::spawn_blocking(move || {
        let folder = record.artifacts_folder();
        // A folder that is not a directory — never created, deleted with
        // the temporary workspace, or replaced by a link — is *absent*,
        // which is a different sentence from *empty*.
        if !folder.is_dir() {
            return None;
        }
        Some(scan(&folder))
    })
    .await
    .map_err(|_| unavailable())?;

    let (found, truncated) = match scanned {
        Some(scanned) => scanned,
        None => {
            return Ok(RunFolder {
                run: target.record.run,
                workspace: target.record.workspace,
                folder_present: false,
                registered: target.registered,
                unregistered: Vec::new(),
                truncated: false,
            });
        }
    };
    let paths: Vec<RelativePath> = found.iter().map(|file| file.path.clone()).collect();
    let run = target.record.run;
    let service = Arc::clone(service);
    let context = context.clone();
    let unregistered: BTreeSet<RelativePath> =
        call(move || service.unregistered(context, run, paths))
            .await?
            .into_iter()
            .collect();
    Ok(RunFolder {
        run,
        workspace: target.record.workspace,
        folder_present: true,
        registered: target.registered,
        unregistered: found
            .into_iter()
            .filter(|file| unregistered.contains(&file.path))
            .map(|file| UnregisteredFile {
                path: file.path,
                size_bytes: file.size_bytes,
                modified_at_ms: file.modified_at_ms,
            })
            .collect(),
        truncated,
    })
}

fn unavailable() -> Failure {
    Failure::new(
        "artifacts_unavailable",
        "The artifact store is not available.",
        true,
    )
}

pub(super) async fn unregistered(
    State(state): State<Arc<GatewayState>>,
    Extension(principal): Extension<GatewayPrincipal>,
    query: Result<Query<UnregisteredQuery>, QueryRejection>,
) -> Response {
    let query = match query {
        Ok(Query(query)) => query,
        Err(_) => {
            return refuse(
                StatusCode::BAD_REQUEST,
                Failure::new(
                    "invalid_request",
                    "The unregistered-results query needs a workspace id.",
                    false,
                ),
            );
        }
    };
    let (service, context) = match admitted(&state, principal, ArtifactScope::Read) {
        Ok(values) => values,
        Err(response) => return response,
    };
    let targets = {
        let service = Arc::clone(&service);
        let context = context.clone();
        call(move || service.run_folders(context, query.workspace)).await
    };
    let targets = match targets {
        Ok(targets) => targets,
        Err(failure) => return refuse(status_for(&failure), failure),
    };
    let mut runs = Vec::with_capacity(targets.len());
    for target in targets {
        match folder_for(&service, &context, target).await {
            Ok(folder) => runs.push(folder),
            Err(failure) => return refuse(status_for(&failure), failure),
        }
    }
    answer(Ok::<_, Failure>(UnregisteredResponse { runs }))
}

/// D19's one click.
///
/// The order is the run endpoint's, for the run endpoint's reason: decide
/// first (may this session register, and where do the bytes go), stage the
/// file under the path rule, file the bytes, and only then register — so a
/// refused call touches no blob store, and a blob is never filed for a
/// registration that cannot land.
pub(super) async fn register_file(
    State(state): State<Arc<GatewayState>>,
    Extension(principal): Extension<GatewayPrincipal>,
    AxumPath(run): AxumPath<String>,
    payload: Result<Json<RegisterFileRequest>, JsonRejection>,
) -> Response {
    let request = match json_request(payload) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let run = match parse_id::<RunId>(&run) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let (service, context) = match admitted(&state, principal.clone(), ArtifactScope::Write) {
        Ok(values) => values,
        Err(response) => return response,
    };
    // The record, not the caller, says which folder and which workspace.
    // Read with the write scope's own context, which also carries Read.
    let target = {
        let service = Arc::clone(&service);
        let context = context.clone();
        call(move || service.run_folder(context, run)).await
    };
    let record = match target {
        Ok(target) => Arc::new(target.record),
        Err(failure) => return refuse(status_for(&failure), failure),
    };
    let target = {
        let service = Arc::clone(&service);
        let record = Arc::clone(&record);
        let context = context.clone();
        let mime = request.mime.clone();
        call(move || service.register_target(context, &record, mime.as_ref())).await
    };
    let target = match target {
        Ok(target) => target,
        Err(failure) => return refuse(status_for(&failure), failure),
    };

    let staged = match stage_and_file(&state, &record, &request.path, &target).await {
        Ok(staged) => staged,
        Err(failure) => return refuse(status_for(&failure), failure),
    };
    let registered = {
        let service = Arc::clone(&service);
        let record = Arc::clone(&record);
        let title = request.title.clone();
        let mime = request.mime.clone();
        call(move || service.register_file(context, &record, staged, title.as_ref(), mime.as_ref()))
            .await
    };
    match registered {
        Ok(registered) => {
            if !registered.retried {
                tracing::info!(
                    run = ?record.run,
                    version = ?registered.version.id,
                    family = ?registered.family.id,
                    ordinal = registered.version.ordinal.get(),
                    "a person registered a run's file as a result"
                );
            }
            answer(Ok::<_, Failure>(registered))
        }
        Err(failure) => refuse(status_for(&failure), failure),
    }
}

/// Stage under P1.11's rule and file the bytes — the run endpoint's own two
/// steps, called here so there is one path rule and one upload path.
async fn stage_and_file(
    state: &Arc<GatewayState>,
    record: &Arc<RunAuthorityRecord>,
    path: &RelativePath,
    target: &koru_artifact_api::RegistrationTarget,
) -> Result<StagedFile, Failure> {
    let staged = {
        let record = Arc::clone(record);
        let path = path.as_str().to_owned();
        tokio::task::spawn_blocking(move || {
            crate::run_mcp::staging::stage(&record, &path, || {})
                .map_err(|error| Failure::new(error.code(), error.message(), false))
        })
        .await
        .map_err(|_| unavailable())??
    };
    let size_bytes =
        crate::run_mcp::blobs::upload(state, record.run, target.workspace, &staged, &target.mime)
            .await
            .map_err(koru_artifact_api::failure_from)?;
    Ok(StagedFile {
        relative_path: staged.relative_path,
        filename: staged.filename,
        blob: staged.hash,
        size_bytes,
    })
}

/// The body limit both routes share: a register body carries a path, a
/// title and a media type, never bytes.
pub(super) fn body_limit() -> DefaultBodyLimit {
    DefaultBodyLimit::max(koru_artifact_wire::METADATA_BODY_LIMIT)
}

#[cfg(test)]
mod tests;
