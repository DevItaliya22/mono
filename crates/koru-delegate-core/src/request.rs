// SPDX-License-Identifier: MPL-2.0

//! What a person asks for when they delegate (D9, D28).
//!
//! The request is the creation intent's payload (§7.1): everything a restart
//! needs to finish creating the run without the person present. It is the
//! *sync* side's input — a [`crate::RunHeader`] is made from it — and it
//! carries nothing about how the attempt will run.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    ActorRef, DeviceId, Objective, RunContextItem, RunId, SessionProfileKey, WorkspaceId,
    WorkspaceScope,
};

/// Where a run came from. The scheduled form is derived from the consumed
/// schedule authority and retained in the creation intent, so recovery cannot
/// turn an unattended firing into a manual start.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum RunOrigin {
    #[default]
    Manual,
    Scheduled {
        schedule: crate::ScheduleId,
        occurrence_ms: i64,
        revision: u32,
    },
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DelegationRequest {
    /// Minted by the asking client, so a retried request is the same run.
    pub run: RunId,
    pub requested_by: ActorRef,
    /// What to do. Redacted everywhere but the surface that shows it.
    pub objective: Objective,
    /// Which agent — a launch profile, built-in or custom. Only profiles that
    /// meet the delegation contract (P1.1, `Delegatable`) are offered.
    pub agent: SessionProfileKey,
    /// The person's explicit choice; absent means the provider default.
    /// Retained intent, never proof of the model the provider actually used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_model: Option<crate::ModelName>,
    pub scope: WorkspaceScope,
    /// Context as the person gave it, already pinned (§7.2).
    pub context: Vec<RunContextItem>,
    /// The workspaces the run's tools may reach (D28). Starts as the
    /// delegating surface's workspace; the person may remove it; an empty
    /// set denies all workspace access. Frozen for the duration of an
    /// attempt — a resume re-reads it and never widens it.
    pub grants: BTreeSet<WorkspaceId>,
    /// The device asked to execute (D3).
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub executing_device: DeviceId,
    /// Manual for direct delegation; schedule identity for an unattended
    /// firing. Older pending intents decode as manual.
    #[serde(default)]
    pub origin: RunOrigin,
}
