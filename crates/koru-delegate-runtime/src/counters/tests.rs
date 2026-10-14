// SPDX-License-Identifier: MPL-2.0

//! The counters are process-global and every test in this binary shares
//! them, so nothing here asserts an absolute value. **Deltas across the
//! work this test did** are the honest assertion, and they hold however
//! many other tests are running beside it.

use super::*;

fn delta(before: Counters, after: Counters) -> [u64; 8] {
    [
        after.intents_recorded - before.intents_recorded,
        after.attempts_opened - before.attempts_opened,
        after.contexts_captured - before.contexts_captured,
        after.captures_failed - before.captures_failed,
        after.publications_queued - before.publications_queued,
        after.artifacts_registered - before.artifacts_registered,
        after.publications_acknowledged - before.publications_acknowledged,
        after.publication_retries - before.publication_retries,
    ]
}

/// The ledger seam: one durable write, one number.
#[test]
fn a_committed_write_moves_exactly_one_counter() {
    let before = snapshot();
    intent_recorded();
    attempt_opened();
    publication_queued(PublicationKind::Lifecycle);
    publication_queued(PublicationKind::ArtifactRegistration);
    publication_acknowledged();
    publication_retried();
    publication_retried();
    let after = snapshot();
    // A registration is a publication *and* a registered result, which is
    // why the fifth number is two and the sixth is one.
    assert_eq!(delta(before, after), [1, 1, 0, 0, 2, 1, 1, 2]);
}

/// The one wait a restart classifies, counted from the report rather than
/// from six places inside the classification.
#[test]
fn a_restart_is_counted_by_verdict() {
    let before = snapshot();
    let mut report = crate::RestartReport::default();
    report.completed_forward.push(run(1));
    report.reconciled.push(run(2));
    report.interrupted.push(run(3));
    report.interrupted.push(run(4));
    report.unresolved.push(run(5));
    restart(&report);
    let after = snapshot();
    assert_eq!(after.restarts - before.restarts, 1);
    assert_eq!(
        after.restart_completed_forward - before.restart_completed_forward,
        1
    );
    assert_eq!(after.restart_reconciled - before.restart_reconciled, 1);
    assert_eq!(after.restart_interrupted - before.restart_interrupted, 2);
    assert_eq!(after.restart_unresolved - before.restart_unresolved, 1);
    assert_eq!(after.restart_started - before.restart_started, 0);
    assert_eq!(after.restart_reattached - before.restart_reattached, 0);
}

/// The decorator counts whichever capture a host composed, and hands back
/// exactly what the inner port answered.
#[test]
fn the_capture_decorator_counts_both_outcomes_and_changes_neither() {
    let before = snapshot();
    let mut counting = CountingCapture::new(Box::new(Scripted { fail: false }));
    let captured = counting
        .capture(run(1), &[], WorkspaceId::from_uuid(uuid(0xa0)))
        .expect("the inner port succeeded");
    assert_eq!(captured.pins.len(), 0);
    let mut counting = CountingCapture::new(Box::new(Scripted { fail: true }));
    let failure = counting
        .capture(run(1), &[], WorkspaceId::from_uuid(uuid(0xa0)))
        .expect_err("the inner port failed");
    assert_eq!(failure.item, 7);
    let after = snapshot();
    assert_eq!(after.contexts_captured - before.contexts_captured, 1);
    assert_eq!(after.captures_failed - before.captures_failed, 1);
}

/// Tokens and numbers. Nothing that could carry what a person wrote.
#[test]
fn the_description_is_tokens_and_numbers() {
    let described = Counters {
        intents_recorded: 3,
        artifacts_registered: 5,
        schedule_wakes: 7,
        schedule_evaluations: 6,
        schedule_documents_evaluated: 11,
        schedule_evaluation_failures: 2,
        schedule_fired: 4,
        schedule_skipped_missed: 1,
        schedule_skipped_busy: 3,
        ..Counters::default()
    }
    .describe();
    assert!(described.contains("intents=3"), "{described}");
    assert!(described.contains("artifacts=5"), "{described}");
    for expected in [
        "schedule_wakes=7",
        "schedule_evaluations=6",
        "schedule_documents=11",
        "schedule_failures=2",
        "schedule_fired=4",
        "schedule_missed=1",
        "schedule_busy=3",
    ] {
        assert!(described.contains(expected), "{described}");
    }
    assert!(
        described.is_ascii(),
        "a counter line is ascii tokens: {described}"
    );
}

// ------------------------------------------------------------- fixtures

fn uuid(n: u8) -> uuid::Uuid {
    format!("00000000-0000-4000-8000-0000000000{n:02x}")
        .parse()
        .unwrap()
}

fn run(n: u8) -> RunId {
    RunId::from_uuid(uuid(n))
}

/// A capture that answers as told and records nothing.
struct Scripted {
    fail: bool,
}

impl ContextCapture for Scripted {
    fn resolve_and_pin(
        &mut self,
        _run: RunId,
        pointers: &[koru_delegate_schedule::ScheduleContextPointer],
        _workspace: WorkspaceId,
    ) -> Result<Vec<RunContextItem>, CaptureFailure> {
        Ok(pointers
            .iter()
            .map(|pointer| match pointer {
                koru_delegate_schedule::ScheduleContextPointer::Document {
                    workspace,
                    document,
                } => RunContextItem::Document {
                    workspace: *workspace,
                    document: *document,
                    version: koru_delegate_core::DocumentVersionId::from_uuid(
                        uuid::Uuid::from_u128(1),
                    ),
                },
                koru_delegate_schedule::ScheduleContextPointer::Block {
                    workspace,
                    document,
                    node,
                } => RunContextItem::Block {
                    workspace: *workspace,
                    document: *document,
                    node: *node,
                    version: koru_delegate_core::DocumentVersionId::from_uuid(
                        uuid::Uuid::from_u128(1),
                    ),
                },
                koru_delegate_schedule::ScheduleContextPointer::Blob {
                    workspace,
                    blob,
                    blob_kind,
                } => RunContextItem::Blob {
                    workspace: *workspace,
                    blob: blob.clone(),
                    blob_kind: *blob_kind,
                },
            })
            .collect())
    }

    fn admit(
        &mut self,
        _run: RunId,
        _pins: &[crate::ContextPin],
    ) -> Result<(), crate::SourceRefusal> {
        unreachable!("the decorator's admit is a pass-through")
    }

    fn capture(
        &mut self,
        _run: RunId,
        _items: &[RunContextItem],
        _workspace: WorkspaceId,
    ) -> Result<CapturedContext, CaptureFailure> {
        if self.fail {
            return Err(CaptureFailure {
                item: 7,
                stage: crate::CaptureStage::Resolve,
            });
        }
        Ok(CapturedContext { pins: Vec::new() })
    }

    fn materialize(
        &mut self,
        _run: RunId,
        _captured: &CapturedContext,
        _run_workspace: &std::path::Path,
    ) -> Result<MaterializedContext, MaterializeFailure> {
        unreachable!("the decorator's materialize is a pass-through")
    }

    fn cleanup(&mut self, _run: RunId, _run_workspace: &std::path::Path) {}
}
