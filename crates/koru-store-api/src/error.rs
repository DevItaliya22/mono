// SPDX-License-Identifier: MPL-2.0

//! Turning a store error into an HTTP response.
//!
//! **This is the only place in the store that knows what a 404 is.**
//! `koru-store-core` reports a [`Class`] — not-found, invalid, conflict — and
//! this file maps it. A second transport (a named pipe, shared memory) maps the
//! same classes its own way, and no error site anywhere changes.
//!
//! The body is always the same shape, and it is the contract:
//!
//! ```json
//! { "code": "workspace_not_found", "message": "no row for the given id" }
//! ```
//!
//! `code` is a stable machine-readable token a client branches on. `message` is
//! **untranslated developer text** that a client must never show to a person —
//! it is for logs and bug reports. A client renders its own message, in its own
//! language, from the code.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use koru_store_core::errors::{Class, Code, Error};

/// The body of every failed request.
///
/// **Declared in `koru-store-wire`**, so the client parses the same shape this
/// writes. `code` is owned here where it used to be `&'static str`: the client
/// deserializes it, and a borrow cannot cross a wire. One allocation on a path
/// that is already an error.
pub use koru_store_wire::Failure;

/// A store error, ready to be returned from a handler.
///
/// `Debug` forwards to the inner error, so an assertion failure names the code
/// rather than printing `ApiError`. Like [`Error`] it carries a stable code and
/// developer text and **never customer content**, so it is safe in a panic
/// message. The second field is display data only: the devices a parked blob
/// hydration is waiting on, for a client that would rather name them than say
/// "the device holding it".
#[derive(Debug)]
pub struct ApiError(pub Error, pub Vec<String>);

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        Self(e, Vec::new())
    }
}

impl ApiError {
    pub fn new(code: Code, detail: impl Into<String>) -> Self {
        Self(Error::new(code, detail), Vec::new())
    }

    pub fn bare(code: Code) -> Self {
        Self(Error::bare(code), Vec::new())
    }

    /// The devices a parked hydration is waiting on, as display names.
    pub fn with_waiting_on(mut self, holders: Vec<String>) -> Self {
        self.1 = holders;
        self
    }

    /// **Why blob transfer is holding**, when that is what refused the
    /// request (D259).
    ///
    /// The blob engine's hold codes are not [`Code`]s and must not become
    /// them, so they travel beside the code the way `waiting_on` does.
    /// Without this, every hold but the three the Store has a code of its own
    /// for arrived at a client as `storage_failed`, and the missing-image
    /// placeholder had nothing true left to say — so it said rung 1's peer
    /// sentence and sent people to restart hardware they did not need.
    pub fn with_hold(self, hold: Option<String>) -> Self {
        Self(self.0.with_hold(hold), self.1)
    }
}

/// The transport-neutral class, spelled in HTTP.
pub fn status_for(class: Class) -> StatusCode {
    match class {
        Class::NotFound => StatusCode::NOT_FOUND,
        Class::Invalid => StatusCode::BAD_REQUEST,
        Class::Unauthenticated => StatusCode::UNAUTHORIZED,
        Class::Forbidden => StatusCode::FORBIDDEN,
        Class::Conflict => StatusCode::CONFLICT,
        // 503 rather than 500: damaged or unreachable state is a condition of
        // the data, and a client may usefully retry after a recovery.
        Class::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        Class::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = status_for(self.0.class());
        tracing::warn!(
            status = status.as_u16(),
            code = self.0.code().as_str(),
            detail = self.0.message(),
            "store request failed"
        );
        let body = Failure {
            code: self.0.code().as_str().to_string(),
            message: self.0.message().to_string(),
            waiting_on: self.1,
            hold: self.0.hold().map(str::to_owned),
        };
        (status, Json(body)).into_response()
    }
}

pub type ApiResult<T> = std::result::Result<T, ApiError>;

#[cfg(test)]
mod tests;
