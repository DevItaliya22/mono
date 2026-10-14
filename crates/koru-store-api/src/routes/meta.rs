// SPDX-License-Identifier: MPL-2.0

//! The API describing itself — twice, to two audiences.
//!
//! Served rather than shipped as a file, so a client integrating against a
//! *running* store gets that store's document. A file in the repository is a
//! description of some build; this is a description of the one answering.
//!
//! ## Two documents, because there are two kinds of route
//!
//! `GET /v1/openapi.json` is **the contract**: the routes an application calls
//! to function, which a customer may build against and which we do not get to
//! move quietly.
//!
//! `GET /v1/debug/openapi.json` is **everything**, diagnostics included. Ours
//! is the whole picture rather than a second partial one — a counter is only
//! useful beside the operation it explains, and our own tooling should not have
//! to read two files and merge them.
//!
//! Both need a bearer token. **Public here means *promised*, not
//! *unauthenticated*** — and the internal one carries a scope, so
//! `only_the_public_facts_are_scope_free` keeps pinning the two genuinely
//! scope-free routes at `/v1/status` and `/v1/openapi.json`.
//!
//! Splitting them changed what is published: all six diagnostic routes used to
//! appear in the public document, so a third party reading it was being shown
//! the metrics surface and implicitly invited to depend on it.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, State};
use koru_store_core::model::Scope;

use crate::api_catalog::{self, Audience};
use crate::auth::Caller;
use crate::error::ApiResult;
use crate::state::AppState;

/// The public contract.
pub async fn openapi(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(api_catalog::openapi(Audience::Public, state.api_version()))
}

/// Every route, diagnostics included.
pub async fn openapi_internal(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
) -> ApiResult<Json<serde_json::Value>> {
    caller.require(Scope::ReadDocuments)?;
    Ok(Json(api_catalog::openapi(
        Audience::Internal,
        state.api_version(),
    )))
}
