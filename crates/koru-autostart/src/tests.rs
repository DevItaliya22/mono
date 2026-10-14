// SPDX-License-Identifier: MPL-2.0

//! These write to the real registry, under a value name **no product uses**.
//!
//! That is deliberate and it is safe: `HKEY_CURRENT_USER` needs no elevation,
//! the key already exists on every Windows install, and a stray value under a
//! name with a test marker in it starts nothing. Mocking the registry would
//! have meant a trait, two implementations and a test that proves the mock
//! agrees with itself — while the thing most likely to be wrong here is the
//! *encoding*, which only the real API can judge.

use super::*;

/// A name nothing ships under, so a failed test cannot leave a real entry.
#[cfg(not(target_os = "macos"))]
const TEST_NAME: &str = "Koru autostart test — safe to delete";

#[cfg(windows)]
#[test]
fn an_entry_can_be_written_read_back_and_removed() {
    // The whole round trip in one test, because the steps are only meaningful
    // in sequence and three tests would race each other over one value name.
    let command = quoted(std::path::Path::new(
        r"C:\Program Files\Koru\koru-store-server.exe",
    ));

    let _ = clear(TEST_NAME);
    assert!(!is_set(TEST_NAME), "the name was already in use");

    set(TEST_NAME, &command).expect("write");
    assert!(is_set(TEST_NAME));
    // Read back exactly, with no trailing NUL: the stored length includes the
    // terminator, and a string carrying one compares unequal to everything it
    // looks identical to.
    assert_eq!(command_of(TEST_NAME).as_deref(), Some(command.as_str()));

    clear(TEST_NAME).expect("clear");
    assert!(!is_set(TEST_NAME));
    assert_eq!(command_of(TEST_NAME), None);
}

#[cfg(windows)]
#[test]
fn clearing_something_that_is_not_there_succeeds() {
    // Disabling what is already disabled is not a failure. A caller forced to
    // tell the two apart would only ever discard the difference.
    let name = "Koru autostart test — never written";
    assert!(!is_set(name));
    clear(name).expect("clearing an absent entry is not an error");
}

#[cfg(windows)]
#[test]
fn a_path_with_spaces_survives_being_quoted() {
    // The normal case on Windows, and the one that breaks silently: an
    // unquoted `C:\Program Files\...` is read as a program called
    // `C:\Program` with an argument.
    let name = "Koru autostart test — spaces";
    let command = quoted(std::path::Path::new(r"C:\Program Files\Koru\koru.exe"));
    assert!(
        command.starts_with('"') && command.ends_with('"'),
        "{command}"
    );

    let _ = clear(name);
    set(name, &command).expect("write");
    assert_eq!(command_of(name).as_deref(), Some(command.as_str()));
    clear(name).expect("clear");
}

#[cfg(windows)]
#[test]
fn two_names_do_not_disturb_each_other() {
    // The property the whole "each program owns its own value name" rule rests
    // on: Koru and the Koru Store must be able to be enabled independently.
    let (a, b) = ("Koru autostart test — one", "Koru autostart test — two");
    let _ = clear(a);
    let _ = clear(b);

    set(a, "\"a.exe\"").expect("a");
    set(b, "\"b.exe\"").expect("b");
    assert_eq!(command_of(a).as_deref(), Some("\"a.exe\""));
    assert_eq!(command_of(b).as_deref(), Some("\"b.exe\""));

    clear(a).expect("clear a");
    assert!(!is_set(a));
    assert!(is_set(b), "clearing one removed the other");

    clear(b).expect("clear b");
}

#[cfg(not(any(windows, target_os = "macos")))]
#[test]
fn every_call_is_unsupported_rather_than_a_silent_no_op() {
    // A caller offering the setting has to be able to say why it did not take.
    assert_eq!(
        set(TEST_NAME, "koru").unwrap_err().kind(),
        std::io::ErrorKind::Unsupported
    );
    assert_eq!(
        clear(TEST_NAME).unwrap_err().kind(),
        std::io::ErrorKind::Unsupported
    );
    assert!(!is_set(TEST_NAME));
    assert_eq!(command_of(TEST_NAME), None);
}

#[test]
fn quoting_wraps_the_path_and_nothing_else() {
    // Runs everywhere: it is string handling, not registry handling.
    let quoted = quoted(std::path::Path::new("koru-store-server.exe"));
    assert_eq!(quoted, "\"koru-store-server.exe\"");
}

// ------------------------------------------------------ one entry per channel
//
// Pure, like `quoting_wraps_the_path_and_nothing_else` above and for the same
// reason: naming is string handling, and nothing here needs the registry to
// judge it. They go through `qualify` rather than `channel_entry` because a
// `cargo test` binary can only ever be one segment — `internal-debug` — so the
// public function cannot be asked about stable, which is the case that carries
// the compatibility promise.

#[test]
fn stable_keeps_the_bare_name() {
    // Every stable build shipped so far wrote `Koru`. Qualifying it would
    // orphan that entry: the checkbox would read no while something still
    // started at sign-in.
    assert_eq!(qualify("Koru", "stable"), "Koru");
    assert_eq!(qualify("Koru Store", "stable"), "Koru Store");
}

#[test]
fn every_other_channel_is_suffixed() {
    // Including the two the `ReleaseChannel` enum cannot tell apart: a debug
    // and a release build of the same tree are different binaries at different
    // paths, so they are different entries.
    assert_eq!(qualify("Koru", "beta"), "Koru (beta)");
    assert_eq!(qualify("Koru", "nightly"), "Koru (nightly)");
    assert_eq!(
        qualify("Koru Store", "internal-debug"),
        "Koru Store (internal-debug)"
    );
    assert_eq!(
        qualify("Koru Store", "internal-release"),
        "Koru Store (internal-release)"
    );
}

#[test]
fn two_programs_on_one_channel_still_differ() {
    // The original rule survives the new one: qualifying by channel must not
    // collapse `Koru` and `Koru Store` into one value on any segment.
    for segment in koru_channel::SEGMENTS {
        assert_ne!(
            qualify("Koru", segment),
            qualify("Koru Store", segment),
            "{segment}"
        );
    }
}

#[test]
fn no_two_channels_share_a_name() {
    // The property the whole change exists for, asserted across the five
    // segments rather than the one this test binary was built as.
    let names: Vec<String> = koru_channel::SEGMENTS
        .iter()
        .map(|segment| qualify("Koru", segment))
        .collect();
    for (i, one) in names.iter().enumerate() {
        for other in &names[i + 1..] {
            assert_ne!(one, other);
        }
    }
}

#[test]
fn the_running_build_agrees_with_the_rule() {
    // The one thing `qualify` cannot prove: that `channel_entry` asks the
    // running build rather than something else.
    assert_eq!(
        channel_entry("Koru"),
        qualify("Koru", koru_channel::segment())
    );
}

/// The macOS pieces that are pure — the label mapping and the plist
/// round-trip — pinned without touching the real `~/Library/LaunchAgents`,
/// which no test may write ([AGENTS.md]: tests must not touch disk).
#[cfg(target_os = "macos")]
mod macos {
    #[test]
    fn the_label_is_deterministic_reverse_dns() {
        assert_eq!(
            crate::platform::label("Koru (beta)"),
            "do.koru.autostart.koru-beta"
        );
        assert_eq!(crate::platform::label("Koru"), "do.koru.autostart.koru");
        assert_eq!(
            crate::platform::label("Koru Store (internal-debug)"),
            "do.koru.autostart.koru-store-internal-debug"
        );
    }

    #[test]
    fn a_command_round_trips_through_the_plist_escaping_included() {
        let command = "\"/Applications/Koru.app/Contents/MacOS/koru\" --background <&>";
        let plist = crate::platform::plist("Koru (beta)", command);
        assert!(plist.contains("do.koru.autostart.koru-beta"));
        assert!(plist.contains("<key>RunAtLoad</key><true/>"));
        assert!(plist.contains("/bin/sh"));
        assert_eq!(
            crate::platform::extract_command(&plist).as_deref(),
            Some(command)
        );
    }
}
