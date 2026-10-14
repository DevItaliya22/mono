// SPDX-License-Identifier: MPL-2.0

use std::collections::HashSet;

use super::*;

#[test]
fn registration_ids_are_bounded_ascii_and_content_free() {
    assert!(RegistrationId::new("occurrence-01").is_ok());
    assert!(RegistrationId::new("A.b_C-09").is_ok());
    assert!(RegistrationId::new("").is_err());
    assert!(RegistrationId::new("é").is_err());
    assert!(RegistrationId::new("contains space").is_err());
    assert!(RegistrationId::new("line\nbreak").is_err());
    assert!(RegistrationId::new("colon:not-portable").is_err());
    assert!(RegistrationId::new("x".repeat(64)).is_ok());
    assert!(RegistrationId::new("x".repeat(65)).is_err());
}

#[test]
fn activation_wire_shape_contains_only_version_registration_and_action() {
    let activation = Activation::new(RegistrationId::new("r-44").unwrap(), Action::Handled);
    let encoded = serde_json::to_string(&activation).unwrap();

    assert_eq!(
        encoded,
        r#"{"schema_version":1,"registration":"r-44","action":"handled"}"#
    );
    assert!(activation.validate().is_ok());
}

#[test]
fn activation_rejects_unknown_versions_and_fields() {
    let future = r#"{"schema_version":2,"registration":"r-44","action":"open"}"#;
    let future: Activation = serde_json::from_str(future).unwrap();
    assert_eq!(future.validate(), Err(UnsupportedActivationVersion(2)));

    let content_leak =
        r#"{"schema_version":1,"registration":"r-44","action":"open","title":"secret"}"#;
    assert!(serde_json::from_str::<Activation>(content_leak).is_err());

    let invalid_registration =
        r#"{"schema_version":1,"registration":"line\nbreak","action":"open"}"#;
    assert!(serde_json::from_str::<Activation>(invalid_registration).is_err());
}

#[test]
fn opaque_activation_round_trips_without_content_or_shell_metacharacters() {
    let activation = Activation::new(RegistrationId::new("r-44").unwrap(), Action::Snooze);
    let opaque = activation.encode_opaque();
    assert!(
        opaque
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    );
    assert_eq!(Activation::decode_opaque(&opaque).unwrap(), activation);
    assert!(!opaque.contains("title"));
    assert!(!opaque.contains("profile"));
    assert!(Activation::decode_opaque("").is_err());
    assert!(Activation::decode_opaque(&"A".repeat(257)).is_err());
}

#[test]
fn selected_roots_get_distinct_content_free_native_namespaces() {
    let registration = RegistrationId::new("registration-1").unwrap();
    let first = NotificationNamespace::from_stable_key("c:/test/root-a");
    let same = NotificationNamespace::from_stable_key("c:/test/root-a");
    let second = NotificationNamespace::from_stable_key("c:/test/root-b");
    assert_eq!(first, same);
    assert_ne!(first, second);
    assert_ne!(
        first.native_id(&registration),
        second.native_id(&registration)
    );
    assert!(!first.native_id(&registration).contains("test/root"));
}

#[test]
fn mac_activation_accepts_only_owned_ids_and_known_actions() {
    let launch = super::mac_owner::activation_from_mac_response(
        "koru.0123456789abcdef.registration-1",
        super::mac_owner::MAC_ACTION_HANDLED,
        MacRootRoute::Root("/private/root".into()),
    )
    .unwrap();
    assert_eq!(launch.activation.action, Action::Handled);
    assert_eq!(launch.activation.registration.as_str(), "registration-1");
    assert_eq!(launch.explicit_root.as_deref(), Some("/private/root"));
    assert!(
        super::mac_owner::activation_from_mac_response(
            "foreign.0123456789abcdef.registration-1",
            super::mac_owner::MAC_ACTION_OPEN,
            MacRootRoute::Default,
        )
        .is_none()
    );
    assert!(
        super::mac_owner::activation_from_mac_response(
            "koru.0123456789abcdef.registration-1",
            "com.apple.UNNotificationDismissActionIdentifier",
            MacRootRoute::Default,
        )
        .is_none()
    );
}

#[test]
fn mac_native_ids_require_the_full_portable_registration() {
    for invalid in [
        "koru.0123456789abcdef",
        "koru.0123456789abcdef.",
        "koru.0123456789abcdef.has:colon",
        "koru.0123456789ABCDEF.registration-1",
    ] {
        assert!(namespace_from_native_id(invalid).is_none(), "{invalid}");
    }
    let too_long = format!("koru.0123456789abcdef.{}", "x".repeat(65));
    assert!(namespace_from_native_id(&too_long).is_none());
    assert_eq!(
        namespace_from_native_id("koru.0123456789abcdef.registration.v1"),
        Some("0123456789abcdef")
    );
}

#[test]
fn mac_root_routes_distinguish_default_rooted_missing_and_corrupt() {
    let default = serde_json::to_vec(&MacRootRoute::Default).unwrap();
    assert_eq!(
        decode_mac_root_route(Some(&default)).unwrap(),
        MacRootRoute::Default
    );
    let rooted = MacRootRoute::Root("/private/root".into());
    let rooted_bytes = serde_json::to_vec(&rooted).unwrap();
    assert_eq!(decode_mac_root_route(Some(&rooted_bytes)).unwrap(), rooted);
    assert!(decode_mac_root_route(None).is_err());
    assert!(decode_mac_root_route(Some(b"")).is_err());
    assert!(decode_mac_root_route(Some(br#"{"kind":"root","root":""}"#)).is_err());
}

#[test]
fn mac_root_routes_reject_non_utf8_paths_instead_of_changing_them() {
    #[cfg(windows)]
    let path = {
        use std::os::windows::ffi::OsStringExt as _;
        std::path::PathBuf::from(std::ffi::OsString::from_wide(&[0xd800]))
    };
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStringExt as _;
        std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![0xff]))
    };
    assert!(MacRootRoute::from_explicit_root(Some(&path)).is_err());
}

#[test]
fn mac_owner_protocol_is_bounded_and_does_not_put_content_in_activation() {
    let request = super::mac_owner::MacOwnerRequest::Schedule {
        native_id: "koru.0123456789abcdef.r-44".into(),
        deliver_at_unix_ms: 42,
        content: Content {
            title: "private title".into(),
            body: None,
        },
        sound: Sound::Silent,
        actions: ActionSet::OpenOnly,
        root_route: MacRootRoute::Root("/private/root".into()),
    };
    let bytes = serde_json::to_vec(&request).unwrap();
    assert_eq!(
        std::str::from_utf8(&bytes).unwrap(),
        r#"{"operation":"schedule","native_id":"koru.0123456789abcdef.r-44","deliver_at_unix_ms":42,"content":{"title":"private title","body":null},"sound":"silent","actions":"open_only","root_route":{"kind":"root","root":"/private/root"}}"#
    );
    assert!(bytes.len() < super::mac_owner::MAX_MAC_OWNER_MESSAGE);
    let activation = super::mac_owner::activation_from_mac_response(
        "koru.0123456789abcdef.r-44",
        super::mac_owner::MAC_ACTION_OPEN,
        MacRootRoute::Root("/private/root".into()),
    )
    .unwrap()
    .activation
    .encode_opaque();
    assert!(!activation.contains("private"));
}

#[test]
fn mac_action_sets_select_distinct_stable_categories() {
    assert_eq!(
        super::mac_owner::mac_category(ActionSet::Reminder),
        super::mac_owner::MAC_CATEGORY_REMINDER
    );
    assert_eq!(
        super::mac_owner::mac_category(ActionSet::OpenOnly),
        super::mac_owner::MAC_CATEGORY_OPEN_ONLY
    );
    assert_ne!(
        super::mac_owner::MAC_CATEGORY_REMINDER,
        super::mac_owner::MAC_CATEGORY_OPEN_ONLY
    );
}

#[test]
fn mac_owner_path_is_the_nested_login_item() {
    let executable = std::path::Path::new("/Applications/Koru.app/Contents/MacOS/koru-agent");
    assert_eq!(
        super::mac_owner::mac_owner_executable_from(executable).unwrap(),
        std::path::Path::new(
            "/Applications/Koru.app/Contents/Library/LoginItems/Koru Agent.app/Contents/MacOS/koru-notification-owner"
        )
    );
}

#[test]
fn mac_owner_resolves_the_outer_agent_and_builds_strict_cli_args() {
    let owner = std::path::Path::new(
        "/Applications/Koru.app/Contents/Library/LoginItems/Koru Agent.app/Contents/MacOS/koru-notification-owner",
    );
    assert_eq!(
        mac_agent_executable_from_owner(owner).unwrap(),
        std::path::Path::new("/Applications/Koru.app/Contents/MacOS/koru-agent")
    );
    assert_eq!(
        mac_owner_info_plist_from(owner).unwrap(),
        std::path::Path::new(
            "/Applications/Koru.app/Contents/Library/LoginItems/Koru Agent.app/Contents/Info.plist"
        )
    );
    assert!(
        mac_agent_executable_from_owner(std::path::Path::new(
            "/tmp/Koru Agent.app/Contents/MacOS/other"
        ))
        .is_none()
    );
    let launch = activation_from_mac_response(
        "koru.0123456789abcdef.r-44",
        MAC_ACTION_OPEN,
        MacRootRoute::Root("/private/root".into()),
    )
    .unwrap();
    let args = mac_agent_launch_args(&launch);
    assert_eq!(args[0], "notification-activation");
    assert_eq!(args[2], "--root");
    assert_eq!(args[3], "/private/root");
    assert!(Activation::decode_opaque(args[1].to_str().unwrap()).is_ok());
}

#[test]
fn mac_delivery_support_requires_the_complete_owner_geometry() {
    let absent = super::mac_owner::mac_delivery_support(false, true);
    assert!(!absent.actions);
    assert!(!absent.stable_replacement);
    assert!(!absent.withdrawal);
    assert!(!absent.custom_sound);
    assert!(!absent.activation_while_stopped);

    let installed = super::mac_owner::mac_delivery_support(true, false);
    assert!(installed.actions);
    assert!(installed.stable_replacement);
    assert!(installed.withdrawal);
    assert!(!installed.custom_sound);
    assert!(installed.activation_while_stopped);
}

#[test]
fn mac_route_temp_names_do_not_reuse_a_crashed_process_slot() {
    let first = super::mac_owner::mac_route_temp_name("0123456789abcdef", 41, 99, 0);
    let next_nonce = super::mac_owner::mac_route_temp_name("0123456789abcdef", 41, 99, 1);
    let next_process = super::mac_owner::mac_route_temp_name("0123456789abcdef", 42, 99, 0);
    assert_ne!(first, next_nonce);
    assert_ne!(first, next_process);
    assert!(first.starts_with("0123456789abcdef.part-41-"));
}

#[test]
fn tone_inventory_is_stable_unique_and_complete() {
    assert_eq!(BUNDLED_TONES.len(), 10);
    assert_eq!(
        BUNDLED_TONES.first().map(|asset| asset.key),
        Some("koru-default")
    );
    assert_eq!(BUNDLED_TONES.last().map(|asset| asset.key), Some("urgent"));
    assert_eq!(
        BUNDLED_TONES
            .iter()
            .map(|asset| asset.key)
            .collect::<HashSet<_>>()
            .len(),
        10
    );
    assert_eq!(
        BUNDLED_TONES
            .iter()
            .map(|asset| asset.file)
            .collect::<HashSet<_>>()
            .len(),
        10
    );
}

#[test]
fn installed_assets_are_searched_before_compiled_source_assets() {
    let executable = std::path::Path::new("install/bin/koru-agent.exe");
    let roots = super::sounds::sound_asset_roots_for(Some(executable), None);
    assert_eq!(
        roots[0],
        std::path::Path::new("install/bin/assets/reminders")
    );
    let compiled = roots
        .iter()
        .position(|root| root.ends_with("../../assets/reminders"))
        .unwrap();
    assert_eq!(compiled, roots.len() - 1);
}

macro_rules! assert_tone {
    ($file:literal) => {{
        let duration =
            validate_notification_wav(include_bytes!(concat!("../../../assets/reminders/", $file)))
                .expect(concat!($file, " must be a portable notification WAV"));
        assert!(duration > 0, concat!($file, " must contain audio"));
    }};
}

#[test]
fn all_ten_source_tones_are_pcm_wav_under_thirty_seconds() {
    assert_tone!("01_Koru_Default.wav");
    assert_tone!("02_Soft.wav");
    assert_tone!("03_Chime.wav");
    assert_tone!("04_Pulse.wav");
    assert_tone!("05_Bright.wav");
    assert_tone!("06_Calm.wav");
    assert_tone!("07_Drop.wav");
    assert_tone!("08_Knock.wav");
    assert_tone!("09_Signal.wav");
    assert_tone!("10_Urgent.wav");
}

#[test]
fn malformed_wav_is_refused_without_panicking() {
    assert_eq!(validate_notification_wav(b""), Err(WavError::Truncated));
    assert_eq!(
        validate_notification_wav(b"not a wave file"),
        Err(WavError::NotWave)
    );
}
