// SPDX-License-Identifier: MPL-2.0

//! [`DelegatePort`]: what the gateway's `/v1/runs` routes call.
//!
//! The same seam the artifact and AI domains have — a context the gateway
//! built from the bearer it admitted, typed bodies in, typed bodies or a
//! [`Failure`] out — with one difference: the run's worker is a separate
//! process, so every implementation is a transport. [`crate::LocalDelegate`]
//! reaches this device's worker over its private IPC; D12's cloud worker
//! is another implementation of exactly this trait, and `runs.rs` does not
//! change for it.
//!
//! **Blocking, by design.** The gateway calls it from `spawn_blocking`, the
//! way it calls the artifact service, so no IPC waits on a request task.

use std::sync::Arc;

use crate::wire::{
    CancelRunResponse, CreateRunRequest, CreateRunResponse, CreateScheduleRequest,
    CreateScheduleResponse, EditScheduleRequest, EditScheduleResponse, Failure, ListRunsResponse,
    ListSchedulesResponse, PauseScheduleRequest, PauseScheduleResponse, PurgeRunResponse,
    RateRunRequest, RateRunResponse, RenameScheduleRequest, RenameScheduleResponse,
    RestoreRunResponse, ResumeRunRequest, ResumeRunResponse, ResumeScheduleRequest,
    ResumeScheduleResponse, RetireScheduleRequest, RetireScheduleResponse, RunStreamItem, RunView,
    ScheduleChange, TimelineQuery, TrashRunRequest, TrashRunResponse,
};
use koru_delegate_core::{RunId, WorkspaceId};

/// The two scopes `/v1/runs` declares.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DelegateScope {
    Read,
    Write,
}

impl DelegateScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "runs.read",
            Self::Write => "runs.write",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "runs.read" => Some(Self::Read),
            "runs.write" => Some(Self::Write),
            _ => None,
        }
    }
}

/// Who is calling: the gateway's label and the run scopes it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegatePrincipal {
    pub label: String,
    pub scopes: Vec<DelegateScope>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestContext {
    pub principal: DelegatePrincipal,
    pub trace_id: String,
}

pub type Result<T> = std::result::Result<T, Failure>;

/// A run's live stream, read on a thread the caller owns: blocking `next`,
/// `None` at the end.
pub trait RunStream: Send {
    fn next(&mut self) -> Option<RunStreamItem>;
}

/// Content-free schedule-list invalidations, read on a caller-owned thread.
pub trait ScheduleStream: Send {
    fn next(&mut self) -> Option<ScheduleChange>;
    fn cancellation(&self) -> Arc<dyn ScheduleStreamCancellation>;
}

/// A separately owned write half that can interrupt a blocked schedule read.
pub trait ScheduleStreamCancellation: Send + Sync {
    fn cancel(&self);
}

/// Host-independent delegation contract used by the gateway's routes.
pub trait DelegatePort: Send + Sync {
    /// Whether the worker is answering right now. A probe, not a call.
    fn serving(&self) -> bool;
    /// `POST /v1/runs`: durable before it answers.
    fn create(
        &self,
        context: RequestContext,
        request: CreateRunRequest,
    ) -> Result<CreateRunResponse>;
    fn list(&self, context: RequestContext, workspace: WorkspaceId) -> Result<ListRunsResponse>;
    fn run(&self, context: RequestContext, run: RunId, window: TimelineQuery) -> Result<RunView>;
    fn cancel(&self, context: RequestContext, run: RunId) -> Result<CancelRunResponse>;
    /// `POST /v2/runs/{run}/resume`: a fresh attempt on an interrupted run,
    /// refused when a pin no longer resolves (D48) and, for a prior attempt
    /// whose fate is unknown, unless the person confirmed (§7.6).
    fn resume(
        &self,
        context: RequestContext,
        run: RunId,
        request: ResumeRunRequest,
    ) -> Result<ResumeRunResponse>;
    /// `POST /v1/runs/{run}/trash`: the worker's half of putting a run in
    /// the trash — refuse one that is still working, and remember what to
    /// do with its results. The caller moves the Run document into the
    /// workspace's bin, which is a manifest edit and not this worker's.
    fn trash(
        &self,
        context: RequestContext,
        run: RunId,
        request: TrashRunRequest,
    ) -> Result<TrashRunResponse>;

    /// `POST /v1/runs/{run}/restore`: the run is coming back out whole.
    fn restore(&self, context: RequestContext, run: RunId) -> Result<RestoreRunResponse>;

    /// `POST /v1/runs/{run}/purge`: the trash was emptied. Everything this
    /// device holds about the run, and its working folder **when Koru made
    /// that folder**.
    fn purge(&self, context: RequestContext, run: RunId) -> Result<PurgeRunResponse>;

    /// `POST /v1/runs/{run}/rate`: one gesture on a finished run (D44).
    fn rate(
        &self,
        context: RequestContext,
        run: RunId,
        request: RateRunRequest,
    ) -> Result<RateRunResponse>;
    /// Every history entry after index `after`, then the live ones as they
    /// are written, and every status change.
    fn stream(
        &self,
        context: RequestContext,
        run: RunId,
        workspace: Option<WorkspaceId>,
        after: Option<u64>,
    ) -> Result<Box<dyn RunStream>>;

    /// `POST /v1/schedules`: the canonical Schedule document is durable
    /// before this answers. Owner, revision, lifecycle, and timestamps are
    /// worker authority and do not appear in the request.
    fn create_schedule(
        &self,
        context: RequestContext,
        request: CreateScheduleRequest,
    ) -> Result<CreateScheduleResponse>;

    fn rename_schedule(
        &self,
        context: RequestContext,
        request: RenameScheduleRequest,
    ) -> Result<RenameScheduleResponse> {
        let _ = (context, request);
        Err(unsupported_schedule_command())
    }

    fn edit_schedule(
        &self,
        context: RequestContext,
        request: EditScheduleRequest,
    ) -> Result<EditScheduleResponse> {
        let _ = (context, request);
        Err(unsupported_schedule_command())
    }

    fn pause_schedule(
        &self,
        context: RequestContext,
        request: PauseScheduleRequest,
    ) -> Result<PauseScheduleResponse> {
        let _ = (context, request);
        Err(unsupported_schedule_command())
    }

    fn resume_schedule(
        &self,
        context: RequestContext,
        request: ResumeScheduleRequest,
    ) -> Result<ResumeScheduleResponse> {
        let _ = (context, request);
        Err(unsupported_schedule_command())
    }

    fn retire_schedule(
        &self,
        context: RequestContext,
        request: RetireScheduleRequest,
    ) -> Result<RetireScheduleResponse> {
        let _ = (context, request);
        Err(unsupported_schedule_command())
    }

    fn list_schedules(
        &self,
        context: RequestContext,
        workspace: WorkspaceId,
    ) -> Result<ListSchedulesResponse>;

    /// Content-free invalidations for the list. `after` suppresses only the
    /// exact generation already observed; it is not a durable cursor.
    fn watch_schedules(
        &self,
        context: RequestContext,
        workspace: WorkspaceId,
        after: Option<ScheduleChange>,
    ) -> Result<Box<dyn ScheduleStream>>;
}

fn unsupported_schedule_command() -> Failure {
    Failure::new(
        "unsupported_schedule_command",
        "This schedule-management command is not available in this worker build.",
        false,
    )
}
