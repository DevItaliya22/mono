// SPDX-License-Identifier: MPL-2.0

//! The six tools a delegated run reaches at `/v1/runs/{run}/mcp`: their
//! inputs, their `structuredContent` outputs and their failure shape.
//!
//! The names are fixed — P1.12's preamble spells them — and none of them
//! takes a run id, an actor or a workspace: all three come from the
//! authority record the bearer resolved to (§7.3, "provenance derives from
//! the authority record, never from a caller-supplied run id"). Every input
//! rejects unknown fields, so a body that names a run is refused rather
//! than ignored, and a failure is `{ code, message }` with a stable code.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ArtifactFamily, ArtifactFamilyId, ArtifactVersion, ArtifactVersionId, RunLink};

/// The tool names, in the order the endpoint lists them.
pub const TOOL_NAMES: [&str; 6] = [
    "create",
    "create_version",
    "get",
    "read",
    "search",
    "attach_to_run",
];

/// The most bytes `read` returns inline. Above it the answer is the size
/// and a reason, never the bytes: an agent that needs a large result reads
/// the file it wrote, and a person reads it through the Artifacts surface.
pub const READ_BOUND_BYTES: u64 = 1024 * 1024;

/// The most families `search` answers with.
pub const SEARCH_LIMIT: usize = 50;

/// `create`: a new family in the run's own workspace, with its first
/// version read from `path`. The path is resolved against
/// `<run-workspace>/.artifacts/<RunId>/` and must stay inside it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    /// A file under the run's `.artifacts/<RunId>/` folder — relative to
    /// it, or absolute and inside it.
    pub path: String,
    /// What a person will call the result.
    pub title: String,
    /// `type/subtype`; `application/octet-stream` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
}

/// `create_version`: another immutable version under a family the run may
/// reach, read from `path` under the same folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateVersionInput {
    pub family: ArtifactFamilyId,
    pub path: String,
    /// `type/subtype`; the family's latest version's type when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
}

/// `get`, `read` and `attach_to_run` all name one version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VersionInput {
    pub version: ArtifactVersionId,
}

/// `search`: families in the granted workspaces whose title contains
/// `query`, case-insensitively.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchInput {
    pub query: String,
}

/// What `create` and `create_version` answer: the version, its family, and
/// whether this call found a version an earlier identical call had made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RegisteredOutput {
    pub version: ArtifactVersion,
    pub family: ArtifactFamily,
    pub retried: bool,
}

/// What `get` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MetadataOutput {
    pub version: ArtifactVersion,
    pub family: ArtifactFamily,
}

/// How `read` spells the bytes it returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContentEncoding {
    /// The bytes were valid UTF-8 and `content` is the text itself.
    Utf8,
    /// `content` is the bytes, base64 (standard alphabet, padded).
    Base64,
}

/// What `read` answers. Under [`READ_BOUND_BYTES`] the bytes are inline;
/// above it `content` and `encoding` are absent and `reason` says why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContentOutput {
    pub version: ArtifactVersionId,
    pub mime: String,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<ContentEncoding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// `content_too_large`, with `bound_bytes`, when the bytes were left
    /// out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_bytes: Option<u64>,
}

/// One `search` hit: a family and its versions by ordinal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchHit {
    pub family: ArtifactFamily,
    pub versions: Vec<ArtifactVersion>,
}

/// What `search` answers, at most [`SEARCH_LIMIT`] families.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchOutput {
    pub hits: Vec<SearchHit>,
    /// More families matched than were returned.
    pub truncated: bool,
}

/// What `attach_to_run` answers: the input link it recorded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachOutput {
    pub link: RunLink,
}

/// A tool's failure, as `structuredContent` with `isError`. The same
/// three fields the desktop bridge's tools use.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolFailure {
    /// A stable code: one of `koru_artifact_core::ArtifactError::code`, an
    /// authority refusal (`stale_generation`, `authority_revoked`,
    /// `authority_unknown`), or a register path code — `path_traversal`,
    /// `path_symlink`, `path_junction`, `path_outside_run_folder`,
    /// `run_folder_missing`, `file_not_found`, `not_a_file`,
    /// `file_changed`, `file_unreadable`.
    pub code: String,
    /// Developer text. Never a path, a title or a filename.
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ToolFailure {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }
}

impl From<koru_artifact_core::ArtifactError> for ToolFailure {
    fn from(error: koru_artifact_core::ArtifactError) -> Self {
        let failure = crate::Failure::from(error);
        Self::new(failure.code, failure.message)
    }
}

impl From<koru_artifact_core::AuthorityRefusal> for ToolFailure {
    fn from(refusal: koru_artifact_core::AuthorityRefusal) -> Self {
        use koru_artifact_core::AuthorityRefusal::*;
        let message = match refusal {
            StaleGeneration => "The token belongs to an earlier attempt of the run.",
            Revoked => "The run's authority was revoked.",
            Unknown => "The token was not recognised.",
        };
        Self::new(refusal.code(), message)
    }
}
