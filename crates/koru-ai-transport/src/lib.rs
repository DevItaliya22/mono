// SPDX-License-Identifier: MPL-2.0

//! Blocking HTTP stream framing shared by native AI provider adapters.
//!
//! This leaf knows bytes, HTTP, NDJSON, and SSE. It deliberately knows no
//! provider, model, request, or product event.

use std::fmt;
use std::time::Duration;

mod ndjson;
mod sse;

#[cfg(feature = "test-server")]
pub mod test_server;

pub use ndjson::NdjsonDecoder;
pub use sse::{SseDecoder, SseFrame};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FrameError {
    Decode,
    Transport,
    Timeout,
    UnexpectedEnd,
}

impl fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Decode => "the streaming frame could not be decoded",
            Self::Transport => "the streaming transport failed",
            Self::Timeout => "the streaming transport timed out",
            Self::UnexpectedEnd => "the streaming transport ended unexpectedly",
        })
    }
}

impl std::error::Error for FrameError {}

pub fn blocking_runtime() -> Result<tokio::runtime::Runtime, FrameError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| FrameError::Transport)
}

pub fn send_with_timeout(
    runtime: &tokio::runtime::Runtime,
    request: reqwest::RequestBuilder,
    timeout: Duration,
) -> Result<reqwest::Response, FrameError> {
    runtime
        .block_on(tokio::time::timeout(timeout, request.send()))
        .map_err(|_| FrameError::Timeout)?
        .map_err(|_| FrameError::Transport)
}
