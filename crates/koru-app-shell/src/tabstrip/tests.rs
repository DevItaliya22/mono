// SPDX-License-Identifier: MPL-2.0

//! Two groups, one flat index, and the four questions the strip asks its tabs.
//!
//! The element is [`TestTab`], the crate's own double: what "pinnable" means is
//! a host rule, so the strip is tested against a tab that simply answers, and
//! Koru's real rules are tested against the real `Tab` in
//! `apps/koru/src/mainview/tab_rules.rs`.

use super::*;
use crate::tests::TestTab;

fn strip_n(n: usize) -> TabStrip<TestTab> {
    let mut s = TabStrip::new(TestTab::floor(), TestTab::floor);
    for _ in 1..n.max(1) {
        s.push(TestTab::floor());
    }
    s.select(0);
    s
}

fn strip() -> TabStrip<TestTab> {
    TabStrip::new(TestTab::floor(), TestTab::floor)
}

#[test]
fn insert_inactive_from_a_pin_lands_in_main_and_keeps_the_pin_showing() {
    let home = Uuid::new_v4();
    let mut s = strip();
    s.push(TestTab::content("Notes"));
    assert!(s.pin(1, home, false));
    s.select(0);
    assert!(s.get(0).unwrap().is_pinned());
    assert!(s.has_placeholder(), "pinning leaves Main's floor");

    let i = s.insert_inactive(TestTab::content("Brief"));

    // The leftover Welcome is only a floor once a pin is holding the strip up,
    // so it is replaced rather than joined — the same rule `push` uses.
    assert_eq!(s.len(), 2);
    assert_eq!(s.pinned_count(), 1, "the pin group did not grow");
    assert_eq!(s.main_count(), 1);
    assert!(!s.has_placeholder());
    assert!(s.get(0).unwrap().is_pinned());
    assert_eq!(s.get(0).unwrap().label, "Notes");
    assert_eq!(s.active(), 0, "the pin is still showing");
    assert!(!s.get(i).unwrap().is_pinned());
    assert_eq!(s.get(i).unwrap().label, "Brief");
}

#[test]
fn pin_moves_into_the_leading_group() {
    let mut s = strip();
    s.push(TestTab::content("Notes"));
    assert!(s.pin(1, Uuid::new_v4(), false));
    assert_eq!(s.pinned_count(), 1);
    assert!(s.get(0).unwrap().is_pinned());
    assert_eq!(s.get(0).unwrap().label, "Notes");
    assert!(!s.get(0).unwrap().show_title);
    assert_eq!(s.active(), 0);
}

#[test]
fn unpin_in_home_returns_to_main() {
    let home = Uuid::new_v4();
    let mut s = strip();
    s.push(TestTab::content("Notes"));
    s.pin(1, home, false);
    assert!(s.unpin(0, Some(home)));
    assert_eq!(s.pinned_count(), 0);
    assert!(!s.get(0).unwrap().is_pinned());
    assert_eq!(s.get(0).unwrap().label, "Notes");
}

#[test]
fn unpin_foreign_closes() {
    let home = Uuid::new_v4();
    let other = Uuid::new_v4();
    let mut s = strip();
    s.push(TestTab::content("Notes"));
    s.pin(1, home, false);
    assert!(s.unpin(0, Some(other)));
    assert_eq!(s.pinned_count(), 0);
    assert!(s.iter().all(|t| t.label != "Notes"));
}

#[test]
fn move_tab_refuses_to_cross_groups() {
    let mut s = strip();
    s.push(TestTab::content("Notes"));
    s.pin(1, Uuid::new_v4(), false);
    s.push(TestTab::floor());
    assert!(!s.move_tab(0, 1));
    assert!(s.get(0).unwrap().is_pinned());
}

#[test]
fn move_tab_reorders_inside_pinned() {
    let home = Uuid::new_v4();
    let mut s = strip();
    s.push(TestTab::content("A"));
    s.pin(1, home, false);
    s.push(TestTab::content("B"));
    s.pin(1, home, false);
    assert_eq!(s.get(0).unwrap().label, "A");
    assert_eq!(s.get(1).unwrap().label, "B");
    assert!(s.move_tab(0, 1));
    assert_eq!(s.get(0).unwrap().label, "B");
    assert_eq!(s.get(1).unwrap().label, "A");
}

#[test]
fn close_all_leaves_pins() {
    let mut s = strip();
    s.push(TestTab::content("Notes"));
    s.pin(1, Uuid::new_v4(), false);
    s.push(TestTab::floor());
    assert!(s.close_all_unpinned());
    assert_eq!(s.pinned_count(), 1);
    assert!(s.get(0).unwrap().is_pinned());
}

#[test]
fn last_main_tab_with_pins_does_not_mint_a_floor_over_them() {
    let mut s = strip();
    s.push(TestTab::content("Notes"));
    s.pin(1, Uuid::new_v4(), false);
    assert_eq!(s.len(), 2);
    assert!(s.close(1));
    assert_eq!(s.pinned_count(), 1);
    assert_eq!(s.len(), 1);
    assert!(s.get(0).unwrap().is_pinned());
}

#[test]
fn show_title_only_on_pins() {
    let mut s = strip_n(2);
    assert!(!s.set_show_title(0, true));
    s.push(TestTab::content("Notes"));
    s.pin(2, Uuid::new_v4(), false);
    assert!(s.set_show_title(0, true));
    assert!(s.get(0).unwrap().show_title);
}

/// **The host's rule, asked once.** A tab that answers `pinnable() == false`
/// goes back where it was and the strip reports nothing happened.
#[test]
fn a_tab_that_refuses_to_be_pinned_stays_where_it_was() {
    let mut s = strip();
    assert!(!s.pin(0, Uuid::new_v4(), false));
    assert_eq!(s.pinned_count(), 0);
    assert_eq!(s.main_count(), 1);
    assert!(s.get(0).unwrap().is_placeholder());
}

#[test]
fn reset_to_floor_with_pins_clears_main() {
    let mut s = strip();
    s.push(TestTab::content("Notes"));
    s.pin(1, Uuid::new_v4(), false);
    s.reset_to_floor();
    assert_eq!(s.pinned_count(), 1);
    assert_eq!(s.main_count(), 0);
    assert!(!s.has_placeholder());
}

#[test]
fn reusable_main_flat_index_names_main_while_a_pin_is_showing() {
    let home = Uuid::new_v4();
    let mut s = strip();
    s.push(TestTab::content("Notes"));
    s.pin(1, home, false);
    s.push(TestTab::content("Brief"));
    s.select(0);
    assert!(s.get(0).unwrap().is_pinned());
    assert_eq!(s.reusable_main_flat_index(), Some(1));
}

#[test]
fn drop_lone_placeholder_if_pinned() {
    let mut s = strip();
    s.push(TestTab::content("Notes"));
    s.pin(1, Uuid::new_v4(), false);
    assert!(s.has_placeholder());
    s.drop_lone_placeholder_if_pinned();
    assert!(!s.has_placeholder());
    assert_eq!(s.pinned_count(), 1);
}

/// Pins have no floor: dropping the last one is allowed, and the strip falls
/// back to Main rather than minting a pin.
#[test]
fn retain_pinned_may_empty_the_pin_group() {
    let mut s = strip();
    s.push(TestTab::content("Notes"));
    s.pin(1, Uuid::new_v4(), false);
    assert!(s.retain_pins(|tab| tab.label != "Notes"));
    assert_eq!(s.pinned_count(), 0);
    assert_eq!(s.main_count(), 1, "sanitize minted Main's floor instead");
    assert_eq!(s.active_loc().group, TabGroup::Main);
}

/// `retain_main` never touches a pin, whatever the predicate says.
#[test]
fn retain_main_leaves_the_pins_alone() {
    let mut s = strip();
    s.push(TestTab::content("Notes"));
    s.pin(1, Uuid::new_v4(), false);
    s.push(TestTab::content("Brief"));
    assert!(s.retain_main(|_| false));
    assert_eq!(s.pinned_count(), 1);
    assert_eq!(s.get(0).unwrap().label, "Notes");
}

/// The flat index runs pins first, and `position` follows it.
#[test]
fn position_searches_pins_before_main() {
    let mut s = strip();
    s.push(TestTab::content("Notes"));
    s.pin(1, Uuid::new_v4(), false);
    s.push(TestTab::content("Brief"));
    assert_eq!(s.position(|t| t.label == "Notes"), Some(0));
    assert_eq!(s.position(|t| t.label == "Brief"), Some(1));
}

/// A placeholder the host marks not reusable — a dedicated page — stays when
/// content is pushed beside a pin.
#[test]
fn a_dedicated_placeholder_survives_a_push_next_to_pins() {
    let mut s = strip();
    s.push(TestTab::content("Notes"));
    s.pin(1, Uuid::new_v4(), false);
    let mut dedicated = TestTab::floor();
    dedicated.reusable = false;
    assert!(dedicated.is_placeholder());
    s.restore_main(vec![dedicated], 0);
    assert!(s.has_placeholder());

    s.push(TestTab::content("Brief"));
    s.insert_inactive(TestTab::content("Plan"));
    assert!(s.has_placeholder(), "the dedicated page was not replaced");
    assert_eq!(s.main_count(), 3);
}

/// Session restore's explicit drop still clears a lone dedicated placeholder
/// it scaffolded under a pinned-only group.
#[test]
fn the_explicit_drop_still_removes_a_dedicated_placeholder() {
    let mut s = strip();
    s.push(TestTab::content("Notes"));
    s.pin(1, Uuid::new_v4(), false);
    let mut dedicated = TestTab::floor();
    dedicated.reusable = false;
    s.restore_main(vec![dedicated], 0);
    s.drop_lone_placeholder_if_pinned();
    assert!(!s.has_placeholder());
}
