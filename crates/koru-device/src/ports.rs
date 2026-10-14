// SPDX-License-Identifier: MPL-2.0

use crate::{
    DeviceDescriptorV1, DeviceSigningKey, ReplicationEndpointSecret, TrustView, VerifiedTrustRecord,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SecretName {
    DeviceSigning,
    ReplicationEndpoint,
}

pub struct SecretBytes(pub [u8; 32]);

impl std::fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretBytes([REDACTED])")
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.0);
    }
}

pub trait SecretStore {
    fn load(&self, name: SecretName) -> Result<Option<SecretBytes>, DeviceError>;
    fn store_new(&self, name: SecretName, value: &SecretBytes) -> Result<(), DeviceError>;
}

pub struct StoredLocalDevice {
    pub descriptor: DeviceDescriptorV1,
    pub signing_key: DeviceSigningKey,
    pub endpoint_secret: ReplicationEndpointSecret,
}

impl std::fmt::Debug for StoredLocalDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredLocalDevice")
            .field("descriptor", &self.descriptor)
            .field("signing_key", &"[REDACTED]")
            .field("endpoint_secret", &"[REDACTED]")
            .finish()
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied { effective_revision: u64 },
    AlreadyApplied { effective_revision: u64 },
    Stale { effective_revision: u64 },
}

pub trait TrustRepository {
    fn local_device(&self) -> Result<Option<DeviceDescriptorV1>, DeviceError>;
    fn initialize(&mut self, identity: &DeviceDescriptorV1) -> Result<(), DeviceError>;
    fn apply(&mut self, verified: VerifiedTrustRecord) -> Result<ApplyOutcome, DeviceError>;
    fn trust_view(&self) -> Result<TrustView, DeviceError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceError {
    pub code: &'static str,
    pub detail: String,
}

impl DeviceError {
    pub fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for DeviceError {}
