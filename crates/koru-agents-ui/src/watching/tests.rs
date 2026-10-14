// SPDX-License-Identifier: MPL-2.0

//! The read-only band's geometry and its words, in every locale.

use super::*;

fn band() -> Rect {
    Rect::new(0.0, 400.0, 640.0, FOOTER_H)
}

/// **Nothing is drawn on a session this window drives.** The band exists
/// only while somebody else has the session; an editable tab is the ordinary
/// composer's, unchanged.
#[test]
fn a_session_this_window_drives_has_no_band() {
    let mut engine = TextEngine::new();
    let laid = layout(band(), &mut engine, Lang::En, Watching::No);
    assert_eq!(laid, Layout::default());
    assert!(!laid.hits_action(320.0, 420.0));
    assert!(!Watching::No.watching());
}

/// **Watching says so, and offers the one control that changes it.** The
/// sentence and the button share the band, and the button is where the hit
/// test says it is — one arithmetic, asked twice.
#[test]
fn watching_says_it_and_the_control_is_where_it_is_drawn() {
    let mut engine = TextEngine::new();
    let b = band();
    let laid = layout(b, &mut engine, Lang::En, Watching::Yes);
    let action = laid.action.expect("the session can be taken");

    assert!(
        !Watching::Yes.line(Lang::En).is_empty(),
        "read-only is said, not merely enforced"
    );
    assert_eq!(Watching::Yes.action(Lang::En), Some("Take the session"));

    // Inside the band, at its right, clear of the sentence.
    assert!(action.x >= laid.line.x + laid.line.w);
    assert!(action.x + action.w <= b.x + b.w);
    assert!(action.y >= b.y && action.y + action.h <= b.y + b.h);

    assert!(laid.hits_action(action.centre().0, action.centre().1));
    assert!(
        !laid.hits_action(laid.line.x + 1.0, b.y + b.h * 0.5),
        "a press on the sentence takes nothing"
    );
}

/// **Once asked, there is nothing left to press** (ADR-354: absent, not
/// disabled), and the band says the request is with the agent rather than
/// claiming a session it has not been given (§7.4).
#[test]
fn asking_replaces_the_control_with_what_is_being_waited_for() {
    let mut engine = TextEngine::new();
    let laid = layout(band(), &mut engine, Lang::En, Watching::Asked);
    assert!(laid.action.is_none(), "no control, not a dimmed one");
    assert_eq!(Watching::Asked.action(Lang::En), None);
    assert!(!laid.hits_action(600.0, 420.0));
    assert_ne!(
        Watching::Asked.line(Lang::En),
        Watching::Yes.line(Lang::En),
        "waiting is a different sentence from watching"
    );
    assert!(Watching::Asked.watching(), "still read-only while waiting");
}

/// **Every word of the band, in all eight locales**, and the control still
/// fits its longest translation.
#[test]
fn the_band_speaks_all_eight_locales() {
    let mut engine = TextEngine::new();
    for lang in Lang::ALL {
        for state in [Watching::Yes, Watching::Asked] {
            let line = state.line(lang);
            assert!(!line.trim().is_empty(), "{lang:?} has no sentence");
            assert!(
                !line.contains("{}"),
                "{lang:?} left a placeholder unfilled: {line}"
            );
        }
        let label = Watching::Yes.action(lang).expect("a control to take it");
        assert!(!label.trim().is_empty(), "{lang:?} has no control word");
        let laid = layout(band(), &mut engine, lang, Watching::Yes);
        let action = laid.action.expect("a control");
        assert!(
            action.w >= ActionButton::new(label, ActionStyle::Secondary, None).width(&mut engine),
            "{lang:?} clips its own label"
        );
        assert!(laid.line.w > 0.0, "{lang:?} left no room for the sentence");
    }
}

/// A band with no height lays nothing out rather than laying it out
/// off-screen — the pane a surface has not been measured in yet.
#[test]
fn an_unmeasured_band_lays_nothing_out() {
    let mut engine = TextEngine::new();
    let laid = layout(
        Rect::new(0.0, 0.0, 640.0, 0.0),
        &mut engine,
        Lang::En,
        Watching::Yes,
    );
    assert_eq!(laid, Layout::default());
}
