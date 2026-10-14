// SPDX-License-Identifier: MPL-2.0

//! One line per request: what was asked, how big, how long, what came back.
//!
//! The store is a background process a person cannot see into. When Koru shows
//! the wrong thing, the first question is always *did it ask, and what did it
//! get* — and without this the only way to answer was to add a `println!` and
//! rebuild.
//!
//! ## What is deliberately not logged
//!
//! **The bearer token, and anything carrying it.** It is a session credential;
//! writing it to a file that rotates for seven days would make the log a
//! credential store. `Authorization` never appears here, and neither does any
//! header — a header is an open-ended place for a secret to arrive.
//!
//! **Bodies.** A request body is a person's writing. The *size* answers the
//! questions a log is for — is a snapshot enormous, is an update empty — and the
//! content answers none of them.
//!
//! **Query values.** A query string is untrusted and may carry a title, search
//! phrase, or other customer content even when a route expects only ids. Logs
//! keep the route path and discard the complete query before validation.
//!
//! ## Why the response size is sometimes unknown
//!
//! A response with a known length reports it. A streamed one does not have a
//! length until it has been sent, and buffering it to find out would make the
//! log change the behaviour it is describing. Those log `-` rather than a
//! plausible number.

use std::time::Instant;

use axum::body::Body;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

/// Log one request, after it has been answered.
///
/// Timed around `next.run`, so the duration is the store's own work — the
/// handler, the lock wait and the disk — and not the client's read of the
/// response.
pub async fn log_request(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    // Query values are untrusted and may be customer content even when this
    // build expects only ids. Discard them before validation and logging.
    let path = log_path(request.uri());
    let request_bytes = body_len(request.body());

    let at = Instant::now();
    let response = next.run(request).await;
    let took = at.elapsed();

    let status = response.status().as_u16();
    let response_bytes = body_len(response.body());

    // A failure is worth more than an ordinary request, and a person reading a
    // log for a problem should not have to filter for it.
    //
    // **101 counts as success.** `is_success` is 2xx only, and the sync route
    // answers `101 Switching Protocols` — so every WebSocket connection, the
    // most ordinary thing this store does, logged as a warning. Caught by
    // reading the output rather than by a test, which is the argument for
    // looking at a log before shipping one.
    let ok = response.status().is_success()
        || response.status() == axum::http::StatusCode::SWITCHING_PROTOCOLS;
    let request_size = size(request_bytes);
    let response_size = size(response_bytes);
    if ok && method == axum::http::Method::GET && status != 101 {
        // Catalog/status reads are maintenance traffic. They remain available
        // at DEBUG for diagnosis, but logging a line at INFO every two seconds
        // makes mutations and sync milestones disappear in normal console
        // output. Upgrades stay INFO because they describe a connection being
        // established rather than a routine read.
        tracing::debug!(
            %method,
            %path,
            status,
            req = %request_size,
            res = %response_size,
            took_ms = took.as_secs_f64() * 1000.0,
            "{method} {path} -> {status} ({response_size} in {took:.1?})"
        );
    } else if ok {
        tracing::info!(
            %method,
            %path,
            status,
            req = %request_size,
            res = %response_size,
            took_ms = took.as_secs_f64() * 1000.0,
            "{method} {path} -> {status} ({response_size} in {took:.1?})"
        );
    } else {
        tracing::warn!(
            %method,
            %path,
            status,
            req = %request_size,
            res = %response_size,
            took_ms = took.as_secs_f64() * 1000.0,
            "{method} {path} -> {status} ({took:.1?})"
        );
    }
    response
}

fn log_path(uri: &axum::http::Uri) -> String {
    uri.path().to_owned()
}

/// A body's length when it declares one.
///
/// `None` for a stream, which is honest — see the module note.
fn body_len(body: &Body) -> Option<u64> {
    use http_body::Body as _;
    body.size_hint().exact()
}

/// Bytes, in the unit a person reads at a glance.
fn size(bytes: Option<u64>) -> String {
    match bytes {
        None => "-".to_string(),
        Some(n) if n < 1024 => format!("{n}B"),
        Some(n) if n < 1024 * 1024 => format!("{:.1}KB", n as f64 / 1024.0),
        Some(n) => format!("{:.1}MB", n as f64 / (1024.0 * 1024.0)),
    }
}

#[cfg(test)]
mod tests;
