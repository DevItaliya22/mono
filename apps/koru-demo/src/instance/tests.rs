// SPDX-License-Identifier: MPL-2.0

//! **These touch no disk and open no rendezvous.**
//!
//! `root_exists` and `root_is_serving` are arguments, so the rule can be tested
//! without a directory or a running Koru. What a *real* `probe_at` answers, and
//! whether a real window reports `KoruWindow`, are the interactive proof's.

use super::*;

fn target() -> Target {
    Target {
        root: PathBuf::from("C:/koru-roots/a11y"),
        process_id: 4242,
        window_handle: 0x1234,
    }
}

fn koru_window() -> RootElement {
    RootElement {
        automation_id: "koru:w3/v0/s:window#window".to_owned(),
        class_name: Some(WINDOW_CLASS.to_owned()),
        framework: Some(FRAMEWORK.to_owned()),
        process_id: Some(4242),
    }
}

#[test]
fn every_later_locator_is_scoped_by_the_window_number_the_root_answered_with() {
    // The bootstrap. A runner that assumed `w1` would search the first window
    // the process ever opened, which after one reopen is not the one it was
    // pointed at.
    let identified = identify(&target(), true, true, &koru_window()).expect("a Koru window");
    assert_eq!(identified.window_slot, 3);
    assert!(identified.root_is_serving);
    assert!(identified.to_string().contains("slot 3"));
}

#[test]
fn a_root_nobody_named_is_refused_before_any_window_is_looked_at() {
    let refusal = identify(&target(), false, false, &koru_window()).expect_err("no such root");
    assert!(matches!(refusal, NotIdentified::NoSuchRoot(_)));
    assert!(
        refusal.to_string().contains("no search path"),
        "the refusal has to say why there is no fall-back: {refusal}"
    );
}

#[test]
fn a_window_in_another_process_is_refused_before_anything_else_is_asked_of_it() {
    // §7.5's *no attaching to the user's ordinary window*, at the only moment it
    // can be caught. Note the ordering: this is refused even though the window
    // is a perfectly good Koru window with a valid locator — being Koru's is not
    // the same as being *this run's*.
    let mut other = koru_window();
    other.process_id = Some(9999);
    let refusal = identify(&target(), true, true, &other).expect_err("another process");
    assert_eq!(
        refusal,
        NotIdentified::OtherProcess {
            named: 4242,
            answered: Some(9999)
        }
    );
}

#[test]
fn a_window_that_is_not_koru_is_named_by_what_it_answered() {
    // Both shapes a non-Koru window takes from out here: somebody else's
    // application, and `accesskit_windows`' placeholder root — which has no
    // `author_id` at all and so reads back as the empty string.
    for automation_id in ["", "NotepadTextArea"] {
        let mut stranger = koru_window();
        stranger.automation_id = automation_id.to_owned();
        let refusal = identify(&target(), true, true, &stranger).expect_err("not Koru");
        assert_eq!(
            refusal,
            NotIdentified::NotKoru {
                automation_id: automation_id.to_owned()
            }
        );
        assert!(refusal.to_string().contains("koru:"));
    }
}

#[test]
fn a_window_of_the_right_process_and_the_wrong_class_is_still_refused() {
    let mut tray = koru_window();
    tray.class_name = Some("Shell_TrayWnd".to_owned());
    assert_eq!(
        identify(&target(), true, true, &tray),
        Err(NotIdentified::WrongClass {
            class_name: Some("Shell_TrayWnd".to_owned())
        })
    );
}

#[test]
fn a_root_that_is_not_serving_is_reported_rather_than_refused() {
    // Deliberately not a refusal. A window can be inspected while its Store is
    // down, and half of what this runner exists to check — the save status, a
    // controlled failure — is only visible when it is. What must never happen
    // is the fact going unsaid, so it is on the identification a run prints.
    let identified = identify(&target(), true, false, &koru_window()).expect("a Koru window");
    assert!(!identified.root_is_serving);
    assert!(
        identified.to_string().contains("nothing is answering"),
        "{identified}"
    );
}

#[test]
fn a_relative_root_is_resolved_before_anything_is_asked_about_it() {
    // The bug this exists for: `koru-ipc` names a rooted endpoint by a
    // *lexical* digest of the path, so a root typed relative to somebody's
    // shell is a different rendezvous from the absolute one the Agent
    // published under. The runner said "nothing is answering this root's
    // rendezvous" at somebody whose Store was running perfectly.
    let absolute = resolved(Path::new("some-root/under-the-shell"));
    assert!(
        absolute.is_absolute(),
        "a relative root must be resolved before it is used to name a pipe: {}",
        absolute.display()
    );
    assert!(absolute.ends_with("under-the-shell"));

    // An already-absolute root is left alone — resolving must not be able to
    // move a run to a different instance than the one that was named.
    #[cfg(windows)]
    let already = Path::new(r"D:\koru-roots\a11y");
    #[cfg(not(windows))]
    let already = Path::new("/koru-roots/a11y");
    assert_eq!(resolved(already), already);
}

// ------------------------------------------------- which binary is answering

/// **An age rather than a timestamp**, because the question is *is this the code
/// I just built* and nobody should have to do timezone arithmetic to answer it.
#[test]
fn a_build_age_reads_as_an_age() {
    use std::time::{Duration, SystemTime};
    let ago = |secs| super::format_built(SystemTime::now() - Duration::from_secs(secs));
    assert_eq!(ago(5), "under a minute ago");
    assert_eq!(ago(60), "1 min ago");
    assert_eq!(ago(60 * 45), "45 min ago");
    // Past ninety minutes a count of minutes stops being readable.
    assert_eq!(ago(60 * 161), "2 h 41 min ago");
}

/// A clock that moved backwards says so rather than reporting a negative age or
/// panicking on the subtraction.
#[test]
fn a_binary_from_the_future_says_the_clock_moved() {
    use std::time::{Duration, SystemTime};
    let built = super::format_built(SystemTime::now() + Duration::from_secs(3600));
    assert!(built.contains("check the clock"), "{built}");
}

/// This process is one the runner can certainly read, so the read itself is
/// exercised rather than only its formatting. On a platform with no
/// implementation the honest answer is `None`, and the runner still runs.
#[test]
fn the_running_process_can_be_identified_or_honestly_cannot() {
    let mine = super::binary_of(std::process::id());
    if cfg!(windows) {
        let mine = mine.expect("this process's own image path is readable");
        assert!(
            mine.path.to_string_lossy().to_lowercase().ends_with(".exe"),
            "{}",
            mine.path.display()
        );
    } else {
        assert!(mine.is_none());
    }
}
