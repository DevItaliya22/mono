// SPDX-License-Identifier: MPL-2.0

//! Fresh device-key evidence for one account registration and one server flow.
use crate::*;
use koru_device::{DeviceSigningKey, SignatureBytes};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceOwnershipChallenge {
    pub flow_id: AuthFlowId,
    /// The native exchange grant flow; `None` means an authenticated registration.
    pub parent_flow_id: Option<AuthFlowId>,
    pub device: AccountDevice,
    pub expires_at_millis: UnixMillis,
    pub nonce: [u8; 32],
}

impl DeviceOwnershipChallenge {
    /// Fixed field order, length-prefixed variable values, and a protocol domain.
    pub fn signing_bytes(&self) -> Vec<u8> {
        fn field(bytes: &mut Vec<u8>, value: &[u8]) {
            bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
            bytes.extend_from_slice(value);
        }
        let mut bytes = b"koru/account/device-ownership/v1\0".to_vec();
        field(&mut bytes, self.flow_id.to_string().as_bytes());
        field(
            &mut bytes,
            self.parent_flow_id
                .map(|id| id.to_string())
                .unwrap_or_default()
                .as_bytes(),
        );
        field(&mut bytes, self.device.device_id().to_string().as_bytes());
        field(
            &mut bytes,
            self.device.descriptor.signing_public_key.as_bytes(),
        );
        bytes.extend_from_slice(&self.device.descriptor.version.to_be_bytes());
        field(&mut bytes, self.device.descriptor.display_name.as_bytes());
        bytes.extend_from_slice(&self.device.descriptor.created_at_millis.to_be_bytes());
        field(&mut bytes, self.device.descriptor.endpoint_id.as_bytes());
        field(
            &mut bytes,
            self.device.descriptor.endpoint_binding_signature.as_bytes(),
        );
        let registration = &self.device.registration;
        field(
            &mut bytes,
            registration.koru_identity_id.to_string().as_bytes(),
        );
        bytes.push(match registration.platform {
            DevicePlatform::Windows => 1,
            DevicePlatform::Macos => 2,
            DevicePlatform::Ios => 3,
            DevicePlatform::Android => 4,
            DevicePlatform::Unknown => 5,
        });
        field(&mut bytes, registration.app_version.as_bytes());
        for version in [
            registration.enrollment_protocol_version,
            registration.peer_protocol_version,
        ] {
            bytes.push(u8::from(version.is_some()));
            bytes.extend_from_slice(&version.unwrap_or_default().to_be_bytes());
        }
        bytes.extend_from_slice(&self.expires_at_millis.to_be_bytes());
        bytes.extend_from_slice(&self.nonce);
        bytes
    }

    pub fn matches_registration(&self, device: &AccountDevice) -> bool {
        self.device.descriptor == device.descriptor
            && self.device.registration.koru_identity_id == device.registration.koru_identity_id
            && self.device.registration.platform == device.registration.platform
            && self.device.registration.app_version == device.registration.app_version
            && self.device.registration.enrollment_protocol_version
                == device.registration.enrollment_protocol_version
            && self.device.registration.peer_protocol_version
                == device.registration.peer_protocol_version
    }

    pub fn sign(self, key: &DeviceSigningKey) -> DeviceOwnershipProof {
        let signature = key.sign_message(&self.signing_bytes());
        DeviceOwnershipProof {
            challenge: self,
            signature,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceOwnershipProof {
    pub challenge: DeviceOwnershipChallenge,
    pub signature: SignatureBytes,
}

impl DeviceOwnershipProof {
    pub fn verify_signature(&self) -> bool {
        self.challenge.device.descriptor.verify().is_ok()
            && self
                .challenge
                .device
                .descriptor
                .signing_public_key
                .verify_message(&self.challenge.signing_bytes(), self.signature)
    }
}

/// The service has verified the signature and encrypted payload. The transaction
/// must still compare the binding, lock the flow, validate expiry, and consume it.
#[derive(Clone, PartialEq, Eq)]
pub struct VerifiedDeviceOwnership {
    pub challenge: DeviceOwnershipChallenge,
    pub state_digest: KeyedDigest,
}

macro_rules! redacted_debug {
    ($($name:ident),+) => { $(impl std::fmt::Debug for $name {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str(concat!(stringify!($name), "(<redacted>)"))
        }
    })+ };
}
redacted_debug!(
    DeviceOwnershipChallenge,
    DeviceOwnershipProof,
    VerifiedDeviceOwnership
);
