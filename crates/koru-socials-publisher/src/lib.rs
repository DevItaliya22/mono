// SPDX-License-Identifier: MPL-2.0

//! Device-owned social delivery. Store owns authoring; the local ledger owns
//! evidence about irreversible requests, including requests with no response.
mod engine;
mod ledger;
mod runtime;

pub use engine::*;
pub use ledger::SqliteLedger;
pub use runtime::Worker;

#[cfg(test)]
mod tests;
