// SPDX-License-Identifier: MPL-2.0

//! `/v1/runs` — delegated runs, answered by the delegate worker (D12).
//!
//! The same shape as the artifact routes: a catalogued table, one handler
//! per row, scope checked before dispatch, the domain answered through its
//! typed port ([`koru_delegate_api::DelegatePort`]) from `spawn_blocking`.
//! The port is composed by the host and handed to
//! [`GatewayState::set_delegate_port`]; when it is absent, or the worker
//! behind it is not answering, every route answers 503
//! `delegate_unavailable`, and the `delegate` capability says whether the
//! host composed one at all.
//!
//! **The transport seam.** These routes name a trait and nothing else.
//! On this device the trait is `LocalDelegate`, one private IPC connection
//! per call to `koru-agent.exe --worker delegate`; a cloud worker is a
//! second implementation, and nothing here changes for it.
//!
//! **A request is durable before it is answered.** `POST /v1/runs` is 202
//! because the worker has written the intent, captured the context and
//! committed the Run document before it replies (§7.1); what follows —
//! admission, the session, the provider — is the run's own business, and
//! `GET /v1/runs/{run}/stream` is how a caller watches it.

// `Result<T, Failure>` is the shape every route in this crate answers with
// (`artifacts.rs`, `ai.rs`); the failure's `details` map makes the variant
// large, and boxing it would make this file the odd one out.
#![allow(clippy::result_large_err)]

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, get, post};
use axum::{Json, Router};
use futures_util::stream;
use koru_delegate_api::wire::{
    CancelRunResponse, CreateRunRequest, CreateRunResponse, Failure, ListRunsQuery,
    ListRunsResponse, PurgeRunResponse, RateRunRequest, RateRunResponse, RestoreRunResponse,
    ResumeRunRequest, ResumeRunResponse, RunStreamItem, RunView, TimelineQuery, TrashRunRequest,
    TrashRunResponse,
};
use koru_delegate_api::{
    DelegatePort, DelegatePrincipal, DelegateScope, REQUEST_BODY_LIMIT, RequestContext, RunStream,
};
use koru_delegate_core::{RunId, WorkspaceId};
use schemars::{Schema, SchemaGenerator};
use serde::Deserialize;

use crate::{GatewayPrincipal, GatewayState};

type SchemaFor = fn(&mut SchemaGenerator) -> Schema;

pub struct RunRoute {
    pub operation_id: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    pub scope: DelegateScope,
    pub summary: &'static str,
    pub returns: &'static str,
    pub handler: fn() -> MethodRouter<Arc<GatewayState>>,
    pub request: Option<SchemaFor>,
    /// Whether the body may be left out entirely. A resume body is all
    /// defaults, so an ordinary resume is a bodyless POST.
    pub optional_body: bool,
    pub response: Option<SchemaFor>,
    pub path_parameter: Option<&'static str>,
    /// `?workspace=<id>`.
    pub workspace_query: bool,
    /// `?from=<index>&len=<count>&workspace=<id>`.
    pub window_query: bool,
    /// `?after=<index>`, and the answer is `text/event-stream`.
    pub stream: bool,
}

pub const ROUTES: &[RunRoute] = &[
    RunRoute {
        operation_id: "runs_create",
        method: "POST",
        path: "/v1/runs",
        scope: DelegateScope::Write,
        summary: "Delegate a piece of work to an agent",
        returns: "202 with the run's id, its Run document and its status. The client mints the run id, so a retried request is the same run. The request is durable — intent, captured context and Run document — before this answers; the start follows on the worker.",
        handler: || post(create).layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT)),
        request: Some(|generator| generator.subschema_for::<CreateRunRequest>()),
        optional_body: false,
        response: Some(|generator| generator.subschema_for::<CreateRunResponse>()),
        path_parameter: None,
        workspace_query: false,
        window_query: false,
        stream: false,
    },
    RunRoute {
        operation_id: "runs_create_v2",
        method: "POST",
        path: "/v2/runs",
        scope: DelegateScope::Write,
        summary: "Preserve model selection: Delegate a piece of work to an agent",
        returns: "202 with the run's id, its Run document and its status. The client mints the run id, so a retried request is the same run. The request is durable — intent, captured context and Run document — before this answers; the start follows on the worker.",
        handler: || post(create).layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT)),
        request: Some(|generator| generator.subschema_for::<CreateRunRequest>()),
        optional_body: false,
        response: Some(|generator| generator.subschema_for::<CreateRunResponse>()),
        path_parameter: None,
        workspace_query: false,
        window_query: false,
        stream: false,
    },
    RunRoute {
        operation_id: "runs_list",
        method: "GET",
        path: "/v1/runs",
        scope: DelegateScope::Read,
        summary: "The runs of a workspace",
        returns: "Every run registered in the workspace, oldest first, as its header — objective, agent, scope, status, executing device, results, economics, rating.",
        handler: || get(list),
        request: None,
        optional_body: false,
        response: Some(|generator| generator.subschema_for::<ListRunsResponse>()),
        path_parameter: None,
        workspace_query: true,
        window_query: false,
        stream: false,
    },
    RunRoute {
        operation_id: "runs_run",
        method: "GET",
        path: "/v1/runs/{run}",
        scope: DelegateScope::Read,
        summary: "One run, with a window of its history",
        returns: "The header, what the answering device holds, the current state of every context pin, and a window of the timeline by index, oldest first (`from`, `len` up to 500). `workspace` names the workspace a run this device did not create is registered in — how a second device reads a run that reached it by sync.",
        handler: || get(run),
        request: None,
        optional_body: false,
        response: Some(|generator| generator.subschema_for::<RunView>()),
        path_parameter: Some("run"),
        workspace_query: false,
        window_query: true,
        stream: false,
    },
    RunRoute {
        operation_id: "runs_cancel",
        method: "POST",
        path: "/v1/runs/{run}/cancel",
        scope: DelegateScope::Write,
        summary: "End the run",
        returns: "The header after the cancellation was asked for. A run mid-turn stays `running` until the provider answers the cancellation; the stream carries the end. Never a takeover — that is the session's own handover verb (D36).",
        handler: || post(cancel),
        request: None,
        optional_body: false,
        response: Some(|generator| generator.subschema_for::<CancelRunResponse>()),
        path_parameter: Some("run"),
        workspace_query: false,
        window_query: false,
        stream: false,
    },
    // No `POST /v1/runs/{run}/resume`: v2 superseded it and every client
    // already called v2 (docs/developers/API.md).
    RunRoute {
        operation_id: "runs_resume_v2",
        method: "POST",
        path: "/v2/runs/{run}/resume",
        scope: DelegateScope::Write,
        summary: "Preserve model selection: Continue an interrupted run as a fresh attempt",
        returns: "The header after the fresh attempt began. Refused with `not_interrupted` unless the run is `interrupted`, with `attempt_live` when the earlier attempt's session is still running, with `source_in_trash` or `source_unavailable` when a source the run was given no longer resolves for anything new (D48), and with `prior_attempt_unknown` when nothing proved the earlier attempt stopped — that one needs `{\"confirm\": true}` from a person. The body is optional and every field defaults.",
        handler: || post(resume),
        request: Some(|generator| generator.subschema_for::<ResumeRunRequest>()),
        optional_body: true,
        response: Some(|generator| generator.subschema_for::<ResumeRunResponse>()),
        path_parameter: Some("run"),
        workspace_query: false,
        window_query: false,
        stream: false,
    },
    RunRoute {
        operation_id: "runs_rate",
        method: "POST",
        path: "/v1/runs/{run}/rate",
        scope: DelegateScope::Write,
        summary: "Rate a finished run",
        returns: "The header carrying the rating. One gesture — good or not good — with an optional note that lives in the run's own content and reaches no event and no metric (D44). Refused with `wrong_status` while the run is still working.",
        handler: || post(rate).layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT)),
        request: Some(|generator| generator.subschema_for::<RateRunRequest>()),
        optional_body: false,
        response: Some(|generator| generator.subschema_for::<RateRunResponse>()),
        path_parameter: Some("run"),
        workspace_query: false,
        window_query: false,
        stream: false,
    },
    RunRoute {
        operation_id: "runs_trash",
        method: "POST",
        path: "/v1/runs/{run}/trash",
        scope: DelegateScope::Write,
        summary: "Put a run in the trash",
        returns: "The header and the Run document. **This is half of the gesture**: a run is a document of the workspace's manifest and putting it in the trash is that document's move into the workspace's bin, which is the caller's to make -- reversible, synced, and exactly what happens to every other document in Koru. What this does is the half only the worker can: refuse a run that is still working with `wrong_status` (cancel it first -- trashing something still writing into a folder is the one case where the reversible operation is not), and remember what the person said should happen to the run's results when the trash is emptied. Nothing on disk and nothing in the artifact store changes. The body is optional and its one field defaults to `keep`.",
        handler: || post(trash).layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT)),
        request: Some(|generator| generator.subschema_for::<TrashRunRequest>()),
        optional_body: true,
        response: Some(|generator| generator.subschema_for::<TrashRunResponse>()),
        path_parameter: Some("run"),
        workspace_query: false,
        window_query: false,
        stream: false,
    },
    RunRoute {
        operation_id: "runs_restore",
        method: "POST",
        path: "/v1/runs/{run}/restore",
        scope: DelegateScope::Write,
        summary: "Take a run back out of the trash",
        returns: "The header and the Run document, which the caller moves back out of the workspace's bin. Nothing was deleted, so the run comes back whole; what this forgets is the decision about its results.",
        handler: || post(restore),
        request: None,
        optional_body: false,
        response: Some(|generator| generator.subschema_for::<RestoreRunResponse>()),
        path_parameter: Some("run"),
        workspace_query: false,
        window_query: false,
        stream: false,
    },
    RunRoute {
        operation_id: "runs_purge",
        method: "POST",
        path: "/v1/runs/{run}/purge",
        scope: DelegateScope::Write,
        summary: "The trash was emptied: delete the run",
        returns: "Everything this device holds about the run -- its ledger rows, its authority record, and its working folder **when Koru made that folder**. A run scoped to a project ran inside the person's own repository, and that directory is left exactly alone: `folder` says `kept`, `removed` or `absent`, because a trash that quietly did less than it offered teaches somebody that it did more. `artifacts_retired` is how many result families went, which is none unless the person asked for them to go with the run. Refused with `run_folder_not_removed` when the filesystem would not remove a folder Koru made -- nothing is changed and the run stays in the trash. The Run document is the caller's to remove once this has answered.",
        handler: || post(purge),
        request: None,
        optional_body: false,
        response: Some(|generator| generator.subschema_for::<PurgeRunResponse>()),
        path_parameter: Some("run"),
        workspace_query: false,
        window_query: false,
        stream: false,
    },
    RunRoute {
        operation_id: "runs_stream",
        method: "GET",
        path: "/v1/runs/{run}/stream",
        scope: DelegateScope::Read,
        summary: "Follow a run's history and status",
        returns: "text/event-stream of `run_stream_item` events: every history entry after `after` with its index, then live entries as they are written, every status change, and `closed` when the run ended. `workspace` names the workspace a run this device did not create is registered in.",
        handler: || get(stream_run),
        request: None,
        optional_body: false,
        response: None,
        path_parameter: Some("run"),
        workspace_query: false,
        window_query: false,
        stream: true,
    },
];

pub fn router() -> Router<Arc<GatewayState>> {
    // `POST /v1/runs` and `GET /v1/runs` share a path and one method router.
    let mut router = Router::new();
    let mut by_path: Vec<(&'static str, MethodRouter<Arc<GatewayState>>)> = Vec::new();
    for route in ROUTES {
        match by_path.iter_mut().find(|(path, _)| *path == route.path) {
            Some((_, existing)) => {
                let merged = std::mem::take(existing).merge((route.handler)());
                *existing = merged;
            }
            None => by_path.push((route.path, (route.handler)())),
        }
    }
    for (path, handler) in by_path {
        router = router.route(path, handler);
    }
    router
}

pub fn extend_openapi(document: &mut serde_json::Value) {
    use serde_json::{Map, Value, json};

    let mut schemas = schemars::generate::SchemaSettings::openapi3().into_generator();
    let failure = schemas.subschema_for::<Failure>();
    let paths = document["paths"]
        .as_object_mut()
        .expect("gateway OpenAPI paths");
    for route in ROUTES {
        let mut operation = json!({
            "operationId": route.operation_id,
            "summary": route.summary,
            "description": route.returns,
            "security": [{"bearerAuth": [route.scope.as_str()]}],
            "responses": {
                "200": {"description": route.returns},
                "400": {"description": "The request body, query or path identity is invalid", "content": {"application/json": {"schema": failure}}},
                "401": {"description": "The gateway bearer is absent or invalid", "content": {"application/json": {"schema": failure}}},
                "403": {"description": "The bearer lacks the operation's run scope", "content": {"application/json": {"schema": failure}}},
                "404": {"description": "No run has that id on this device", "content": {"application/json": {"schema": failure}}},
                "409": {"description": "The run is not in a status the operation applies to, its earlier attempt is still live, or its prior attempt is unknown and a person has not confirmed the resume", "content": {"application/json": {"schema": failure}}},
                "413": {"description": "The request body exceeds the route limit", "content": {"application/json": {"schema": failure}}},
                "422": {"description": "A context item could not be captured, or a source the run was given is now in the trash or unavailable (D48)", "content": {"application/json": {"schema": failure}}},
                "503": {"description": "The delegate worker is not answering", "content": {"application/json": {"schema": failure}}}
            }
        });
        if matches!(route.operation_id, "runs_create" | "runs_create_v2") {
            operation["responses"]["202"] = operation["responses"]["200"].take();
            operation["responses"]
                .as_object_mut()
                .expect("responses")
                .remove("200");
        }
        if let Some(request) = route.request {
            operation["requestBody"] = json!({
                "required": !route.optional_body,
                "content": {"application/json": {"schema": request(&mut schemas)}}
            });
        }
        let mut parameters = Vec::new();
        if let Some(parameter) = route.path_parameter {
            parameters.push(json!({
                "name": parameter,
                "in": "path",
                "required": true,
                "schema": {"type": "string", "format": "uuid"}
            }));
        }
        if route.workspace_query {
            parameters.push(json!({
                "name": "workspace",
                "in": "query",
                "required": true,
                "schema": {"type": "string", "format": "uuid"}
            }));
        }
        if route.window_query {
            parameters.push(json!({"name": "from", "in": "query", "required": false, "schema": {"type": "integer", "format": "uint64", "default": 0}}));
            parameters.push(json!({"name": "len", "in": "query", "required": false, "schema": {"type": "integer", "format": "uint32", "default": 100, "maximum": 500}}));
            parameters.push(json!({"name": "workspace", "in": "query", "required": false, "schema": {"type": "string", "format": "uuid"}}));
        }
        if route.stream {
            parameters.push(json!({"name": "after", "in": "query", "required": false, "schema": {"type": "integer", "format": "uint64"}}));
            parameters.push(json!({"name": "workspace", "in": "query", "required": false, "schema": {"type": "string", "format": "uuid"}}));
        }
        if !parameters.is_empty() {
            operation["parameters"] = Value::Array(parameters);
        }
        let success = if matches!(route.operation_id, "runs_create" | "runs_create_v2") {
            "202"
        } else {
            "200"
        };
        if let Some(response) = route.response {
            operation["responses"][success]["content"] =
                json!({"application/json": {"schema": response(&mut schemas)}});
        } else if route.stream {
            operation["responses"][success]["content"] =
                json!({"text/event-stream": {"schema": schemas.subschema_for::<RunStreamItem>()}});
        }
        let entry = paths
            .entry(route.path.to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        entry
            .as_object_mut()
            .expect("run OpenAPI path")
            .insert(route.method.to_lowercase(), operation);
    }
    let components = document["components"]["schemas"]
        .as_object_mut()
        .expect("gateway OpenAPI schemas");
    components.extend(
        schemas
            .take_definitions(true)
            .into_iter()
            .map(|(name, schema)| (name, serde_json::to_value(schema).expect("run schema"))),
    );
}

// ------------------------------------------------------------------ handlers

async fn create(
    State(state): State<Arc<GatewayState>>,
    Extension(principal): Extension<GatewayPrincipal>,
    payload: Result<Json<CreateRunRequest>, JsonRejection>,
) -> Response {
    let request = match json_request(payload) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let (port, context) = match admitted(&state, principal, DelegateScope::Write) {
        Ok(values) => values,
        Err(response) => return response,
    };
    match call(move || port.create(context, request)).await {
        Ok(created) => (StatusCode::ACCEPTED, Json(created)).into_response(),
        Err(failure) => refuse(status_for(&failure), failure),
    }
}

async fn list(
    State(state): State<Arc<GatewayState>>,
    Extension(principal): Extension<GatewayPrincipal>,
    query: Result<Query<ListRunsQuery>, QueryRejection>,
) -> Response {
    let query = match query {
        Ok(Query(query)) => query,
        Err(_) => {
            return refuse(
                StatusCode::BAD_REQUEST,
                Failure::new(
                    "invalid_request",
                    "The runs query needs a workspace id.",
                    false,
                ),
            );
        }
    };
    let (port, context) = match admitted(&state, principal, DelegateScope::Read) {
        Ok(values) => values,
        Err(response) => return response,
    };
    answer(call(move || port.list(context, query.workspace)).await)
}

#[derive(Deserialize)]
struct WindowQuery {
    #[serde(default)]
    from: u64,
    len: Option<u32>,
    #[serde(default)]
    workspace: Option<WorkspaceId>,
}

async fn run(
    State(state): State<Arc<GatewayState>>,
    Extension(principal): Extension<GatewayPrincipal>,
    Path(run): Path<String>,
    query: Result<Query<WindowQuery>, QueryRejection>,
) -> Response {
    let run = match parse_run(&run) {
        Ok(run) => run,
        Err(response) => return response,
    };
    let window = match query {
        Ok(Query(query)) => TimelineQuery {
            from: query.from,
            len: query.len.unwrap_or(100),
            workspace: query.workspace,
        },
        Err(_) => {
            return refuse(
                StatusCode::BAD_REQUEST,
                Failure::new("invalid_request", "The window query is invalid.", false),
            );
        }
    };
    let (port, context) = match admitted(&state, principal, DelegateScope::Read) {
        Ok(values) => values,
        Err(response) => return response,
    };
    answer(call(move || port.run(context, run, window)).await)
}

async fn cancel(
    State(state): State<Arc<GatewayState>>,
    Extension(principal): Extension<GatewayPrincipal>,
    Path(run): Path<String>,
) -> Response {
    let run = match parse_run(&run) {
        Ok(run) => run,
        Err(response) => return response,
    };
    let (port, context) = match admitted(&state, principal, DelegateScope::Write) {
        Ok(values) => values,
        Err(response) => return response,
    };
    answer(call(move || port.cancel(context, run)).await)
}

async fn resume(
    State(state): State<Arc<GatewayState>>,
    Extension(principal): Extension<GatewayPrincipal>,
    Path(run): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let run = match parse_run(&run) {
        Ok(run) => run,
        Err(response) => return response,
    };
    let request = match resume_body(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let (port, context) = match admitted(&state, principal, DelegateScope::Write) {
        Ok(values) => values,
        Err(response) => return response,
    };
    answer(call(move || port.resume(context, run, request)).await)
}

async fn trash(
    State(state): State<Arc<GatewayState>>,
    Extension(principal): Extension<GatewayPrincipal>,
    Path(run): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let run = match parse_run(&run) {
        Ok(run) => run,
        Err(response) => return response,
    };
    let request = match optional_body::<TrashRunRequest>(&body, "trash") {
        Ok(request) => request,
        Err(response) => return response,
    };
    let (port, context) = match admitted(&state, principal, DelegateScope::Write) {
        Ok(values) => values,
        Err(response) => return response,
    };
    answer(call(move || port.trash(context, run, request)).await)
}

async fn restore(
    State(state): State<Arc<GatewayState>>,
    Extension(principal): Extension<GatewayPrincipal>,
    Path(run): Path<String>,
) -> Response {
    let run = match parse_run(&run) {
        Ok(run) => run,
        Err(response) => return response,
    };
    let (port, context) = match admitted(&state, principal, DelegateScope::Write) {
        Ok(values) => values,
        Err(response) => return response,
    };
    answer(call(move || port.restore(context, run)).await)
}

async fn purge(
    State(state): State<Arc<GatewayState>>,
    Extension(principal): Extension<GatewayPrincipal>,
    Path(run): Path<String>,
) -> Response {
    let run = match parse_run(&run) {
        Ok(run) => run,
        Err(response) => return response,
    };
    let (port, context) = match admitted(&state, principal, DelegateScope::Write) {
        Ok(values) => values,
        Err(response) => return response,
    };
    answer(call(move || port.purge(context, run)).await)
}

/// A body that may be left out entirely: nothing at all is the request's
/// own defaults. Anything sent must be the request object, so a typo is a
/// refusal rather than a silent default.
fn optional_body<T: serde::de::DeserializeOwned + Default>(
    body: &[u8],
    what: &'static str,
) -> Result<T, Response> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|_| {
        refuse(
            StatusCode::BAD_REQUEST,
            Failure::new(
                "invalid_request",
                format!("The {what} request body is invalid."),
                false,
            ),
        )
    })
}

/// The resume body is optional: nothing at all is `confirm: false`, which
/// is every resume of an ordinary interruption. Anything sent must be the
/// request object, so a typo is a refusal rather than a silent default.
fn resume_body(body: &[u8]) -> Result<ResumeRunRequest, Response> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(ResumeRunRequest::default());
    }
    serde_json::from_slice(body).map_err(|_| {
        refuse(
            StatusCode::BAD_REQUEST,
            Failure::new(
                "invalid_request",
                "The resume request body is invalid.",
                false,
            ),
        )
    })
}

#[derive(Deserialize)]
struct StreamQuery {
    after: Option<u64>,
    #[serde(default)]
    workspace: Option<WorkspaceId>,
}

/// The run's stream, read on a blocking thread and fed to the SSE
/// response through a bounded channel — no IPC on a request task.
async fn rate(
    State(state): State<Arc<GatewayState>>,
    Extension(principal): Extension<GatewayPrincipal>,
    Path(run): Path<String>,
    payload: Result<Json<RateRunRequest>, JsonRejection>,
) -> Response {
    let run = match parse_run(&run) {
        Ok(run) => run,
        Err(response) => return response,
    };
    let request = match json_request(payload) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let (port, context) = match admitted(&state, principal, DelegateScope::Write) {
        Ok(values) => values,
        Err(response) => return response,
    };
    answer(call(move || port.rate(context, run, request)).await)
}

async fn stream_run(
    State(state): State<Arc<GatewayState>>,
    Extension(principal): Extension<GatewayPrincipal>,
    Path(run): Path<String>,
    Query(query): Query<StreamQuery>,
) -> Response {
    let run = match parse_run(&run) {
        Ok(run) => run,
        Err(response) => return response,
    };
    let (port, context) = match admitted(&state, principal, DelegateScope::Read) {
        Ok(values) => values,
        Err(response) => return response,
    };
    let opened = call(move || port.stream(context, run, query.workspace, query.after)).await;
    let mut items: Box<dyn RunStream> = match opened {
        Ok(items) => items,
        Err(failure) => return refuse(status_for(&failure), failure),
    };
    let (send, receive) = tokio::sync::mpsc::channel::<RunStreamItem>(32);
    std::thread::Builder::new()
        .name("koru-gateway-run-stream".into())
        .stack_size(128 * 1024)
        .spawn(move || {
            while let Some(item) = items.next() {
                let closed = matches!(item, RunStreamItem::Closed);
                if send.blocking_send(item).is_err() || closed {
                    break;
                }
            }
        })
        .ok();
    let events = stream::unfold(receive, |mut receive| async move {
        let item = receive.recv().await?;
        let event = Event::default()
            .event("run_stream_item")
            .json_data(item)
            .expect("run stream DTO always serializes");
        Some((Ok::<_, Infallible>(event), receive))
    });
    Sse::new(events).into_response()
}

// ------------------------------------------------------------------ plumbing

type Port = Arc<dyn DelegatePort>;

fn admitted(
    state: &GatewayState,
    principal: GatewayPrincipal,
    required: DelegateScope,
) -> Result<(Port, RequestContext), Response> {
    if !principal
        .scopes
        .iter()
        .any(|scope| scope == required.as_str())
    {
        return Err(refuse(
            StatusCode::FORBIDDEN,
            Failure::new(
                "forbidden",
                "The gateway session does not permit this run operation.",
                false,
            ),
        ));
    }
    let Some(port) = state.delegate_port() else {
        return Err(refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            Failure::unavailable(),
        ));
    };
    let scopes = principal
        .scopes
        .iter()
        .filter_map(|scope| DelegateScope::parse(scope))
        .collect();
    Ok((
        port,
        RequestContext {
            principal: DelegatePrincipal {
                label: principal.label,
                scopes,
            },
            trace_id: uuid::Uuid::new_v4().to_string(),
        },
    ))
}

async fn call<T, F>(operation: F) -> Result<T, Failure>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, Failure> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| Failure::unavailable())?
}

fn answer<T: serde::Serialize>(result: Result<T, Failure>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(failure) => refuse(status_for(&failure), failure),
    }
}

fn refuse(status: StatusCode, failure: Failure) -> Response {
    let mut response = Json(failure).into_response();
    *response.status_mut() = status;
    response
}

/// The stable codes, one status each.
fn status_for(failure: &Failure) -> StatusCode {
    match failure.code.as_str() {
        "forbidden" => StatusCode::FORBIDDEN,
        "unknown_run" => StatusCode::NOT_FOUND,
        "wrong_status"
        | "not_interrupted"
        | "attempt_live"
        | "prior_attempt_unknown"
        | "illegal_transition"
        | "intent_conflict" => StatusCode::CONFLICT,
        "context_capture" | "source_in_trash" | "source_unavailable" | "source_unanswered" => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        "invalid_request" => StatusCode::BAD_REQUEST,
        // The folder is still there and nothing was deleted; trying again
        // once whatever is holding it lets go is the answer, which is what
        // 409 says here as it does for every other "not in a state this
        // applies to" above.
        "run_folder_not_removed" => StatusCode::CONFLICT,
        "not_delegatable" => StatusCode::NOT_IMPLEMENTED,
        "delegate_unavailable" | "delegate_timeout" | "delegate_incompatible" => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        _ if failure.retryable => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn parse_run(value: &str) -> Result<RunId, Response> {
    value.parse().map_err(|_| {
        refuse(
            StatusCode::BAD_REQUEST,
            Failure::new(
                "invalid_request",
                "The run path identity is not a canonical v4 UUID.",
                false,
            ),
        )
    })
}

fn json_request<T>(payload: Result<Json<T>, JsonRejection>) -> Result<T, Response> {
    payload.map(|Json(value)| value).map_err(|rejection| {
        if let JsonRejection::BytesRejection(_) = rejection {
            refuse(
                StatusCode::PAYLOAD_TOO_LARGE,
                Failure::new(
                    "payload_too_large",
                    "The run request body exceeds the route limit.",
                    false,
                ),
            )
        } else {
            refuse(
                StatusCode::BAD_REQUEST,
                Failure::new("invalid_request", "The run request body is invalid.", false),
            )
        }
    })
}

#[cfg(test)]
mod tests;
