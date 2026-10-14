// SPDX-License-Identifier: MPL-2.0

//! Explicit owner/workspace/reminder routes for the personal CRDT unit.

use super::*;
use koru_store_core::WorkspaceId;

fn scoped_ids(
    profile: &str,
    workspace: &str,
    reminder: &str,
) -> ApiResult<(ProfileId, WorkspaceId, ReminderId)> {
    let (profile, reminder) = ids(profile, reminder)?;
    Ok((
        profile,
        WorkspaceId::parse(workspace).ok_or_else(invalid_id)?,
        reminder,
    ))
}

pub async fn get_one(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Path((profile, workspace, reminder)): Path<(String, String, String)>,
) -> ApiResult<Json<StoredReminder>> {
    caller.require(Scope::ReadDocuments)?;
    let (profile, workspace, reminder) = scoped_ids(&profile, &workspace, &reminder)?;
    let result = lock(&state)?.reminder_in(profile, workspace, reminder)?;

    Ok(Json(result))
}

pub async fn update(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Path((profile, workspace, reminder)): Path<(String, String, String)>,
    Json(body): Json<RuleRequest>,
) -> ApiResult<Json<StoredReminder>> {
    caller.require(Scope::EditDocuments)?;
    let (profile, workspace, reminder) = scoped_ids(&profile, &workspace, &reminder)?;
    let result = {
        validate_rule_path(reminder, &body.rule)?;
        validate_authored_rule(&body.rule)?;
        lock(&state)?.update_reminder_in(profile, workspace, &body.rule)?
    };
    state.notify_replication();
    Ok(Json(result))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Path((profile, workspace, reminder)): Path<(String, String, String)>,
) -> ApiResult<Json<StoredReminder>> {
    caller.require(Scope::EditDocuments)?;
    let (profile, workspace, reminder) = scoped_ids(&profile, &workspace, &reminder)?;
    let result = lock(&state)?.delete_reminder_rule_in(profile, workspace, reminder)?;
    state.notify_replication();
    Ok(Json(result))
}

pub async fn pause(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Path((profile, workspace, reminder)): Path<(String, String, String)>,
    Json(body): Json<RevisionRequest>,
) -> ApiResult<Json<StoredReminder>> {
    caller.require(Scope::EditDocuments)?;
    let (profile, workspace, reminder) = scoped_ids(&profile, &workspace, &reminder)?;
    let result = lock(&state)?.set_reminder_status_in(
        profile,
        workspace,
        reminder,
        body.revision,
        RuleStatus::Paused,
    )?;
    state.notify_replication();
    Ok(Json(result))
}

pub async fn resume(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Path((profile, workspace, reminder)): Path<(String, String, String)>,
    Json(body): Json<RevisionRequest>,
) -> ApiResult<Json<StoredReminder>> {
    caller.require(Scope::EditDocuments)?;
    let (profile, workspace, reminder) = scoped_ids(&profile, &workspace, &reminder)?;
    let result = lock(&state)?.set_reminder_status_in(
        profile,
        workspace,
        reminder,
        body.revision,
        RuleStatus::Enabled,
    )?;
    state.notify_replication();
    Ok(Json(result))
}

pub async fn snooze(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Path((profile, workspace, reminder)): Path<(String, String, String)>,
    Json(body): Json<ActionRequest>,
) -> ApiResult<Json<StoredReminder>> {
    caller.require(Scope::EditDocuments)?;
    let (profile, workspace, reminder) = scoped_ids(&profile, &workspace, &reminder)?;
    let result = {
        validate_action_route(&body.action.kind, true)?;
        lock(&state)?.append_reminder_action_in(profile, workspace, reminder, &body.action)?
    };
    state.notify_replication();
    Ok(Json(result))
}

pub async fn handle(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Path((profile, workspace, reminder)): Path<(String, String, String)>,
    Json(body): Json<ActionRequest>,
) -> ApiResult<Json<StoredReminder>> {
    caller.require(Scope::EditDocuments)?;
    let (profile, workspace, reminder) = scoped_ids(&profile, &workspace, &reminder)?;
    let result = {
        validate_action_route(&body.action.kind, false)?;
        lock(&state)?.append_reminder_action_in(profile, workspace, reminder, &body.action)?
    };
    state.notify_replication();
    Ok(Json(result))
}
