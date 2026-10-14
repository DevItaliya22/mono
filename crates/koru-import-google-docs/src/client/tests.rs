// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn a_registration_needs_both_halves() {
    let id = Some("fixture-desktop.apps.googleusercontent.com");
    let secret = Some("fixture-not-a-secret");
    assert_eq!(
        Registration::from_values(id, secret),
        Some(Registration {
            client_id: "fixture-desktop.apps.googleusercontent.com".into(),
            client_secret: "fixture-not-a-secret".into(),
        })
    );
    assert_eq!(Registration::from_values(id, None), None);
    assert_eq!(Registration::from_values(None, secret), None);
    assert_eq!(Registration::from_values(id, Some("  ")), None);
    assert_eq!(Registration::from_values(Some(""), secret), None);
}

#[test]
fn values_are_trimmed() {
    let registration = Registration::from_values(
        Some(
            " id.apps.googleusercontent.com
",
        ),
        Some(" s "),
    )
    .unwrap();
    assert_eq!(registration.client_id, "id.apps.googleusercontent.com");
    assert_eq!(registration.client_secret, "s");
}

#[test]
fn debug_never_prints_the_secret() {
    let registration = Registration::from_values(Some("id"), Some("fixture-secret")).unwrap();
    let printed = format!("{registration:?}");
    assert!(printed.contains("id"));
    assert!(!printed.contains("fixture-secret"));
}
