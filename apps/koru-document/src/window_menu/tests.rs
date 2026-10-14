// SPDX-License-Identifier: MPL-2.0

use super::*;

use crate::menu::Row;

fn labels(pinned: bool, maximized: bool) -> Vec<String> {
    shape(pinned, maximized)
        .into_iter()
        .filter_map(|r| match r {
            Row::Item { label, .. } => Some(label.into_owned()),
            _ => None,
        })
        .collect()
}

fn shortcut_for(pinned: bool, maximized: bool, label: &str) -> Option<String> {
    shape(pinned, maximized).into_iter().find_map(|r| match r {
        Row::Item {
            label: l, shortcut, ..
        } if l == label => Some(shortcut.map(|c| c.text())),
        _ => None,
    })?
}

/// Teaching the chords is why the four buttons became a menu, so a row losing
/// its chord is the feature quietly going away.
#[test]
fn every_window_row_that_has_a_chord_names_it() {
    // `Win+Down`, never `Win+M`: the latter minimises the whole desktop.
    assert_eq!(
        shortcut_for(false, false, "Minimize"),
        cfg!(windows).then(|| "Win+Down".to_string())
    );
    assert_eq!(
        shortcut_for(false, false, "Maximize"),
        cfg!(windows).then(|| "Win+Up".to_string())
    );
    assert_eq!(
        shortcut_for(false, true, "Restore"),
        cfg!(windows).then(|| "Win+Down".to_string())
    );
    // `Ctrl+W`, not `Alt+F4` — both close this window, and this is the one a
    // reader would not already know. See the row's comment.
    assert_eq!(
        shortcut_for(false, false, "Close Window"),
        Some(koru_keys::label_of(
            koru_keys::ShortcutId::CloseDocumentWindow
        ))
    );
}

#[test]
fn maximise_becomes_restore_when_the_window_fills_the_screen() {
    assert!(labels(false, false).iter().any(|l| l == "Maximize"));
    assert!(!labels(false, false).iter().any(|l| l == "Restore"));
    assert!(labels(false, true).iter().any(|l| l == "Restore"));
    assert!(!labels(false, true).iter().any(|l| l == "Maximize"));
    // Either way it is the same row and the same request.
    assert_eq!(choice_at(false, false, 2), Some(Choice::ToggleMaximise));
    assert_eq!(choice_at(false, true, 2), Some(Choice::ToggleMaximise));
}

#[test]
fn always_on_top_flips_its_label_when_the_window_is_pinned() {
    assert!(labels(false, false).iter().any(|l| l == "Always on Top"));
    assert!(
        labels(true, false)
            .iter()
            .any(|l| l == "Turn Off Always on Top")
    );
    assert_eq!(choice_at(false, false, 0), Some(Choice::TogglePin));
    assert_eq!(choice_at(true, false, 0), Some(Choice::TogglePin));
}

/// Every row does something. A window menu with an inert row would be four
/// buttons plus a decoration.
#[test]
fn every_row_carries_a_choice() {
    for pinned in [false, true] {
        for maximized in [false, true] {
            let rows = rows(pinned, maximized);
            // The two snap rows are Windows-only — see `koru_window::snap`.
            // There is no tray row here at all; see the module header.
            assert_eq!(rows.len(), if cfg!(windows) { 6 } else { 4 });
            for (i, (_, choice)) in rows.iter().enumerate() {
                assert!(choice.is_some(), "row {i} ({pinned}, {maximized}) is inert");
            }
        }
    }
}

/// `shape` drives the geometry and `rows` drives the dispatch, so a row present
/// in one and not the other would make the click land one row off.
#[test]
fn the_shape_and_the_rows_stay_the_same_length() {
    for pinned in [false, true] {
        for maximized in [false, true] {
            assert_eq!(
                shape(pinned, maximized).len(),
                rows(pinned, maximized).len()
            );
        }
    }
}

/// Always on Top names the chord that does the same thing. It is the one window
/// action with a Koru chord rather than a Windows one, and a blank column there
/// taught nothing about a key that already existed.
#[test]
fn always_on_top_names_its_chord() {
    assert_eq!(
        shortcut_for(false, false, "Always on Top"),
        Some(koru_keys::label_of(koru_keys::ShortcutId::PinWindow))
    );
    assert_eq!(
        shortcut_for(true, false, "Turn Off Always on Top"),
        Some(koru_keys::label_of(koru_keys::ShortcutId::PinWindow))
    );
}

/// The two halves, and the chords they stand in for.
#[test]
#[cfg(windows)]
fn the_snap_rows_name_their_chords() {
    assert_eq!(
        shortcut_for(false, false, "Snap Left"),
        cfg!(windows).then(|| "Win+Left".to_string())
    );
    assert_eq!(
        shortcut_for(false, false, "Snap Right"),
        cfg!(windows).then(|| "Win+Right".to_string())
    );
}

/// Close stays last however long the list gets, so the row you least want to
/// hit by accident never moves under the pointer.
#[test]
fn close_is_always_the_last_row() {
    for pinned in [false, true] {
        for maximized in [false, true] {
            assert_eq!(
                labels(pinned, maximized).last().map(String::as_str),
                Some("Close Window")
            );
        }
    }
}
