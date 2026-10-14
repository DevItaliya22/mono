// SPDX-License-Identifier: MPL-2.0

//! `GET /v1/status` — what this store is, and what it can do.
//!
//! The first call any client makes. It reports the API version and a
//! **capability list**, so a client asks "can this store do X" rather than
//! deducing it from a version number. Adding a capability is additive and needs
//! no version bump; removing one does.
//!
//! What it deliberately does not report: the data directory, the schema
//! versions of the databases, how many workspaces exist, or anything else that
//! would let a caller reason about the physical layout. Those belong on the
//! debug routes, which exist to be read by a person diagnosing something rather
//! than by a client making decisions.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use koru_store_wire::Status;

use crate::state::AppState;

pub async fn status(State(state): State<Arc<AppState>>) -> Json<Status> {
    Json(Status {
        api_version: state.api_version(),
        // **Owned rather than `&'static [&'static str]`**, because the client
        // deserializes this same type and a borrow cannot cross a wire. One
        // small allocation on the call every client makes exactly once, in
        // exchange for the server and the client sharing a declaration.
        capabilities: state.capabilities().iter().map(|s| s.to_string()).collect(),
        channel: state.channel.clone(),
        started_at: state.started_at,
    })
}
