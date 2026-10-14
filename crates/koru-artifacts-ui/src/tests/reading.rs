// SPDX-License-Identifier: MPL-2.0

//! **Opening a version is a request, not a mode.**
//!
//! The column lists results and asks the host to open one as a tab; the
//! reading, the editing and the saving all belong to
//! [`crate::ArtifactTab`], whose own suite covers them (ADR-369). What is
//! here is the half the *column* still owns: the offer, what travels with
//! it, and the fact that the list never goes away.

use super::*;

/// A column with one family of two versions, the older history row selected.
fn selected() -> ArtifactsSurface {
    let one = family(1, "Quarterly findings", 500);
    let versions = history(&one, 2);
    let present = all_present(&[(one.clone(), versions.clone())]);
    let older = versions[0].id;
    // The workspace first: setting one clears whatever was on screen,
    // because another workspace's results are not this one's.
    let mut surface = ArtifactsSurface::without_reader();
    surface.set_now(10_000_000);
    surface.set_workspace(Some(workspace()));
    surface.take_catalog(Ok(Catalog::build(
        vec![(one.clone(), versions)],
        Vec::new(),
        Some(present),
    )));
    surface.toggle_family(one.id);
    surface.select_version(older);
    surface
}

/// An older selected version offers **Open**, under its provenance sentence
/// and pressable. The latest version is the primary family row itself.
#[test]
fn a_selected_version_offers_open() {
    let mut surface = selected();
    let rows = rows_of(&mut surface);
    let open = rows
        .iter()
        .find(|row| matches!(row, Row::Open { .. }))
        .copied()
        .expect("an Open row");
    assert!(open.pressable());
    assert!(
        rows.iter().any(|row| matches!(row, Row::Provenance { .. })),
        "the sentence is still there: {rows:?}"
    );
}

/// **The primary row is the artifact.** Pressing its label opens the latest
/// version directly; pressing its caret reveals history without opening it.
#[test]
fn the_primary_artifact_opens_and_its_caret_only_reveals_history() {
    let one = family(1, "Quarterly findings", 500);
    let versions = history(&one, 2);
    let latest = versions[1].id;
    let present = all_present(&[(one.clone(), versions.clone())]);
    let mut surface = showing(Catalog::build(
        vec![(one.clone(), versions)],
        Vec::new(),
        Some(present),
    ));
    let (_, rect) = surface
        .rows(tall(), TOP)
        .into_iter()
        .find(|(row, _)| *row == Row::Family(0))
        .expect("the primary artifact");
    let parts = surface.row_parts(Row::Family(0), rect);

    assert!(surface.mouse_down(
        tall(),
        TOP,
        parts.label.x + 2.0,
        parts.label.y + parts.label.h * 0.5,
    ));
    assert_eq!(
        surface.take_wants_open().map(|open| open.version),
        Some(latest)
    );
    assert!(!surface.is_expanded(one.id));

    assert!(surface.mouse_down(
        tall(),
        TOP,
        parts.caret.x + parts.caret.w * 0.5,
        parts.caret.y + parts.caret.h * 0.5,
    ));
    assert!(surface.is_expanded(one.id));
    assert!(surface.take_wants_open().is_none());
}

/// **The column stays on its list.** Pressing Open leaves a request behind
/// for the host, and nothing about what is on screen changes — no reader, no
/// *back* button, no mode.
#[test]
fn opening_a_version_asks_the_host_and_keeps_the_list() {
    let mut surface = selected();
    let before = rows_of(&mut surface);
    surface.ask_to_open(0, 0);

    let one = family(1, "Quarterly findings", 500);
    let asked = surface.take_wants_open().expect("a request for a tab");
    // Row 0 of an opened family is the *newest* version, which is the one
    // the person is looking at.
    assert_eq!(asked.version, history(&one, 2)[1].id);
    assert_eq!(asked.title, "Quarterly findings");
    assert_eq!(asked.size_bytes, 2_048);
    assert_eq!(asked.produced_by, Some(run(1)));
    assert_eq!(asked.held_elsewhere, None);

    assert_eq!(rows_of(&mut surface), before, "the list did not move");
    assert!(
        surface.take_wants_open().is_none(),
        "a request is taken once"
    );
}

/// **D27 travels with the request.** A version this device does not hold is
/// named as elsewhere before a byte is asked for, so the tab never puts a
/// question to a gateway that would have to refuse it.
#[test]
fn a_version_this_device_does_not_hold_says_so_in_the_request() {
    let one = family(1, "Quarterly findings", 500);
    let versions = history(&one, 2);
    let mut surface = ArtifactsSurface::without_reader();
    surface.set_now(10_000_000);
    surface.set_workspace(Some(workspace()));
    surface.take_catalog(Ok(Catalog::build(
        vec![(one.clone(), versions)],
        Vec::new(),
        // The blob listing answered, and holds none of them.
        Some(std::collections::HashSet::new()),
    )));
    surface.toggle_family(one.id);
    surface.ask_to_open(0, 0);
    let asked = surface.take_wants_open().expect("a request for a tab");
    assert_eq!(
        asked.held_elsewhere,
        Some(None),
        "elsewhere, and no device this window can name"
    );
}

/// The Open row is reachable from the keyboard, and Enter on it is the same
/// press the pointer makes.
#[test]
fn open_is_reachable_from_the_keyboard() {
    let mut surface = selected();
    surface.focus_first(tall(), TOP);
    for _ in 0..12 {
        if matches!(surface.focus(), Some(Row::Open { .. })) {
            break;
        }
        surface.key(tall(), TOP, crate::Press::Down);
    }
    assert!(
        matches!(surface.focus(), Some(Row::Open { .. })),
        "{:?}",
        surface.focus()
    );
    surface.key(tall(), TOP, crate::Press::Enter);
    assert!(surface.take_wants_open().is_some());
}

/// Switching workspace drops a request nobody drained: another workspace's
/// results are not this one's.
#[test]
fn changing_workspace_drops_an_undrained_request() {
    let mut surface = selected();
    surface.ask_to_open(0, 0);
    surface.set_workspace(Some(WorkspaceId::from_uuid(v4(0xa1))));
    assert!(surface.take_wants_open().is_none());
}
