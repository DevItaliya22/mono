// SPDX-License-Identifier: MPL-2.0

//! The timeline: lean entries appended one commit at a time, read back by
//! index in windows, with an entry this build cannot read taking its place
//! rather than disappearing.

use super::*;

#[test]
fn a_window_is_read_by_index_newest_first() {
    let run = a_run_with(120);
    assert_eq!(run.timeline_len(), 120);

    let newest = run.timeline_window(0, 50);
    assert_eq!(newest.len(), 50);
    assert_eq!(newest[0].step().unwrap().at_ms, 1_000 + 119);
    assert_eq!(newest[49].step().unwrap().at_ms, 1_000 + 70);

    let older = run.timeline_window(50, 50);
    assert_eq!(older[0].step().unwrap().at_ms, 1_000 + 69);
    assert_eq!(older[49].step().unwrap().at_ms, 1_000 + 20);

    let oldest = run.timeline_window(100, 50);
    assert_eq!(oldest.len(), 20, "the last window is what is left");
    assert_eq!(oldest[19].step().unwrap().at_ms, 1_000);

    assert!(run.timeline_window(120, 50).is_empty());
    assert!(run.timeline_window(500, 50).is_empty());
}

/// Steps and usage facts share the list, in the order they were appended,
/// and each comes back as what it is.
#[test]
fn steps_and_usage_facts_share_the_list_in_append_order() {
    let run = RunDocument::create(&header()).unwrap();
    run.append_step(&step(StepKind::Started, 1)).unwrap();
    run.record_usage(&usage(1, 0, 100)).unwrap();
    run.append_step(&step(StepKind::Completed, 3)).unwrap();

    let all = run.timeline_all();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].step().unwrap().kind, StepKind::Started);
    assert_eq!(all[1].usage().unwrap(), &usage(1, 0, 100));
    assert_eq!(all[2].step().unwrap().kind, StepKind::Completed);

    let window = run.timeline_window(0, 3);
    assert_eq!(window[0].step().unwrap().kind, StepKind::Completed);
    assert_eq!(window[1].usage().unwrap(), &usage(1, 0, 100));
    assert_eq!(window[2].step().unwrap().kind, StepKind::Started);
}

/// An entry a newer build wrote — a step kind this build has no variant
/// for, or an envelope it has never seen — is kept in the document and
/// shown at its position as unrecognized. A window never has fewer items
/// than the list has entries, and nothing is rewritten by being read.
#[test]
fn an_entry_from_a_newer_build_keeps_its_place_as_unrecognized() {
    let run = RunDocument::create(&header()).unwrap();
    run.append_step(&step(StepKind::Started, 1)).unwrap();
    let list = run.doc().get_list("timeline");
    let newer_step: std::collections::HashMap<String, loro::LoroValue> = [
        ("entry".to_owned(), loro::LoroValue::from("step")),
        (
            "id".to_owned(),
            loro::LoroValue::from(v4(7).hyphenated().to_string()),
        ),
        (
            "kind".to_owned(),
            loro::LoroValue::from(
                [("step".to_owned(), loro::LoroValue::from("a_step_from_m5"))]
                    .into_iter()
                    .collect::<std::collections::HashMap<_, _>>(),
            ),
        ),
        ("at_ms".to_owned(), loro::LoroValue::from(2_i64)),
    ]
    .into_iter()
    .collect();
    list.push(loro::LoroValue::from(newer_step)).unwrap();
    let newer_envelope: std::collections::HashMap<String, loro::LoroValue> = [(
        "entry".to_owned(),
        loro::LoroValue::from("an_envelope_from_m5"),
    )]
    .into_iter()
    .collect();
    list.push(loro::LoroValue::from(newer_envelope)).unwrap();
    run.doc().commit();
    run.append_step(&step(StepKind::Completed, 4)).unwrap();

    let before = run.export_snapshot().unwrap();
    let window = run.timeline_window(0, 10);
    assert_eq!(window.len(), 4);
    assert_eq!(window[0].step().unwrap().kind, StepKind::Completed);
    assert_eq!(window[1], TimelineItem::Unrecognized);
    assert_eq!(window[2], TimelineItem::Unrecognized);
    assert_eq!(window[3].step().unwrap().kind, StepKind::Started);
    assert_eq!(run.economics(10).orchestrator_turns, 0);
    assert_eq!(
        run.export_snapshot().unwrap(),
        before,
        "reading rewrote nothing"
    );
}

/// Each step is its own commit, so the delta after one step is one step:
/// what the executing device sends after each, and what a replica imports.
#[test]
fn each_step_is_one_commit_and_one_delta() {
    let executing = a_run_with(10);
    let replica = RunDocument::from_snapshot(&executing.export_snapshot().unwrap()).unwrap();
    for i in 10..15 {
        let before = executing.version();
        executing
            .append_step(&step(
                StepKind::TurnStarted {
                    originator: TurnOriginator::Orchestrator,
                },
                i,
            ))
            .unwrap();
        let delta = executing.export_updates_since(&before).unwrap();
        replica.import(&delta).unwrap();
        assert_eq!(replica.timeline_len(), i as usize + 1);
    }
    assert_eq!(replica.timeline_all(), executing.timeline_all());
}

/// The refs a step carries come back as they went in: what a step touched,
/// by id, and never what it said.
#[test]
fn a_steps_references_round_trip() {
    let run = RunDocument::create(&header()).unwrap();
    let refs = TimelineRefs::new([
        TimelineRef::Turn { turn: turn(2) },
        TimelineRef::Document {
            document: DocumentId::from_uuid(v4(5)),
            version: DocumentVersionId::from_uuid(v4(6)),
        },
        TimelineRef::Model { model: model(0x30) },
    ])
    .unwrap();
    let entry = TimelineEntry::new(
        StepKind::TurnCompleted {
            originator: TurnOriginator::Orchestrator,
            artifacts_registered: 1,
        },
        refs,
        7,
    );
    run.append_step(&entry).unwrap();
    assert_eq!(run.timeline_window(0, 1)[0].step().unwrap(), &entry);
}
