// SPDX-License-Identifier: MPL-2.0

//! Subject, correlation and retry identity (§7.1, §7.5, §17.2).

use koru_events::{AttrValue, Subject, SubjectKind};

use super::*;

/// Every row of a run is addressed at the Run document — never at the
/// `RunId`, which is an attribute — and scoped to the workspace.
#[test]
fn every_row_is_about_the_run_document() {
    let expected = Subject::document(run_ref().document.to_storage());
    for (name, envelope) in every_row() {
        if name == "delegate.scope.skipped" {
            assert_eq!(envelope.subject, None, "M1: a schedule has no document");
            continue;
        }
        if name.starts_with("delegate.schedule.") {
            continue;
        }
        let subject = envelope
            .subject
            .as_ref()
            .unwrap_or_else(|| panic!("{name}"));
        assert_eq!(subject, &expected, "{name}");
        assert_eq!(subject.kind, SubjectKind::Document);
        assert_eq!(
            DocumentId::parse(&subject.id),
            Some(run_ref().document),
            "{name}: the subject id is the Run document's, as the store spells it"
        );
        assert_eq!(
            envelope.scope.workspace_id,
            Some(koru_events::WorkspaceId(run_ref().workspace.as_uuid())),
            "{name}"
        );
    }
}

#[test]
fn every_schedule_row_is_about_the_schedule_document() {
    let expected = Subject::document(schedule_ref().document.to_storage());
    for (name, envelope) in every_row() {
        if !name.starts_with("delegate.schedule.") {
            continue;
        }
        let subject = envelope
            .subject
            .as_ref()
            .unwrap_or_else(|| panic!("{name}"));
        assert_eq!(subject, &expected, "{name}");
        assert_eq!(subject.kind, SubjectKind::Document);
        assert_eq!(
            DocumentId::parse(&subject.id),
            Some(schedule_ref().document),
            "{name}: the subject id is the Schedule document's"
        );
        assert_eq!(
            envelope.scope.workspace_id,
            Some(koru_events::WorkspaceId(schedule_ref().workspace.as_uuid())),
            "{name}"
        );
    }
}

/// `delegate.run.created` is the run chain root; every later run row joins it
/// and names no causation.
#[test]
fn the_created_row_is_the_root_and_every_other_row_joins_it() {
    let rows = every_row();
    let (root_name, root) = &rows[0];
    assert_eq!(*root_name, "delegate.run.created");
    assert_eq!(root.correlation_id, None, "the root is its own chain");
    assert_eq!(root.chain_root(), root.event_id);
    assert_eq!(
        root.event_id,
        stamp(0).event_id,
        "the root's id is the stamp's"
    );
    for (name, envelope) in &rows[1..] {
        if *name == "delegate.scope.skipped" {
            assert_eq!(
                envelope.correlation_id, None,
                "a skipped firing starts its own chain"
            );
            continue;
        }
        if name.starts_with("delegate.schedule.") {
            continue;
        }
        assert_eq!(envelope.correlation_id, Some(root.event_id), "{name}");
        assert_eq!(envelope.chain_root(), root.event_id, "{name}");
        assert_eq!(envelope.causation_id, None, "{name}");
    }
}

#[test]
fn schedule_created_is_its_chain_root() {
    let rows = every_row();
    let (_, root) = rows
        .iter()
        .find(|(name, _)| *name == "delegate.schedule.created")
        .unwrap();
    assert_eq!(root.correlation_id, None);
    assert_eq!(root.event_id, stamp(27).event_id);
    for (name, envelope) in rows.iter().filter(|(name, _)| {
        name.starts_with("delegate.schedule.") && *name != "delegate.schedule.created"
    }) {
        assert_eq!(envelope.correlation_id, Some(root.event_id), "{name}");
        assert_eq!(envelope.chain_root(), root.event_id, "{name}");
        assert_eq!(envelope.causation_id, None, "{name}");
    }
}

#[test]
fn a_rejoined_schedule_chain_builds_the_same_rows() {
    let (created, original) = ScheduleChain::created(
        &ctx(),
        schedule_ref(),
        stamp(27),
        ScheduleCreated {
            requested_by: person(),
            revision: 1,
        },
    );
    let rejoined = ScheduleChain::rejoin(&ctx(), schedule_ref(), created.event_id);
    assert_eq!(rejoined.root(), original.root());
    let fact = ScheduleFired {
        revision: 2,
        occurrence_ms: T0,
        run: scheduled_run(),
    };
    assert_eq!(
        original.fired(stamp(30), fact),
        rejoined.fired(stamp(30), fact)
    );
}

/// A restart rejoins the chain from the id `runs.db` kept, and builds rows
/// indistinguishable from the ones the creating process would have built.
#[test]
fn a_rejoined_chain_builds_the_same_rows() {
    let (created, original) = RunChain::created(&ctx(), run_ref(), stamp(0), created_fact());
    let rejoined = RunChain::rejoin(&ctx(), run_ref(), created.event_id);
    assert_eq!(rejoined.root(), original.root());
    let fact = RunStarted {
        attempt: attempt(),
        generation: AttemptGeneration::FIRST,
        session: session(),
    };
    assert_eq!(
        original.started(stamp(1), fact),
        rejoined.started(stamp(1), fact)
    );
}

/// §7.1: a re-publication from the pending list is the same envelope under
/// the same id — what the journal dedupes on.
#[test]
fn a_retry_under_the_stored_stamp_is_the_same_envelope() {
    let chain = chain();
    let usage = TurnUsage {
        fact: usage_fact(Some(100), 0),
        cached_relationship: CachedInputRelationship::Included,
    };
    let first = chain.turn_usage(stamp(60), &usage);
    let retried = chain.turn_usage(stamp(60), &usage);
    assert_eq!(first, retried);
    assert_eq!(first.event_id, stamp(60).event_id);
}

/// §17.2: a correction keeps the fact's identity, raises the revision and
/// takes a fresh `event_id`, so deduplication cannot swallow it.
#[test]
fn an_amendment_keeps_the_fact_identity_and_takes_a_fresh_event_id() {
    let chain = chain();
    let first = chain.turn_usage(
        stamp(61),
        &TurnUsage {
            fact: usage_fact(Some(100), 0),
            cached_relationship: CachedInputRelationship::Included,
        },
    );
    let amended = chain.turn_usage(
        Stamp::fresh(T0 + 5_000),
        &TurnUsage {
            fact: usage_fact(Some(140), 1),
            cached_relationship: CachedInputRelationship::Included,
        },
    );
    assert_ne!(first.event_id, amended.event_id);
    assert_eq!(first.attrs.get("usage"), amended.attrs.get("usage"));
    assert_eq!(first.attrs.get("attempt"), amended.attrs.get("attempt"));
    assert_eq!(first.attrs.get("turn"), amended.attrs.get("turn"));
    assert_eq!(first.attrs.get("revision"), Some(&AttrValue::Count(0)));
    assert_eq!(amended.attrs.get("revision"), Some(&AttrValue::Count(1)));
    assert_eq!(amended.attrs.get("input"), Some(&AttrValue::Count(140)));
    assert_eq!(amended.correlation_id, first.correlation_id);
}

/// `Stamp::fresh` mints; two fresh stamps never collide.
#[test]
fn a_fresh_stamp_is_a_new_identity() {
    assert_ne!(Stamp::fresh(T0).event_id, Stamp::fresh(T0).event_id);
}

/// Unavailable is unavailable: a missing count is an absent attribute,
/// never zero — on the turn fact and on the fold alike.
#[test]
fn an_unavailable_count_is_absent_never_zero() {
    let chain = chain();
    let turn = chain.turn_usage(
        stamp(62),
        &TurnUsage {
            fact: usage_fact(None, 0),
            cached_relationship: CachedInputRelationship::Undeclared,
        },
    );
    assert!(!turn.attrs.contains_key("input"));
    assert!(!turn.attrs.contains_key("cached_input"));
    assert_eq!(turn.attrs.get("output"), Some(&AttrValue::Count(140)));

    let fold = chain.run_usage(
        stamp(63),
        &RunUsage {
            summary: summary(),
            outcome: RunOutcome::Completed,
        },
    );
    // Three turns; input reported by two, output by three, cached by none.
    assert_eq!(fold.attrs.get("input"), Some(&AttrValue::Count(1_200)));
    assert_eq!(
        fold.attrs.get("input_unavailable_turns"),
        Some(&AttrValue::Count(1))
    );
    assert_eq!(fold.attrs.get("output"), Some(&AttrValue::Count(400)));
    assert!(
        !fold.attrs.contains_key("cached_input"),
        "no turn reported it, so it is not zero"
    );
    assert_eq!(
        fold.attrs.get("cached_input_unavailable_turns"),
        Some(&AttrValue::Count(3))
    );
    assert_eq!(fold.attrs.get("elapsed_ms"), Some(&AttrValue::Int(65_000)));
    assert_eq!(fold.attrs.get("models"), Some(&AttrValue::Count(1)));
    assert_eq!(fold.attrs.get("complete"), Some(&AttrValue::Flag(false)));
}

/// A run that never started has no elapsed time — absent, not zero.
#[test]
fn a_run_that_never_started_has_no_elapsed_time() {
    let fold = chain().run_usage(
        stamp(64),
        &RunUsage {
            summary: EconomicsSummary::default(),
            outcome: RunOutcome::Cancelled,
        },
    );
    assert!(!fold.attrs.contains_key("elapsed_ms"));
    assert!(!fold.attrs.contains_key("input"));
}

/// Two runs in one workspace are two subjects: nothing but the workspace is
/// shared, so a per-subject read cannot answer one with the other.
#[test]
fn two_runs_in_one_workspace_are_two_subjects() {
    let first = chain();
    let (_, second) = RunChain::created(&ctx(), other_run_ref(), stamp(70), created_fact());
    let fact = ControlReturned { attempt: attempt() };
    let a = first.control_returned(stamp(71), fact);
    let b = second.control_returned(stamp(72), fact);
    assert_ne!(a.subject, b.subject);
    assert_ne!(a.correlation_id, b.correlation_id);
    assert_ne!(a.attrs.get("run"), b.attrs.get("run"));
    assert_eq!(a.scope.workspace_id, b.scope.workspace_id);
}
