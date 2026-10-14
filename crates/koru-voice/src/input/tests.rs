// SPDX-License-Identifier: MPL-2.0

use super::*;

fn three() -> Vec<InputDevice> {
    vec![
        InputDevice::new("Microphone Array (Realtek)", true),
        InputDevice::new("Yeti Nano", false),
        InputDevice::new("Steven's AirPods", false),
    ]
}

/// **The stored form is a name, and it comes back as the same name.** The
/// whole point of the type: an index would round-trip too, and would mean a
/// different device after a reboot.
#[test]
fn a_choice_round_trips_through_its_token() {
    for choice in [
        InputChoice::SystemDefault,
        InputChoice::named("Yeti Nano"),
        // A name with the punctuation a real device carries. Nothing escapes
        // it, so nothing can un-escape it wrongly.
        InputChoice::named("Steven's AirPods (Hands-Free)"),
    ] {
        assert_eq!(
            InputChoice::parse(choice.token()),
            choice,
            "{choice:?} did not survive its own token"
        );
    }
}

/// A file with nothing in that row, or a token from a build that meant
/// something else, is *follow the OS* — never a refusal to open a microphone.
#[test]
fn an_unwritten_or_unreadable_token_is_the_system_default() {
    for token in ["", "   ", SYSTEM_DEFAULT, "System-Default"] {
        assert_eq!(
            InputChoice::parse(token),
            InputChoice::SystemDefault,
            "{token:?}"
        );
    }
}

/// The token is not the label: it is a fixed ASCII word, so a settings file
/// written on a Japanese installation reads the same on an English one.
#[test]
fn the_system_default_token_is_not_a_label() {
    assert_eq!(InputChoice::SystemDefault.token(), "system-default");
    assert!(
        !SYSTEM_DEFAULT.contains(' '),
        "a stored token must not read like a sentence somebody would translate"
    );
}

#[test]
fn the_system_default_resolves_to_the_hosts_own_choice() {
    let resolved = resolve(&InputChoice::SystemDefault, &three());
    assert_eq!(resolved, InputResolution::SystemDefault);
    assert_eq!(resolved.device(), None);
    assert_eq!(resolved.missing(), None);
}

#[test]
fn a_named_device_that_is_plugged_in_is_opened() {
    let resolved = resolve(&InputChoice::named("Yeti Nano"), &three());
    assert_eq!(resolved, InputResolution::Device("Yeti Nano".into()));
    assert_eq!(resolved.device(), Some("Yeti Nano"));
}

/// **A host may re-case a device string between releases**, and a
/// capitalisation is not a different microphone. The name that comes back is
/// the *host's* spelling, so what is opened is what was found.
#[test]
fn a_recased_name_still_finds_its_device() {
    let resolved = resolve(&InputChoice::named("yeti NANO"), &three());
    assert_eq!(resolved.device(), Some("Yeti Nano"));
}

/// The one that matters: a headset that is asleep must not be an error, must
/// not be silence, and must not be mistaken for never having chosen.
#[test]
fn a_remembered_device_that_is_gone_falls_back_and_says_which() {
    let resolved = resolve(&InputChoice::named("Steven's AirPods"), &[]);
    assert_eq!(resolved.device(), None, "it opens the host's default");
    assert_eq!(
        resolved.missing(),
        Some("Steven's AirPods"),
        "and it still knows what was asked for, so a surface can say so"
    );
    assert_ne!(
        resolved,
        InputResolution::SystemDefault,
        "falling back is not the same answer as choosing to follow the OS"
    );
}

/// An exact match wins over a case-insensitive one, so two devices differing
/// only in case cannot make the choice ambiguous.
#[test]
fn an_exact_match_wins() {
    let devices = vec![
        InputDevice::new("MIC", false),
        InputDevice::new("mic", false),
    ];
    assert_eq!(
        resolve(&InputChoice::named("mic"), &devices).device(),
        Some("mic")
    );
}

/// A choice is `serde`-carried on the wire, so it has to survive JSON as well
/// as its own token.
#[test]
fn a_choice_survives_the_wire() {
    for choice in [InputChoice::SystemDefault, InputChoice::named("Yeti Nano")] {
        let json = serde_json::to_string(&choice).expect("a choice serialises");
        let back: InputChoice = serde_json::from_str(&json).expect("a choice deserialises");
        assert_eq!(back, choice, "{json}");
    }
}

#[test]
fn a_device_survives_the_wire() {
    let device = InputDevice::new("Yeti Nano", true);
    let json = serde_json::to_string(&device).expect("a device serialises");
    let back: InputDevice = serde_json::from_str(&json).expect("a device deserialises");
    assert_eq!(back, device);
}
