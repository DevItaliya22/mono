// SPDX-License-Identifier: MPL-2.0

//! The one thing this surface does to the world, and the thread it does it on.
//!
//! Pressing *Delegate* pins the context (ROADMAP §7.2) and creates the run
//! (`POST /v1/runs`); pressing *Create schedule* converts those sources to live
//! pointers and creates the schedule (`POST /v1/schedules`). All of that is
//! I/O, so **none happens on the window thread**: `start` clones the gateway
//! client, hands the request to a worker named for it, and the answer comes
//! back down an `mpsc` channel that the container drains in `maintain`. Queue
//! first, wake second, the rule every per-window worker in this app follows.
//!
//! **One path creates every run.** [`create_run`] takes a [`RunSpec`] — a
//! plain value with no modal type in it — and is what the modal's *Run once*
//! and a canvas's agent node both reach: the modal through its own [`Job`],
//! which also carries the crumb and the correlation only a modal has, and a
//! canvas through [`start_delegation`], one worker per run so many can be in
//! flight while the modal's single job is too.
//!
//! **The pin is `koru-delegate-context`'s, not a second one.** A window has no
//! `koru_store_core::service::Store` — it reaches the store over the gateway —
//! so [`GatewayStore`] implements that crate's own `ContextStore` port against
//! the same seams a worker uses: `capture_history` for a document's version,
//! the blob record and its bytes for retention. What P1.7 decides about
//! ordering, all-or-nothing and how a failure is named stays in P1.7.

use std::collections::BTreeSet;
use std::sync::mpsc::{self, Receiver};

use koru_delegate_api::wire::{CreateRunRequest, CreateScheduleRequest, EditScheduleRequest};
use koru_delegate_context::{
    CaptureFailure, ContextSource, ContextStore, Fault, RunContextItem, StoreContext,
};
use koru_delegate_core::{
    ActorRef, BlobHash, DocumentId, DocumentVersionId, ModelName, Objective, RunId, ScheduleId,
    SessionProfileKey, WorkspaceId, WorkspaceScope,
};
use koru_delegate_schedule::{DelegationTemplate, ScheduleContextPointer};
use koru_store_core::model::{Blob, DocumentEntry, Timestamp};
use koru_store_core::ports::DocumentVersion;
use koru_store_wire::CaptureHistory;

use super::modal::{Phase, Request, RequestCorrelation, Submission};

/// What the worker answers with, stamped with the run it is about so a late
/// answer for a surface that has since been replaced is discarded rather than
/// shown.
pub(crate) struct Answer {
    pub(crate) correlation: RequestCorrelation,
    pub(crate) phase: Phase,
}

/// One delegation in flight.
pub(crate) struct Job {
    pub(crate) correlation: RequestCorrelation,
    /// Where this delegation's pill goes (D8), held here rather than sent
    /// through the worker: the worker has no business with a document, and an
    /// answer that arrives after the surface has closed still has to leave the
    /// crumb the gesture promised.
    pub(crate) pill_anchor: Option<super::world::PillAnchor>,
    /// Authored fallback label for a newly-created schedule pill. Edits never
    /// mint another pill and therefore carry none.
    pub(crate) schedule_name: Option<koru_delegate_schedule::ScheduleName>,
    pub(crate) receiver: Receiver<Answer>,
}

impl Job {
    /// The worker's answer, if it has one. Never blocks.
    pub(crate) fn try_take(&self) -> Option<Answer> {
        self.receiver.try_recv().ok()
    }
}

/// The code a refusal carries when this device has no delegate transport at
/// all — the same token the gateway answers `/v1/runs/*` with when the host
/// composed none, so the sentence a person reads is the same either way.
const UNAVAILABLE: &str = "delegate_unavailable";
/// A refusal this window minted rather than the gateway: the objective was
/// longer than a run may carry, or held something a run may not.
const OBJECTIVE_REFUSED: &str = "objective_refused";
/// The worker could not be started. Not a failure of the delegation.
const NO_WORKER: &str = "worker_unavailable";

/// Start the work. Returns the job to drain, or the refusal to show at once
/// when there was nothing to start it with.
pub(crate) fn start(
    client: Option<koru_api_client::Client>,
    request: Request,
    wake: Option<winit::event_loop::EventLoopProxy<koru_workspace_host::app::Wake>>,
) -> Result<Job, Phase> {
    let correlation = request.correlation();
    let (pill_anchor, schedule_name) = match &request.submission {
        Submission::RunOnce { .. } => (request.pill_anchor, None),
        Submission::Repeat { name, .. } => (request.pill_anchor, Some(name.clone())),
        Submission::Edit { .. } => (None, None),
    };
    let Some(client) = client else {
        // No store client: this window was built without one, which is every
        // test container. An honest refusal beats a thread that answers never.
        return Err(Phase::Refused {
            code: UNAVAILABLE.into(),
        });
    };
    let (send, receiver) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("koru-delegate".into())
        .spawn(move || {
            let phase = run_it(&client, request);
            // Queue first, wake second: the other order lets the loop find an
            // empty channel and sleep past the answer that has just arrived.
            if send.send(Answer { correlation, phase }).is_ok()
                && let Some(proxy) = wake
            {
                let _ = proxy.send_event(koru_workspace_host::app::Wake::Store);
            }
        });
    match spawned {
        Ok(_) => Ok(Job {
            correlation,
            pill_anchor,
            schedule_name,
            receiver,
        }),
        Err(_) => Err(Phase::Refused {
            code: NO_WORKER.into(),
        }),
    }
}

/// The whole of the worker: either pin and create a run, or retain live
/// pointers and create a schedule.
fn run_it(client: &koru_api_client::Client, request: Request) -> Phase {
    match request.submission.clone() {
        Submission::RunOnce { run } => run_once(client, request, run),
        Submission::Repeat {
            schedule,
            name,
            recurrence,
        } => repeat(client, request, schedule, name, recurrence),
        Submission::Edit {
            schedule,
            expected_revision,
            expected_name,
            requested_by,
            name,
            recurrence,
        } => edit(
            client,
            request,
            schedule,
            expected_revision,
            expected_name,
            requested_by,
            name,
            recurrence,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn edit(
    client: &koru_api_client::Client,
    request: Request,
    schedule: ScheduleId,
    expected_revision: u32,
    expected_name: koru_delegate_schedule::ScheduleName,
    requested_by: ActorRef,
    name: koru_delegate_schedule::ScheduleName,
    recurrence: koru_recurrence::RecurrenceRule,
) -> Phase {
    if !client.schedules().available() {
        return Phase::Refused {
            code: UNAVAILABLE.into(),
        };
    }
    let body = match schedule_template(request, requested_by) {
        Ok(template) => EditScheduleRequest {
            schedule,
            workspace: template.0,
            expected_revision,
            expected_name,
            name,
            template: template.1,
            recurrence,
        },
        Err(phase) => return phase,
    };
    match client.schedules().edit(&body) {
        Ok(response) => Phase::Scheduled {
            schedule: response.schedule.header.schedule,
        },
        Err(failure) => Phase::Refused { code: failure.code },
    }
}

/// The modal's *Run once*: its request, as a [`RunSpec`], through the one
/// path every delegated run is created by.
fn run_once(client: &koru_api_client::Client, request: Request, run: RunId) -> Phase {
    let mut gateway = ClientGateway {
        client: client.clone(),
    };
    match create_run(&mut gateway, RunSpec::of_request(request, run)) {
        RunAnswer::Done {
            run,
            document,
            busy,
        } => Phase::Done {
            run,
            document,
            busy,
        },
        RunAnswer::Refused { code } => Phase::Refused { code },
    }
}

// ------------------------------------------------------------ one run, plainly

/// **One delegated run, as a plain value** — everything `POST /v1/runs` needs
/// except the pinned context, which the worker makes from `sources`.
///
/// No modal type is in it: the delegation modal's *Run once* is turned into
/// one ([`RunSpec::of_request`]) and a canvas's agent node builds one
/// ([`RunSpec::new`]), and both reach [`start_delegation`] / [`create_run`].
///
/// **`run` is minted per press.** Pressing again is a new run with a new id:
/// retrying an old id after re-pinning its sources could capture different
/// document versions under the same `RunId`.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RunSpec {
    pub(crate) run: RunId,
    pub(crate) workspace: WorkspaceId,
    /// Who is asking, as the profile they act under.
    pub(crate) profile: koru_store_core::ProfileId,
    /// What to do. Validated on the worker (non-blank, bounded); a refusal
    /// comes back as `objective_refused`.
    pub(crate) objective: String,
    /// The launch profile to run it with — one ready on this machine.
    pub(crate) agent: SessionProfileKey,
    /// The person's explicit model, or the provider's default.
    pub(crate) model: Option<ModelName>,
    pub(crate) scope: WorkspaceScope,
    /// What the run is given, pinned on the worker before the run exists.
    pub(crate) sources: Vec<ContextSource>,
    /// D28: the workspaces the run's tools may reach. Empty denies all.
    pub(crate) grants: BTreeSet<WorkspaceId>,
}

impl RunSpec {
    /// **A fresh press with the defaults a canvas node uses**: a new run id,
    /// a temporary folder of Koru's making, no explicit model, no context yet
    /// and this workspace granted (D28's default).
    // Called by `MainView::canvas_run_spec`, whose caller is the canvas
    // surface; allowed until that is wired (see `canvas_runs`).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new(
        workspace: WorkspaceId,
        profile: koru_store_core::ProfileId,
        objective: impl Into<String>,
        agent: SessionProfileKey,
    ) -> Self {
        Self {
            run: RunId::mint(),
            workspace,
            profile,
            objective: objective.into(),
            agent,
            model: None,
            scope: WorkspaceScope::Temporary,
            sources: Vec::new(),
            grants: BTreeSet::from([workspace]),
        }
    }

    /// The modal's request, under the run id its press minted.
    pub(crate) fn of_request(request: Request, run: RunId) -> Self {
        Self {
            run,
            workspace: request.workspace,
            profile: request.profile,
            objective: request.objective,
            agent: request.agent,
            model: request.requested_model,
            scope: request.scope,
            sources: request.sources,
            grants: request.grants,
        }
    }
}

impl std::fmt::Debug for RunSpec {
    /// **The objective is content** (standing requirement 7): ids and counts
    /// only.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunSpec")
            .field("run", &self.run)
            .field("workspace", &self.workspace)
            .field("agent", &self.agent)
            .field("sources", &self.sources.len())
            .field("grants", &self.grants.len())
            .finish_non_exhaustive()
    }
}

/// What one run's worker answers.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum RunAnswer {
    /// `202 Accepted`: the run exists. `document` is the **Run document**,
    /// which is not the run's id — a pill and a canvas node point at a
    /// document. `busy` names a run already holding the chosen folder (D34's
    /// warning, which is not a refusal).
    Done {
        run: RunId,
        document: koru_core::DocumentId,
        busy: Option<RunId>,
    },
    /// Not created, with a stable code: `delegate_unavailable`,
    /// `objective_refused`, `context_<fault>`, `worker_unavailable`, or the
    /// gateway's own.
    Refused { code: String },
}

/// One run being created, on a worker of its own.
pub(crate) struct RunJob {
    pub(crate) run: RunId,
    receiver: Receiver<RunAnswer>,
}

impl RunJob {
    /// A job whose answer arrives down `receiver` — the seam a test answers
    /// through instead of a worker.
    #[cfg(test)]
    pub(crate) fn from_receiver(run: RunId, receiver: Receiver<RunAnswer>) -> Self {
        Self { run, receiver }
    }

    /// The worker's answer, if it has one. Never blocks. A worker that went
    /// away without answering is a refusal, not a run that starts forever.
    pub(crate) fn try_take(&self) -> Option<RunAnswer> {
        match self.receiver.try_recv() {
            Ok(answer) => Some(answer),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(RunAnswer::Refused {
                code: NO_WORKER.into(),
            }),
        }
    }
}

/// **Start one delegated run off the window thread**: pin its context and
/// create it on a `koru-delegate` worker, answering once.
///
/// The refusal comes back at once when there is nothing to start it with —
/// no store client on this window, or no thread.
pub(crate) fn start_delegation(
    client: Option<koru_api_client::Client>,
    spec: RunSpec,
    wake: Option<winit::event_loop::EventLoopProxy<koru_workspace_host::app::Wake>>,
) -> Result<RunJob, RunAnswer> {
    let Some(client) = client else {
        return Err(RunAnswer::Refused {
            code: UNAVAILABLE.into(),
        });
    };
    let run = spec.run;
    let (send, receiver) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("koru-delegate".into())
        .spawn(move || {
            let answer = create_run(&mut ClientGateway { client }, spec);
            // Queue first, wake second.
            if send.send(answer).is_ok()
                && let Some(proxy) = wake
            {
                let _ = proxy.send_event(koru_workspace_host::app::Wake::Store);
            }
        });
    match spawned {
        Ok(_) => Ok(RunJob { run, receiver }),
        Err(_) => Err(RunAnswer::Refused {
            code: NO_WORKER.into(),
        }),
    }
}

/// The four things creating a run asks of the world. [`ClientGateway`]
/// answers them over the gateway; a test answers them from memory.
pub(crate) trait RunGateway {
    /// Whether this device has a delegate transport at all.
    fn available(&self) -> bool;
    /// §7.2 steps 1–2: the sources, pinned.
    fn pin(&mut self, sources: &[ContextSource]) -> Result<Vec<RunContextItem>, CaptureFailure>;
    /// A run already working in this scope, if any (D34).
    fn busy_holder(&mut self, workspace: WorkspaceId, scope: &WorkspaceScope) -> Option<RunId>;
    /// `POST /v1/runs`: the run and its Run document, or the refusal's code.
    fn create(&mut self, body: &CreateRunRequest) -> Result<(RunId, DocumentId), String>;
}

/// **The whole of creating a run**, in the order the modal always did it:
/// the transport, the objective, the pin, the busy warning, the create.
pub(crate) fn create_run(gateway: &mut impl RunGateway, spec: RunSpec) -> RunAnswer {
    if !gateway.available() {
        return RunAnswer::Refused {
            code: UNAVAILABLE.into(),
        };
    }
    let Ok(objective) = Objective::new(spec.objective) else {
        return RunAnswer::Refused {
            code: OBJECTIVE_REFUSED.into(),
        };
    };
    // §7.2 steps 1–2, through P1.7's own pin: one `capture_history` version
    // per document, a retention assertion per blob, all or nothing.
    let context = match gateway.pin(&spec.sources) {
        Ok(items) => items,
        Err(failure) => {
            return RunAnswer::Refused {
                code: format!("context_{}", failure.fault.as_str()),
            };
        }
    };
    // **D34's warning, taken before the run is created** so it describes what
    // was true when the person pressed the button. It warns and allows: the
    // run is created either way, and the surface names the holder. The
    // authoritative check is the orchestrator's, against the real filesystem
    // (P1.5 admission); this one compares the scope a person chose against the
    // scopes runs already carry, which needs no path and no disk.
    let busy = gateway.busy_holder(spec.workspace, &spec.scope);
    let body = CreateRunRequest {
        run: spec.run,
        workspace: spec.workspace,
        requested_by: ActorRef::Person {
            profile: spec.profile,
        },
        objective,
        agent: spec.agent,
        requested_model: spec.model,
        scope: spec.scope,
        context,
        grants: spec.grants,
    };
    match gateway.create(&body) {
        Ok((run, document)) => RunAnswer::Done {
            run,
            // The Run document, not the run id: the pill points at a document.
            document: koru_core::DocumentId::from_uuid(document.as_uuid()),
            busy,
        },
        Err(code) => RunAnswer::Refused { code },
    }
}

/// [`RunGateway`] over the window's gateway client.
struct ClientGateway {
    client: koru_api_client::Client,
}

impl RunGateway for ClientGateway {
    fn available(&self) -> bool {
        self.client.runs().available()
    }

    fn pin(&mut self, sources: &[ContextSource]) -> Result<Vec<RunContextItem>, CaptureFailure> {
        let mut store = GatewayStore {
            client: self.client.clone(),
        };
        StoreContext::new(&mut store).pin(sources)
    }

    fn busy_holder(&mut self, workspace: WorkspaceId, scope: &WorkspaceScope) -> Option<RunId> {
        busy_holder(&self.client, workspace, scope)
    }

    fn create(&mut self, body: &CreateRunRequest) -> Result<(RunId, DocumentId), String> {
        self.client
            .runs()
            .create(body)
            .map(|response| (response.run, response.document))
            .map_err(|failure| failure.code)
    }
}

fn repeat(
    client: &koru_api_client::Client,
    request: Request,
    schedule: ScheduleId,
    name: koru_delegate_schedule::ScheduleName,
    recurrence: koru_recurrence::RecurrenceRule,
) -> Phase {
    if !client.schedules().available() {
        return Phase::Refused {
            code: UNAVAILABLE.into(),
        };
    }
    let body = match schedule_body(request, schedule, name, recurrence) {
        Ok(body) => body,
        Err(phase) => return phase,
    };
    match client.schedules().create(&body) {
        Ok(response) => Phase::Scheduled {
            schedule: response.schedule.header.schedule,
        },
        Err(failure) => Phase::Refused { code: failure.code },
    }
}

/// Construct the schedule request without touching the client. Keeping this
/// seam pure makes the crucial no-pin boundary and complete template copy
/// directly testable without disk or network.
pub(crate) fn schedule_body(
    request: Request,
    schedule: ScheduleId,
    name: koru_delegate_schedule::ScheduleName,
    recurrence: koru_recurrence::RecurrenceRule,
) -> Result<CreateScheduleRequest, Phase> {
    let requested_by = ActorRef::Person {
        profile: request.profile,
    };
    let (workspace, template) = schedule_template(request, requested_by)?;
    Ok(CreateScheduleRequest {
        schedule,
        workspace,
        name,
        template,
        recurrence,
    })
}

fn schedule_template(
    request: Request,
    requested_by: ActorRef,
) -> Result<(WorkspaceId, DelegationTemplate), Phase> {
    let Ok(objective) = Objective::new(request.objective) else {
        return Err(Phase::Refused {
            code: OBJECTIVE_REFUSED.into(),
        });
    };
    let context = request.sources.into_iter().map(schedule_pointer).collect();
    Ok((
        request.workspace,
        DelegationTemplate {
            requested_by,
            objective,
            agent: request.agent,
            requested_model: request.requested_model,
            scope: request.scope,
            grants: request.grants,
            context,
        },
    ))
}

/// A schedule retains live identity only. This conversion cannot capture a
/// document version or read blob bytes, and therefore cannot accidentally pin
/// context before the first occurrence is claimed.
pub(crate) fn schedule_pointer(source: ContextSource) -> ScheduleContextPointer {
    match source {
        ContextSource::Document {
            workspace,
            document,
        } => ScheduleContextPointer::Document {
            workspace,
            document,
        },
        ContextSource::Block {
            workspace,
            document,
            node,
        } => ScheduleContextPointer::Block {
            workspace,
            document,
            node,
        },
        ContextSource::Blob {
            workspace,
            blob,
            blob_kind,
        } => ScheduleContextPointer::Blob {
            workspace,
            blob,
            blob_kind,
        },
    }
}

/// Which run, if any, is already working in this scope.
///
/// Structural identity only — the same project, or the same project and
/// folder. A temporary scope is a fresh directory per run and can never be
/// busy. Aliases, junctions and nesting are the scope guard's to resolve
/// against the filesystem, which is not a window's to touch; a warning that
/// misses one of those is a warning the run's own timeline still records.
fn busy_holder(
    client: &koru_api_client::Client,
    workspace: WorkspaceId,
    scope: &WorkspaceScope,
) -> Option<RunId> {
    if matches!(scope, WorkspaceScope::Temporary | WorkspaceScope::None) {
        return None;
    }
    client
        .runs()
        .list(workspace)
        .ok()?
        .runs
        .into_iter()
        .find(|header| !header.status.is_terminal() && header.scope == *scope)
        .map(|header| header.run)
}

/// `koru-delegate-context`'s store port, answered over the gateway.
///
/// The four questions `pin` asks have routes: a document's catalogue entry,
/// the `capture_history` commit that mints its version, the blob's record and
/// its bytes. The fifth — reading a version back by its id — does not, and is
/// answered so below.
struct GatewayStore {
    client: koru_api_client::Client,
}

impl ContextStore for GatewayStore {
    fn capture_version(
        &mut self,
        workspace: WorkspaceId,
        document: DocumentId,
        _at: Timestamp,
    ) -> Result<DocumentVersionId, Fault> {
        // `manual`, the same reason the worker's own capture uses: a person
        // delegating *is* asking for this moment to be kept, and it is the one
        // reason the History panel shows as pinned. No label and no message —
        // both are free text a person would read, and a fixed English string
        // is not localization. The store stamps its own time, which is why the
        // caller's `at` is not sent.
        self.client
            .capture_history(
                workspace,
                document,
                &CaptureHistory {
                    reason: "manual".into(),
                    frontiers: None,
                    label: None,
                    message: None,
                    boundary: None,
                },
            )
            .map(|entry| entry.version)
            .map_err(|error| Fault::from_store(&error))
    }

    /// **The gateway cannot answer this one**, and nothing on this path asks
    /// it: `POST /v1/workspaces/{w}/history/{entry}/snapshot` is addressed by
    /// the history *entry* that made a version, not by the version id, and a
    /// window holds no `Store` to ask directly. `pin` never calls it —
    /// verifying a pin is `capture`'s, and `capture` runs in the worker over a
    /// real store. A caller that did ask gets a store failure rather than a
    /// confident wrong answer.
    fn version(
        &mut self,
        _workspace: WorkspaceId,
        _version: DocumentVersionId,
    ) -> Result<Option<DocumentVersion>, Fault> {
        Err(Fault::StoreFailed)
    }

    fn entries(&mut self, workspace: WorkspaceId) -> Result<Vec<DocumentEntry>, Fault> {
        self.client
            .documents(workspace)
            .map_err(|error| Fault::from_store(&error))
    }

    fn blob(&mut self, workspace: WorkspaceId, hash: &BlobHash) -> Result<Option<Blob>, Fault> {
        self.client
            .blobs(workspace)
            .map(|blobs| blobs.into_iter().find(|blob| blob.hash == *hash))
            .map_err(|error| Fault::from_store(&error))
    }

    fn blob_bytes(&mut self, workspace: WorkspaceId, hash: &BlobHash) -> Result<Vec<u8>, Fault> {
        self.client
            .get_blob(workspace, hash)
            .map_err(|error| Fault::from_store(&error))
    }
}

#[cfg(test)]
mod tests;
