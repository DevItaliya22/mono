// SPDX-License-Identifier: MPL-2.0

//! What the corpus promises: the shape rev6 §6 names, and the same corpus
//! twice.

use super::*;
use koru_core::time::DateOnly;

#[test]
fn the_roadmap_corpus_is_the_shape_the_measurements_name() {
    let corpus = TaskCorpus::roadmap().plan();

    assert_eq!(corpus.workspaces.len(), 3);
    assert_eq!(corpus.cold().count(), 1, "one workspace is left cold");
    assert_eq!(corpus.warm().count(), 2);
    assert_eq!(
        corpus
            .workspaces
            .iter()
            .map(|w| w.documents.len())
            .sum::<usize>(),
        250
    );
    assert_eq!(corpus.tasks().count(), 5_000);
    assert_eq!(corpus.other_rows, 50_000);

    // Every workspace holds documents, so "cold" is a workspace with content
    // nobody has swept rather than an empty one — the two converge differently
    // and only one of them is the case worth testing.
    for workspace in &corpus.workspaces {
        assert!(!workspace.documents.is_empty(), "{}", workspace.name);
        assert!(
            workspace.documents.iter().any(|d| !d.tasks.is_empty()),
            "{} holds tasks",
            workspace.name
        );
    }
}

/// **The identity is the seed and nothing else.** Two builds are the same
/// corpus down to the reference strings — which is what lets a page be walked
/// twice, or a measurement compared with last week's.
#[test]
fn the_same_seed_builds_the_same_corpus() {
    let one = TaskCorpus::small().plan();
    let two = TaskCorpus::small().plan();
    let refs = |c: &Corpus| -> Vec<String> { c.tasks().map(|t| t.reference.clone()).collect() };
    assert_eq!(refs(&one), refs(&two));
    assert_eq!(
        one.tasks().map(|t| t.due.clone()).collect::<Vec<_>>(),
        two.tasks().map(|t| t.due.clone()).collect::<Vec<_>>()
    );

    // …and a different seed is a different corpus, or the seed is decoration.
    let other = TaskCorpus::small().with_seed(SEED ^ 0xFFFF).plan();
    assert_ne!(refs(&one), refs(&other));
}

/// The generated identities are the shapes the rest of the system parses: a
/// task reference parses back to the same triple, and the ids are UUIDs.
#[test]
fn the_generated_identities_parse_everywhere_a_real_one_does() {
    let corpus = TaskCorpus::small().plan();
    for task in corpus.tasks() {
        let parsed =
            koru_core::ids::EntityRef::parse(&task.reference).expect("the reference parses");
        assert!(matches!(parsed, koru_core::ids::EntityRef::Task { .. }));
        assert_eq!(
            parsed.workspace_id().to_storage(),
            task.workspace.to_storage()
        );
        assert_eq!(
            parsed.document_id().to_storage(),
            task.document.to_storage()
        );
        if let Some(due) = &task.due {
            assert!(DateOnly::parse(due).is_some(), "{due} is a date");
        }
        assert!(
            koru_core::tasks::TASK_CAPABLE_KINDS
                .iter()
                .any(|k| k.as_str() == task.kind),
            "{} is task capable",
            task.kind
        );
    }
}

/// The corpus is varied enough to be worth querying: every dimension a filter
/// names has both answers in it.
#[test]
fn every_dimension_a_filter_names_has_both_answers_in_the_corpus() {
    let corpus = TaskCorpus::roadmap().plan();
    let any = |f: fn(&PlannedTask) -> bool| corpus.tasks().any(f);
    for (what, present, absent) in [
        (
            "a due date",
            any(|t| t.due.is_some()),
            any(|t| t.due.is_none()),
        ),
        (
            "a marker",
            any(|t| t.marker.is_some()),
            any(|t| t.marker.is_none() && t.due.is_some()),
        ),
        (
            "a priority",
            any(|t| t.priority == Some(Priority::Urgent)),
            any(|t| t.priority.is_none()),
        ),
        (
            "a list",
            any(|t| t.list.is_some()),
            any(|t| t.list.is_none()),
        ),
        ("completion", any(|t| t.completed), any(|t| !t.completed)),
        ("the node bin", any(|t| t.in_trash), any(|t| !t.in_trash)),
        (
            "a binned document",
            any(|t| t.document_trashed),
            any(|t| !t.document_trashed),
        ),
    ] {
        assert!(present && absent, "the corpus has both answers for {what}");
    }
}

/// Writing is the plan and nothing else: what went in is what the read model
/// holds, and the cold workspace holds nothing until it is swept.
#[test]
fn what_is_written_is_the_warm_half_and_a_sweep_brings_the_rest() {
    let mut p = Projections::in_memory().unwrap();
    let corpus = TaskCorpus::small().plan();
    corpus.write_into(&mut p, 1).unwrap();

    let warm_documents: usize = corpus.warm().map(|w| w.documents.len()).sum();
    assert_eq!(p.documents().unwrap(), warm_documents);
    for workspace in corpus.warm() {
        assert!(p.listing_digest(workspace.id).unwrap().is_some());
    }
    let cold = corpus.cold().next().expect("one cold workspace").id;
    assert!(
        p.listing_digest(cold).unwrap().is_none(),
        "a cold workspace is what a port names in updating_workspaces"
    );

    corpus.sweep_in(&mut p, cold, 2).unwrap();
    assert_eq!(
        p.documents().unwrap(),
        corpus
            .workspaces
            .iter()
            .map(|w| w.documents.len())
            .sum::<usize>()
    );
    assert!(p.listing_digest(cold).unwrap().is_some());
}
