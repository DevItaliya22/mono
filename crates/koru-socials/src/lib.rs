// SPDX-License-Identifier: MPL-2.0

//! Social authoring and delivery values. No window, network, clock or disk.
//!
//! Authoring is mutable; delivery evidence is retained per destination and
//! thread part. A provider timeout never establishes that nothing was posted.

mod document;
mod model;
mod validation;

pub use document::{Document, DocumentError, POST_KIND, SCHEMA_VERSION};
pub use model::*;
pub use validation::{Issue, split_thread, validate, weighted_length};

#[cfg(test)]
mod tests;
