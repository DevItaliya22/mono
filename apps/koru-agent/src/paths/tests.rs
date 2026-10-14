// SPDX-License-Identifier: MPL-2.0

//! **"It did not touch my machine" is not something you notice passing.**
//!
//! Which is why each of the three refusals is asserted rather than trusted to
//! the reading of one `if`.

use super::*;
use std::str::FromStr as _;

use koru_session_protocol::{AttachmentId, SessionId};

fn rooted() -> Paths {
    Paths::for_root(PathBuf::from("/tmp/koru-agent-test"))
}

fn channel() -> Paths {
    Paths::for_channel(PathBuf::from("/some/channel/root"))
}

#[test]
fn an_explicit_root_owns_no_default_machine_integration() {
    // Headless roots use this answer and claim nothing. The interactive
    // launcher separately opts into its tray and OS-arbitrated global input;
    // the one Run-key value remains unavailable to every rooted launch.
    assert!(!rooted().owns_the_machine());
}

#[test]
fn the_channels_own_root_owns_the_machine() {
    // The ordinary launch, which is the whole point of the agent existing.
    assert!(channel().owns_the_machine());
}

#[test]
fn an_explicit_root_keeps_its_device_identity_under_it() {
    // A gate must not mint a device id into a developer's real profile — and
    // `device.json` is an identity, which ADR-39 says is never derived from a
    // path and never regenerated once written.
    let paths = rooted();
    assert!(paths.device_file().starts_with(paths.root()));
    assert!(paths.device_file().ends_with("agent/device.json"));
}

#[test]
fn reminder_ledger_is_content_free_device_state_under_the_selected_agent_dir() {
    let paths = rooted();
    assert_eq!(
        paths.reminder_ledger_file(),
        paths.agent_dir().join("reminder-delivery-v1.json")
    );
}

#[test]
fn the_channels_own_root_keeps_its_device_identity_under_it_too() {
    // Same place, different root. The redirection is not a special case; the
    // *root* is what differs.
    let paths = channel();
    assert!(paths.device_file().starts_with(paths.root()));
}

#[test]
fn routing_diagnostics_stay_in_the_agents_directory() {
    let paths = rooted();
    assert_eq!(paths.routing_log_label(), "agent/routing.log");
    assert_eq!(
        paths.routing_log_file(),
        paths.agent_dir().join("routing.log")
    );
}

#[test]
fn an_explicit_root_redirects_window_geometry_too() {
    let paths = rooted();
    assert_eq!(
        paths.window_geometry_file(),
        Some(paths.agent_dir().join("windows.conf"))
    );
}

/// The profile global search reopens on is machine-local and shared with Koru,
/// so a gate must not be able to change the one a person is using.
#[test]
fn an_explicit_root_redirects_the_remembered_search_profile_too() {
    let paths = rooted();
    assert_eq!(
        paths.search_state_file(),
        Some(paths.agent_dir().join("search.conf"))
    );
    // The ordinary launch reads the file Koru itself writes — the whole point
    // of remembering it at all.
    assert_eq!(
        channel().search_state_file(),
        koru_window::search_state_path()
    );
}

#[test]
fn only_an_explicit_root_redirects_the_preferences() {
    // `None` means "wherever `DesktopPreferences` puts it", which is the file a
    // person's real Koru reads. A test that rewrote the global shortcut there
    // would break the machine it ran on.
    assert!(channel().preferences_file().is_none());
    let paths = rooted();
    let file = paths.preferences_file().expect("redirected under a root");
    assert!(file.starts_with(paths.root()));
}

#[test]
fn reminder_delivery_choice_is_always_agent_owned_and_root_isolated() {
    for paths in [channel(), rooted()] {
        assert_eq!(
            paths.reminder_preferences_file(),
            paths.agent_dir().join("reminder-preferences-v1.json")
        );
        assert!(paths.reminder_preferences_file().starts_with(paths.root()));
    }
}

#[test]
fn only_an_explicit_root_is_passed_on_to_the_store() {
    // `None` is what `Launcher` and `Endpoint::for_service_root` take to mean
    // "the channel's own", which the store works out for itself — and which has
    // to produce the same endpoint name a plain client dials.
    assert!(channel().explicit_root().is_none());
    assert_eq!(rooted().explicit_root(), Some(rooted().root()));
}

#[test]
fn the_store_under_an_explicit_root_is_inside_it_and_not_the_root_itself() {
    // **What the lifecycle gate caught.** The root was handed to the store
    // unchanged, so a rooted store wrote `catalog.db` and `workspaces/` loose in
    // the same directory the agent keeps `agent/device.json` in — and "clear the
    // store" there would have taken the device identity with it. An explicit
    // root is a *channel* root, so the two are siblings exactly as they are by
    // default.
    let paths = rooted();
    let store = koru_channel::store_root_under(paths.root());
    assert_eq!(paths.store_dir(), store);
    assert_eq!(store.parent(), Some(paths.root()));
    assert_eq!(paths.agent_dir().parent(), store.parent());
    assert!(
        !paths.agent_dir().starts_with(&store),
        "the agent's directory is inside the store's: {}",
        paths.agent_dir().display()
    );
}

#[test]
fn a_rooted_trays_local_and_shared_locations_are_isolated_under_the_agent() {
    let paths = rooted();
    assert_eq!(paths.local_data_dir(), Some(paths.agent_dir()));
    assert_eq!(paths.shared_data_dir(), Some(paths.agent_dir()));
}

#[test]
fn the_agents_directory_is_beside_the_stores_and_not_inside_it() {
    // **The property, not the arithmetic.** The first version of this asserted
    // `agent_dir().parent() == root()`, which is true by construction and was
    // true while the default root *was* the store's — so the agent's device
    // identity sat in `Koru/<channel>/store/agent/`, where a repair that clears
    // the store would have taken it, and the test that named this property
    // passed throughout.
    let Some(store) = koru_channel::store_root() else {
        return; // no local data directory on this machine
    };
    let Some(channel) = koru_channel::channel_root() else {
        return;
    };
    let paths = Paths::for_channel(channel);
    assert!(paths.agent_dir().ends_with("agent"));
    assert!(
        !paths.agent_dir().starts_with(&store),
        "the agent's directory is inside the store's: {}",
        paths.agent_dir().display()
    );
    // Siblings: one can be deleted without the other noticing.
    assert_eq!(paths.agent_dir().parent(), store.parent());
}

/// Consent is machine-local and shared with Koru, so a gate must not be able to
/// agree to a download on the machine owner's behalf — or to read an answer
/// they gave for real and pass because of it.
#[test]
fn an_explicit_root_redirects_the_voice_consent_record_too() {
    let paths = rooted();
    assert_eq!(
        paths.voice_consent_file(),
        Some(paths.agent_dir().join("voice-consent.conf"))
    );
    assert_eq!(
        channel().voice_consent_file(),
        koru_window::voice_consent_path()
    );
}

#[test]
fn session_storage_has_one_rooted_layout_and_atomic_siblings() {
    let paths = rooted();
    let sessions = paths.session_paths();
    let session = SessionId::mint();
    let attachment = AttachmentId::mint();

    assert_eq!(sessions.directory(), paths.agent_dir().join("sessions"));
    assert_eq!(
        sessions.profiles_file(),
        paths.agent_dir().join("sessions/profiles.conf")
    );
    assert_eq!(
        sessions.attachments_directory(),
        paths.agent_dir().join("sessions/attachments")
    );

    let profile_write = sessions.profiles_atomic_path();
    assert_eq!(profile_write.destination(), sessions.profiles_file());
    assert_eq!(
        profile_write.staging(),
        sessions.directory().join("profiles.conf.part")
    );

    let attachment_write =
        sessions.attachment_atomic_path(session, attachment, VerifiedAttachmentExtension::Png);
    assert_eq!(
        attachment_write.staging().parent(),
        attachment_write.destination().parent(),
        "an atomic replacement must be staged on the destination filesystem"
    );
    assert!(
        attachment_write
            .staging()
            .ends_with(format!("{attachment}.png.part"))
    );
}

#[test]
fn adversarial_identifiers_cannot_become_attachment_paths() {
    for value in [
        "../outside",
        "..\\outside",
        "/absolute",
        "C:\\outside",
        "file.png:stream",
        "6BA7B810-9DAD-41D1-80B4-00C04FD430C8",
        "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
    ] {
        assert!(
            SessionId::from_str(value).is_err(),
            "session accepted {value:?}"
        );
        assert!(
            AttachmentId::from_str(value).is_err(),
            "attachment accepted {value:?}"
        );
    }

    let sessions = rooted().session_paths();
    let file = sessions.attachment_file(
        SessionId::mint(),
        AttachmentId::mint(),
        VerifiedAttachmentExtension::Webp,
    );
    assert!(file.starts_with(sessions.attachments_directory()));
    assert_eq!(
        file.extension().and_then(|value| value.to_str()),
        Some("webp")
    );
}

#[test]
fn cleanup_requires_exclusive_root_authority_and_a_bound_session_endpoint() {
    for (exclusive_root, endpoint_bound, expected) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (true, true, true),
    ] {
        let predicate = CleanupPredicate::new(exclusive_root, endpoint_bound);
        assert_eq!(predicate.satisfied(), expected);
        assert_eq!(predicate.authority().is_some(), expected);
    }
}

#[test]
fn cleanup_is_confined_to_abandoned_attachment_entries() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::for_root(root.path().to_owned());
    let sessions = paths.session_paths();
    let abandoned = sessions.attachment_directory(SessionId::mint());
    std::fs::create_dir_all(&abandoned).unwrap();
    std::fs::write(abandoned.join("payload.png"), b"discard me").unwrap();

    let profiles = sessions.profiles_file();
    std::fs::create_dir_all(profiles.parent().unwrap()).unwrap();
    std::fs::write(&profiles, b"keep profiles").unwrap();
    let outside = root.path().join("outside-sentinel");
    std::fs::write(&outside, b"keep outside").unwrap();

    let authority = CleanupPredicate::new(true, true).authority().unwrap();
    let report = sessions.cleanup_abandoned_attachments(authority).unwrap();

    assert_eq!(report.removed_entries, 1);
    assert!(sessions.attachments_directory().is_dir());
    assert_eq!(
        std::fs::read_dir(sessions.attachments_directory())
            .unwrap()
            .count(),
        0
    );
    assert_eq!(std::fs::read(&profiles).unwrap(), b"keep profiles");
    assert_eq!(std::fs::read(&outside).unwrap(), b"keep outside");
}

#[cfg(unix)]
#[test]
fn cleanup_removes_a_nested_symlink_without_following_it() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let sentinel = outside.path().join("sentinel");
    std::fs::write(&sentinel, b"must survive").unwrap();

    let sessions = Paths::for_root(root.path().to_owned()).session_paths();
    std::fs::create_dir_all(sessions.attachments_directory()).unwrap();
    symlink(
        outside.path(),
        sessions.attachments_directory().join("escape"),
    )
    .unwrap();

    let authority = CleanupPredicate::new(true, true).authority().unwrap();
    sessions.cleanup_abandoned_attachments(authority).unwrap();
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"must survive");
}

#[cfg(windows)]
#[test]
fn cleanup_removes_a_nested_reparse_point_without_following_it_when_supported() {
    use std::os::windows::fs::symlink_dir;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let sentinel = outside.path().join("sentinel");
    std::fs::write(&sentinel, b"must survive").unwrap();

    let sessions = Paths::for_root(root.path().to_owned()).session_paths();
    std::fs::create_dir_all(sessions.attachments_directory()).unwrap();
    let link = sessions.attachments_directory().join("escape");
    if let Err(error) = symlink_dir(outside.path(), &link) {
        eprintln!("skipped reparse-point assertion: {error}");
        return;
    }

    let authority = CleanupPredicate::new(true, true).authority().unwrap();
    sessions.cleanup_abandoned_attachments(authority).unwrap();
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"must survive");
}

#[cfg(unix)]
#[test]
fn an_attachment_root_symlink_is_refused_instead_of_followed() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let sentinel = outside.path().join("sentinel");
    std::fs::write(&sentinel, b"must survive").unwrap();
    let sessions = Paths::for_root(root.path().to_owned()).session_paths();
    std::fs::create_dir_all(sessions.directory()).unwrap();
    symlink(outside.path(), sessions.attachments_directory()).unwrap();

    let authority = CleanupPredicate::new(true, true).authority().unwrap();
    let error = sessions
        .cleanup_abandoned_attachments(authority)
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"must survive");
}

#[cfg(windows)]
#[test]
fn an_attachment_root_reparse_point_is_refused_when_supported() {
    use std::os::windows::fs::symlink_dir;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let sentinel = outside.path().join("sentinel");
    std::fs::write(&sentinel, b"must survive").unwrap();
    let sessions = Paths::for_root(root.path().to_owned()).session_paths();
    std::fs::create_dir_all(sessions.directory()).unwrap();
    if let Err(error) = symlink_dir(outside.path(), sessions.attachments_directory()) {
        eprintln!("skipped attachment-root reparse assertion: {error}");
        return;
    }

    let authority = CleanupPredicate::new(true, true).authority().unwrap();
    let error = sessions
        .cleanup_abandoned_attachments(authority)
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"must survive");
}
