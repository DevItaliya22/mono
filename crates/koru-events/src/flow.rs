// SPDX-License-Identifier: MPL-2.0

//! The shapes Milestone 3's flow-rule engine shares with everything else.
//!
//! Defined here so the enum exists once, consumed by nothing in Milestone 1.
//! The contract around it — what the bus guarantees, what the rule engine
//! owes — is `docs/architecture/eventbus/AGENTIC_FLOWS.md`. The idempotency formula lives
//! there as prose and here as a doc comment, deliberately not as code: it is
//! the rule engine's own store that enforces it, and this crate links no
//! hashing.
//!
//! **Idempotency:** a rule executes at most once per
//! `execution_id = sha256(rule_id ‖ ":" ‖ event_id)` (hex), however many
//! times the bus delivers the event.

use serde::{Deserialize, Serialize};

/// Where a flow rule runs when the event that matched it is visible on more
/// than one device. A property of the *rule*, never of the envelope: the
/// event says what happened, the rule says who acts.
///
/// Milestone 1's journals are single-device, so every scope degenerates to
/// `OriginDevice`; the enum exists now so rules written early need no
/// migration. `WorkspaceOnce` needs a coordination point (a designated host,
/// or a CRDT claim) and is documented as unresolved in AGENTIC_FLOWS.md.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionScope {
    /// Only the device the event originated on.
    OriginDevice,
    /// Any of the user's devices may act; dedupe by execution id.
    AnyUserDevice,
    /// A device the user designated as the agent host.
    DesignatedHost,
    /// At most once per workspace, whoever gets there first.
    WorkspaceOnce,
}
