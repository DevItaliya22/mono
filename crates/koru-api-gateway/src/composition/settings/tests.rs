// SPDX-License-Identifier: MPL-2.0

use super::*;

/// **A sibling, never a `PATH` lookup.** The binaries are built, signed
/// and shipped as one unit; the first `koru.exe` on somebody's `PATH` could
/// belong to any channel, and a startup entry naming it would launch a
/// different Koru at every sign-in. Same rule, same shape, as the Agent's
/// `the_supervised_binary_is_a_sibling_not_a_path_lookup`.
#[cfg(windows)]
#[test]
fn the_startup_entry_names_the_agent_beside_this_executable() {
    let directory = std::path::Path::new("/opt/koru/nightly");
    let named = agent_executable_beside(directory);
    assert_eq!(named.parent(), Some(directory));
    assert_eq!(
        named.file_stem().and_then(|name| name.to_str()),
        Some("koru-agent")
    );
}

/// **Not the Gateway's own executable**, which is what
/// `std::env::current_exe()` answers inside this process — an entry naming
/// it would start a service at sign-in and no window at all. That is the
/// whole reason `StartOnLoginPort` is a port rather than a call the
/// authority makes itself.
#[cfg(windows)]
#[test]
fn the_startup_entry_never_names_the_gateway() {
    let exe = std::env::current_exe().expect("test executable");
    let directory = exe.parent().expect("test executable has a directory");
    assert_ne!(agent_executable_beside(directory), exe);
}

#[cfg(windows)]
#[test]
fn startup_names_only_the_silent_agent_command() {
    let named = agent_executable_beside(std::path::Path::new("/opt/koru/nightly"));
    let command = startup_command(&named);
    assert!(command.contains("koru-agent"));
    assert!(!command.contains(koru_autostart::KORU_BACKGROUND_ARGUMENT));
    assert!(!command.contains("koru.exe"));
}

#[cfg(not(windows))]
#[test]
fn non_windows_preserves_the_existing_background_product_command() {
    let named = agent_executable_beside(std::path::Path::new("/opt/koru/nightly"));
    assert_eq!(
        startup_command(&named),
        r#""/opt/koru/nightly/koru" --background"#
    );
}

#[test]
fn startup_refusal_never_exposes_a_candidate_path() {
    let error = startup_action_unavailable();
    assert_eq!(error, "the startup action is unavailable");
    assert!(!error.contains(std::path::MAIN_SEPARATOR));
}

#[test]
fn only_the_ordinary_channel_authority_can_change_machine_startup() {
    let ordinary = std::path::Path::new("C:/Koru/stable");
    assert!(owns_machine_startup(ordinary, Some(ordinary)));
    assert!(!owns_machine_startup(
        std::path::Path::new("C:/rehearsal/root"),
        Some(ordinary)
    ));
    assert!(!owns_machine_startup(ordinary, None));
}

/// **Koru's entry, qualified by channel, and not the Store's.** Two
/// programs writing one value name would mean enabling either one silently
/// disabled the other; five channels install side by side, so the base name
/// is never the value name.
#[test]
fn the_entry_is_korus_own_and_channel_qualified() {
    assert_eq!(koru_autostart::KORU_ENTRY, "Koru");
    let entry = koru_autostart::channel_entry(koru_autostart::KORU_ENTRY);
    assert!(entry.starts_with("Koru"), "{entry}");
    assert!(!entry.contains("Store"), "{entry}");
    if koru_channel::segment() != "stable" {
        assert!(
            entry.contains(koru_channel::segment()),
            "a nightly beside a stable must not overwrite its entry: {entry}"
        );
    }
}

/// **A rooted run never resolves the installed channel's settings.** The
/// rehearsal root gets `<root>/settings`, which is the guarantee that keeps a
/// test from replacing somebody's preferences.
#[test]
fn a_rooted_gateway_keeps_its_settings_under_its_own_root() {
    let rooted = std::path::Path::new("C:/rehearsal/root");
    // A path that is not the ordinary channel root, whatever that is on this
    // machine, always resolves under itself.
    if koru_channel::channel_root().as_deref() == Some(rooted) {
        return;
    }
    let paths = settings_paths(rooted).expect("a rooted run always resolves");
    assert!(
        paths
            .preferences_file()
            .starts_with(rooted.join("settings")),
        "{}",
        paths.preferences_file().display()
    );
}
