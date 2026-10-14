// SPDX-License-Identifier: MPL-2.0

use super::*;

/// The codes a client matches on. Pinned, because a route, a test in the
/// sqlite crate and P1.11's tools all spell them.
#[test]
fn error_codes_are_stable() {
    let cases = [
        (ArtifactError::FamilyNotFound, "artifact_family_not_found"),
        (ArtifactError::VersionNotFound, "artifact_not_found"),
        (ArtifactError::FamilyExists, "artifact_family_exists"),
        (ArtifactError::FamilyRetired, "artifact_family_retired"),
        (
            ArtifactError::VersionImmutable,
            "artifact_version_immutable",
        ),
        (ArtifactError::RegistrationConflict, "registration_conflict"),
        (ArtifactError::PermissionDenied, "permission_denied"),
        (ArtifactError::BlobNotFound, "blob_not_found"),
        (ArtifactError::InvalidRequest, "invalid_request"),
        (ArtifactError::AuthorityExists, "authority_exists"),
        (ArtifactError::AuthorityNotFound, "authority_not_found"),
        (ArtifactError::StaleGeneration, "stale_generation"),
        (ArtifactError::AuthorityRevoked, "authority_revoked"),
        (
            ArtifactError::Storage {
                operation: "open",
                detail: String::new(),
            },
            "artifact_storage_failed",
        ),
    ];
    for (error, code) in cases {
        assert_eq!(error.code(), code);
        assert_eq!(
            error.retryable(),
            matches!(error, ArtifactError::Storage { .. })
        );
    }
}

#[test]
fn display_is_the_code_except_for_a_storage_fault() {
    assert_eq!(
        ArtifactError::VersionImmutable.to_string(),
        "artifact_version_immutable"
    );
    assert_eq!(
        ArtifactError::Storage {
            operation: "register a version",
            detail: "disk full".into(),
        }
        .to_string(),
        "could not register a version: disk full"
    );
}
