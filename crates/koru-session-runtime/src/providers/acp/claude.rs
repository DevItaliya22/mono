// SPDX-License-Identifier: MPL-2.0

//! Exact 0.76.0 wrapper policy. Extension payloads never leave this module.
use super::*;

#[derive(Clone, Copy)]
pub(super) enum WrapperAuth {
    Authenticated,
    SignedOut,
    Unverified,
}

pub(super) fn auth_status(params: &Value) -> WrapperAuth {
    match params.pointer("/authStatus/kind").and_then(Value::as_str) {
        Some("account" | "api_key") => WrapperAuth::Authenticated,
        Some("none") => WrapperAuth::SignedOut,
        // A configured external cloud backend is not proof of credentials;
        // this route never performs ACP gateway authentication.
        _ => WrapperAuth::Unverified,
    }
}

pub(super) fn validate_handshake(initialized: &Value) -> Result<(), ProviderFailure> {
    if initialized
        .pointer("/agentInfo/version")
        .and_then(Value::as_str)
        != Some("0.76.0")
        || !initialized
            .pointer("/agentCapabilities/_meta/authStatus")
            .is_some_and(Value::is_object)
    {
        return Err(handshake_rejected());
    }
    Ok(())
}

impl AcpControl {
    pub(super) fn await_wrapper_auth(&mut self) -> Result<(), ProviderFailure> {
        let deadline = Instant::now() + self.config.request_timeout;
        loop {
            match self.wrapper_auth {
                Some(WrapperAuth::Authenticated) => return Ok(()),
                Some(WrapperAuth::SignedOut) => {
                    return Err(failure(
                        ErrorCode::ProviderUnavailable,
                        ProviderStage::Initialize,
                        ProviderDiagnostic::AuthenticationRequired,
                    ));
                }
                Some(WrapperAuth::Unverified) => {
                    return Err(failure(
                        ErrorCode::ProviderFailed,
                        ProviderStage::Initialize,
                        ProviderDiagnostic::HandshakeRejected,
                    ));
                }
                None => {}
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(failure(
                    ErrorCode::ProviderFailed,
                    ProviderStage::Initialize,
                    ProviderDiagnostic::Timeout,
                ));
            }
            match self.transport()?.receive(remaining)? {
                TransportReceive::Frame(frame) => {
                    self.diagnostics.frames_read = self.diagnostics.frames_read.saturating_add(1);
                    self.handle_frame(frame)?;
                }
                TransportReceive::Timeout => {
                    return Err(failure(
                        ErrorCode::ProviderFailed,
                        ProviderStage::Initialize,
                        ProviderDiagnostic::Timeout,
                    ));
                }
                TransportReceive::Woken => {}
            }
        }
    }
}

/// Extend, never replace, caller restrictions. Invalid metadata cannot discard
/// the mandatory exclusions silently.
pub(super) fn enforce_child_exclusions(meta: &mut Value) -> Result<(), ProviderFailure> {
    let invalid = || {
        failure(
            ErrorCode::InvalidRequest,
            ProviderStage::Initialize,
            ProviderDiagnostic::InvalidInput,
        )
    };
    if !meta.is_object() {
        return Err(invalid());
    }
    if meta.get("claudeCode").is_none() {
        meta["claudeCode"] = json!({});
    }
    let claude = meta
        .get_mut("claudeCode")
        .filter(|v| v.is_object())
        .ok_or_else(invalid)?;
    if claude.get("options").is_none() {
        claude["options"] = json!({});
    }
    let options = claude
        .get_mut("options")
        .filter(|v| v.is_object())
        .ok_or_else(invalid)?;
    if options.get("disallowedTools").is_none() {
        options["disallowedTools"] = json!([]);
    }
    let disallowed = options
        .get_mut("disallowedTools")
        .and_then(Value::as_array_mut)
        .ok_or_else(invalid)?;
    if !disallowed.iter().all(Value::is_string) {
        return Err(invalid());
    }
    for name in ["Agent", "Task"] {
        if !disallowed.iter().any(|v| v.as_str() == Some(name)) {
            disallowed.push(json!(name));
        }
    }
    Ok(())
}
