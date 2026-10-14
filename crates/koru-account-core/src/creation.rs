// SPDX-License-Identifier: MPL-2.0

//! Prepared explicit account creation. Only the repository transaction may
//! turn a confirmation into identities and a recoverable continuation.

use crate::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreationResolution {
    Unseen,
    Returning {
        koru_identity_id: KoruIdentityId,
        auth_identity_id: AuthIdentityId,
    },
    Collision {
        targets: Vec<KoruIdentityId>,
    },
}

#[derive(Clone, PartialEq, Eq)]
pub struct PreparedCreationResolution {
    pub expected: CreationResolution,
    /// Proposed rows for Unseen; existing rows for Returning; absent for Collision.
    pub identity: Option<KoruIdentity>,
    pub auth_identity: Option<AuthIdentity>,
    pub collision_flow: Option<TemporaryAuthFlow>,
}

/// The trusted service constructs this only after opening its protected
/// artifact and checking the caller's secret. All prepared fields are derived
/// from that immutable opened artifact, never caller-supplied owner/claim data.
/// The repository compares both the digest and exact sealed artifact before
/// resolution, including after acquiring the authority row lock.
#[derive(Clone, PartialEq, Eq)]
pub struct AccountCreationRequest {
    pub confirmation_flow_id: AuthFlowId,
    pub kind: AuthFlowKind,
    pub state_digest: KeyedDigest,
    pub expected_sealed_artifact: SealedValue,
    pub expires_at_millis: UnixMillis,
    pub now_millis: UnixMillis,
    pub decision: AccountCreationDecision,
}

#[derive(Clone, PartialEq, Eq)]
pub enum AccountCreationDecision {
    Cancel {
        sealed_terminal_result: SealedValue,
    },
    Confirm {
        verified: VerifiedExternalIdentity,
        /// The original provider verification time, never the confirmation time.
        verified_at_millis: UnixMillis,
        expected: CreationResolution,
        new_identity: Option<KoruIdentity>,
        new_auth_identity: Option<AuthIdentity>,
        /// One exchange grant, or collision candidate plus fresh-proof start.
        child_flows: Vec<TemporaryAuthFlow>,
        sealed_terminal_result: SealedValue,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountCreationOutcome {
    /// The stored terminal outcome wins even when this request asks for the
    /// opposite decision. The service decodes it without resolving again.
    Terminal {
        sealed_terminal_result: SealedValue,
        replayed: bool,
    },
    /// No mutation committed. Prepare a continuation for this current state
    /// and retry within the original confirmation deadline.
    Reprepare(CreationResolution),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountCreationError {
    FlowNotFound,
    FlowExpired,
    InvalidBinding,
    IdentityDisabled,
    InvalidPreparation,
    Unavailable,
    Corrupt,
}

macro_rules! redacted_creation_debug {
    ($($ty:ty),+ $(,)?) => { $(
        impl std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!(stringify!($ty), "(<redacted>)"))
            }
        }
    )+ };
}
redacted_creation_debug!(
    PreparedCreationResolution,
    AccountCreationRequest,
    AccountCreationDecision
);
