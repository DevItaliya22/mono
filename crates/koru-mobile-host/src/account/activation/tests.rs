// SPDX-License-Identifier: MPL-2.0

use super::*;

fn configuration(platform: DevicePlatform, issuer: &str) -> TrustedAccountConfig {
    TrustedAccountConfig {
        origin: "https://accounts.example.test".into(),
        issuer: issuer.into(),
        google_server_client_id: "test.apps.googleusercontent.com".into(),
        credential_channel: "test".into(),
        platform,
        credential_rollout: None,
        blob_byte_origins: String::new(),
    }
}

#[test]
fn trusted_mobile_configuration_requires_a_mobile_platform_and_bounded_issuer() {
    for platform in [DevicePlatform::Android, DevicePlatform::Ios] {
        assert!(
            configuration(platform, "https://accounts.example.test")
                .validate()
                .is_ok()
        );
    }
    assert_eq!(
        configuration(DevicePlatform::Windows, "https://accounts.example.test").validate(),
        Err("account_platform_invalid")
    );
    for issuer in ["", "https://accounts.example.test x", "issuer\0"] {
        assert_eq!(
            configuration(DevicePlatform::Android, issuer).validate(),
            Err("account_issuer_invalid")
        );
    }
    assert_eq!(
        configuration(DevicePlatform::Android, &"x".repeat(2049)).validate(),
        Err("account_issuer_invalid")
    );
}
