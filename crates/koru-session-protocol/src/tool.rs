// SPDX-License-Identifier: MPL-2.0

//! **What a tool call was, and why it did not work** — as tokens, never as
//! the provider's own words.
//!
//! A provider names its tools in its own strings and describes a failure in
//! prose that quotes paths, arguments and output. Neither may cross this
//! boundary: an adapter *classifies* instead of copying, and everything
//! above it reads a closed vocabulary.
//!
//! That is not a new rule, it is the rule this crate already kept: the
//! adapter has always mapped Codex's `item.type` onto one of six labels of
//! its own, and always collapsed `item.status` and `item.error` into a
//! `bool`. [`ToolKind`] gives the first of those a type, and [`ToolFailure`]
//! keeps the second's classification instead of throwing it away — the same
//! amount of provider text as before, which is none.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// **Which tool ran**, as the adapter that watched it declares.
///
/// Not the provider's name for it: an adapter maps its provider's own tool
/// vocabulary onto this list, and a tool it has no token for is
/// [`Other`](Self::Other) — named as unknown rather than passed through as
/// text.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Debug,
    Serialize,
    Deserialize,
    JsonSchema,
    Default,
)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// Running a command.
    CommandExecution,
    /// Editing files.
    FileChange,
    /// A tool an MCP server offers.
    McpTool,
    /// A tool the provider loaded at runtime.
    DynamicTool,
    WebSearch,
    ImageGeneration,
    /// A tool this build has no token for. The honest answer when an
    /// adapter — or an older peer that sent no kind at all — did not say.
    #[default]
    Other,
}

impl ToolKind {
    /// The token, which is also the label the session view has always shown.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CommandExecution => "command_execution",
            Self::FileChange => "file_change",
            Self::McpTool => "mcp_tool",
            Self::DynamicTool => "dynamic_tool",
            Self::WebSearch => "web_search",
            Self::ImageGeneration => "image_generation",
            Self::Other => "other",
        }
    }

    /// Every kind, for exhaustive tests and vocabulary checks.
    pub const ALL: [Self; 7] = [
        Self::CommandExecution,
        Self::FileChange,
        Self::McpTool,
        Self::DynamicTool,
        Self::WebSearch,
        Self::ImageGeneration,
        Self::Other,
    ];
}

/// The most of a failed tool's own message that crosses the session
/// boundary, in characters (protocol v17). A sentence or two: enough to say
/// *Tool not found: koru__list_workspaces*, never a transcript.
pub const TOOL_DETAIL_CHARS: usize = 400;

/// `text`, trimmed and cut to [`TOOL_DETAIL_CHARS`] characters; `None` when
/// nothing readable is left.
pub fn bound_tool_detail(text: &str) -> Option<String> {
    bound_chars(text, TOOL_DETAIL_CHARS)
}

/// The most of what a tool was asked and what it answered that crosses the
/// session boundary, each, in characters (protocol v17, UC17). Enough to
/// read a command and the first screen of its output, or a page of a diff;
/// never a transcript.
pub const TOOL_CONTENT_CHARS: usize = 4_000;

/// `text`, trimmed and cut to [`TOOL_CONTENT_CHARS`] characters; `None` when
/// nothing readable is left.
pub fn bound_tool_content(text: &str) -> Option<String> {
    bound_chars(text, TOOL_CONTENT_CHARS)
}

fn bound_chars(text: &str, chars: usize) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.chars().take(chars).collect())
}

/// **Why a tool call did not work**, classified by the adapter from the
/// structured fields it already reads.
///
/// Five classes and no sixth escape hatch that carries text. A provider
/// that failed a call in a way the adapter cannot place is
/// [`Unclassified`](Self::Unclassified) — which is a fact worth showing
/// ("it failed, and the provider did not say how") and is never a guess.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Debug,
    Serialize,
    Deserialize,
    JsonSchema,
    Default,
)]
#[serde(rename_all = "snake_case")]
pub enum ToolFailure {
    /// It was not allowed to run — an approval refused, a permission denied.
    Denied,
    /// It was stopped before it finished.
    Cancelled,
    /// It ran out of time.
    TimedOut,
    /// It ran and reported an error.
    Errored,
    /// It did not succeed, and the provider did not say why in a way this
    /// adapter can place.
    #[default]
    Unclassified,
}

impl ToolFailure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Denied => "denied",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::Errored => "errored",
            Self::Unclassified => "unclassified",
        }
    }

    pub const ALL: [Self; 5] = [
        Self::Denied,
        Self::Cancelled,
        Self::TimedOut,
        Self::Errored,
        Self::Unclassified,
    ];
}
