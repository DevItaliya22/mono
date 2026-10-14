// SPDX-License-Identifier: MPL-2.0

//! Geometry and behaviour. What cannot be tested here is whether the *body* is
//! true — whether "nothing was deleted" is a fact — and that is the one thing
//! that matters most, which is why every raise site is read by hand against its
//! own rollback rather than trusted.

use super::*;
use koru_core::WorkspaceId;
use koru_workspace_host::{ErrorReport, PaneRequest};

fn engine() -> TextEngine {
    TextEngine::new()
}

fn report() -> ErrorReport {
    ErrorReport::new(
        "Couldn't export \u{201c}Research\u{201d}",
        "The folder isn't writable, so no file was created. Nothing was changed.",
    )
    .with_details("Access is denied. (os error 5)")
    .with_action(koru_workspace_host::ErrorAction::Retry(
        PaneRequest::ExportWorkspace(WorkspaceId::new()),
    ))
}

#[test]
fn every_target_hits_back_to_itself() {
    let d = ErrorDialog::new(report());
    let mut e = engine();
    let vp = (1280.0, 720.0);
    for (hit, r) in d.targets(&mut e, vp) {
        let (x, y) = r.centre();
        assert_eq!(d.hit_at(&mut e, vp, x, y), Some(hit), "{hit:?}");
    }
}

/// Cancel is always there; the offered actions come after it, left to right,
/// with the primary owning the right-hand corner.
#[test]
fn the_buttons_run_left_to_right_without_overlapping() {
    let d = ErrorDialog::new(report());
    let mut e = engine();
    let vp = (1280.0, 720.0);
    let panel = d.panel(&mut e, vp);
    let mut buttons: Vec<_> = d
        .targets(&mut e, vp)
        .into_iter()
        .filter(|(h, _)| matches!(h, Hit::Cancel | Hit::Action(_)))
        .map(|(_, r)| r)
        .collect();
    buttons.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap());
    for pair in buttons.windows(2) {
        assert!(pair[0].x + pair[0].w <= pair[1].x, "buttons overlap");
    }
    let last = buttons.last().unwrap();
    assert!(
        last.x + last.w <= panel.x + panel.w,
        "the primary stays inside the panel"
    );
}

/// Opening the fold makes the panel taller. If it did not, the well would be
/// drawn over the buttons.
#[test]
fn opening_the_details_makes_room_for_them() {
    let mut e = engine();
    let vp = (1280.0, 720.0);
    let mut d = ErrorDialog::new(report());
    let shut = d.panel(&mut e, vp).h;
    let (x, y) = d
        .targets(&mut e, vp)
        .into_iter()
        .find(|(h, _)| *h == Hit::Details)
        .unwrap()
        .1
        .centre();
    assert!(matches!(d.mouse_down(&mut e, vp, x, y), Outcome::Keep));
    assert!(d.panel(&mut e, vp).h > shut, "the panel grew");
}

/// A report with nothing to show offers no fold and no copy link — an empty
/// `Details` that opens onto nothing is worse than no `Details`.
#[test]
fn a_report_without_details_offers_neither_the_fold_nor_the_copy() {
    let d = ErrorDialog::new(ErrorReport::new("Couldn't do it", "Nothing changed."));
    let mut e = engine();
    let hits: Vec<_> = d
        .targets(&mut e, (1280.0, 720.0))
        .into_iter()
        .map(|(h, _)| h)
        .collect();
    assert!(!hits.contains(&Hit::Details));
    assert!(!hits.contains(&Hit::Copy));
    assert!(hits.contains(&Hit::Cancel));
}

/// **The one that matters.** A press outside does not dismiss. An error is what
/// a failed save has instead of silence, and a stray click past it must not be
/// the thing that makes it silent again.
#[test]
fn a_press_outside_does_not_dismiss_the_dialog() {
    let mut d = ErrorDialog::new(report());
    let mut e = engine();
    let vp = (1280.0, 720.0);
    assert!(matches!(d.mouse_down(&mut e, vp, 4.0, 4.0), Outcome::Keep));
    let panel = d.panel(&mut e, vp);
    assert!(matches!(
        d.mouse_down(&mut e, vp, panel.x + 10.0, panel.y + 10.0),
        Outcome::Keep,
    ));
}

/// Enter runs the primary — the corner button — and Escape always closes.
#[test]
fn enter_runs_the_primary_and_escape_closes() {
    use winit::keyboard::{Key, NamedKey};
    let mut d = ErrorDialog::new(report());
    assert!(matches!(
        d.key(&Key::Named(NamedKey::Enter)),
        Outcome::Act(_)
    ));
    assert!(matches!(
        d.key(&Key::Named(NamedKey::Escape)),
        Outcome::Close
    ));

    // With nothing offered, Enter is Cancel — the only thing there is to do.
    let mut bare = ErrorDialog::new(ErrorReport::new("Couldn't do it", "Nothing changed."));
    assert!(matches!(
        bare.key(&Key::Named(NamedKey::Enter)),
        Outcome::Close
    ));
}

/// Everything else is swallowed. A modal that let keystrokes through would take
/// dictation into the document behind it while a warning was on screen.
#[test]
fn other_keys_are_swallowed_rather_than_passed_on() {
    use winit::keyboard::Key;
    let mut d = ErrorDialog::new(report());
    assert!(matches!(d.key(&Key::Character("a".into())), Outcome::Keep));
}

/// Copying leaves the dialog up: it is what you do on the way to reporting the
/// error, not instead of reading it.
#[test]
fn copying_the_error_hands_back_the_raw_text_and_stays_open() {
    let mut d = ErrorDialog::new(report());
    let mut e = engine();
    let vp = (1280.0, 720.0);
    let (x, y) = d
        .targets(&mut e, vp)
        .into_iter()
        .find(|(h, _)| *h == Hit::Copy)
        .unwrap()
        .1
        .centre();
    match d.mouse_down(&mut e, vp, x, y) {
        Outcome::Copy(text) => assert_eq!(text, "Access is denied. (os error 5)"),
        other => panic!("expected the raw error, got {}", label(&other)),
    }
}

fn label(o: &Outcome) -> &'static str {
    match o {
        Outcome::Keep => "Keep",
        Outcome::Close => "Close",
        Outcome::Copy(_) => "Copy",
        Outcome::Act(_) => "Act",
    }
}

/// A window smaller than the dialog still puts it on screen.
#[test]
fn a_tiny_window_still_keeps_the_dialog_on_screen() {
    let d = ErrorDialog::new(report());
    let mut e = engine();
    let p = d.panel(&mut e, (200.0, 160.0));
    assert!(p.x >= 0.0 && p.y >= 0.0);
    assert!(p.w <= 200.0 && p.h <= 160.0);
}

/// A long error scrolls inside its well instead of overflowing onto the
/// controls below — the well is fixed-height, the text is not.
#[test]
fn long_details_scroll_inside_the_well_and_clamp() {
    let long = "store_unreachable: could not reach the store: error sending request for url \
                (http://127.0.0.1:60112/v1/workspaces/5184dae2-d5fa-4b37-88b3-2b754a4aa512/\
                blobs/b3:1f92ba9552124f3a084d65763b0d233ef29bef80e3055c26bfdf8681fb2870d4)";
    let mut d = ErrorDialog::new(
        ErrorReport::new(
            "The file could not be opened",
            "The workspace was not changed.",
        )
        .with_details(long),
    );
    let mut e = engine();
    let vp = (1280.0, 720.0);

    // Closed: the wheel does nothing.
    assert!(!d.scroll_by(&mut e, vp, 40.0));

    d.open = true;
    let (well, content_h) = d.details_well(&mut e, vp).expect("open details");
    assert!(
        content_h > well.h,
        "the fixture must overflow ({content_h} <= {}) or this pins nothing",
        well.h
    );
    assert!(d.scroll_by(&mut e, vp, 40.0));
    let max = content_h - well.h;
    assert!(d.scroll_by(&mut e, vp, 10_000.0));
    assert_eq!(d.scroll, max, "scroll clamps to the overflow");
    assert!(!d.scroll_by(&mut e, vp, 1.0), "at the floor, nothing moves");
    assert!(d.scroll_by(&mut e, vp, -10_000.0));
    assert_eq!(d.scroll, 0.0, "and back to the top");

    // A short error never scrolls.
    let mut short = ErrorDialog::new(report());
    short.open = true;
    assert!(!short.scroll_by(&mut e, vp, 40.0));
}
