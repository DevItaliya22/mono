// SPDX-License-Identifier: MPL-2.0

use koru_session_protocol::{ErrorClass, ErrorCode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HttpStatus(pub u16);

pub const SESSION_ERROR_STATUSES: &[(ErrorClass, HttpStatus)] = &[
    (ErrorClass::InvalidRequest, HttpStatus(400)),
    (ErrorClass::Conflict, HttpStatus(409)),
    (ErrorClass::NotFound, HttpStatus(404)),
    (ErrorClass::Unavailable, HttpStatus(503)),
    (ErrorClass::Unsupported, HttpStatus(422)),
    (ErrorClass::Forbidden, HttpStatus(403)),
    (ErrorClass::Internal, HttpStatus(500)),
];

pub const fn public_status_for_class(class: ErrorClass) -> HttpStatus {
    match class {
        ErrorClass::InvalidRequest => HttpStatus(400),
        ErrorClass::Conflict => HttpStatus(409),
        ErrorClass::NotFound => HttpStatus(404),
        ErrorClass::Unavailable => HttpStatus(503),
        ErrorClass::Unsupported => HttpStatus(422),
        ErrorClass::Forbidden => HttpStatus(403),
        ErrorClass::Internal => HttpStatus(500),
    }
}

pub fn public_status(code: &ErrorCode) -> HttpStatus {
    public_status_for_class(code.class())
}

/// Gateway-owned failures that do not originate in the session runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GatewayError {
    Unauthenticated,
    PermissionDenied,
    CapabilityUnavailable,
    GatewaySaturated,
    DeadlineExceeded,
    ServerUnavailable,
}

impl GatewayError {
    pub const ALL: &[Self] = &[
        Self::Unauthenticated,
        Self::PermissionDenied,
        Self::CapabilityUnavailable,
        Self::GatewaySaturated,
        Self::DeadlineExceeded,
        Self::ServerUnavailable,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unauthenticated => "unauthenticated",
            Self::PermissionDenied => "permission_denied",
            Self::CapabilityUnavailable => "capability_unavailable",
            Self::GatewaySaturated => "gateway_saturated",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::ServerUnavailable => "server_unavailable",
        }
    }

    pub const fn status(self) -> HttpStatus {
        match self {
            Self::Unauthenticated => HttpStatus(401),
            Self::PermissionDenied => HttpStatus(403),
            Self::CapabilityUnavailable | Self::GatewaySaturated | Self::ServerUnavailable => {
                HttpStatus(503)
            }
            Self::DeadlineExceeded => HttpStatus(504),
        }
    }
}

pub const GATEWAY_ERROR_STATUSES: &[(GatewayError, HttpStatus)] = &[
    (GatewayError::Unauthenticated, HttpStatus(401)),
    (GatewayError::PermissionDenied, HttpStatus(403)),
    (GatewayError::CapabilityUnavailable, HttpStatus(503)),
    (GatewayError::GatewaySaturated, HttpStatus(503)),
    (GatewayError::DeadlineExceeded, HttpStatus(504)),
    (GatewayError::ServerUnavailable, HttpStatus(503)),
];
