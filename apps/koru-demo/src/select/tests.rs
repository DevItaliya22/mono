// SPDX-License-Identifier: MPL-2.0

//! **These prove the rule, and nothing about a live tree.**
//!
//! Every element here is a struct somebody typed. That a real provider ever
//! returns two matches for one locator, that a disabled control really reports
//! `CurrentIsEnabled() == false`, that an off-screen paragraph really sets
//! `IsOffscreen` — none of that is established here. It is established by the
//! interactive proof, against a real projection.

use super::*;
use crate::inspect::Handle;

fn facts(slot: u64, locator: &str) -> ElementFacts {
    ElementFacts {
        handle: Handle {
            generation: 1,
            slot,
        },
        locator: Some(locator.to_owned()),
        role: Role::Button,
        enabled: true,
        offscreen: false,
        bounds: Some(Rect {
            left: 10,
            top: 20,
            right: 110,
            bottom: 60,
        }),
        process_id: Some(4242),
    }
}

#[test]
fn one_match_in_the_right_process_is_the_only_success() {
    let selection = select(
        vec![facts(1, "koru:w1/v0/s:window#close")],
        Some(4242),
        Requirements::ACTIONABLE,
    );
    assert_eq!(
        selection.found().map(|f| f.handle.slot),
        Some(1),
        "{selection}"
    );
}

#[test]
fn two_matches_fail_and_name_every_candidate() {
    // The case this exists for: one document open in two tabs. Both occurrences
    // are real, both are correct, and a runner that picked one would be acting
    // on whichever the provider walked to first.
    let selection = select(
        vec![
            facts(1, "koru:w1/v1/n:6f9619ff8b86d011b42d00cf4fc964ff#text"),
            facts(2, "koru:w1/v2/n:6f9619ff8b86d011b42d00cf4fc964ff#text"),
        ],
        Some(4242),
        Requirements::INSPECT,
    );
    let Selection::Ambiguous(candidates) = &selection else {
        panic!("two matches must not resolve to one: {selection}");
    };
    assert_eq!(candidates.len(), 2);

    let printed = selection.to_string();
    for wanted in ["koru:w1/v1/n:", "koru:w1/v2/n:", "will not choose for you"] {
        assert!(
            printed.contains(wanted),
            "the failure must say why: {printed}"
        );
    }
}

#[test]
fn nothing_a_candidate_prints_could_be_a_document_title() {
    // The structural half of §7.4: there is no name to print, because
    // `ElementFacts` has no name field. This test exists so that adding one
    // later is a test failure rather than a quiet privacy regression.
    let candidate = Candidate::from(&facts(1, "koru:w1/v1/s:body#root"));
    let printed = candidate.to_string();
    assert!(printed.contains("koru:w1/v1/s:body#root"));
    assert!(printed.contains("Button"));
    assert!(printed.contains("at 10,20 100x40"));
    assert!(printed.contains("in pid 4242"));
    // Everything printed came out of the locator, the control type, the state,
    // the rectangle and the pid. Nothing else is reachable from here.
    assert_eq!(
        printed,
        "koru:w1/v1/s:body#root Button at 10,20 100x40 in pid 4242"
    );
}

#[test]
fn a_disabled_control_is_not_the_same_answer_as_a_missing_one() {
    let mut disabled = facts(1, "koru:w1/v1/s:save#retry");
    disabled.enabled = false;

    // Inspecting it is fine — a person asking what the tree holds wants to know
    // the control is there and greyed.
    assert!(
        select(vec![disabled.clone()], Some(4242), Requirements::INSPECT)
            .found()
            .is_some()
    );

    let selection = select(vec![disabled], Some(4242), Requirements::ACTIONABLE);
    assert!(
        matches!(
            selection,
            Selection::Unusable {
                why: Unusable::Disabled,
                ..
            }
        ),
        "{selection}"
    );
    assert!(selection.to_string().contains("it is disabled"));
}

#[test]
fn an_off_screen_control_is_discoverable_and_is_not_actionable() {
    // Both halves matter. The roadmap requires an off-screen paragraph to be
    // *discoverable* — a screen reader reaches it and scrolling reveals it — and
    // a click derived from its rectangle would go to a place nobody can see.
    let mut offscreen = facts(1, "koru:w1/v1/n:6f9619ff8b86d011b42d00cf4fc964ff#text");
    offscreen.offscreen = true;

    assert!(
        select(vec![offscreen.clone()], Some(4242), Requirements::INSPECT)
            .found()
            .is_some(),
        "an off-screen paragraph is still a control"
    );
    assert!(matches!(
        select(vec![offscreen], Some(4242), Requirements::ACTIONABLE),
        Selection::Unusable {
            why: Unusable::OffScreen,
            ..
        }
    ));
}

#[test]
fn a_control_with_no_rectangle_is_refused_rather_than_clicked_at_the_origin() {
    for bounds in [
        None,
        Some(Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        }),
    ] {
        let mut placeless = facts(1, "koru:w1/v1/s:body#root");
        placeless.bounds = bounds;
        assert!(
            matches!(
                select(vec![placeless], Some(4242), Requirements::ACTIONABLE),
                Selection::Unusable {
                    why: Unusable::NoBounds,
                    ..
                }
            ),
            "{bounds:?} is not somewhere to act"
        );
    }
}

#[test]
fn a_match_in_another_koru_is_refused_by_name_rather_than_reported_as_missing() {
    // §7.5, caught at the moment it would have happened: the runner is pointed
    // at one instance, and a second Koru on the same desktop answers the same
    // locator because window slots restart at 1 in every process.
    let mut other = facts(1, "koru:w1/v0/s:window#close");
    other.process_id = Some(7777);

    let selection = select(vec![other], Some(4242), Requirements::INSPECT);
    let Selection::WrongProcess { wanted, candidates } = &selection else {
        panic!("a second instance must not be reported as absence: {selection}");
    };
    assert_eq!(*wanted, 4242);
    assert_eq!(candidates[0].process_id, Some(7777));
    assert!(selection.to_string().contains("check --pid"));
}

#[test]
fn the_instance_filter_runs_before_the_count() {
    // One match here and one in another process is *not* an ambiguity: there is
    // exactly one candidate in the instance this run is pointed at, and
    // reporting "two matches, pick one" would send somebody looking for a
    // projection defect that does not exist.
    let mut other = facts(2, "koru:w1/v0/s:window#close");
    other.process_id = Some(7777);

    let selection = select(
        vec![facts(1, "koru:w1/v0/s:window#close"), other],
        Some(4242),
        Requirements::INSPECT,
    );
    assert_eq!(selection.found().map(|f| f.handle.slot), Some(1));
}

#[test]
fn no_match_at_all_is_its_own_answer() {
    assert_eq!(
        select(Vec::new(), Some(4242), Requirements::INSPECT),
        Selection::NotFound
    );
}
