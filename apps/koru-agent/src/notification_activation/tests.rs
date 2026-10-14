// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn a_private_route_resolves_an_opaque_activation_without_entering_its_payload() {
    let registration = koru_native_notifications::RegistrationId::new("registration-1").unwrap();
    let namespace =
        koru_native_notifications::NotificationNamespace::from_stable_key("C:/private-profile");
    let target = RouteTarget::Explicit {
        root: PathBuf::from(r"C:\private-profile"),
    };
    let mut routes = RouteFile::default();
    routes.insert(&namespace, &registration, target.clone());

    let private_json = serde_json::to_string(&routes).unwrap();
    assert!(private_json.contains("private-profile"));
    let decoded = RouteFile::decode(private_json.as_bytes()).unwrap();
    assert_eq!(decoded.target(&namespace, &registration), Some(target));

    let activation = koru_native_notifications::Activation::new(
        registration.clone(),
        koru_native_notifications::Action::Open,
    );
    let envelope =
        koru_native_notifications::WindowsActivationEnvelope::new(namespace.clone(), activation);
    let native_payload = envelope.encode_opaque();
    assert!(!native_payload.contains("private-profile"));
    let wire = serde_json::to_string(&envelope).unwrap();
    assert!(!wire.contains("root"));
    assert!(!wire.contains("profile"));
    assert!(!wire.contains("title"));
    let decoded =
        koru_native_notifications::WindowsActivationEnvelope::decode_opaque(&native_payload)
            .unwrap();
    assert_eq!(decoded.namespace, namespace);
    assert_eq!(decoded.registration, registration);
}

#[test]
fn two_roots_with_the_same_registration_keep_distinct_routes() {
    let registration = koru_native_notifications::RegistrationId::new("same-occurrence").unwrap();
    let first_namespace =
        koru_native_notifications::NotificationNamespace::from_stable_key("C:/first");
    let second_namespace =
        koru_native_notifications::NotificationNamespace::from_stable_key("C:/second");
    let first = RouteTarget::Explicit {
        root: PathBuf::from(r"C:\first"),
    };
    let second = RouteTarget::Explicit {
        root: PathBuf::from(r"C:\second"),
    };
    let mut routes = RouteFile::default();
    routes.insert(&first_namespace, &registration, first.clone());
    routes.insert(&second_namespace, &registration, second.clone());

    assert_eq!(routes.target(&first_namespace, &registration), Some(first));
    assert_eq!(
        routes.target(&second_namespace, &registration),
        Some(second)
    );
    assert_eq!(routes.routes.len(), 2);
}

#[test]
fn route_maps_are_versioned_strict_and_require_composite_keys_and_absolute_roots() {
    assert!(
        RouteFile::decode(br#"{"schema_version":2,"routes":{}}"#)
            .unwrap_err()
            .to_string()
            .contains("unsupported schema")
    );
    assert!(RouteFile::decode(
        br#"{"schema_version":1,"routes":{"a354fd1ff0c467c5:registration-1":{"kind":"explicit","root":"relative"}}}"#
    )
    .unwrap_err()
    .to_string()
    .contains("invalid route"));
    assert!(
        RouteFile::decode(
            br#"{"schema_version":1,"routes":{"registration-1":{"kind":"channel"}}}"#
        )
        .unwrap_err()
        .to_string()
        .contains("invalid route")
    );
    assert!(
        RouteFile::decode(br#"{"schema_version":1,"routes":{},"content":"must not be accepted"}"#)
            .unwrap_err()
            .to_string()
            .contains("malformed")
    );
}

#[test]
fn corrupt_activation_reads_fail_closed_and_the_next_registration_repairs_the_map() {
    let corrupt = b"not a route map";
    assert_eq!(RouteFile::decode(corrupt), Err(RouteDecodeError::Malformed));

    let mut preserved = false;
    let mut repaired = decode_for_mutation(corrupt, || {
        preserved = true;
        Ok(())
    })
    .unwrap();
    let namespace = koru_native_notifications::NotificationNamespace::from_stable_key("root");
    let registration = koru_native_notifications::RegistrationId::new("registration-1").unwrap();
    repaired.insert(&namespace, &registration, RouteTarget::Channel);

    assert!(preserved);
    assert_eq!(
        repaired.target(&namespace, &registration),
        Some(RouteTarget::Channel)
    );
}

#[test]
fn broker_spawn_argv_is_exact_and_strictly_parseable() {
    let root = Path::new(r"C:\isolated-koru");
    let namespace = koru_native_notifications::NotificationNamespace::from_stable_key("root");
    let activation = koru_native_notifications::Activation::new(
        koru_native_notifications::RegistrationId::new("registration-1").unwrap(),
        koru_native_notifications::Action::Handled,
    );
    let envelope = koru_native_notifications::WindowsActivationEnvelope::new(namespace, activation)
        .encode_opaque();
    let arguments = activation_arguments(Some(root), &envelope);
    assert_eq!(
        arguments,
        [
            std::ffi::OsString::from("--root"),
            root.as_os_str().to_owned(),
            std::ffi::OsString::from("notification-activation"),
            std::ffi::OsString::from(&envelope),
        ]
    );
    let parsed = crate::cli::parse(
        std::iter::once("koru-agent".to_owned()).chain(
            arguments
                .into_iter()
                .map(|value| value.into_string().unwrap()),
        ),
    )
    .unwrap();
    assert_eq!(parsed.root.as_deref(), Some(root));
    assert_eq!(
        parsed.command,
        crate::cli::Command::NotificationActivation(envelope)
    );

    let channel_arguments = activation_arguments(None, "opaque-channel-activation");
    assert_eq!(
        channel_arguments,
        [
            std::ffi::OsString::from("notification-activation"),
            std::ffi::OsString::from("opaque-channel-activation"),
        ]
    );
}
