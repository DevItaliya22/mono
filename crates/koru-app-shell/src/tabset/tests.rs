// SPDX-License-Identifier: MPL-2.0

//! The rule this type exists to hold, and the behaviour built on it.
//!
//! Nothing here needs a viewport: `TabSet` owns the list, not the geometry, so
//! every operation is testable without a window. Nothing here needs a host
//! either — the element is [`TestTab`], the crate's own double, because a
//! dependency is never compiled with `cfg(test)` and a Koru `Tab` could not
//! reach here even if the dependency direction allowed it.
//!
//! The rule used to be two: the active index stays valid, **and** the permanent
//! tab is never removed. There is no permanent tab any more, so what is left is
//! the first half, plus the floor that replaced the second: the list is never
//! empty, and closing the last tab leaves the host's floor tab in its place.

use super::*;
use crate::tests::TestTab;

/// A set of `n` floor tabs, showing the first.
fn tabs(n: usize) -> TabSet<TestTab> {
    let mut set = TabSet::new(TestTab::floor(), TestTab::floor);
    for _ in 1..n.max(1) {
        set.push(TestTab::floor());
    }
    set.select(0);
    set
}

/// Identifying tabs by label, since `close` is a `Vec` remove and every element
/// after the gap moves in memory. See `moving_another_tab_keeps_the_same_one_showing`.
fn named(labels: &[&str]) -> TabSet<TestTab> {
    let tabs: Vec<TestTab> = labels.iter().map(|name| TestTab::named(name)).collect();
    let mut set = TabSet::new(TestTab::floor(), TestTab::floor);
    set.restore(tabs, 0);
    set
}

/// One way the list can change, and what to call it when it breaks the rule.
type Mutation = (&'static str, fn(&mut TabSet<TestTab>));

/// Every way the list can change. The invariant test runs all of them.
fn mutations() -> Vec<Mutation> {
    vec![
        ("select(0)", |s| {
            s.select(0);
        }),
        ("select(past the end)", |s| {
            s.select(99);
        }),
        ("cycle(+1)", |s| {
            s.cycle(1);
        }),
        ("cycle(-1)", |s| {
            s.cycle(-1);
        }),
        ("push", |s| {
            s.push(TestTab::floor());
        }),
        ("close(0)", |s| {
            s.close(0);
        }),
        ("close(last)", |s| {
            s.close(s.len() - 1);
        }),
        ("close(past the end)", |s| {
            s.close(99);
        }),
        ("close every tab", |s| {
            for i in (0..s.len()).rev() {
                s.close(i);
            }
        }),
        ("close_others(0)", |s| {
            s.close_others(0);
        }),
        ("close_others(active)", |s| {
            s.close_others(s.active());
        }),
        ("close_others(last)", |s| {
            s.close_others(s.len() - 1);
        }),
        ("close_left(active)", |s| {
            s.close_left(s.active());
        }),
        ("close_left(last)", |s| {
            s.close_left(s.len() - 1);
        }),
        ("close_right(0)", |s| {
            s.close_right(0);
        }),
        ("close_right(active)", |s| {
            s.close_right(s.active());
        }),
        ("move first to last", |s| {
            s.move_tab(0, s.len() - 1);
        }),
        ("move last to first", |s| {
            s.move_tab(s.len() - 1, 0);
        }),
        ("retain nothing", |s| {
            s.retain(|_| false);
        }),
        ("reset to the floor", |s| {
            s.reset_to_floor();
        }),
        ("restore over it", |s| {
            s.restore(vec![TestTab::floor(), TestTab::floor()], 7);
        }),
        ("restore with nothing", |s| {
            s.restore(Vec::new(), 3);
        }),
    ]
}

/// **The reason this is a type.** `active` is an index into `tabs`, and every
/// operation that removes or reorders one has to keep it valid. It used to be
/// five hand-written clamps in five methods; this asserts the property itself,
/// over every mutation and from every starting selection.
#[test]
fn the_active_index_is_always_valid() {
    for (name, apply) in mutations() {
        for size in 1..=5 {
            for start in 0..size {
                let mut s = tabs(size);
                s.select(start);
                apply(&mut s);

                assert!(
                    !s.is_empty(),
                    "{name} (n={size}, active={start}) emptied the set"
                );
                assert!(
                    s.active() < s.len(),
                    "{name} (n={size}, active={start}) left active={} of {}",
                    s.active(),
                    s.len()
                );
                // And the accessors that index by it cannot panic.
                let _ = s.active_tab();
                // The visit trail is parallel to the list, whatever moved.
                assert_eq!(
                    s.visits_len(),
                    s.len(),
                    "{name} (n={size}, active={start}) desynced the visit trail"
                );
            }
        }
    }
}

/// A set built with [`TabSet::empty`] has no floor and is **allowed** to empty:
/// that is the pinned group, which legitimately holds nothing.
#[test]
fn a_floorless_set_is_allowed_to_empty() {
    let mut s: TabSet<TestTab> = TabSet::empty();
    s.push(TestTab::content("a"));
    assert!(s.close(0));
    assert!(s.is_empty(), "no factory, so nothing was minted");
    assert_eq!(s.active(), 0);
}

// ------------------------------------------------------------------ selecting

#[test]
fn selecting_reports_only_a_real_change() {
    let mut s = tabs(3);
    assert!(!s.select(0), "already showing");
    assert!(s.select(1));
    assert_eq!(s.active(), 1);
    assert!(!s.select(9), "there is no ninth tab");
    assert_eq!(s.active(), 1, "and the selection did not move");
}

#[test]
fn cycling_wraps_in_both_directions() {
    let mut s = tabs(2);
    assert_eq!(s.len(), 2);
    assert!(s.cycle(1));
    assert_eq!(s.active(), 1);
    assert!(s.cycle(1));
    assert_eq!(s.active(), 0, "wraps forward");
    assert!(s.cycle(-1));
    assert_eq!(s.active(), 1, "and backward");
}

#[test]
fn a_lone_tab_has_nowhere_to_cycle_to() {
    let mut s = tabs(1);
    assert!(!s.cycle(1));
    assert!(!s.cycle(-1));
}

// -------------------------------------------------------------------- closing

/// **The floor.** Every tab can be closed, including the last one — and what is
/// left is a fresh tab from the host's factory, not an empty window. Every
/// method here is written against "there is always an active tab".
#[test]
fn closing_the_last_tab_mints_the_hosts_floor() {
    let mut s = tabs(1);
    s.restore(vec![TestTab::named("Notes")], 0);

    assert!(s.close(0), "the last tab closes");
    assert_eq!(s.len(), 1);
    assert_eq!(s.active(), 0);
    assert_eq!(
        s.active_tab().label,
        "floor",
        "and the factory's tab replaced it"
    );
}

/// Closing them one at a time, from anywhere, always lands on the same floor.
#[test]
fn closing_every_tab_in_turn_leaves_one_floor_tab() {
    for n in 1..=5 {
        let mut s = tabs(n);
        for i in (0..s.len()).rev() {
            s.close(i);
        }
        assert_eq!(s.len(), 1, "n={n}");
        assert_eq!(s.active_tab().label, "floor", "n={n}");
    }
}

/// Closing the tab you are looking at goes back along the trail of what was
/// shown, not to the nearest index. Open a, b, c; revisit a; return to c and
/// close it — you were in a before c, so a is where you land, not b.
#[test]
fn closing_the_active_tab_returns_along_the_trail() {
    let mut s = named(&["a", "b", "c"]);
    s.select(1); // b
    s.select(2); // c
    s.select(0); // a — write something…
    s.select(2); // …then back to c
    assert!(s.close(2));
    assert_eq!(
        s.active_tab().label,
        "a",
        "back to where you were before c, not the neighbour"
    );
}

/// With no trail — a freshly restored session where nothing but the closed
/// tab was ever shown — the selection falls to the neighbour on the left.
#[test]
fn closing_without_a_trail_focuses_the_neighbour() {
    let mut s = named(&["a", "b", "c"]);
    assert!(s.close(0), "the only tab ever shown");
    assert_eq!(s.active_tab().label, "b", "no trail, so the neighbour");
}

/// Closing something *after* the active tab must not move the selection.
#[test]
fn closing_a_later_tab_leaves_the_selection_alone() {
    let mut s = tabs(4);
    s.select(1);
    assert!(s.close(3));
    assert_eq!(s.active(), 1);
}

#[test]
fn insert_after_active_does_not_change_which_tab_is_showing() {
    let mut s = tabs(2);
    s.select(0);
    let i = s.insert(1, TestTab::floor());
    assert_eq!(i, 1);
    assert_eq!(s.len(), 3);
    assert_eq!(s.active(), 0);
}

#[test]
fn closing_past_the_end_is_a_noop() {
    let mut s = tabs(3);
    let before = (s.len(), s.active());
    assert!(!s.close(99));
    assert_eq!((s.len(), s.active()), before);
}

// --------------------------------------------------------------- bulk closing
//
// The three context-menu operations. Each keeps `i`, and resolves the selection
// by one rule: stay put if the active tab survives, otherwise land on `i` — the
// right-clicked tab.

/// "Close others" leaves only the one you pointed at.
#[test]
fn close_others_keeps_only_i() {
    let mut s = named(&["a", "b", "c"]);
    assert!(s.close_others(1)); // "b"
    assert_eq!(s.len(), 1);
    assert_eq!(s.active_tab().label, "b");
}

/// A non-surviving active tab hands the selection to the right-clicked one.
#[test]
fn close_others_lands_on_i_when_active_is_closed() {
    let mut s = named(&["a", "b", "c"]);
    s.select(0); // "a" — which close_others(1) removes
    assert!(s.close_others(1));
    assert_eq!(s.active_tab().label, "b");
}

/// "Close to the left" removes only tabs before `i`.
#[test]
fn close_left_removes_only_those_before_i() {
    let mut s = named(&["a", "b", "c", "d"]);
    s.select(1); // "b" — to the left of i=2, so it gets removed
    assert!(s.close_left(2)); // "c"
    assert_eq!(
        s.active_tab().label,
        "c",
        "active was removed, so land on i"
    );
    let labels: Vec<_> = s.iter().map(|t| t.label.as_str()).collect();
    assert_eq!(labels, &["c", "d"]);
}

/// A surviving active tab keeps its place when left tabs close.
#[test]
fn close_left_keeps_active_when_it_survives() {
    let mut s = named(&["a", "b", "c", "d"]);
    s.select(3); // "d" — to the right of i=1, survives
    assert!(s.close_left(1)); // "b"
    assert_eq!(s.active_tab().label, "d");
    let labels: Vec<_> = s.iter().map(|t| t.label.as_str()).collect();
    assert_eq!(labels, &["b", "c", "d"]);
}

/// "Close to the right" removes only tabs after `i`.
#[test]
fn close_right_removes_only_those_after_i() {
    let mut s = named(&["a", "b", "c", "d"]);
    s.select(3); // "d" — to the right of i=0, removed
    assert!(s.close_right(0)); // "a"
    assert_eq!(
        s.active_tab().label,
        "a",
        "active was removed, so land on i"
    );
    let labels: Vec<_> = s.iter().map(|t| t.label.as_str()).collect();
    assert_eq!(labels, &["a"]);
}

/// A surviving active tab keeps its place when right tabs close.
#[test]
fn close_right_keeps_active_when_it_survives() {
    let mut s = named(&["a", "b", "c", "d"]);
    s.select(0); // "a" — to the left of i=1, survives
    assert!(s.close_right(1)); // "b"
    assert_eq!(s.active_tab().label, "a");
    let labels: Vec<_> = s.iter().map(|t| t.label.as_str()).collect();
    assert_eq!(labels, &["a", "b"]);
}

/// Nothing to remove reports false and changes nothing.
#[test]
fn a_bulk_close_with_nothing_to_do_reports_false() {
    let mut s = named(&["a"]);
    let before = (s.len(), s.active());
    assert!(!s.close_left(0), "nothing left of i");
    assert!(!s.close_right(0), "nothing right of i");
    assert!(!s.close_others(0), "i is the only tab");
    assert_eq!((s.len(), s.active()), before);
}

/// A past-the-end index is a no-op, the way `close(99)` is.
#[test]
fn a_bulk_close_past_the_end_is_a_noop() {
    let mut s = tabs(3);
    assert!(!s.close_others(99));
    assert!(!s.close_left(99));
    assert!(!s.close_right(99));
}

/// Whichever bulk close runs, `i` survives — which is what makes it impossible
/// for any of them to empty the list.
#[test]
fn every_bulk_close_keeps_the_tab_it_was_asked_about() {
    type Bulk = fn(&mut TabSet<TestTab>, usize) -> bool;
    for i in 0..4 {
        for (name, apply) in [
            ("others", TabSet::close_others as Bulk),
            ("left", TabSet::close_left),
            ("right", TabSet::close_right),
        ] {
            let mut s = named(&["a", "b", "c", "d"]);
            let target = s.get(i).map(|t| t.label.clone()).unwrap();
            apply(&mut s, i);
            assert!(
                s.iter().any(|t| t.label == target),
                "close_{name}({i}) removed the tab it was asked about"
            );
            assert!(!s.is_empty());
        }
    }
}

// ------------------------------------------------------------------- reorder

/// Dragging the tab you are looking at must not switch you to a different one.
#[test]
fn moving_the_active_tab_carries_the_selection() {
    let mut s = tabs(4);
    s.select(1);
    assert!(s.move_tab(1, 3));
    assert_eq!(s.active(), 3, "still the same tab");
}

/// …and dragging a *different* tab past it must not switch either.
///
/// Identified by label rather than by address: `move_tab` is a `Vec` remove and
/// insert, so every element after the smaller index moves in memory and a
/// pointer comparison would fail even when the right tab is showing.
#[test]
fn moving_another_tab_keeps_the_same_one_showing() {
    let mut s = named(&["a", "b", "c"]);
    s.select(1); // "b"
    assert_eq!(s.active_tab().label, "b");

    assert!(s.move_tab(0, 2), "the first tab dragged to the end");
    assert_eq!(s.active_tab().label, "b", "a different tab is showing now");
}

#[test]
fn a_move_that_changes_nothing_reports_nothing() {
    let mut s = tabs(3);
    assert!(!s.move_tab(1, 1));
    assert!(!s.move_tab(0, 9));
    assert!(!s.move_tab(9, 0));
}

// -------------------------------------------------------------------- retain

/// The host's predicate decides; the walk is here — and it walks back to front,
/// so an earlier removal cannot shift a later index out from under the loop.
#[test]
fn retain_drops_what_the_host_rejects_and_keeps_the_rest() {
    let mut s = named(&["a", "b", "c", "d"]);
    assert!(s.retain(|tab| tab.label != "b" && tab.label != "c"));
    let labels: Vec<_> = s.iter().map(|t| t.label.as_str()).collect();
    assert_eq!(labels, &["a", "d"]);
}

#[test]
fn retain_that_rejects_nothing_reports_nothing() {
    let mut s = named(&["a", "b"]);
    assert!(!s.retain(|_| true));
    assert_eq!(s.len(), 2);
}

/// …and when the last survivor goes, the floor still holds.
#[test]
fn retaining_nothing_leaves_the_floor() {
    let mut s = named(&["a"]);
    assert!(s.retain(|_| false));
    assert_eq!(s.len(), 1);
    assert_eq!(s.active_tab().label, "floor");
}

// -------------------------------------------------------------------- restore

#[test]
fn restoring_replaces_the_list_and_clamps_the_selection() {
    let mut s = tabs(1);
    s.restore(vec![TestTab::floor(), TestTab::floor()], 1);
    assert_eq!(s.len(), 2);
    assert_eq!(s.active(), 1);
}

/// Everything restore is handed came off disk, so none of it is trusted.
#[test]
fn restoring_clamps_what_it_is_given() {
    let mut s = tabs(1);
    s.restore(vec![TestTab::floor()], 99);
    assert_eq!(s.len(), 1);
    assert_eq!(s.active(), 0, "clamped to the end");
}

/// A settings file with nothing usable in it must still leave a window that can
/// draw something and take a keystroke.
#[test]
fn restoring_nothing_still_leaves_a_floor_tab() {
    let mut s = tabs(3);
    s.restore(Vec::new(), 0);
    assert_eq!(s.len(), 1);
    assert_eq!(s.active_tab().label, "floor");
}

/// What a **workspace switch** leaves: every tab named something in the
/// document that has just closed, so none of them can survive.
#[test]
fn resetting_leaves_exactly_one_floor_tab() {
    let mut s = named(&["a", "b", "c"]);
    s.select(2);
    s.reset_to_floor();
    assert_eq!(s.len(), 1);
    assert_eq!(s.active(), 0);
    assert_eq!(s.active_tab().label, "floor");
}

// -------------------------------------------------------------------- lookup

/// The search is the shell's; the question is the host's.
#[test]
fn position_finds_the_first_tab_the_host_accepts() {
    let s = named(&["a", "b", "c"]);
    assert_eq!(s.position(|tab| tab.label == "b"), Some(1));
    assert_eq!(s.position(|tab| tab.label == "z"), None);
}
