// SPDX-License-Identifier: MPL-2.0

//! **The four independent regional cryptographic purposes (D30/D57/D59).**
//!
//! A region operates four key hierarchies. They share no key, no key id space
//! and no rotation schedule, and the review gate rejects any design in which
//! one key serves two of them:
//!
//! | Purpose | Kind | What it does | Where the private half lives |
//! | --- | --- | --- | --- |
//! | [`RegionalPurpose::TokenCapabilitySigning`] | asymmetric, signing | Signs sync tokens, relay tokens and object capabilities (Plans 3.5–3.7) | Regional KMS |
//! | [`RegionalPurpose::TrustResetAttestation`] | asymmetric, signing | Signs `ProfileTrustEpochReset` (D22, Plan 2.5) | Regional KMS |
//! | [`RegionalPurpose::WorkspaceRecoveryUnwrap`] | asymmetric, HPKE recipient | Opens the recovery grant that reaches a WEK (D15, Plan 2.2) | **KMS/HSM, non-exportable** |
//! | [`RegionalPurpose::AccountMetadataEncryption`] | **symmetric**, a KEK | Wraps the per-account metadata key (D59) | Regional KMS |
//!
//! The fourth is symmetric and the other three are not, which is worth saying
//! out loud: it is the one that cannot be "just another signing key with a
//! different label", and it is the one whose destruction is a customer-visible
//! act.
//!
//! # The recovery authority is the crown jewel (D88)
//!
//! Beyond not being reused, the workspace-recovery/unwrap key is **versioned,
//! non-exportable, separately permissioned, separately audited, and callable
//! only through the recovery service**. Those five are operational properties
//! that no Rust type can enforce; what this crate can do is make the format
//! carry what they need — a per-grant KEM (so the key may be P-256 if the
//! provider cannot do X25519 in protected hardware), a `recipient_key_id` with
//! a version, and a `recovery_key_id` that allows sharding later without a
//! format change.
//!
//! # Why the type system carries this and not just a comment
//!
//! Four newtypes over the same 16 bytes cost nothing and make "use the metadata
//! KEK id where a recovery key id belongs" a compile error rather than an
//! incident. The cryptographic half of the separation — a key of one purpose
//! failing when offered for another — is in the `purpose` bound into every
//! context, and `tests.rs` proves it with raw bytes rather than with types.

use crate::keys::KeyId;

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RegionalPurpose {
    TokenCapabilitySigning,
    TrustResetAttestation,
    WorkspaceRecoveryUnwrap,
    AccountMetadataEncryption,
}

impl RegionalPurpose {
    /// Every purpose, so a test can assert the set has four members and that no
    /// two of them share anything.
    pub const ALL: [Self; 4] = [
        Self::TokenCapabilitySigning,
        Self::TrustResetAttestation,
        Self::WorkspaceRecoveryUnwrap,
        Self::AccountMetadataEncryption,
    ];

    /// The stable label, used in audit records and in the format document. Not
    /// a translated string and never shown to a person.
    pub const fn label(self) -> &'static str {
        match self {
            Self::TokenCapabilitySigning => "TOKEN_CAPABILITY_SIGNING",
            Self::TrustResetAttestation => "TRUST_RESET_ATTESTATION",
            Self::WorkspaceRecoveryUnwrap => "WORKSPACE_RECOVERY_UNWRAP",
            Self::AccountMetadataEncryption => "ACCOUNT_METADATA_ENCRYPTION",
        }
    }

    /// Whether the hierarchy is symmetric. Exactly one is.
    pub const fn is_symmetric(self) -> bool {
        matches!(self, Self::AccountMetadataEncryption)
    }
}

macro_rules! regional_key_id {
    ($(#[$doc:meta])* $name:ident, $purpose:expr) => {
        $(#[$doc])*
        ///
        /// A newtype over [`KeyId`] with **no conversion from any other
        /// regional key id**. That is D30 expressed where it is cheapest to
        /// enforce.
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(KeyId);

        impl $name {
            pub const PURPOSE: RegionalPurpose = $purpose;

            pub fn new(id: KeyId) -> Self {
                Self(id)
            }

            pub fn key_id(self) -> KeyId {
                self.0
            }
        }
    };
}

regional_key_id!(
    /// Names a key in the token/capability signing hierarchy.
    TokenSigningKeyId,
    RegionalPurpose::TokenCapabilitySigning
);
regional_key_id!(
    /// Names a key in the trust-reset attestation hierarchy.
    TrustResetKeyId,
    RegionalPurpose::TrustResetAttestation
);
regional_key_id!(
    /// Names a key in the workspace-recovery/unwrap hierarchy — the crown
    /// jewel of D88.
    RecoveryUnwrapKeyId,
    RegionalPurpose::WorkspaceRecoveryUnwrap
);
regional_key_id!(
    /// Names the regional KEK that wraps per-account metadata keys.
    MetadataKekId,
    RegionalPurpose::AccountMetadataEncryption
);

/// Identifies a **recovery authority**, not a key.
///
/// D88 puts this in every grant so that recovery keys can later be sharded — by
/// account, by workspace bucket, by anything — **without a format change**. It
/// is the shard; [`RecoveryUnwrapKeyId`] with its version is the key inside the
/// shard that a particular grant was sealed to.
///
/// It is meaningful **only on a recovery grant**. On a device or replica grant
/// the field is [`KeyId::NONE`] always — Koru-managed or customer-managed
/// alike — so that it can never be read as a promise about what Koru will do.
/// The enforcement of D89 is the absence of the grant, not the contents of a
/// field. See [`crate::grant::GrantSet`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecoveryKeyId(KeyId);

impl RecoveryKeyId {
    pub fn new(id: KeyId) -> Self {
        Self(id)
    }

    pub fn key_id(self) -> KeyId {
        self.0
    }
}

#[cfg(test)]
mod tests;
