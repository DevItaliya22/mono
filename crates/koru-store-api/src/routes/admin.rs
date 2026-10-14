// SPDX-License-Identifier: MPL-2.0

//! `POST /v1/shutdown` — stop the daemon.
//!
//! ## Why a route rather than a signal
//!
//! Gateway-owned Store shutdown has to work on Windows, where there is no
//! `SIGTERM` and `TerminateProcess` is not a graceful stop — it is the abrupt
//! kill the durability tests use deliberately. A route means one mechanism on
//! every platform, and it means stopping the worker goes through the same
//! authentication as everything else rather than through whoever can name the
//! process id.
//!
//! It requires `manage_workspaces`, so a read-only credential handed to a
//! plugin cannot take the store down.
//!
//! **This is a graceful stop.** It returns first, then trips the shutdown
//! signal, so the caller gets an answer rather than a dropped connection; the
//! daemon then stops accepting, finishes what is in flight, releases the lease
//! and clears its handle. Nothing acknowledged is lost, because nothing is
//! acknowledged before it is committed.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, State};
use koru_store_core::errors::{Code, Error};
use koru_store_core::model::Scope;
use koru_store_wire::{Stopping, StoreBackup, StoreBackupResult};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// The archive implementation supplied by the daemon.
///
/// This port keeps ZIP, SQLite snapshotting, and the Store's physical layout
/// out of the HTTP crate. The handler still owns the live Store lock while it
/// calls through it, which is what makes a backup one consistent instant.
pub trait BackupService: Send + Sync {
    fn create(&self, destination: &std::path::Path) -> koru_store_core::errors::Result<()>;
}

/// Write the complete Store to one portable file.
pub async fn backup(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<StoreBackup>,
) -> ApiResult<Json<StoreBackupResult>> {
    caller.require(Scope::ManageWorkspaces)?;
    if request.destination.trim().is_empty() {
        return Err(ApiError::new(
            Code::InvalidRequest,
            "the backup destination is empty",
        ));
    }
    let destination = std::path::PathBuf::from(&request.destination);
    let service = state
        .backup_service()
        .ok_or_else(|| ApiError::new(Code::StorageFailed, "the backup service is unavailable"))?;
    let state_for_work = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let _store = state_for_work
            .store
            .lock()
            .map_err(|_| Error::new(Code::StorageFailed, "the Store lock was poisoned"))?;
        service.create(&destination)
    })
    .await
    .map_err(|error| ApiError::new(Code::Internal, format!("backup task failed: {error}")))??;

    Ok(Json(StoreBackupResult {
        destination: request.destination,
    }))
}

/// `?only_if_idle=true` — stop only if nothing is still connected.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct ShutdownQuery {
    pub only_if_idle: bool,
}

pub async fn shutdown(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    axum::extract::Query(query): axum::extract::Query<ShutdownQuery>,
) -> ApiResult<Json<Stopping>> {
    caller.require(Scope::ManageWorkspaces)?;

    // **The store is the only party that can answer "am I the last".**
    //
    // The agent stops the store when it quits, but only when it is the last
    // thing left; when the agent has already gone, the last *window* out has
    // to do it instead — and no window can see the others. Asking here is what
    // makes that safe: the count is taken under the same lock a join takes, so
    // a window closing cannot race another window connecting and leave one of
    // them talking to a store that is on its way down.
    //
    // A caller asks *after* dropping its own sockets, which is why this counts
    // every connection rather than trying to exclude the asking one — an
    // ordinary HTTP request holds none.
    if query.only_if_idle {
        let live = state.rooms.live_connections();
        if live > 0 {
            tracing::info!(live, "shutdown declined: something is still connected");
            return Ok(Json(Stopping { stopping: false }));
        }
    }

    // Answer first, stop second. `notify_waiters` after a short yield would be
    // racy; instead the signal is tripped on a detached task, which cannot run
    // until this response has been handed back to the runtime.
    let signal = state.shutdown.clone();
    tokio::spawn(async move {
        signal.notify_waiters();
    });
    Ok(Json(Stopping { stopping: true }))
}
