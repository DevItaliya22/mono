// SPDX-License-Identifier: MPL-2.0

use super::*;

/// These are process globals, so a test that moves one has to take a lock or
/// it races the others under cargo's parallel runner — the same rule the theme
/// and the clipboard already follow.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn fixture() -> Vec<InputDevice> {
    vec![
        InputDevice::new("Microphone Array (Realtek(R) Audio)", true),
        InputDevice::new("Yeti Nano", false),
    ]
}

/// **Nothing asked is not the same answer as nothing found.**
///
/// A settings page built before any worker has answered must not say *that
/// microphone is gone* about a headset that is plugged in — that would be a
/// sentence about this process rather than about the machine.
#[test]
fn a_remembered_device_is_not_reported_missing_before_anything_has_asked() {
    let _held = LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let was_choice = crate::voice_prefs::input();
    let was_list = KNOWN
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    *KNOWN.write().unwrap_or_else(|error| error.into_inner()) = None;

    crate::voice_prefs::set_input(InputChoice::named("Yeti Nano"));
    assert!(!asked());
    assert_eq!(resolution(), InputResolution::Device("Yeti Nano".into()));
    assert_eq!(resolution().missing(), None);

    crate::voice_prefs::set_input(was_choice);
    *KNOWN.write().unwrap_or_else(|error| error.into_inner()) = was_list;
}

/// Once a worker has answered, the same name is read against what it said —
/// present, or a fall-back that names what it was looking for.
#[test]
fn an_answer_is_what_a_remembered_name_is_read_against() {
    let _held = LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let was_choice = crate::voice_prefs::input();
    let was_list = KNOWN
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .clone();

    set(fixture());
    assert!(asked());

    crate::voice_prefs::set_input(InputChoice::named("Yeti Nano"));
    assert_eq!(resolution(), InputResolution::Device("Yeti Nano".into()));

    crate::voice_prefs::set_input(InputChoice::named("Steven's AirPods"));
    let gone = resolution();
    assert_eq!(gone.device(), None, "it falls back to the system default");
    assert_eq!(
        gone.missing(),
        Some("Steven's AirPods"),
        "and it still names what it could not find"
    );

    crate::voice_prefs::set_input(InputChoice::SystemDefault);
    assert_eq!(resolution(), InputResolution::SystemDefault);

    crate::voice_prefs::set_input(was_choice);
    *KNOWN.write().unwrap_or_else(|error| error.into_inner()) = was_list;
}

/// An answer that says the same thing as the last one is not a change, so a
/// window is not asked to redraw for it.
#[test]
fn an_unchanged_answer_moves_nothing() {
    let _held = LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let was_list = KNOWN
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .clone();

    // Start from a list nothing else uses, so the first `set` is a real move
    // whatever ran before this test.
    set(vec![InputDevice::new("only this one", true)]);
    let before = generation();
    assert!(set(fixture()), "a different list is a change");
    assert_ne!(generation(), before);
    let after = generation();
    assert!(!set(fixture()), "the same list twice is not a change");
    assert_eq!(generation(), after);

    *KNOWN.write().unwrap_or_else(|error| error.into_inner()) = was_list;
}
