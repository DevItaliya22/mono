// SPDX-License-Identifier: MPL-2.0

//! What this worker has done since it started, as a handful of integers.
//!
//! **Always on, and that is the point.** *Free when off* is the rule for a
//! trace that formats a line per event; it does not apply to twenty-two
//! `AtomicU64`s incremented with `Relaxed`, any more than it applies to
//! the store's counters (PRINCIPLES, *free when off, cheap enough to leave
//! on* — "the store's counters are a handful of integers and are always
//! live"). A counter somebody has to enable and rebuild for is one that
//! was off during the incident, so there is no switch here and no
//! environment read.
//!
//! **Content-free by construction.** Every number is a count of things
//! that happened; nothing here can hold an id, a path or a payload, so the
//! snapshot is safe wherever it is pasted.
//!
//! ## Where they are counted, and why there
//!
//! At the seams this crate owns, so the numbers cannot drift from the
//! writes they describe:
//!
//! - [`crate::ledger`] — the `RunStore` port over `runs.db`. Every
//!   consequential thing the orchestrator does is durable before it is
//!   anything else (§7.1), and that write goes through there: the intent,
//!   the attempt, the publication, its acknowledgement, its retry. A
//!   publication of kind `ArtifactRegistration` is a result an agent
//!   registered, which is why *artifacts registered* is counted with the
//!   queue rather than guessed from a fact.
//! - [`CountingCapture`], the decorator a host wraps its capture port in,
//!   because §7.2's capture is the one consequential step that leaves no
//!   row in the ledger of its own.
//! - The host's restart path, which hands [`restart`] the report §7.6
//!   produced — one call, six verdicts.
//!
//! *Runs active* is deliberately **not** here: it is a gauge rather than a
//! total, the actor already publishes it on the worker's hello, and a
//! second copy of it would be a second answer.

use std::sync::atomic::{AtomicU64, Ordering};

use koru_delegate_core::{PublicationKind, RunContextItem, RunId, WorkspaceId};
use koru_delegate_schedule::ScheduleFiringOutcome;

use crate::{
    CaptureFailure, CapturedContext, ContextCapture, MaterializeFailure, MaterializedContext,
};

/// The worker's totals since this process started.
///
/// Every field is monotonic: a total, never a gauge, so two readings
/// subtract into "what happened in between" without any state of their own.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Counters {
    /// Delegation requests recorded durably (§7.1 step 1).
    pub intents_recorded: u64,
    /// Execution attempts opened, across every run and generation.
    pub attempts_opened: u64,
    /// Context captures that succeeded (§7.2).
    pub contexts_captured: u64,
    /// Captures that failed and abandoned their request.
    pub captures_failed: u64,
    /// Facts committed to §7.1's queue before travelling.
    pub publications_queued: u64,
    /// Of those, results an agent registered through the artifact tools.
    pub artifacts_registered: u64,
    /// Publications the bus acknowledged.
    pub publications_acknowledged: u64,
    /// Re-deliveries of a publication under the same `EventId`.
    pub publication_retries: u64,
    /// Times this worker classified its ledger at startup (§7.6).
    pub restarts: u64,
    /// Intents completed forward from their own payload.
    pub restart_completed_forward: u64,
    /// Runs that existed and had never started, started by a restart.
    pub restart_started: u64,
    /// Attempts whose session the host still held.
    pub restart_reattached: u64,
    /// Attempts reconciled from the host's retained answer.
    pub restart_reconciled: u64,
    /// Attempts marked `Interrupted` — never `Failed` (D10).
    pub restart_interrupted: u64,
    /// Reservations a re-ask never resolved; left running, claim kept.
    pub restart_unresolved: u64,
    /// Coalesced schedule wakes accepted while no wake was already pending.
    pub schedule_wakes: u64,
    /// Real passes through the background schedule evaluator.
    pub schedule_evaluations: u64,
    /// Canonical schedule documents the evaluator actually considered.
    pub schedule_documents_evaluated: u64,
    /// Failures observed during schedule evaluation, including partial catalog rows.
    pub schedule_evaluation_failures: u64,
    /// Newly appended fired outcomes. Reconciliation/replay never moves it.
    pub schedule_fired: u64,
    /// Newly appended missed outcomes. Reconciliation/replay never moves it.
    pub schedule_skipped_missed: u64,
    /// Newly appended busy outcomes. Reconciliation/replay never moves it.
    pub schedule_skipped_busy: u64,
}

impl Counters {
    /// One line of tokens and numbers, for a log or a status pane.
    pub fn describe(&self) -> String {
        format!(
            "delegate intents={} attempts={} captured={} capture_failed={} \
             queued={} artifacts={} acknowledged={} retries={} restarts={} \
             schedule_wakes={} schedule_evaluations={} schedule_documents={} \
             schedule_failures={} schedule_fired={} schedule_missed={} schedule_busy={}",
            self.intents_recorded,
            self.attempts_opened,
            self.contexts_captured,
            self.captures_failed,
            self.publications_queued,
            self.artifacts_registered,
            self.publications_acknowledged,
            self.publication_retries,
            self.restarts,
            self.schedule_wakes,
            self.schedule_evaluations,
            self.schedule_documents_evaluated,
            self.schedule_evaluation_failures,
            self.schedule_fired,
            self.schedule_skipped_missed,
            self.schedule_skipped_busy,
        )
    }
}

macro_rules! counters {
    ($($slot:ident : $field:ident),* $(,)?) => {
        $(static $slot: AtomicU64 = AtomicU64::new(0);)*

        /// The counters as they stand. A read, never a reset — resetting
        /// would make two readers disagree about the same worker.
        pub fn snapshot() -> Counters {
            Counters {
                $($field: $slot.load(Ordering::Relaxed),)*
            }
        }
    };
}

// One row per counter, pairing the slot with the field it is read into, so
// a counter cannot exist without being readable and the two lists cannot
// drift out of step.
counters!(
    INTENTS_RECORDED: intents_recorded,
    ATTEMPTS_OPENED: attempts_opened,
    CONTEXTS_CAPTURED: contexts_captured,
    CAPTURES_FAILED: captures_failed,
    PUBLICATIONS_QUEUED: publications_queued,
    ARTIFACTS_REGISTERED: artifacts_registered,
    PUBLICATIONS_ACKNOWLEDGED: publications_acknowledged,
    PUBLICATION_RETRIES: publication_retries,
    RESTARTS: restarts,
    RESTART_COMPLETED_FORWARD: restart_completed_forward,
    RESTART_STARTED: restart_started,
    RESTART_REATTACHED: restart_reattached,
    RESTART_RECONCILED: restart_reconciled,
    RESTART_INTERRUPTED: restart_interrupted,
    RESTART_UNRESOLVED: restart_unresolved,
    SCHEDULE_WAKES: schedule_wakes,
    SCHEDULE_EVALUATIONS: schedule_evaluations,
    SCHEDULE_DOCUMENTS_EVALUATED: schedule_documents_evaluated,
    SCHEDULE_EVALUATION_FAILURES: schedule_evaluation_failures,
    SCHEDULE_FIRED: schedule_fired,
    SCHEDULE_SKIPPED_MISSED: schedule_skipped_missed,
    SCHEDULE_SKIPPED_BUSY: schedule_skipped_busy,
);

fn bump(counter: &AtomicU64, by: u64) {
    counter.fetch_add(by, Ordering::Relaxed);
}

// Runtime tests execute in parallel while these production counters are
// process-global. Mirror only the schedule increments per test thread so a
// counter assertion cannot absorb work performed by an unrelated scenario.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ScheduleTestCounters {
    pub wakes: u64,
    pub evaluations: u64,
    pub documents_evaluated: u64,
    pub evaluation_failures: u64,
    pub fired: u64,
    pub skipped_missed: u64,
    pub skipped_busy: u64,
}

#[cfg(test)]
std::thread_local! {
    static TEST_SCHEDULE_COUNTERS: std::cell::Cell<ScheduleTestCounters> =
        std::cell::Cell::new(ScheduleTestCounters::default());
}

#[cfg(test)]
fn observe_schedule(change: impl FnOnce(&mut ScheduleTestCounters)) {
    TEST_SCHEDULE_COUNTERS.with(|observed| {
        let mut counters = observed.get();
        change(&mut counters);
        observed.set(counters);
    });
}

#[cfg(test)]
pub(crate) fn schedule_test_snapshot() -> ScheduleTestCounters {
    TEST_SCHEDULE_COUNTERS.with(std::cell::Cell::get)
}

// ------------------------------------------------------- the ledger seam

pub(crate) fn intent_recorded() {
    bump(&INTENTS_RECORDED, 1);
}

pub(crate) fn attempt_opened() {
    bump(&ATTEMPTS_OPENED, 1);
}

pub(crate) fn publication_queued(kind: PublicationKind) {
    bump(&PUBLICATIONS_QUEUED, 1);
    if kind == PublicationKind::ArtifactRegistration {
        bump(&ARTIFACTS_REGISTERED, 1);
    }
}

pub(crate) fn publication_acknowledged() {
    bump(&PUBLICATIONS_ACKNOWLEDGED, 1);
}

pub(crate) fn publication_retried() {
    bump(&PUBLICATION_RETRIES, 1);
}

// ----------------------------------------------------- the schedule seam

/// One false-to-true transition of the scheduler's sticky wake bit.
pub(crate) fn schedule_woken() {
    bump(&SCHEDULE_WAKES, 1);
    #[cfg(test)]
    observe_schedule(|counters| counters.wakes += 1);
}

/// One real scheduler evaluation pass, immediately before it begins.
pub(crate) fn schedule_evaluation() {
    bump(&SCHEDULE_EVALUATIONS, 1);
    #[cfg(test)]
    observe_schedule(|counters| counters.evaluations += 1);
}

/// One canonical schedule document handed to the evaluator.
pub(crate) fn schedule_document_evaluated() {
    bump(&SCHEDULE_DOCUMENTS_EVALUATED, 1);
    #[cfg(test)]
    observe_schedule(|counters| counters.documents_evaluated += 1);
}

/// Evaluation failures are counted by occurrence, not by log transition.
pub(crate) fn schedule_evaluation_failed(by: u64) {
    bump(&SCHEDULE_EVALUATION_FAILURES, by);
    #[cfg(test)]
    observe_schedule(|counters| counters.evaluation_failures += by);
}

/// A firing outcome newly appended to its Schedule document. Callers must
/// exclude replay and reconciliation even if those paths repair a missing
/// append: these are live process totals, not a reconstruction of history.
pub(crate) fn schedule_firing_applied(outcome: ScheduleFiringOutcome) {
    match outcome {
        ScheduleFiringOutcome::Fired => {
            bump(&SCHEDULE_FIRED, 1);
            #[cfg(test)]
            observe_schedule(|counters| counters.fired += 1);
        }
        ScheduleFiringOutcome::SkippedMissed => {
            bump(&SCHEDULE_SKIPPED_MISSED, 1);
            #[cfg(test)]
            observe_schedule(|counters| counters.skipped_missed += 1);
        }
        ScheduleFiringOutcome::SkippedBusy => {
            bump(&SCHEDULE_SKIPPED_BUSY, 1);
            #[cfg(test)]
            observe_schedule(|counters| counters.skipped_busy += 1);
        }
    }
}

// ------------------------------------------------------ the restart seam

/// One restart, classified. Called by the host with the report §7.6
/// produced, so the six verdicts are counted from the one place that has
/// them all rather than from six call sites inside the classification.
pub fn restart(report: &crate::RestartReport) {
    bump(&RESTARTS, 1);
    bump(
        &RESTART_COMPLETED_FORWARD,
        report.completed_forward.len() as u64,
    );
    bump(&RESTART_STARTED, report.started.len() as u64);
    bump(&RESTART_REATTACHED, report.reattached.len() as u64);
    bump(&RESTART_RECONCILED, report.reconciled.len() as u64);
    bump(&RESTART_INTERRUPTED, report.interrupted.len() as u64);
    bump(&RESTART_UNRESOLVED, report.unresolved.len() as u64);
}

// ------------------------------------------------------ the capture seam

/// A [`ContextCapture`] that counts what passes through it.
///
/// A decorator rather than a counter inside the capture implementation,
/// because §7.2's capture is P1.7's and its host adapter is not this
/// crate's to reach into — and because a wrapper counts whichever
/// implementation a host composes, including a test's.
pub struct CountingCapture(Box<dyn ContextCapture>);

impl CountingCapture {
    pub fn new(inner: Box<dyn ContextCapture>) -> Self {
        Self(inner)
    }
}

impl ContextCapture for CountingCapture {
    fn resolve_and_pin(
        &mut self,
        run: RunId,
        pointers: &[koru_delegate_schedule::ScheduleContextPointer],
        workspace: WorkspaceId,
    ) -> Result<Vec<RunContextItem>, CaptureFailure> {
        self.0.resolve_and_pin(run, pointers, workspace)
    }

    /// Read-only, so it counts nothing: D48's question is asked before an
    /// attempt exists and answers about the world, not about this worker.
    fn admit(
        &mut self,
        run: RunId,
        pins: &[crate::ContextPin],
    ) -> Result<(), crate::SourceRefusal> {
        self.0.admit(run, pins)
    }

    fn capture(
        &mut self,
        run: RunId,
        items: &[RunContextItem],
        workspace: WorkspaceId,
    ) -> Result<CapturedContext, CaptureFailure> {
        let captured = self.0.capture(run, items, workspace);
        match &captured {
            Ok(_) => bump(&CONTEXTS_CAPTURED, 1),
            Err(_) => bump(&CAPTURES_FAILED, 1),
        }
        captured
    }

    fn materialize(
        &mut self,
        run: RunId,
        captured: &CapturedContext,
        run_workspace: &std::path::Path,
    ) -> Result<MaterializedContext, MaterializeFailure> {
        self.0.materialize(run, captured, run_workspace)
    }

    fn cleanup(&mut self, run: RunId, run_workspace: &std::path::Path) {
        self.0.cleanup(run, run_workspace);
    }
}

#[cfg(test)]
mod tests;
