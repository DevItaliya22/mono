// SPDX-License-Identifier: MPL-2.0

//! The bodies of the two routes a **person's** Artifacts surface needs that
//! a run does not: what is sitting in a run's `.artifacts/<RunId>/` folder
//! without being a result, and the one-click Register that makes one
//! (D19, D29, D37).
//!
//! **Nothing here widens what a session may do.** A gateway session can
//! already create a family and register a version in any workspace it
//! reaches; these two routes are strictly narrower — they read and register
//! only files *inside* the run's own folder, under the same register path
//! rule the run's own tools obey, and every fact that decides where the
//! bytes land (the workspace, the provenance, the family's identity) is
//! read from the durable authority record rather than from anything the
//! call says. What they add is reach *into a folder*, and only that folder.
//!
//! A path is content, so [`RelativePath`] redacts in `Debug` and the
//! request body does too.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use koru_artifact_core::{MAX_RELATIVE_PATH_CHARS, RelativePath};

use crate::{ArtifactFamily, ArtifactTitle, ArtifactVersion, MediaType, RunId, WorkspaceId};

/// The most files one run's folder listing carries. A folder above it is
/// answered with the first [`MAX_RUN_FOLDER_FILES`] and `truncated`, because
/// a surface that cannot draw ten thousand rows gains nothing from being
/// sent them, and the body has a limit.
pub const MAX_RUN_FOLDER_FILES: usize = 500;

/// How deep the scan walks below `.artifacts/<RunId>/`. An agent's results
/// are files it wrote, not a checkout; a folder deeper than this is not
/// descended and the listing says it was truncated.
pub const MAX_RUN_FOLDER_DEPTH: usize = 8;

/// One file under a run's `.artifacts/<RunId>/` folder that is **not yet a
/// result** (D19).
///
/// `path` is what [`crate::run_folder::RegisterFileRequest`] echoes back;
/// `size_bytes` and `modified_at_ms` are what the surface draws beside it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnregisteredFile {
    pub path: RelativePath,
    pub size_bytes: u64,
    /// The file's modification time, when the filesystem reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_at_ms: Option<i64>,
}

impl fmt::Debug for UnregisteredFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnregisteredFile")
            .field("size_bytes", &self.size_bytes)
            .finish_non_exhaustive()
    }
}

/// One run's folder, as the surface reads it.
///
/// `registered` is how many versions the run has already produced, so D29's
/// state — *finished having registered nothing* — is a fact the surface can
/// state rather than infer from an empty list it might simply not have
/// fetched. `folder_present` separates *the agent wrote nothing* from *the
/// folder is gone*, which are different sentences.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunFolder {
    pub run: RunId,
    pub workspace: WorkspaceId,
    /// `<run-workspace>/.artifacts/<RunId>/` exists and could be read.
    pub folder_present: bool,
    /// Versions this run has registered.
    pub registered: usize,
    /// Files in the folder that are not results, shallowest first then by
    /// path, at most [`MAX_RUN_FOLDER_FILES`].
    pub unregistered: Vec<UnregisteredFile>,
    /// The folder held more than the scan's bounds allow.
    pub truncated: bool,
}

/// `GET /v1/artifacts/unregistered?workspace=…`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnregisteredQuery {
    pub workspace: WorkspaceId,
}

/// Every run of the workspace with a folder to look in, newest run first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnregisteredResponse {
    pub runs: Vec<RunFolder>,
}

/// `POST /v1/artifacts/runs/{run}/register` — D19's one click.
///
/// The title is optional because the click carries no words: absent, the
/// file's own last segment names the result, which is what the person is
/// looking at when they press it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegisterFileRequest {
    pub path: RelativePath,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<ArtifactTitle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<MediaType>,
}

impl fmt::Debug for RegisterFileRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegisterFileRequest")
            .field("titled", &self.title.is_some())
            .finish_non_exhaustive()
    }
}

/// What a Register answers: the version, its family, and whether an
/// identical earlier registration had already made it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RegisteredFile {
    pub version: ArtifactVersion,
    pub family: ArtifactFamily,
    pub retried: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A path never reaches a `Debug`, on the way out or on the way in.
    #[test]
    fn the_run_folder_bodies_redact_the_path() {
        let file = UnregisteredFile {
            path: RelativePath::new("out/PATH-SENTINEL.md").unwrap(),
            size_bytes: 12,
            modified_at_ms: Some(7),
        };
        assert!(!format!("{file:?}").contains("PATH-SENTINEL"));
        let request = RegisterFileRequest {
            path: file.path.clone(),
            title: None,
            mime: None,
        };
        assert!(!format!("{request:?}").contains("PATH-SENTINEL"));
        // It does cross, once, because the surface drew it and the rule
        // resolves it.
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["path"], "out/PATH-SENTINEL.md");
        assert!(json.get("title").is_none());
    }

    /// An unknown field is refused rather than ignored: a body that names a
    /// workspace or a family is a body that thinks it decides where the
    /// bytes go, and it does not.
    #[test]
    fn a_register_body_refuses_a_field_it_does_not_have() {
        let forged = serde_json::json!({ "path": "a.md", "workspace": "x" });
        assert!(serde_json::from_value::<RegisterFileRequest>(forged).is_err());
        let escape = serde_json::json!({ "path": "../../secret" });
        assert!(serde_json::from_value::<RegisterFileRequest>(escape).is_err());
    }

    #[test]
    fn every_run_folder_body_has_a_schema() {
        let mut generator = schemars::SchemaGenerator::default();
        generator.subschema_for::<UnregisteredResponse>();
        generator.subschema_for::<RegisterFileRequest>();
        generator.subschema_for::<RegisteredFile>();
        let definitions = generator.take_definitions(true);
        assert!(definitions.contains_key("RunFolder"));
        assert!(definitions.contains_key("UnregisteredFile"));
    }
}
