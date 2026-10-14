// SPDX-License-Identifier: MPL-2.0

//! The economics fold over the document (§17.2, proposal §6): usage facts
//! committed in the run's own storage, replayed to the highest revision per
//! `(attempt, turn)`, with the model they name resolved through the
//! header's `models` map.

use super::*;

/// Delivered 100, corrected to 140, the 100 replayed: 140. A later
/// correction to 90: 90. Read from the document, on the writer and on a
/// replica that imported the snapshot.
#[test]
fn the_fold_selects_the_highest_revision_regardless_of_delivery_order() {
    let run = RunDocument::create(&header()).unwrap();
    run.append_step(&step(StepKind::Started, 1_000)).unwrap();
    run.append_step(&step(
        StepKind::TurnStarted {
            originator: TurnOriginator::Orchestrator,
        },
        1_001,
    ))
    .unwrap();
    run.record_usage(&usage(1, 0, 100)).unwrap();
    run.record_usage(&usage(1, 1, 140)).unwrap();
    run.record_usage(&usage(1, 0, 100)).unwrap();
    assert_eq!(run.timeline_len(), 5, "the list keeps every delivery");
    let folded = run.economics(2_000);
    assert_eq!(folded.turns_with_usage, 1);
    assert_eq!(folded.input.complete(), Some(140));
    assert_eq!(folded.orchestrator_turns, 1);
    assert_eq!(folded.elapsed_ms, Some(1_000));

    run.record_usage(&usage(1, 2, 90)).unwrap();
    run.record_usage(&usage(2, 0, 7)).unwrap();
    let folded = run.economics(2_000);
    assert_eq!(folded.turns_with_usage, 2);
    assert_eq!(folded.input.complete(), Some(97));

    let replica = RunDocument::from_snapshot(&run.export_snapshot().unwrap()).unwrap();
    assert_eq!(replica.economics(2_000), folded);
}

/// The header's `economics` is the fold, cached: empty until refreshed,
/// equal to the fold after, and read by a device that never replays.
#[test]
fn refresh_writes_the_fold_into_the_header() {
    let run = RunDocument::create(&header()).unwrap();
    run.append_step(&step(StepKind::Started, 1_000)).unwrap();
    run.record_usage(&usage(1, 0, 100)).unwrap();
    assert_eq!(run.header().unwrap().economics, EconomicsSummary::default());

    let written = run.refresh_economics(1_500).unwrap();
    assert_eq!(written, run.economics(1_500));
    assert_eq!(run.header().unwrap().economics, written);
    assert_eq!(written.input.complete(), Some(100));
    assert!(written.models.contains(&model(0x30)));

    let replica = RunDocument::from_snapshot(&run.export_snapshot().unwrap()).unwrap();
    assert_eq!(replica.header().unwrap().economics, written);
}

/// A usage fact names its model by reference; the metadata the reference
/// points at is in the same document (§17.3) and resolves after sync.
#[test]
fn a_models_metadata_is_retained_beside_the_facts_that_name_it() {
    let run = RunDocument::create(&header()).unwrap();
    assert!(run.models().unwrap().is_empty());
    run.record_model(&identity(0x30)).unwrap();
    run.record_model(&identity(0x31)).unwrap();
    run.record_usage(&usage(1, 0, 100)).unwrap();

    let replica = RunDocument::from_snapshot(&run.export_snapshot().unwrap()).unwrap();
    let models = replica.models().unwrap();
    assert_eq!(models.len(), 2);
    let fact = replica.timeline_all()[0].usage().unwrap().clone();
    assert_eq!(models[&fact.model], identity(0x30));

    // Writing the same reference again replaces what it held, and a
    // reference written twice is one entry.
    let mut corrected = identity(0x30);
    corrected.reported = ModelReport::Named {
        name: ModelName::new("gpt-5-codex-2").unwrap(),
    };
    run.record_model(&corrected).unwrap();
    assert_eq!(run.models().unwrap()[&model(0x30)], corrected);
    assert_eq!(run.models().unwrap().len(), 2);
}

/// Unavailable is unavailable, never zero — through the document too.
#[test]
fn a_missing_count_stays_missing_through_storage() {
    let run = RunDocument::create(&header()).unwrap();
    let mut fact = usage(1, 0, 100);
    fact.output = None;
    fact.completeness = Completeness::Incomplete {
        reason: IncompleteReason::NoUsageReported,
    };
    run.record_usage(&fact).unwrap();
    let back = run.timeline_all()[0].usage().unwrap().clone();
    assert_eq!(back.output, None);
    assert_eq!(back, fact);
    let folded = run.economics(0);
    assert_eq!(folded.output.complete(), None);
    assert_eq!(folded.output.unavailable_turns, 1);
    assert!(!folded.complete);
}
