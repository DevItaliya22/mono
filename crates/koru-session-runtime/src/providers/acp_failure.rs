// SPDX-License-Identifier: MPL-2.0

//! Normalization of ACP discovery refusals, below the provider boundary.

use koru_session_protocol::ErrorCode;
use serde_json::Value;

use super::{ProviderDiagnostic, ProviderFailure, ProviderStage};

/// Normalize a validated ACP error object from an initialize/session-new probe.
/// The caller correlates the response first; provider text and data never leave
/// this function. `claude_wrapper` identifies the actual launched wrapper, not
/// the independent raw Claude executable.
pub fn acp_probe_failure(error: &Value, claude_wrapper: bool) -> ProviderFailure {
    let diagnostic = match error.get("code").and_then(Value::as_i64) {
        Some(-32000) => ProviderDiagnostic::AuthenticationRequired,
        Some(-32603)
            if claude_wrapper
                && error.get("message").and_then(Value::as_str) == Some("Internal error")
                && error
                    .pointer("/data/details")
                    .and_then(Value::as_str)
                    .is_some_and(|details| {
                        wrapper_runtime_missing(details) || wrapper_override_missing(details)
                    }) =>
        {
            ProviderDiagnostic::RuntimeUnavailable
        }
        _ => ProviderDiagnostic::HandshakeRejected,
    };
    ProviderFailure::new(
        if diagnostic == ProviderDiagnostic::AuthenticationRequired {
            ErrorCode::ProviderUnavailable
        } else {
            ErrorCode::ProviderFailed
        },
        ProviderStage::Initialize,
        diagnostic,
    )
}

fn wrapper_runtime_missing(message: &str) -> bool {
    let Some(platform) = message
        .strip_prefix("Claude native binary not found for ")
        .and_then(|rest| {
            rest.strip_suffix(
                ". Reinstall @anthropic-ai/claude-agent-sdk without --omit=optional, or set CLAUDE_CODE_EXECUTABLE.",
            )
        })
    else {
        return false;
    };
    // Match the fixed resolver error, never search arbitrary output. This slot
    // contains a platform token rather than a path or free text.
    matches!(
        platform,
        "win32-x64" | "win32-arm64" | "darwin-x64" | "darwin-arm64" | "linux-x64" | "linux-arm64"
    )
}

fn wrapper_override_missing(message: &str) -> bool {
    message.strip_prefix("Claude Code native binary not found at ")
        .and_then(|rest| rest.strip_suffix(". Please ensure Claude Code is installed via native installer or specify a valid path with options.pathToClaudeCodeExecutable."))
        .is_some_and(|path| !path.is_empty() && !path.chars().any(char::is_control))
}

#[cfg(test)]
mod tests;
