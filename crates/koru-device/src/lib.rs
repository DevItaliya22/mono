// SPDX-License-Identifier: MPL-2.0

//! Device identity and trust policy shared by Agent and Store-facing control code.
//!
//! This crate is deliberately platform-free. It defines signed facts and pure
//! authorization decisions; the Agent owns durable storage and OS key protection.

mod authorization;
pub mod cloud;
mod ids;
mod keys;
mod ports;
mod records;
mod signing;

pub use authorization::{
    Authorization, AuthorizationDecision, AuthorizationEntry, AuthorizationSnapshot,
    AuthorizedDevice, AuthorizedReplica, DenyReason, GossipRejection, ProfileLifecycleOrder,
    TrustView, compare_profile_lifecycle, verify_gossip_record,
};
pub use ids::{DeviceId, RecordId, RelationshipId};
pub use keys::{
    DevicePublicKey, DeviceSigningKey, ReplicationEndpointId, ReplicationEndpointSecret,
    SignatureBytes,
};
pub use ports::{
    ApplyOutcome, DeviceError, SecretBytes, SecretName, SecretStore, StoredLocalDevice,
    TrustRepository,
};
pub use records::{
    AllDataGrantV1, BuildDrift, BuildPayload, DeviceBuildV1, DeviceDescriptorV1, DeviceNameV1,
    DriftSeverity, EndpointBindingV1, MAX_BUILD_FIELD_CHARS, MAX_BUILD_PAYLOAD_BYTES,
    MAX_DEVICE_NAME_CHARS, ProfileGrantV1, ProfileLifecycleStateV1, ProfileLifecycleV1,
    RevocationV1, TrustRecord, TrustRelationshipV1, VerifiedTrustRecord, VerifyError,
    sort_for_gossip,
};
pub use signing::transcript_hash;

#[cfg(test)]
mod tests;
