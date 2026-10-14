// SPDX-License-Identifier: MPL-2.0

//! A legacy-issuance policy whose cutover is zero admits nothing it issues.
//! Two Android devices on one account spent an evening reporting
//! `attestation_invalid` against hosted attestations the server had signed
//! correctly seconds earlier, because the build carried exactly that policy.
use super::*;

const LEGACY: &str = "http://localhost:8080";
const REGIONAL: &str = "http://127.0.0.1:8080";
/// Any real wall clock. The device that found this was at ~1.79e12.
const NOW: i64 = 1_789_589_116_000;

fn policy(issuance: CredentialIssuance, cutover: i64) -> RegionalCredentialPolicy {
    RegionalCredentialPolicy {
        region: Region::Us,
        regional_issuer: REGIONAL.into(),
        legacy_issuer: LEGACY.into(),
        legacy_cutover_millis: cutover,
        issuance,
        legacy_access_kids: ["access-local-1".into()].into(),
        legacy_attestation_kids: ["attestation-local-1".into()].into(),
    }
}

fn json(issuance: &str, cutover: i64) -> String {
    format!(
        r#"{{"region":"US","regional_issuer":"{REGIONAL}","legacy_issuer":"{LEGACY}","legacy_cutover_millis":{cutover},"issuance":"{issuance}","legacy_access_kids":["access-local-1"],"legacy_attestation_kids":["attestation-local-1"]}}"#
    )
}

#[test]
fn legacy_issuance_with_a_zero_cutover_is_refused_as_configuration() {
    // The exact string an Android build carried. It must not parse: the app
    // throws `account_configuration_invalid` at startup instead of enrolling
    // forever against credentials it can never admit.
    assert!(RegionalCredentialPolicy::parse(&json("legacy", 0)).is_err());
    assert!(policy(CredentialIssuance::Legacy, 0).validate().is_err());

    // The same deployment with the cutover the API actually serves.
    assert!(RegionalCredentialPolicy::parse(&json("legacy", 2_104_866_917_464)).is_ok());
}

#[test]
fn a_zero_cutover_refuses_every_legacy_credential_it_issues() {
    // Why the configuration had to be refused, stated without `validate`:
    // `admits` gates on `issued_at <= legacy_cutover_millis`, so a credential
    // minted at any real time is born already refused, on every device
    // carrying the policy, for as long as it carries it. Enrollment reported
    // that as `attestation_invalid`.
    let closed = policy(CredentialIssuance::Legacy, 0);
    assert!(!closed.admits(
        LEGACY,
        None,
        NOW,
        NOW,
        "attestation-local-1",
        CredentialUse::Attestation,
    ));

    // The same policy with a cutover the mint has not yet passed admits it.
    let open = policy(CredentialIssuance::Legacy, NOW + 60_000);
    assert!(open.admits(
        LEGACY,
        None,
        NOW,
        NOW,
        "attestation-local-1",
        CredentialUse::Attestation,
    ));
}

#[test]
fn a_closed_legacy_window_stays_valid_once_issuance_has_moved_regional() {
    // Zero is the honest value for a deployment that never issued legacy and
    // now signs regionally; only claiming to *issue* legacy alongside it is
    // the contradiction. The fix must not cost this case.
    assert!(RegionalCredentialPolicy::parse(&json("regional", 0)).is_ok());
    assert!(policy(CredentialIssuance::Regional, 0).validate().is_ok());
}
