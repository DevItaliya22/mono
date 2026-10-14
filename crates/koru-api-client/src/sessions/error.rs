// SPDX-License-Identifier: MPL-2.0

use koru_session_protocol::SessionFailure;
use koru_store_client::PublicError;
use koru_store_core::errors::Error;

/// The owner of a stable public failure token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionFailureCode {
    Session(koru_session_protocol::ErrorCode),
    Gateway(koru_session_api::GatewayError),
    Unknown(String),
}

impl SessionFailureCode {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Session(code) => code.as_str(),
            Self::Gateway(code) => code.as_str(),
            Self::Unknown(code) => code,
        }
    }
}

/// A non-success HTTP answer whose stable token remains available for policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionApiFailure {
    pub status: u16,
    pub code: SessionFailureCode,
    pub message: String,
    pub waiting_on: Vec<String>,
}

#[derive(Debug)]
pub enum SessionsError {
    /// Discovery, authentication transport, connection, or response decoding.
    Transport(Error),
    /// A structured non-success response from the public session API.
    Api(SessionApiFailure),
    /// A request rejected before content crossed the process boundary.
    Local(SessionFailure),
}

impl std::fmt::Display for SessionsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(error) => error.fmt(formatter),
            Self::Api(failure) => {
                write!(formatter, "{}: {}", failure.code.as_str(), failure.message)
            }
            Self::Local(failure) => write!(formatter, "{}: {}", failure.code, failure.message),
        }
    }
}

impl std::error::Error for SessionsError {}

pub type SessionsResult<T> = Result<T, SessionsError>;

pub(super) fn map_public(error: PublicError) -> SessionsError {
    match error {
        PublicError::Transport(error) => SessionsError::Transport(error),
        PublicError::Failure(failure) => SessionsError::Api(SessionApiFailure {
            status: failure.status,
            code: classify_failure(&failure.code),
            message: failure.message,
            waiting_on: failure.waiting_on,
        }),
    }
}

fn classify_failure(value: &str) -> SessionFailureCode {
    if let Some(code) = koru_session_api::GatewayError::ALL
        .iter()
        .find(|code| code.as_str() == value)
    {
        return SessionFailureCode::Gateway(*code);
    }
    if let Some(code) = koru_session_protocol::ErrorCode::ALL
        .iter()
        .find(|code| code.as_str() == value)
    {
        return SessionFailureCode::Session(code.clone());
    }
    SessionFailureCode::Unknown(value.to_owned())
}

pub(super) fn validate_input(bytes: usize, nonempty: bool) -> SessionsResult<()> {
    if !nonempty {
        return Err(local(
            koru_session_protocol::ErrorCode::InvalidRequest,
            "process input is empty",
        ));
    }
    if bytes > koru_session_api::MAX_INPUT_BYTES {
        return Err(local(
            koru_session_protocol::ErrorCode::InputTooLarge,
            "process input exceeds the public one MiB limit",
        ));
    }
    Ok(())
}

pub(super) fn local(code: koru_session_protocol::ErrorCode, message: &str) -> SessionsError {
    SessionsError::Local(SessionFailure {
        code,
        message: message.to_owned(),
    })
}

pub(super) fn local_io(action: &str, error: std::io::Error) -> SessionsError {
    local(
        koru_session_protocol::ErrorCode::InvalidRequest,
        &format!("could not {action} attachment file: {}", error.kind()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_tokens_remain_typed_and_unknown_remains_explicit() {
        assert_eq!(
            classify_failure("stale_lease"),
            SessionFailureCode::Session(koru_session_protocol::ErrorCode::StaleLease)
        );
        assert_eq!(
            classify_failure("gateway_saturated"),
            SessionFailureCode::Gateway(koru_session_api::GatewayError::GatewaySaturated)
        );
        assert_eq!(
            classify_failure("future_failure"),
            SessionFailureCode::Unknown("future_failure".into())
        );
    }

    #[test]
    fn local_limits_reject_before_encoding() {
        assert!(validate_input(1, true).is_ok());
        assert!(matches!(
            validate_input(koru_session_api::MAX_INPUT_BYTES + 1, true),
            Err(SessionsError::Local(SessionFailure {
                code: koru_session_protocol::ErrorCode::InputTooLarge,
                ..
            }))
        ));
    }
}
