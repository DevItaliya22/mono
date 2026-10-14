// SPDX-License-Identifier: MPL-2.0

//! The six tools: their definitions as `tools/list` returns them, and
//! their dispatch.
//!
//! A definition is the SDK's [`Tool`] with a schema generated from the
//! wire's input type (which rejects unknown fields) and from its output
//! type; a result is the SDK's [`CallToolResult`] whose
//! `structuredContent` is always a JSON object, and a failure is
//! `{ code, message }` under `isError` — the same conventions as the
//! desktop bridge's tools, and the same three fields.
//!
//! Every call takes the resolved [`RunAuthorityRecord`] and nothing else
//! about the caller. `create` and `create_version` decide first (may this
//! run register here, and where do the bytes go), stage the file under the
//! register path rule, file the bytes through the Store, and only then
//! register — so a refused call touches no blob store, and a blob is never
//! filed for a registration that cannot land.

use std::sync::Arc;
use std::time::Instant;

use base64::Engine as _;
use koru_artifact_api::StagedFile;
use koru_artifact_core::RunAuthorityRecord;
use koru_artifact_wire::run_tools::{
    AttachOutput, ContentEncoding, ContentOutput, CreateInput, CreateVersionInput, MetadataOutput,
    READ_BOUND_BYTES, RegisteredOutput, SearchInput, SearchOutput, TOOL_NAMES, ToolFailure,
    VersionInput,
};
use rmcp::handler::server::tool::schema_for_type;
use rmcp::model::{CallToolRequestParams, CallToolResult, Tool, ToolAnnotations};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{blobs, staging};
use crate::GatewayState;

fn annotations(title: &str, read_only: bool, idempotent: bool) -> ToolAnnotations {
    ToolAnnotations::with_title(title)
        .read_only(read_only)
        .destructive(false)
        .idempotent(idempotent)
        .open_world(false)
}

fn tool<I: schemars::JsonSchema + 'static, O: schemars::JsonSchema + 'static>(
    name: &'static str,
    title: &str,
    description: &'static str,
    read_only: bool,
    idempotent: bool,
) -> Tool {
    Tool::new(name, description, schema_for_type::<I>())
        .with_title(title)
        .with_output_schema::<O>()
        .with_annotations(annotations(title, read_only, idempotent))
}

/// The six, in [`TOOL_NAMES`]' order.
pub(super) fn definitions() -> Vec<Tool> {
    let tools = vec![
        tool::<CreateInput, RegisteredOutput>(
            "create",
            "Register a new result",
            "Register a file you wrote under your .artifacts/<RunId>/ folder as a new artifact in the run's workspace. `path` is relative to that folder (or absolute and inside it); `title` is what a person will call the result. A second call with the same path and the same bytes returns the same version; the same path with new bytes registers the next version of the same artifact.",
            false,
            true,
        ),
        tool::<CreateVersionInput, RegisteredOutput>(
            "create_version",
            "Register a new version of an existing result",
            "Register a file under your .artifacts/<RunId>/ folder as the next immutable version of an artifact family you may reach — one you created, or one in a workspace granted to this run. The new version derives from the family's latest.",
            false,
            true,
        ),
        tool::<VersionInput, MetadataOutput>(
            "get",
            "Read a version's metadata",
            "The version — identity, family, ordinal, size, media type, filename, producer, derived_from — and its family. Reachable for a version this run made or one in a granted workspace.",
            true,
            true,
        ),
        tool::<VersionInput, ContentOutput>(
            "read",
            "Read a version's content",
            "The version's bytes, inline: UTF-8 text as `content` with encoding `utf8`, other bytes base64 with encoding `base64`. Above 1 MiB the answer is the size and reason `content_too_large` without the bytes.",
            true,
            true,
        ),
        tool::<SearchInput, SearchOutput>(
            "search",
            "Find artifacts by title",
            "Artifact families in the workspaces granted to this run whose title contains `query` (case-insensitive; an empty query lists them), each with its versions by ordinal. At most 50 families; `truncated` says whether more matched. With no granted workspace the call is permission_denied.",
            true,
            true,
        ),
        tool::<VersionInput, AttachOutput>(
            "attach_to_run",
            "Record a version as this run's input",
            "Record that this run consumed the version — input provenance, so a later reader can see what this result was built from. Idempotent.",
            false,
            true,
        ),
    ];
    debug_assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>(),
        TOOL_NAMES
    );
    tools
}

fn structured<T: Serialize>(value: &T) -> CallToolResult {
    CallToolResult::structured(serde_json::to_value(value).expect("a tool output serializes"))
}

fn failed(failure: &ToolFailure) -> CallToolResult {
    CallToolResult::structured_error(
        serde_json::to_value(failure).expect("a tool failure serializes"),
    )
}

fn arguments<T: DeserializeOwned>(params: &CallToolRequestParams) -> Result<T, ToolFailure> {
    let object = params.arguments.clone().unwrap_or_default();
    serde_json::from_value(serde_json::Value::Object(object)).map_err(|error| {
        // serde's message names the offending *field*, never a value.
        ToolFailure::new(
            "invalid_request",
            format!("The tool's arguments do not match its schema: {error}"),
        )
    })
}

/// Dispatch one `tools/call`. Answers a result whatever happens: a refusal
/// is a structured error, never a JSON-RPC error, so the model reads the
/// code and can act on it.
pub(super) async fn call(
    state: &Arc<GatewayState>,
    authority: Arc<RunAuthorityRecord>,
    params: CallToolRequestParams,
) -> CallToolResult {
    let started = Instant::now();
    let name = params.name.to_string();
    let outcome = match name.as_str() {
        "create" => create(state, &authority, &params).await,
        "create_version" => create_version(state, &authority, &params).await,
        "get" => get(state, &authority, &params).await,
        "read" => read(state, &authority, &params).await,
        "search" => search(state, &authority, &params).await,
        "attach_to_run" => attach_to_run(state, &authority, &params).await,
        _ => Err(ToolFailure::new(
            "unknown_tool",
            "The run endpoint offers exactly six tools; tools/list names them.",
        )),
    };
    let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
    match &outcome {
        Ok(_) if super::debug_enabled() => {
            tracing::info!(tool = %name, run = ?authority.run, elapsed_ms, "run tool completed");
        }
        Ok(_) => {}
        Err(failure) => {
            tracing::warn!(
                tool = %name,
                run = ?authority.run,
                code = %failure.code,
                elapsed_ms,
                "run tool refused"
            );
        }
    }
    match outcome {
        Ok(result) => result,
        Err(failure) => failed(&failure),
    }
}

async fn blocking<T, F>(operation: F) -> Result<T, ToolFailure>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ToolFailure> + Send + 'static,
{
    tokio::task::spawn_blocking(operation).await.map_err(|_| {
        ToolFailure::new(
            "artifacts_unavailable",
            "The artifact store did not answer.",
        )
    })?
}

fn domain(
    state: &Arc<GatewayState>,
) -> Result<Arc<dyn koru_artifact_api::ArtifactDomain>, ToolFailure> {
    state.artifact_domain().ok_or_else(|| {
        ToolFailure::new(
            "artifacts_unavailable",
            "The artifact store is not available.",
        )
    })
}

/// Stage under the rule, then file the bytes: the two steps every
/// registration shares once the decision has been made.
async fn stage_and_file(
    state: &Arc<GatewayState>,
    authority: &Arc<RunAuthorityRecord>,
    path: String,
    target: &koru_artifact_api::RegistrationTarget,
) -> Result<StagedFile, ToolFailure> {
    let record = Arc::clone(authority);
    let staged = blocking(move || {
        staging::stage(&record, &path, || {})
            .map_err(|error| ToolFailure::new(error.code(), error.message()))
    })
    .await?;
    let size_bytes = blobs::upload(
        state,
        authority.run,
        target.workspace,
        &staged,
        &target.mime,
    )
    .await?;
    Ok(StagedFile {
        relative_path: staged.relative_path,
        filename: staged.filename,
        blob: staged.hash,
        size_bytes,
    })
}

async fn create(
    state: &Arc<GatewayState>,
    authority: &Arc<RunAuthorityRecord>,
    params: &CallToolRequestParams,
) -> Result<CallToolResult, ToolFailure> {
    let input: CreateInput = arguments(params)?;
    let domain = domain(state)?;
    let target = {
        let (domain, record, input) = (Arc::clone(&domain), Arc::clone(authority), input.clone());
        blocking(move || domain.create_target(&record, &input.title, input.mime.as_deref())).await?
    };
    let staged = stage_and_file(state, authority, input.path.clone(), &target).await?;
    let record = Arc::clone(authority);
    let registered =
        blocking(move || domain.create(&record, staged, &input.title, input.mime.as_deref()))
            .await?;
    if !registered.retried {
        tracing::info!(
            run = ?authority.run,
            version = ?registered.version.id,
            family = ?registered.family.id,
            ordinal = registered.version.ordinal.get(),
            "a run registered an artifact version"
        );
    }
    Ok(structured(&registered))
}

async fn create_version(
    state: &Arc<GatewayState>,
    authority: &Arc<RunAuthorityRecord>,
    params: &CallToolRequestParams,
) -> Result<CallToolResult, ToolFailure> {
    let input: CreateVersionInput = arguments(params)?;
    let domain = domain(state)?;
    let target = {
        let (domain, record, input) = (Arc::clone(&domain), Arc::clone(authority), input.clone());
        blocking(move || domain.create_version_target(&record, input.family, input.mime.as_deref()))
            .await?
    };
    let staged = stage_and_file(state, authority, input.path.clone(), &target).await?;
    let record = Arc::clone(authority);
    let registered = blocking(move || {
        domain.create_version(&record, input.family, staged, input.mime.as_deref())
    })
    .await?;
    if !registered.retried {
        tracing::info!(
            run = ?authority.run,
            version = ?registered.version.id,
            family = ?registered.family.id,
            ordinal = registered.version.ordinal.get(),
            "a run registered an artifact version"
        );
    }
    Ok(structured(&registered))
}

async fn get(
    state: &Arc<GatewayState>,
    authority: &Arc<RunAuthorityRecord>,
    params: &CallToolRequestParams,
) -> Result<CallToolResult, ToolFailure> {
    let input: VersionInput = arguments(params)?;
    let domain = domain(state)?;
    let record = Arc::clone(authority);
    let metadata = blocking(move || domain.get(&record, input.version)).await?;
    Ok(structured(&metadata))
}

async fn read(
    state: &Arc<GatewayState>,
    authority: &Arc<RunAuthorityRecord>,
    params: &CallToolRequestParams,
) -> Result<CallToolResult, ToolFailure> {
    let input: VersionInput = arguments(params)?;
    let domain = domain(state)?;
    let record = Arc::clone(authority);
    let (version, workspace) =
        blocking(move || domain.content_target(&record, input.version)).await?;
    let mut output = ContentOutput {
        version: version.id,
        mime: version.mime.as_str().to_owned(),
        size_bytes: version.size_bytes,
        encoding: None,
        content: None,
        reason: None,
        bound_bytes: None,
    };
    if version.size_bytes > READ_BOUND_BYTES {
        output.reason = Some("content_too_large".into());
        output.bound_bytes = Some(READ_BOUND_BYTES);
        return Ok(structured(&output));
    }
    let bytes = blobs::download(state, authority.run, workspace, &version.blob).await?;
    match String::from_utf8(bytes) {
        Ok(text) => {
            output.encoding = Some(ContentEncoding::Utf8);
            output.content = Some(text);
        }
        Err(not_text) => {
            output.encoding = Some(ContentEncoding::Base64);
            output.content =
                Some(base64::engine::general_purpose::STANDARD.encode(not_text.into_bytes()));
        }
    }
    Ok(structured(&output))
}

async fn search(
    state: &Arc<GatewayState>,
    authority: &Arc<RunAuthorityRecord>,
    params: &CallToolRequestParams,
) -> Result<CallToolResult, ToolFailure> {
    let input: SearchInput = arguments(params)?;
    let domain = domain(state)?;
    let record = Arc::clone(authority);
    let hits = blocking(move || domain.search(&record, &input.query)).await?;
    Ok(structured(&hits))
}

async fn attach_to_run(
    state: &Arc<GatewayState>,
    authority: &Arc<RunAuthorityRecord>,
    params: &CallToolRequestParams,
) -> Result<CallToolResult, ToolFailure> {
    let input: VersionInput = arguments(params)?;
    let domain = domain(state)?;
    let record = Arc::clone(authority);
    let attached = blocking(move || domain.attach_to_run(&record, input.version)).await?;
    Ok(structured(&attached))
}
