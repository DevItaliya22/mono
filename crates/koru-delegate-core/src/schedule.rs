// SPDX-License-Identifier: MPL-2.0

//! Closed schedule outcomes shared by the synced document and local events.
//!
//! These types carry no recurrence rule, template or storage shape. Keeping
//! the small result vocabulary in the leaf prevents the document and event
//! crates from growing independent spellings that can drift silently.

use serde::{Deserialize, Serialize};

/// A content-free reason a schedule is ineligible to fire.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SchedulePauseReason {
    Manual,
    ProviderNotDelegatable,
    ScopeUnavailable,
    ContextUnavailable {
        item: u32,
        stage: ScheduleContextFailureStage,
    },
}

/// The context operation that made a future firing unsafe.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ScheduleContextFailureStage {
    Resolve,
    Commit,
    Retention,
    Materialize,
}

/// The immutable outcome of one schedule occurrence.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ScheduleFiringOutcome {
    Fired,
    SkippedMissed,
    SkippedBusy,
}
