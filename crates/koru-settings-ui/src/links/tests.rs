// SPDX-License-Identifier: MPL-2.0

//! What the link wrapper adds to `SectionLink`: what is outstanding, and
//! whether anybody is home.

use super::*;

#[test]
fn a_surface_with_no_links_holds_none() {
    let links = SettingsLinks::disabled();
    assert!(!links.holds_global());
    assert!(!links.holds_terminal());
    assert_eq!(links.deadline(), None);
    assert!(
        !links.is_overdue(),
        "nothing was promised, so nothing is late"
    );
}

#[test]
fn a_link_that_holds_a_section_says_so() {
    let links = SettingsLinks::inert(false, true);
    assert!(!links.holds_global());
    assert!(links.holds_terminal());
    assert!(links.deadline().is_some());
}

/// A patch that has gone out and not come back is what the surface compares
/// the next arriving section against.
#[test]
fn a_sent_patch_stays_outstanding_until_a_section_arrives() {
    let mut links = SettingsLinks::inert(false, true);
    assert!(links.take_outstanding_terminal().is_none());
    links.save_terminal(TerminalPatch {
        composer_default: Some(false),
        ..TerminalPatch::default()
    });
    let outstanding = links
        .take_outstanding_terminal()
        .expect("the patch is remembered");
    assert_eq!(outstanding.composer_default, Some(false));
    assert!(
        links.take_outstanding_terminal().is_none(),
        "and it is taken once"
    );
}

/// **The deadline is what makes silence legible.** An inert link never
/// publishes, which is exactly what an unreachable Gateway looks like, so the
/// wrapper calls it overdue once the budget has passed.
#[test]
fn a_link_that_never_publishes_goes_overdue() {
    let mut links = SettingsLinks::inert(false, true);
    assert!(
        !links.is_overdue(),
        "not yet: a window that opened while the Agent was starting the \
         Gateway is not down for being early"
    );
    // Reach in rather than sleep four seconds: the clock is the thing under
    // test, and a suite that waited on a real one would cost the same four
    // seconds on every run.
    links.terminal.as_mut().unwrap().due = std::time::Instant::now();
    assert!(links.is_overdue());
}

/// A section arriving clears both the outstanding patch and the deadline: the
/// authority answered, so nothing is late and nothing is unanswered.
#[test]
fn an_arriving_section_clears_what_was_outstanding() {
    let mut links = SettingsLinks::inert(false, true);
    links.save_terminal(TerminalPatch {
        theme: Some(koru_settings::ThemeChoice::Dark),
        ..TerminalPatch::default()
    });
    links.terminal.as_mut().unwrap().due = std::time::Instant::now();
    assert!(links.is_overdue());
    links.terminal.as_mut().unwrap().arrived();
    assert!(!links.is_overdue());
    assert!(links.take_outstanding_terminal().is_none());
}

/// A link the surface holds answers the registry's defaults from the moment it
/// is built — defaults now, correct later, and never a hang.
#[test]
fn a_link_answers_the_registrys_defaults_before_anything_has_arrived() {
    let links = SettingsLinks::inert(true, true);
    assert_eq!(links.global_authoritative(), GlobalSettings::default());
    assert_eq!(links.terminal_authoritative(), TerminalSettings::default());
}
