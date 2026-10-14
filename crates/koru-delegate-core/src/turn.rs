// SPDX-License-Identifier: MPL-2.0

//! Turn identity: `(RunAttemptId, TurnId, originator)`.
//!
//! §7.5: every turn carries all three, and the originator decides what the
//! turn may do to the run. Only an orchestrator-originated turn can complete
//! the objective; a human turn — typed while a person holds control (§7.4) —
//! is recorded and never terminalizes the run, whichever way it ends.

use serde::{Deserialize, Serialize};

use crate::{AgentTurnId, RunAttemptId};

/// Who submitted a turn.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TurnOriginator {
    /// The delegate worker, pursuing the objective.
    Orchestrator,
    /// A person who took control of the session (§7.4).
    Human,
}

/// One turn, named completely. The turn id alone is ambiguous across attempts
/// and says nothing about who asked; the triple is what §7.5's rules and
/// §17.2's accounting key on.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TurnIdentity {
    pub attempt: RunAttemptId,
    pub turn: AgentTurnId,
    pub originator: TurnOriginator,
}
