// SPDX-License-Identifier: MPL-2.0

//! `/v1/schedules` — durable schedule creation, listing, and content-free
//! invalidation through the delegate worker.

#![allow(clippy::result_large_err)]

use std::{convert::Infallible, sync::Arc};

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Extension, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, get, post};
use axum::{Json, Router};
use futures_util::stream;
use koru_delegate_api::wire::{
    CreateScheduleRequest, CreateScheduleResponse, EditScheduleRequest, EditScheduleResponse,
    Failure, ListSchedulesQuery, ListSchedulesResponse, PauseScheduleRequest,
    PauseScheduleResponse, RenameScheduleRequest, RenameScheduleResponse, ResumeScheduleRequest,
    ResumeScheduleResponse, RetireScheduleRequest, RetireScheduleResponse, ScheduleChange,
};
use koru_delegate_api::{
    DelegatePort, DelegatePrincipal, DelegateScope, REQUEST_BODY_LIMIT, RequestContext,
    ScheduleStream, ScheduleStreamCancellation,
};
use schemars::{Schema, SchemaGenerator};
use serde::Deserialize;

use crate::{GatewayPrincipal, GatewayState};

type SchemaFor = fn(&mut SchemaGenerator) -> Schema;

pub struct ScheduleRoute {
    pub operation_id: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    pub scope: DelegateScope,
    pub summary: &'static str,
    pub returns: &'static str,
    pub handler: fn() -> MethodRouter<Arc<GatewayState>>,
    pub request: Option<SchemaFor>,
    pub response: Option<SchemaFor>,
    pub workspace_query: bool,
    pub stream: bool,
}

pub const ROUTES: &[ScheduleRoute] = &[
    ScheduleRoute {
        operation_id: "schedules_pause",
        method: "POST",
        path: "/v1/schedules/pause",
        scope: DelegateScope::Write,
        summary: "Pause a schedule",
        returns: "200 with the canonical paused schedule projection. A claimed Run is not cancelled.",
        handler: || post(pause).layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT)),
        request: Some(|generator| generator.subschema_for::<PauseScheduleRequest>()),
        response: Some(|generator| generator.subschema_for::<PauseScheduleResponse>()),
        workspace_query: false,
        stream: false,
    },
    ScheduleRoute {
        operation_id: "schedules_resume",
        method: "POST",
        path: "/v1/schedules/resume",
        scope: DelegateScope::Write,
        summary: "Resume a schedule",
        returns: "200 with the canonical active schedule projection. Elapsed paused occurrences are not caught up.",
        handler: || post(resume).layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT)),
        request: Some(|generator| generator.subschema_for::<ResumeScheduleRequest>()),
        response: Some(|generator| generator.subschema_for::<ResumeScheduleResponse>()),
        workspace_query: false,
        stream: false,
    },
    ScheduleRoute {
        operation_id: "schedules_retire",
        method: "POST",
        path: "/v1/schedules/retire",
        scope: DelegateScope::Write,
        summary: "Delete a schedule",
        returns: "200 with the canonical retired schedule projection. Existing Runs and their provenance remain unchanged.",
        handler: || post(retire).layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT)),
        request: Some(|generator| generator.subschema_for::<RetireScheduleRequest>()),
        response: Some(|generator| generator.subschema_for::<RetireScheduleResponse>()),
        workspace_query: false,
        stream: false,
    },
    ScheduleRoute {
        operation_id: "schedules_edit",
        method: "POST",
        path: "/v1/schedules/edit",
        scope: DelegateScope::Write,
        summary: "Edit a schedule",
        returns: "200 with the canonical schedule projection after one durable revision-checked replacement of its editable definition and name.",
        handler: || post(edit).layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT)),
        request: Some(|generator| generator.subschema_for::<EditScheduleRequest>()),
        response: Some(|generator| generator.subschema_for::<EditScheduleResponse>()),
        workspace_query: false,
        stream: false,
    },
    ScheduleRoute {
        operation_id: "schedules_create",
        method: "POST",
        path: "/v1/schedules",
        scope: DelegateScope::Write,
        summary: "Create repeating delegated work",
        returns: "201 with the canonical durable Schedule document projection. The client supplies the ScheduleId, workspace, reusable delegation template and recurrence rule; the worker stamps owner, revision, lifecycle and times.",
        handler: || post(create).layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT)),
        request: Some(|generator| generator.subschema_for::<CreateScheduleRequest>()),
        response: Some(|generator| generator.subschema_for::<CreateScheduleResponse>()),
        workspace_query: false,
        stream: false,
    },
    ScheduleRoute {
        operation_id: "schedules_rename",
        method: "POST",
        path: "/v1/schedules/rename",
        scope: DelegateScope::Write,
        summary: "Rename a schedule",
        returns: "200 with the canonical schedule projection after its durable owner-authorized name change.",
        handler: || post(rename).layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT)),
        request: Some(|generator| generator.subschema_for::<RenameScheduleRequest>()),
        response: Some(|generator| generator.subschema_for::<RenameScheduleResponse>()),
        workspace_query: false,
        stream: false,
    },
    ScheduleRoute {
        operation_id: "schedules_list",
        method: "GET",
        path: "/v1/schedules",
        scope: DelegateScope::Read,
        summary: "The schedules of a workspace",
        returns: "Canonical definitions, reduced ownership/lifecycle state, truthful next-firing projections, and firing history. A firing carries a RunId only when it actually fired.",
        handler: || get(list),
        request: None,
        response: Some(|generator| generator.subschema_for::<ListSchedulesResponse>()),
        workspace_query: true,
        stream: false,
    },
    ScheduleRoute {
        operation_id: "schedules_stream",
        method: "GET",
        path: "/v1/schedules/stream",
        scope: DelegateScope::Read,
        summary: "Watch a schedule list for changes",
        returns: "A bounded text/event-stream of content-free schedule generations. Each item means to read the workspace list again; no objective, context, path, document id or schedule id travels in the event.",
        handler: || get(watch),
        request: None,
        response: None,
        workspace_query: true,
        stream: true,
    },
];

pub fn router() -> Router<Arc<GatewayState>> {
    let mut router = Router::new();
    let mut by_path: Vec<(&'static str, MethodRouter<Arc<GatewayState>>)> = Vec::new();
    for route in ROUTES {
        match by_path.iter_mut().find(|(path, _)| *path == route.path) {
            Some((_, existing)) => {
                *existing = std::mem::take(existing).merge((route.handler)());
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
    let paths = document["paths"].as_object_mut().expect("gateway paths");
    for route in ROUTES {
        let success = if route.operation_id == "schedules_create" {
            "201"
        } else {
            "200"
        };
        let mut operation = json!({
            "operationId": route.operation_id,
            "summary": route.summary,
            "description": route.returns,
            "security": [{"bearerAuth": [route.scope.as_str()]}],
            "responses": {
                "400": {"description": "The request body or workspace query is invalid", "content": {"application/json": {"schema": failure}}},
                "401": {"description": "The gateway bearer is absent or invalid", "content": {"application/json": {"schema": failure}}},
                "403": {"description": "The bearer lacks the required delegation scope", "content": {"application/json": {"schema": failure}}},
                "503": {"description": "The delegate worker or schedule store is not answering", "content": {"application/json": {"schema": failure}}}
            }
        });
        if route.operation_id == "schedules_create" {
            let responses = operation["responses"]
                .as_object_mut()
                .expect("schedule responses");
            responses.insert(
                "409".into(),
                json!({"description": "The schedule identity cannot be used in this workspace", "content": {"application/json": {"schema": failure}}}),
            );
            responses.insert(
                "413".into(),
                json!({"description": "The schedule request exceeds the body limit", "content": {"application/json": {"schema": failure}}}),
            );
            responses.insert(
                "422".into(),
                json!({"description": "The schedule definition is invalid", "content": {"application/json": {"schema": failure}}}),
            );
        } else if matches!(
            route.operation_id,
            "schedules_rename"
                | "schedules_edit"
                | "schedules_pause"
                | "schedules_resume"
                | "schedules_retire"
        ) {
            let responses = operation["responses"]
                .as_object_mut()
                .expect("schedule responses");
            responses.insert(
                "404".into(),
                json!({"description": "The schedule is missing or belongs to another workspace", "content": {"application/json": {"schema": failure}}}),
            );
            responses.insert(
                "409".into(),
                json!({"description": "The observed name is stale, or the schedule is retired or read-only", "content": {"application/json": {"schema": failure}}}),
            );
            responses.insert(
                "413".into(),
                json!({"description": "The schedule request exceeds the body limit", "content": {"application/json": {"schema": failure}}}),
            );
        }
        operation["responses"][success] = json!({"description": route.returns});
        if let Some(request) = route.request {
            operation["requestBody"] = json!({
                "required": true,
                "content": {"application/json": {"schema": request(&mut schemas)}}
            });
        }
        if route.workspace_query {
            let mut parameters = vec![json!({
                "name": "workspace", "in": "query", "required": true,
                "schema": {"type": "string", "format": "uuid"}
            })];
            if route.stream {
                parameters.extend([
                    json!({"name": "after_instance", "in": "query", "required": false, "schema": {"type": "string", "format": "uuid"}}),
                    json!({"name": "after_generation", "in": "query", "required": false, "schema": {"type": "integer", "format": "uint64"}}),
                ]);
            }
            operation["parameters"] = Value::Array(parameters);
        }
        if let Some(response) = route.response {
            operation["responses"][success]["content"] =
                json!({"application/json": {"schema": response(&mut schemas)}});
        } else if route.stream {
            operation["responses"][success]["content"] =
                json!({"text/event-stream": {"schema": schemas.subschema_for::<ScheduleChange>()}});
        }
        let entry = paths
            .entry(route.path.to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        entry
            .as_object_mut()
            .expect("schedule OpenAPI path")
            .insert(route.method.to_lowercase(), operation);
    }
    document["components"]["schemas"]
        .as_object_mut()
        .expect("gateway schemas")
        .extend(
            schemas
                .take_definitions(true)
                .into_iter()
                .map(|(name, schema)| {
                    (name, serde_json::to_value(schema).expect("schedule schema"))
                }),
        );
}

async fn create(
    State(state): State<Arc<GatewayState>>,
    Extension(principal): Extension<GatewayPrincipal>,
    payload: Result<Json<CreateScheduleRequest>, JsonRejection>,
) -> Response {
    let request = match json_request(payload) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let (port, context) = match admitted(&state, principal, DelegateScope::Write) {
        Ok(values) => values,
        Err(response) => return response,
    };
    match call(move || port.create_schedule(context, request)).await {
        Ok(created) => (StatusCode::CREATED, Json(created)).into_response(),
        Err(failure) => refuse(status_for(&failure), failure),
    }
}

async fn rename(
    State(state): State<Arc<GatewayState>>,
    Extension(principal): Extension<GatewayPrincipal>,
    payload: Result<Json<RenameScheduleRequest>, JsonRejection>,
) -> Response {
    let request = match json_request(payload) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let (port, context) = match admitted(&state, principal, DelegateScope::Write) {
        Ok(values) => values,
        Err(response) => return response,
    };
    answer(call(move || port.rename_schedule(context, request)).await)
}

async fn edit(
    State(state): State<Arc<GatewayState>>,
    Extension(principal): Extension<GatewayPrincipal>,
    payload: Result<Json<EditScheduleRequest>, JsonRejection>,
) -> Response {
    let request = match json_request(payload) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let (port, context) = match admitted(&state, principal, DelegateScope::Write) {
        Ok(values) => values,
        Err(response) => return response,
    };
    answer(call(move || port.edit_schedule(context, request)).await)
}

async fn pause(
    State(state): State<Arc<GatewayState>>,
    Extension(principal): Extension<GatewayPrincipal>,
    payload: Result<Json<PauseScheduleRequest>, JsonRejection>,
) -> Response {
    let request = match json_request(payload) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let (port, context) = match admitted(&state, principal, DelegateScope::Write) {
        Ok(values) => values,
        Err(response) => return response,
    };
    answer(call(move || port.pause_schedule(context, request)).await)
}

async fn resume(
    State(state): State<Arc<GatewayState>>,
    Extension(principal): Extension<GatewayPrincipal>,
    payload: Result<Json<ResumeScheduleRequest>, JsonRejection>,
) -> Response {
    let request = match json_request(payload) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let (port, context) = match admitted(&state, principal, DelegateScope::Write) {
        Ok(values) => values,
        Err(response) => return response,
    };
    answer(call(move || port.resume_schedule(context, request)).await)
}

async fn retire(
    State(state): State<Arc<GatewayState>>,
    Extension(principal): Extension<GatewayPrincipal>,
    payload: Result<Json<RetireScheduleRequest>, JsonRejection>,
) -> Response {
    let request = match json_request(payload) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let (port, context) = match admitted(&state, principal, DelegateScope::Write) {
        Ok(values) => values,
        Err(response) => return response,
    };
    answer(call(move || port.retire_schedule(context, request)).await)
}

async fn list(
    State(state): State<Arc<GatewayState>>,
    Extension(principal): Extension<GatewayPrincipal>,
    query: Result<Query<ListSchedulesQuery>, QueryRejection>,
) -> Response {
    let workspace = match query {
        Ok(Query(query)) => query.workspace,
        Err(_) => return invalid_query(),
    };
    let (port, context) = match admitted(&state, principal, DelegateScope::Read) {
        Ok(values) => values,
        Err(response) => return response,
    };
    answer(call(move || port.list_schedules(context, workspace)).await)
}

#[derive(Deserialize)]
struct WatchQuery {
    workspace: koru_delegate_core::WorkspaceId,
    #[serde(default)]
    after_instance: Option<String>,
    #[serde(default)]
    after_generation: Option<u64>,
}

async fn watch(
    State(state): State<Arc<GatewayState>>,
    Extension(principal): Extension<GatewayPrincipal>,
    query: Result<Query<WatchQuery>, QueryRejection>,
) -> Response {
    let query = match query {
        Ok(Query(query)) => query,
        Err(_) => return invalid_query(),
    };
    let after = match (query.after_instance, query.after_generation) {
        (None, None) => None,
        (Some(instance), Some(generation)) => {
            let Ok(instance) = uuid::Uuid::parse_str(&instance) else {
                return invalid_query();
            };
            Some(ScheduleChange {
                instance,
                generation,
            })
        }
        _ => return invalid_query(),
    };
    let (port, context) = match admitted(&state, principal, DelegateScope::Read) {
        Ok(values) => values,
        Err(response) => return response,
    };
    let opened = call(move || port.watch_schedules(context, query.workspace, after)).await;
    let mut changes: Box<dyn ScheduleStream> = match opened {
        Ok(changes) => changes,
        Err(failure) => return refuse(status_for(&failure), failure),
    };
    let cancellation = changes.cancellation();
    let (send, receive) = tokio::sync::mpsc::channel::<ScheduleChange>(16);
    let spawned = std::thread::Builder::new()
        .name("koru-gateway-schedule-stream".into())
        .stack_size(128 * 1024)
        .spawn(move || {
            while let Some(change) = changes.next() {
                if send.try_send(change).is_err() {
                    break;
                }
            }
        });
    let bridge = match spawned {
        Ok(bridge) => bridge,
        Err(_) => return refuse(StatusCode::SERVICE_UNAVAILABLE, Failure::unavailable()),
    };
    let state = ScheduleSseState {
        receive,
        cancellation,
    };
    drop(bridge);
    let events = stream::unfold(state, |mut state| async move {
        let change = state.receive.recv().await?;
        let event = Event::default()
            .event("schedule_change")
            .json_data(change)
            .expect("schedule change always serializes");
        Some((Ok::<_, Infallible>(event), state))
    });
    Sse::new(events).into_response()
}

struct ScheduleSseState {
    receive: tokio::sync::mpsc::Receiver<ScheduleChange>,
    cancellation: Arc<dyn ScheduleStreamCancellation>,
}

impl Drop for ScheduleSseState {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

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
                "The gateway session does not permit this schedule operation.",
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

fn json_request<T>(payload: Result<Json<T>, JsonRejection>) -> Result<T, Response> {
    payload.map(|Json(value)| value).map_err(|rejection| {
        if let JsonRejection::BytesRejection(_) = rejection {
            refuse(
                StatusCode::PAYLOAD_TOO_LARGE,
                Failure::new(
                    "payload_too_large",
                    "The schedule request body is too large.",
                    false,
                ),
            )
        } else {
            refuse(
                StatusCode::BAD_REQUEST,
                Failure::new(
                    "invalid_request",
                    "The schedule request body is invalid.",
                    false,
                ),
            )
        }
    })
}

fn invalid_query() -> Response {
    refuse(
        StatusCode::BAD_REQUEST,
        Failure::new(
            "invalid_request",
            "The schedules query needs a workspace id and a complete optional generation.",
            false,
        ),
    )
}

fn status_for(failure: &Failure) -> StatusCode {
    match failure.code.as_str() {
        "forbidden" => StatusCode::FORBIDDEN,
        "invalid_request" => StatusCode::BAD_REQUEST,
        "invalid_schedule" => StatusCode::UNPROCESSABLE_ENTITY,
        "schedule_conflict"
        | "schedule_name_conflict"
        | "schedule_revision_conflict"
        | "schedule_handoff_pending"
        | "schedule_state_conflict" => StatusCode::CONFLICT,
        "schedule_not_found" | "schedule_wrong_workspace" => StatusCode::NOT_FOUND,
        "schedule_not_owner" => StatusCode::FORBIDDEN,
        "schedule_retired" | "schedule_read_only" => StatusCode::CONFLICT,
        "payload_too_large" => StatusCode::PAYLOAD_TOO_LARGE,
        "delegate_unavailable"
        | "delegate_timeout"
        | "delegate_incompatible"
        | "schedule_unavailable" => StatusCode::SERVICE_UNAVAILABLE,
        _ if failure.retryable => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn refuse(status: StatusCode, failure: Failure) -> Response {
    let mut response = Json(failure).into_response();
    *response.status_mut() = status;
    response
}

#[cfg(test)]
mod tests;
