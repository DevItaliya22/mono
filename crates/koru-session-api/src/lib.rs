// SPDX-License-Identifier: MPL-2.0

//! Public HTTP/SSE contract for Koru-owned process sessions.
//!
//! This crate owns route names, authorization scopes, public request/response
//! shapes, stable error translation, and stream metadata. It owns no HTTP
//! listener, bearer authority, private Agent credential, IPC client, runtime,
//! process, root, or UI. The public Gateway composes the catalog and supplies
//! mechanisms.

mod catalog;
mod error;
mod openapi;
mod stream;
mod types;

pub use catalog::{
    Access, Method, ROUTES, RequestLocation, Route, RouteId, authorize, route_by_id, route_for,
};
pub use error::{
    GATEWAY_ERROR_STATUSES, GatewayError, HttpStatus, SESSION_ERROR_STATUSES, public_status,
    public_status_for_class,
};
pub use openapi::{extend_asyncapi, extend_openapi};
pub use stream::{SESSION_STREAMS, StreamChannel, StreamMessage};
pub use types::*;

pub const CAPABILITY: &str = "sessions";

/// Public request limits. The runtime uses the same one-mebibyte bound for
/// queued process input and the default attachment policy admits eight MiB.
/// Publishing them here lets clients reject oversized local files before
/// allocating their base64 representation.
pub const MAX_INPUT_BYTES: usize = 1024 * 1024;
pub const MAX_ATTACHMENT_BYTES: u64 = 8 * 1024 * 1024;

pub const METADATA_SCOPE: &str = "sessions.metadata.read";
pub const OUTPUT_SCOPE: &str = "sessions.output.read";
pub const CONTROL_SCOPE: &str = "sessions.control";
pub const ATTACHMENT_SCOPE: &str = "sessions.attachments.write";
pub const TERMINATE_SCOPE: &str = "sessions.terminate";

pub const ALL_SCOPES: &[&str] = &[
    METADATA_SCOPE,
    OUTPUT_SCOPE,
    CONTROL_SCOPE,
    ATTACHMENT_SCOPE,
    TERMINATE_SCOPE,
];

#[cfg(test)]
mod tests;
