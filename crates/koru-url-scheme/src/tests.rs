// SPDX-License-Identifier: MPL-2.0

//! What would be written, and whose registration a command line describes.
//!
//! **Nothing here touches `Software\Classes\koru`.** A developer running the
//! suite has a browser pointed at whatever is registered, and a test that
//! rewrote it would change which build their own links open. The write path is
//! exercised under a root of its own; everything else is asserted against the
//! two pure functions, which is where all the judgement lives anyway.

use super::*;
use std::path::PathBuf;

fn exe() -> PathBuf {
    PathBuf::from("C:\\Program Files\\Koru\\koru-agent.exe")
}

#[test]
fn the_entries_are_the_four_keys_windows_needs() {
    let entries = entries(&exe());
    let names: Vec<_> = entries
        .iter()
        .map(|(subkey, value, _)| (subkey.as_str(), *value))
        .collect();

    assert_eq!(
        names,
        vec![
            ("koru", ""),
            ("koru", "URL Protocol"),
            ("koru\\DefaultIcon", ""),
            ("koru\\shell\\open\\command", ""),
        ]
    );
}

/// `URL Protocol` is a flag: Windows looks for the value's *presence*, and
/// ignores what is in it. Putting something there would imply it mattered.
#[test]
fn the_url_protocol_flag_is_present_and_empty() {
    let entries = entries(&exe());
    let (_, _, data) = entries
        .iter()
        .find(|(_, value, _)| *value == "URL Protocol")
        .expect("the flag");
    assert!(data.is_empty());
}

/// The exe path usually has a space in it, and `%1` arrives holding whatever a
/// web page put in the URL. Both are quoted; the parser refuses a quote as
/// well, and the two halves are meant to be read together.
#[test]
fn the_command_quotes_the_executable_and_the_placeholder() {
    let entries = entries(&exe());
    let (_, _, command) = entries
        .iter()
        .find(|(subkey, _, _)| subkey == "koru\\shell\\open\\command")
        .expect("the command");

    assert_eq!(
        command,
        "\"C:\\Program Files\\Koru\\koru-agent.exe\" open-url \"%1\""
    );
}

#[test]
fn an_absent_command_reads_as_not_registered() {
    assert_eq!(classify(None, &exe()), Registration::Absent);
    assert_eq!(classify(Some("   "), &exe()), Registration::Absent);
}

#[test]
fn a_command_naming_this_exe_reads_as_registered_to_this_build() {
    let command = format!("\"{}\" open-url \"%1\"", exe().display());
    assert_eq!(classify(Some(&command), &exe()), Registration::ThisBuild);
}

/// Every channel installs side by side, so this is the ordinary state rather
/// than an error -- and the tray is expected to name the build that owns it.
/// Windows path semantics: `\\` is a separator only there, and these
/// commands only ever come out of the Windows registry.
#[cfg(windows)]
#[test]
fn a_command_naming_another_koru_reads_as_another_build() {
    let other = "C:\\Koru-nightly\\koru-agent.exe";
    let command = format!("\"{other}\" open-url \"%1\"");

    assert_eq!(
        classify(Some(&command), &exe()),
        Registration::Other {
            exe: other.to_owned()
        }
    );
}

/// Registering over somebody else is allowed -- last one wins. Deleting their
/// key is not ours to offer.
#[test]
fn a_command_for_a_different_product_reads_as_foreign() {
    let command = "\"C:\\Other\\thing.exe\" \"%1\"";
    assert!(matches!(
        classify(Some(command), &exe()),
        Registration::Foreign { .. }
    ));
}

/// Not what we write, but what a person editing the registry by hand leaves
/// behind. Reading it is cheaper than being confused by it later.
/// Windows path semantics: `\\` is a separator only there, and these
/// commands only ever come out of the Windows registry.
#[cfg(windows)]
#[test]
fn an_unquoted_command_is_still_classified() {
    let command = "C:\\Koru-nightly\\koru-agent.exe open-url %1";
    assert!(matches!(
        classify(Some(command), &exe()),
        Registration::Other { .. }
    ));
}

/// Registration state is per user and mutable by anything; a build that was
/// registered and then moved or deleted still has to classify as something.
/// Windows path semantics: `\\` is a separator only there, and these
/// commands only ever come out of the Windows registry.
#[cfg(windows)]
#[test]
fn a_command_naming_a_build_that_is_gone_still_classifies() {
    let command = "\"D:\\gone\\koru-agent.exe\" open-url \"%1\"";
    assert!(matches!(
        classify(Some(command), &exe()),
        Registration::Other { .. }
    ));
}

#[cfg(windows)]
mod windows_only {
    use super::*;

    /// A root of our own, never `Software\Classes`. The suite must not decide
    /// which build the developer's browser opens.
    const TEST_ROOT: &str = "Software\\Koru\\Test\\Classes";
    /// ...and the whole of it goes away again. A test that leaves keys behind
    /// is litter in somebody's registry, which is worse than litter in a temp
    /// directory: nothing ever sweeps it up.
    const TEST_TRUNK: (&str, &str) = ("Software\\Koru", "Test");

    /// The registry is process-wide shared state and cargo runs tests in
    /// threads, so these serialise against each other.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn writing_and_reading_back_under_a_test_root_round_trips() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _ = platform::delete_tree(TEST_ROOT, "koru");

        platform::write_all(TEST_ROOT, &entries(&exe())).expect("the tree was created");
        let command = platform::read(TEST_ROOT, "koru\\shell\\open\\command", "");

        assert_eq!(
            classify(command.as_deref(), &exe()),
            Registration::ThisBuild,
            "what was written reads back as this build"
        );

        platform::delete_tree(TEST_ROOT, "koru").expect("and goes away again");
        assert_eq!(
            platform::read(TEST_ROOT, "koru\\shell\\open\\command", ""),
            None
        );
        let _ = platform::delete_tree(TEST_TRUNK.0, TEST_TRUNK.1);
    }

    /// The caller asked for it to be gone, and it is.
    #[test]
    fn deleting_something_absent_succeeds() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        assert!(platform::delete_tree(TEST_ROOT, "koru-never-written").is_ok());
        let _ = platform::delete_tree(TEST_TRUNK.0, TEST_TRUNK.1);
    }
}
