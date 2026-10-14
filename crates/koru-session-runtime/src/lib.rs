// SPDX-License-Identifier: MPL-2.0

//! Headless terminal session runtime.
//!
//! A PTY reader hands borrowed batches directly to its owner. Writes are
//! bounded and off-thread. One session-local worker combines that process edge
//! with the bounded emulator; the registry stores handles and summaries only.
//! This crate owns no window, root discovery, public HTTP, logging sink, or
//! product UI callback.

mod command;
mod creation;
mod error;
mod platform;
pub mod providers;
mod reader;
mod registry;
mod stream;
mod transport;
mod worker;
mod writer;

pub use command::*;
pub use error::*;
pub use reader::*;
pub use registry::*;
pub use stream::*;
pub use transport::*;
pub use worker::*;
pub use writer::*;

pub const READ_BATCH_BYTES: usize = 64 * 1024;
pub const WRITE_CHUNK_BYTES: usize = 16 * 1024;
pub const MAX_QUEUED_WRITE_BYTES: usize = 1024 * 1024;
