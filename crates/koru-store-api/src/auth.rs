// SPDX-License-Identifier: MPL-2.0

//! Who is allowed to talk to the store.
//!
//! **Every route is authenticated, including `/v1/status`.** There is no
//! unauthenticated endpoint, not even one that only reports a version: an
//! unauthenticated probe is how anything on the machine learns a Koru Store is
//! running, on which port, and what it can do. Binding to loopback keeps the
//! network out; the token keeps out everything else on the machine that is not
//! Koru.
//!
//! ## The bearer token, and where scopes come in
//!
//! The bootstrap credential is `store.secret`, and it holds every scope. Later
//! clients get their own tokens with a subset — `read_documents`,
//! `edit_documents`, `read_tasks`, `manage_workspaces` — stored as a hash
//! against an [`ApiClient`] row. Both go through here: the middleware resolves
//! a token to a set of scopes and attaches it to the request, and a handler
//! asks for what it needs.
//!
//! The token travels on the **WebSocket upgrade** too, not inside the protocol.
//! That is what lets a stock Loro Protocol client — one that knows nothing
//! about Koru — authenticate with an ordinary HTTP header and then speak
//! unmodified protocol for the rest of the connection.
//!
//! ## Comparison is constant-time
//!
//! `==` on a token returns as soon as two bytes differ, which leaks how much of
//! a guess was right. The loop in [`constant_time_eq`] has no early return; the
//! property that matters is visible in six lines, which is why it is not a
//! dependency.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use koru_store_core::errors::Code;
use koru_store_core::model::Scope;

use crate::error::ApiError;
use crate::state::AppState;

/// What a request was allowed to do, attached once the token is resolved.
#[derive(Clone, Debug)]
pub struct Caller {
    pub scopes: Vec<Scope>,
    /// A name for logs. Never a person, never a device.
    pub label: String,
}

impl Caller {
    pub fn allows(&self, scope: Scope) -> bool {
        self.scopes.contains(&scope)
    }

    /// Refuse unless the caller holds `scope`.
    pub fn require(&self, scope: Scope) -> Result<(), ApiError> {
        if self.allows(scope) {
            return Ok(());
        }
        // The scope is named because a developer integrating against the API
        // has to know which one to ask for. It reveals nothing: the caller
        // already knows what they attempted.
        Err(ApiError::new(
            Code::Forbidden,
            format!("this credential does not hold {scope}"),
        ))
    }
}

/// Resolve the bearer token on every request, or refuse it.
pub async fn require_token(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let presented = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or_default();

    if presented.is_empty() {
        return Err(ApiError::new(
            Code::Unauthenticated,
            "a bearer token is required on every route",
        ));
    }

    let caller = state.resolve(presented).ok_or_else(|| {
        // Deliberately the same answer for "no such token" and "revoked": a
        // difference between them is an oracle for guessing valid tokens.
        ApiError::new(Code::Unauthenticated, "the bearer token was not recognised")
    })?;

    request.extensions_mut().insert(caller);
    Ok(next.run(request).await)
}

/// Compare two secrets without leaking where they first differ.
///
/// `|=` over the whole length, never `&&` and never an early `return` — an
/// implementation that short-circuits tells an attacker how many leading bytes
/// of a guess were correct, which turns a search of the whole space into a
/// search of one byte at a time.
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut differing = 0u8;
    for i in 0..a.len() {
        differing |= a[i] ^ b[i];
    }
    differing == 0
}

#[cfg(test)]
mod tests;
