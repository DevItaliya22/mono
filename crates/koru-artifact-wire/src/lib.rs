// SPDX-License-Identifier: MPL-2.0

//! The wire of `/v1/artifacts`.
//!
//! The model types themselves — [`ArtifactFamily`], [`ArtifactVersion`],
//! [`Provenance`], [`RunLink`] — cross as they are, because they are already
//! serde bodies with their ids as strings and their texts redacted in
//! `Debug`. What this crate adds is the envelopes a route reads and writes,
//! the one failure shape, the capability token the gateway publishes, and
//! the size a metadata body may not exceed. Bytes never cross here: a
//! version names its content by [`BlobHash`], and the bytes travel through
//! the Store's own blob routes.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use koru_artifact_core::{
    Access, ArtifactFamily, ArtifactFamilyId, ArtifactTitle, ArtifactVersion, ArtifactVersionId,
    AttemptGeneration, BlobHash, Filename, LinkRole, MediaType, Ordinal, Permission, Principal,
    ProducedBy, Provenance, RegistrationId, RelativePath, RunId, RunLink, WorkspaceId,
};

pub mod authority;
pub mod run_folder;
pub mod run_tools;

/// The capability the gateway publishes when `artifacts.db` opened. Absent
/// when it did not, and `/v1/artifacts/*` then answers 503.
pub const CAPABILITY: &str = "artifacts";

/// The largest metadata body `/v1/artifacts` reads. A registration names
/// bytes already in the blob store; it never carries them, and the limit is
/// what asserts that. Large content goes through
/// `PUT /v1/workspaces/{id}/blobs`, whose own limit is the Store's.
pub const METADATA_BODY_LIMIT: usize = 64 * 1024;

/// `POST /v1/artifacts/families`. The id is the caller's, so a retried
/// creation is refused as a duplicate rather than made twice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreateFamilyRequest {
    pub family_id: ArtifactFamilyId,
    pub workspace: WorkspaceId,
    pub title: ArtifactTitle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by_run: Option<RunId>,
}

/// `GET /v1/artifacts/families?workspace=…&include_retired=…`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListFamiliesQuery {
    pub workspace: WorkspaceId,
    #[serde(default)]
    pub include_retired: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListFamiliesResponse {
    pub families: Vec<ArtifactFamily>,
}

/// `GET /v1/artifacts/families/{family}`: the family and its versions by
/// ordinal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FamilyResponse {
    pub family: ArtifactFamily,
    pub versions: Vec<ArtifactVersion>,
}

/// `POST /v1/artifacts/families/{family}/versions`. Metadata only: the
/// blob is an address in the family's workspace, already uploaded through
/// the Store's blob route, and the gateway verifies it is there before the
/// version exists. The size is the Store's word, not the caller's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RegisterVersionRequest {
    pub registration_id: RegistrationId,
    pub blob: BlobHash,
    pub mime: MediaType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<Filename>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub produced_by: Option<ProducedBy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_from: Option<ArtifactVersionId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RegisterVersionResponse {
    pub version: ArtifactVersion,
    /// This call found the version an earlier call with the same
    /// registration id had made.
    pub retried: bool,
}

/// `POST /v1/artifacts/versions/{version}/runs`: record that a run consumed
/// or produced the version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LinkRunRequest {
    pub run: RunId,
    pub role: LinkRole,
}

/// The one failure shape, the same fields as the AI and Store failures so a
/// gateway client reads every domain's error the same way.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "ArtifactFailure")]
pub struct Failure {
    /// A stable code from `koru_artifact_core::ArtifactError::code`, an
    /// authority refusal (`koru_artifact_core::AuthorityRefusal::code`), or
    /// one of the gateway's own: `forbidden`, `artifacts_unavailable`,
    /// `payload_too_large`, `invalid_request`.
    pub code: String,
    /// Safe developer-facing detail. Never a title, a filename or a
    /// principal.
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, serde_json::Value>,
}

impl Failure {
    pub fn new(code: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
            details: BTreeMap::new(),
        }
    }
}

impl From<koru_artifact_core::ArtifactError> for Failure {
    fn from(error: koru_artifact_core::ArtifactError) -> Self {
        let message = match &error {
            koru_artifact_core::ArtifactError::Storage { .. } => {
                "The artifact store could not complete the operation.".to_owned()
            }
            other => describe(other),
        };
        Self::new(error.code(), message, error.retryable())
    }
}

fn describe(error: &koru_artifact_core::ArtifactError) -> String {
    use koru_artifact_core::ArtifactError::*;
    match error {
        FamilyNotFound => "No artifact family has that id.",
        VersionNotFound => "No artifact version has that id.",
        FamilyExists => "An artifact family with that id already exists.",
        FamilyRetired => "The artifact family is retired and accepts no new version.",
        VersionImmutable => "An artifact version cannot be changed; register a new one.",
        RegistrationConflict => {
            "The registration id was used before for a different family or different bytes."
        }
        PermissionDenied => "The caller holds no access that permits this operation.",
        BlobNotFound => "The blob is not in the workspace's store.",
        InvalidRequest => "The artifact request is invalid.",
        AuthorityExists => "An authority for that run was already issued; advance or revoke it.",
        AuthorityNotFound => "No authority was issued for that run.",
        StaleGeneration => "The generation is not newer than the run's current attempt.",
        AuthorityRevoked => "The run's authority was revoked.",
        Storage { .. } => "The artifact store could not complete the operation.",
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_carries_the_stable_code_and_no_content() {
        let failure = Failure::from(koru_artifact_core::ArtifactError::VersionImmutable);
        assert_eq!(failure.code, "artifact_version_immutable");
        assert!(!failure.retryable);
        let storage = Failure::from(koru_artifact_core::ArtifactError::Storage {
            operation: "register a version",
            detail: "disk full".into(),
        });
        assert_eq!(storage.code, "artifact_storage_failed");
        assert!(storage.retryable);
        assert!(
            !storage.message.contains("disk full"),
            "the driver's text stays in the log"
        );
        let json = serde_json::to_value(&failure).unwrap();
        assert!(json.get("details").is_none());
    }

    #[test]
    fn the_metadata_limit_holds_the_largest_registration_and_the_capability_is_spelled_once() {
        assert_eq!(CAPABILITY, "artifacts");
        // The widest metadata a registration can carry — every optional
        // field present, the filename at its bound — fits under the limit
        // with room, so the limit refuses bytes and never a legitimate
        // registration.
        let widest = RegisterVersionRequest {
            registration_id: RegistrationId::mint(),
            blob: BlobHash::from_digest("b3", &"ff".repeat(32)).unwrap(),
            mime: MediaType::new("application/vnd.koru.report+json").unwrap(),
            filename: Some(
                Filename::new("報".repeat(koru_artifact_core::MAX_FILENAME_CHARS)).unwrap(),
            ),
            produced_by: Some(ProducedBy {
                run: RunId::mint(),
                generation: koru_artifact_core::AttemptGeneration::FIRST,
            }),
            derived_from: Some(ArtifactVersionId::mint()),
        };
        let encoded = serde_json::to_vec(&widest).unwrap().len();
        assert!(encoded * 4 < METADATA_BODY_LIMIT, "{encoded} bytes");
    }

    #[test]
    fn every_body_has_a_schema() {
        let mut generator = schemars::SchemaGenerator::default();
        generator.subschema_for::<CreateFamilyRequest>();
        generator.subschema_for::<RegisterVersionRequest>();
        generator.subschema_for::<FamilyResponse>();
        generator.subschema_for::<RegisterVersionResponse>();
        generator.subschema_for::<LinkRunRequest>();
        generator.subschema_for::<Failure>();
        generator.subschema_for::<Provenance>();
        generator.subschema_for::<authority::IssueAuthorityRequest>();
        generator.subschema_for::<authority::IssuedAuthority>();
        generator.subschema_for::<authority::AuthorityView>();
        generator.subschema_for::<authority::RunVersionsResponse>();
        generator.subschema_for::<run_tools::CreateInput>();
        generator.subschema_for::<run_tools::RegisteredOutput>();
        generator.subschema_for::<run_tools::ContentOutput>();
        generator.subschema_for::<run_tools::SearchOutput>();
        generator.subschema_for::<run_tools::ToolFailure>();
        generator.subschema_for::<run_folder::UnregisteredResponse>();
        generator.subschema_for::<run_folder::RegisterFileRequest>();
        generator.subschema_for::<run_folder::RegisteredFile>();
        let definitions = generator.take_definitions(true);
        assert!(definitions.contains_key("ArtifactFailure"));
        assert!(definitions.contains_key("ArtifactVersion"));
        assert!(definitions.contains_key("AuthorityState"));
    }

    /// The token and the path cross once and are never rendered.
    #[test]
    fn the_authority_bodies_redact_the_token_and_the_path() {
        let path = if cfg!(windows) {
            r"C:\PATH-SENTINEL"
        } else {
            "/PATH-SENTINEL"
        };
        let request = authority::IssueAuthorityRequest {
            run: RunId::mint(),
            workspace: WorkspaceId::from_uuid(uuid::Uuid::new_v4()),
            generation: koru_artifact_core::AttemptGeneration::FIRST,
            grants: Default::default(),
            run_workspace: authority::RunWorkspace::new(std::path::Path::new(path)).unwrap(),
        };
        assert!(!format!("{request:?}").contains("PATH-SENTINEL"));
        let issued = authority::IssuedAuthority {
            run: request.run,
            generation: request.generation,
            url: "http://127.0.0.1:1/v1/runs/x/mcp".into(),
            token: "TOKEN-SENTINEL".into(),
        };
        assert!(!format!("{issued:?}").contains("TOKEN-SENTINEL"));
        let json = serde_json::to_value(&issued).unwrap();
        assert_eq!(json["token"], "TOKEN-SENTINEL", "it does cross, once");
        // An unknown field — a run id smuggled into a tool input — is
        // refused, never ignored.
        let forged = serde_json::json!({ "path": "a.md", "title": "A", "run": "x" });
        assert!(serde_json::from_value::<run_tools::CreateInput>(forged).is_err());
        assert_eq!(run_tools::TOOL_NAMES.len(), 6);
    }
}
