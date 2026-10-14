// SPDX-License-Identifier: MPL-2.0

//! The ladder, walked against real captions. No `TextEngine` and no window:
//! the label width arrives measured, which is the whole point of caching it.

use super::*;
use crate::host::{BannerChip, BannerKey};
use crate::keys::{ProfileKey, WorkspaceKey};
use koru_draw::theme;

fn both() -> Banner {
    Banner {
        profile: Some(BannerChip::new(
            "Steven",
            BannerKey::Profile(ProfileKey::from_uuid(uuid::Uuid::new_v4())),
        )),
        workspace: Some(BannerChip::new(
            "Notes",
            BannerKey::Workspace(WorkspaceKey::from_uuid(uuid::Uuid::new_v4())),
        )),
        settings: true,
    }
}

/// A wide window, pane shut.
fn wide() -> Rect {
    Rect::new(0.0, 0.0, 1600.0, theme::CAPTION_H)
}

/// The floor both apps share, with the explore pane open — DESIGN §4's worked
/// case: `solve(720, h, 232, 0)` leaves the caption 488 px starting at 232.
fn floor_with_pane() -> Rect {
    Rect::new(232.0, 0.0, 488.0, theme::CAPTION_H)
}

#[test]
fn a_roomy_window_keeps_the_full_name() {
    let mut state = BannerState::default();
    state.measured(120.0);
    state.settle(both(), wide(), None, true, false);
    assert_eq!(state.rung(), Rung::Full);
    assert_eq!(
        state.slots().workspace,
        banner::Workspace::Named(120.0),
        "nothing was taken away"
    );
    assert!(state.slots().settings);
}

/// One step at a time, each triggered by the lane's reservation being unmet —
/// never two at once, and never a step that was not needed.
#[test]
fn the_ladder_takes_the_first_rung_that_fits() {
    let mut state = BannerState::default();
    state.measured(180.0);
    state.settle(both(), floor_with_pane(), None, true, false);
    assert!(
        state.rung() > Rung::Full,
        "a 180 px name does not fit at the floor with the pane open"
    );
    assert!(
        banner::fits(floor_with_pane(), true, false, state.slots()),
        "and where it stopped, the lane has its reservation: {:?}",
        state.rung()
    );
    // The rung above it is exactly the one that did not fit.
    let rungs = Rung::ALL;
    let index = rungs.iter().position(|r| *r == state.rung()).unwrap();
    if index > 0 {
        let mut previous = state.clone();
        previous.rung = rungs[index - 1];
        previous.slots = previous.at(rungs[index - 1]);
        assert!(
            !banner::fits(floor_with_pane(), true, false, previous.slots),
            "the ladder did not stop one rung too early"
        );
    }
}

/// **The identity slot is never dropped.** Whatever the ladder takes, the
/// leading square stays — that is what keeps both selectors reachable, and it is
/// why step 4 takes Settings instead.
#[test]
fn the_identity_square_survives_every_rung() {
    let mut state = BannerState::default();
    state.measured(400.0);
    for width in [200.0, 300.0, 400.0, 720.0, 1600.0] {
        state.settle(
            both(),
            Rect::new(0.0, 0.0, width, theme::CAPTION_H),
            None,
            true,
            false,
        );
        assert!(
            state.slots().profile,
            "w={width} rung={:?} dropped the identity slot",
            state.rung()
        );
    }
}

/// Settings goes only at the bottom rung, after the merge.
#[test]
fn settings_is_the_last_thing_taken() {
    let mut state = BannerState::default();
    state.measured(400.0);
    state.settle(both(), wide(), None, true, false);
    for rung in Rung::ALL {
        let slots = state.at(rung);
        assert_eq!(
            slots.settings,
            rung != Rung::NoSettings,
            "{rung:?} disagreed about the settings slot"
        );
    }
}

/// From the merged rung down, a press on the leading square means *both*
/// selectors — and the host is told so rather than being handed a profile press
/// it would answer with the wrong popover.
#[test]
fn the_merged_square_reports_identity_and_not_profile() {
    let mut state = BannerState::default();
    state.measured(0.0);
    state.settle(both(), wide(), None, true, false);
    assert_eq!(state.slot_for(banner::Hit::Profile), BannerSlot::Profile);
    assert!(!state.caption_merged(), "a roomy caption merges nothing");

    // The narrowest caption there is: the ladder has nowhere left to go, so the
    // leading square is the merged one and the host is told so.
    state.settle(
        both(),
        Rect::new(0.0, 0.0, 200.0, theme::CAPTION_H),
        None,
        true,
        false,
    );
    assert!(state.rung() >= Rung::Identity, "{:?}", state.rung());
    assert!(state.caption_merged());
    assert_eq!(state.slot_for(banner::Hit::Profile), BannerSlot::Identity);
    assert_eq!(state.slot_for(banner::Hit::Settings), BannerSlot::Settings);
}

// --------------------------------------------------- the identity in the pane
//
// The other arrangement (owner, 2026-09-02). Every assertion here is the
// caption's own promise re-asked of the band that carries the selectors now.

/// The explore column's band, as `ShellWindow::pane_header` cuts it.
fn a_band(w: f32) -> Rect {
    Rect::new(0.0, 0.0, w, theme::CAPTION_H)
}

/// **Pane open: the selectors are in the band and the caption keeps nothing of
/// them.** The lane gets the room back, which is the whole point of the move.
#[test]
fn with_the_pane_open_the_identity_is_in_the_band_and_the_caption_slots_are_zero_width() {
    let mut state = BannerState::default();
    state.measured(90.0);
    state.settle(
        both(),
        floor_with_pane(),
        Some(a_band(theme::SIDEBAR_W)),
        true,
        false,
    );
    assert!(state.pane_slots().profile, "the avatar is in the band");
    assert_eq!(
        state.pane_slots().workspace,
        banner::Workspace::Named(90.0),
        "and so is the name"
    );
    assert!(!state.slots().profile, "the caption offers no avatar");
    assert_eq!(state.slots().workspace, banner::Workspace::Hidden);
    assert!(
        state.slots().settings,
        "Settings never moved — it is caption furniture"
    );
    assert!(
        !state.pane_slots().settings,
        "and the band never carries it"
    );
    assert_eq!(
        state.pane_slot_for(banner::Hit::Profile),
        BannerSlot::Profile
    );
    assert_eq!(
        state.pane_slot_for(banner::Hit::Workspace),
        BannerSlot::Workspace
    );
}

/// **Pane shut: the merged square appears in the caption, and only then.**
///
/// Step 3 rather than step 0 — the caption is not the identity's home any more,
/// it is the fallback that keeps both selectors reachable when there is no band
/// on screen.
#[test]
fn shutting_the_pane_moves_the_identity_into_the_caption_as_one_merged_square() {
    let mut state = BannerState::default();
    state.measured(90.0);
    let caption = Rect::new(0.0, 0.0, 720.0, theme::CAPTION_H);
    state.settle(both(), caption, Some(a_band(0.0)), true, false);
    assert_eq!(
        state.pane_slots(),
        banner::Slots::NONE,
        "no band to draw in"
    );
    assert!(state.slots().profile, "the merged square is in the caption");
    assert_eq!(
        state.slots().workspace,
        banner::Workspace::Hidden,
        "one square, not two: it opens both sections"
    );
    assert!(state.caption_merged());
    assert_eq!(state.slot_for(banner::Hit::Profile), BannerSlot::Identity);
}

/// The band's ladder is the caption's ladder against the band's own budget: a
/// name too long for the column truncates rather than overhanging it.
#[test]
fn the_bands_ladder_takes_the_first_rung_that_fits_the_column() {
    let mut state = BannerState::default();
    state.measured(400.0);
    let band = a_band(theme::SIDEBAR_W);
    state.settle(both(), floor_with_pane(), Some(band), true, false);
    assert!(state.rung() > Rung::Full, "400 px does not fit 232");
    assert!(
        banner::pane_fits(band, state.pane_slots()),
        "and where it stopped, it fits: {:?}",
        state.rung()
    );
    assert!(
        state.pane_slots().profile,
        "the identity square survives every rung here too"
    );
}

/// A column dragged narrower than one square plus a name merges in the band,
/// and the band says so rather than the caption's flag.
#[test]
fn a_narrow_column_merges_in_the_band_and_reports_identity() {
    let mut state = BannerState::default();
    state.measured(400.0);
    state.settle(both(), wide(), Some(a_band(40.0)), true, false);
    assert!(state.pane_merged(), "rung {:?}", state.rung());
    assert_eq!(
        state.pane_slot_for(banner::Hit::Profile),
        BannerSlot::Identity
    );
    assert!(
        !state.caption_merged(),
        "the caption is empty while the pane is open, merged or not"
    );
}

/// A host offering nothing gets nothing, at every width: the ladder never
/// invents a slot.
#[test]
fn a_host_that_offers_no_banner_gets_none() {
    let mut state = BannerState::default();
    for width in [200.0, 720.0, 1600.0] {
        state.settle(
            Banner::default(),
            Rect::new(0.0, 0.0, width, theme::CAPTION_H),
            None,
            true,
            false,
        );
        assert_eq!(state.slots(), banner::Slots::NONE, "w={width}");
    }
}

/// Re-measuring the same label is not a reason to walk the ladder again.
#[test]
fn a_measurement_that_did_not_move_reports_nothing() {
    let mut state = BannerState::default();
    assert!(state.measured(90.0));
    assert!(!state.measured(90.0));
    assert!(state.measured(91.0));
}
