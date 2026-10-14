// SPDX-License-Identifier: MPL-2.0

//! The attribute query — `POST /v1/attributes/query`.
//!
//! The handler follows the outline route's pattern: check the scope, hand the
//! typed body to a port the daemon installed, return what it answers. The port
//! exists because the answer lives in the derived read model and the
//! workspace system documents, and this HTTP crate links neither
//! `koru-projection` nor the application's node model — wire types cross the
//! seam, projection types never do.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, State};
use koru_store_core::errors::Code;
use koru_store_core::model::Scope;
use koru_store_wire::attributes::{AttributeQuery, AttributeQueryResponse};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Why the port did not answer. Split so a caller's mistake comes back as a
/// 400 with a reason and a broken read model as a 500 — collapsing them into
/// one string made every typo in a priority token look like storage trouble.
#[derive(Debug)]
pub enum AttributeServiceError {
    /// The query itself is malformed — an unknown sort token, a bound naming
    /// both domains, a tag that is not a `TagId`.
    Invalid(String),
    /// The query was fine and the read model was not.
    Failed(String),
}

/// What the API needs to answer an attribute query.
///
/// The daemon implements it over `projection.db` (sweep-first, the
/// `DocumentOutlineService` pattern) plus the workspace system documents for
/// custom definitions and the tag table.
pub trait AttributeService: Send + Sync + 'static {
    fn query(
        &self,
        query: &AttributeQuery,
    ) -> Result<AttributeQueryResponse, AttributeServiceError>;
}

/// The attribute rows matching a query, from the read model, without opening
/// a document.
///
/// A missing service is not an error, the outline rule: a store built without
/// a projection thread still serves documents, and a caller gets the honest
/// empty answer — no rows, no definitions, `projected: false` — rather than a
/// failure.
pub async fn query(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<AttributeQuery>,
) -> ApiResult<Json<AttributeQueryResponse>> {
    caller.require(Scope::ReadDocuments)?;
    let Some(service) = state.attribute_service() else {
        return Ok(Json(AttributeQueryResponse {
            rows: Vec::new(),
            definitions: Vec::new(),
            tags: Vec::new(),
            groups: Vec::new(),
            projected: false,
            updating_workspaces: body.workspaces.clone(),
            limit: 0,
            offset: body.offset.unwrap_or(0),
        }));
    };
    match service.query(&body) {
        Ok(response) => Ok(Json(response)),
        Err(AttributeServiceError::Invalid(why)) => Err(ApiError::new(Code::InvalidRequest, why)),
        Err(AttributeServiceError::Failed(why)) => Err(ApiError::new(Code::StorageFailed, why)),
    }
}
