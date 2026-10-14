// SPDX-License-Identifier: MPL-2.0

//! `GET /v1/internal/welcome-progress` and
//! `PUT /v1/internal/welcome-progress/{row}` — the Welcome page's ticks.
//!
//! Internal: the rows are this installation's own first-run progress, shared
//! by every Profile on it and promised to no third party. The storage and why
//! it is machine-wide are in `koru_store_core::service::welcome_progress`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use koru_store_core::errors::{Code, Error};
use koru_store_core::model::Scope;
use koru_store_wire::WelcomeProgress;
use serde::Deserialize;

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// `?rows=first-document,learn` — the rows to ask about, comma-separated.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ProgressQuery {
    pub rows: String,
}

fn lock(state: &Arc<AppState>) -> ApiResult<std::sync::MutexGuard<'_, crate::state::KoruStore>> {
    state.store.lock().map_err(|_| {
        ApiError::from(Error::new(
            Code::Internal,
            "the store lock was poisoned by an earlier failure",
        ))
    })
}

/// The asked-about rows that are ticked, in the order asked.
pub async fn progress(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Query(query): Query<ProgressQuery>,
) -> ApiResult<Json<WelcomeProgress>> {
    caller.require(Scope::ReadDocuments)?;
    let rows: Vec<String> = query
        .rows
        .split(',')
        .map(str::trim)
        .filter(|row| !row.is_empty())
        .map(str::to_owned)
        .collect();
    let done = lock(&state)?.welcome_progress(&rows)?;
    Ok(Json(WelcomeProgress { done }))
}

/// Tick one row for every Profile on this installation.
pub async fn mark_done(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Path(row): Path<String>,
) -> ApiResult<Json<WelcomeProgress>> {
    caller.require(Scope::EditDocuments)?;
    lock(&state)?.mark_welcome_done(&row)?;
    Ok(Json(WelcomeProgress { done: vec![row] }))
}

#[cfg(test)]
mod tests;
