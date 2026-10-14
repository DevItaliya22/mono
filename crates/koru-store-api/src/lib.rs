// SPDX-License-Identifier: MPL-2.0

//! **The Koru Store's API — the only way in.**
//!
//! Every persistent mutation the first-party Koru UI will ever make has an
//! operation here or a standard Loro operation over the WebSocket. There is no
//! private path into the store: the app that ships with Koru is a client of
//! this API on exactly the terms a third party is.
//!
//! ## What never crosses this boundary
//!
//! - **A filesystem path.** The data directory appears once, as an opaque
//!   string on a debug route.
//! - **A table name, a column, a statement.** A client never needs to learn
//!   that SQLite is underneath, and telling them makes it a compatibility
//!   surface we would then have to keep.
//! - **A translated string.** Errors are `{ code, message }` where `code` is a
//!   stable token and `message` is untranslated developer text. Clients render
//!   their own words, in their own language.
//!
//! ## Versioning
//!
//! The `/v1` segment is the API's version. `GET /v1/status` reports it
//! alongside a capability list, so a client negotiates rather than guesses —
//! and a capability is additive, which is what keeps most changes off the
//! version number entirely.

pub mod access_log;
pub mod api_catalog;
pub mod auth;
pub mod catalog_events;
pub mod cloud_activity;
pub mod cloud_changes;
pub mod cloud_waiting;
pub mod error;
pub mod protocol;
pub mod rooms;
pub mod routes;
pub mod sessions;
pub mod state;

use std::sync::Arc;

use axum::Router;

pub use catalog_events::CatalogEventSink;
pub use error::{ApiError, ApiResult};
pub use state::{AppState, KoruStore, token_hash};

/// The whole API.
///
/// **The paths come from `api_catalog::ROUTES` and appear nowhere else**, so a
/// route that is not in the table is not served. This function adds only what
/// wraps every route: authentication, then logging.
///
/// Authentication wraps **everything**, including status: an unauthenticated
/// probe is how anything on the machine learns a store is running and what it
/// can do.
pub fn router(state: Arc<AppState>) -> Router {
    api_catalog::routes()
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_token,
        ))
        // **Outside authentication**, so a rejected request is logged too. A
        // 401 that leaves no trace is the one you most want a line for — it is
        // what a client with a stale token looks like.
        .layer(axum::middleware::from_fn(access_log::log_request))
        .with_state(state)
}

/// The same typed route catalog for a co-located trusted server principal.
/// Public authentication has already happened at the gateway; this inserts
/// only the effective Store authority and never mints or validates a Store
/// bearer token.
pub fn router_for_trusted_requests(state: Arc<AppState>) -> Router {
    api_catalog::routes()
        .layer(axum::middleware::from_fn(access_log::log_request))
        .with_state(state)
}

#[cfg(test)]
mod body_limit_tests;
