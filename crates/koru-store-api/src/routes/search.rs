// SPDX-License-Identifier: MPL-2.0

//! `POST /v1/search` and `POST /v1/search/navigated`.
//!
//! ## Why the query is in a body
//!
//! The access logger now records only the path and omits every query value.
//! The body remains defense in depth: URLs also travel through client history,
//! proxies, diagnostics, and copied links. `GLOBAL_SEARCH.md` is explicit that
//! query text never enters those incidental surfaces.
//!
//! That makes these read-only POSTs, which
//! `a_mutation_never_requires_only_a_read_scope` has to be told about. The
//! exception is named there rather than the rule widened.
//!
//! ## Why the index is a trait
//!
//! `koru-search` links `koru-projection`, which links `koru-core` — the node
//! model, `loro`, and beneath them a whole FTS engine. This crate's own module
//! doc lists what never crosses its boundary, and acquiring the node model to
//! serve one route would invert it.
//!
//! So the shape is a port here and an adapter in the daemon: the same
//! arrangement `koru-store-core` already uses for persistence, one layer up.
//! The types are shared rather than translated, because they live in
//! `koru-store-wire` and both sides name the same declarations.
//!
//! ## Why every call goes through `spawn_blocking`
//!
//! [`SearchService::search`] does real work on a real thread: SQLite reads, and
//! a wait of up to half a second for a workspace nobody has compared against
//! the store since it came up. Running that on a runtime worker would park a
//! thread that is supposed to be driving other requests.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, State};
use koru_store_core::errors::{Code, Error};
use koru_store_core::model::Scope;
use koru_store_wire::search::{Navigation, SearchRequest, SearchResponse};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Trusted workspace authorization, deliberately separate from the user's
/// requested search filters. `Allowed(empty)` is deny-all; it can never acquire
/// the query model's convenient empty-means-everywhere spelling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceSearchScope {
    AllLocal,
    Allowed(BTreeSet<koru_store_core::WorkspaceId>),
}

impl WorkspaceSearchScope {
    pub fn allows(&self, workspace: koru_store_core::WorkspaceId) -> bool {
        match self {
            Self::AllLocal => true,
            Self::Allowed(allowed) => allowed.contains(&workspace),
        }
    }
}

/// What this crate needs from an index, and nothing more.
///
/// Blocking on purpose — the implementation is SQLite and a sweep wait, and
/// pretending otherwise would only move the block somewhere less visible.
/// `Send + Sync` because one instance answers every request.
pub trait SearchService: Send + Sync + 'static {
    fn search(
        &self,
        scope: &WorkspaceSearchScope,
        request: &SearchRequest,
    ) -> Result<SearchResponse, Error>;
    fn record_navigation(
        &self,
        scope: &WorkspaceSearchScope,
        navigation: &Navigation,
    ) -> Result<(), Error>;
}

/// The index, or an honest refusal.
///
/// **A store with no index still serves documents.** `search.db` failing to
/// open is a store whose search is stale, not one that has lost anything, and
/// refusing to serve a workspace because a cache is broken would be refusing
/// for the wrong reason.
///
/// `search_unavailable` is its own code rather than `storage_failed`, which is
/// the other thing that answers 503 here: a client should be able to tell
/// *this store has no search* from *the disk is broken* without reading the
/// message, and the two want different responses.
fn service(state: &AppState) -> ApiResult<Arc<dyn SearchService>> {
    state.search_service().ok_or_else(|| {
        ApiError::new(
            Code::SearchUnavailable,
            "this store is not running a search index",
        )
    })
}

pub async fn search(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Json(mut request): Json<SearchRequest>,
) -> ApiResult<Json<SearchResponse>> {
    caller.require(Scope::ReadDocuments)?;
    let service = service(&state)?;

    // **Clamped, not refused.** A page is built in memory before it is written,
    // so the ceiling is there to protect the store; a caller asking for a
    // million rows is being hopeful rather than wrong.
    request.query.page_size = request
        .query
        .page_size
        .clamp(1, koru_store_wire::search::MAX_PAGE_SIZE);

    // MVP local sessions intentionally have all-data grants. Phase 2 changes
    // this one construction site to the profile/workspace set carried by the
    // caller; the service contract already makes an empty set deny-all.
    let scope = WorkspaceSearchScope::AllLocal;
    blocking(move || service.search(&scope, &request)).await
}

pub async fn navigated(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Json(navigation): Json<Navigation>,
) -> ApiResult<Json<()>> {
    // **`EditDocuments`, and it is not a perfect fit.** This edits no document.
    // What it does do is write state a later search reads back — a recorded
    // query returns as `recent_queries`, a recorded open feeds a hit's
    // `workspace_last_used_at_ms` — so a read-only credential must not be able
    // to reach it. The alternative was a fifth `Scope` variant for one route,
    // in a core enum with a `parse`, an `as_str` and a wire contract, for a
    // distinction nothing acts on: every local client is minted with
    // `Scope::ALL`. The consequence is right either way — a read-only tool can
    // search, and its searches are not remembered.
    caller.require(Scope::EditDocuments)?;
    let service = service(&state)?;
    let scope = WorkspaceSearchScope::AllLocal;
    blocking(move || service.record_navigation(&scope, &navigation)).await
}

/// Run a blocking call off the runtime's worker threads.
///
/// A panic inside becomes a `JoinError`, which is reported as an internal
/// failure rather than taking the runtime down with it.
async fn blocking<T, F>(work: F) -> ApiResult<Json<T>>
where
    F: FnOnce() -> Result<T, Error> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => Ok(Json(result?)),
        Err(join) => Err(ApiError::new(
            Code::Internal,
            format!("the search thread did not finish: {join}"),
        )),
    }
}
