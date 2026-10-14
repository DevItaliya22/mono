// SPDX-License-Identifier: MPL-2.0

use sha2::{Digest, Sha256};

use crate::{DeviceId, DevicePublicKey, RecordId, RelationshipId, ReplicationEndpointId};

pub(crate) struct Canonical(Vec<u8>);

impl Canonical {
    pub(crate) fn new(domain: &'static [u8]) -> Self {
        let mut bytes = Vec::with_capacity(domain.len() + 128);
        bytes.extend_from_slice(domain);
        Self(bytes)
    }

    pub(crate) fn u32(mut self, value: u32) -> Self {
        self.0.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub(crate) fn u64(mut self, value: u64) -> Self {
        self.0.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub(crate) fn i64(mut self, value: i64) -> Self {
        self.0.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub(crate) fn device(mut self, value: DeviceId) -> Self {
        self.0.extend_from_slice(value.as_uuid().as_bytes());
        self
    }

    pub(crate) fn relationship(mut self, value: RelationshipId) -> Self {
        self.0.extend_from_slice(value.as_uuid().as_bytes());
        self
    }

    pub(crate) fn record(mut self, value: RecordId) -> Self {
        self.0.extend_from_slice(value.as_uuid().as_bytes());
        self
    }

    pub(crate) fn public_key(mut self, value: DevicePublicKey) -> Self {
        self.0.extend_from_slice(value.as_bytes());
        self
    }

    pub(crate) fn endpoint(mut self, value: ReplicationEndpointId) -> Self {
        self.0.extend_from_slice(value.as_bytes());
        self
    }

    pub(crate) fn fixed32(mut self, value: [u8; 32]) -> Self {
        self.0.extend_from_slice(&value);
        self
    }

    pub(crate) fn variable(mut self, value: &[u8]) -> Self {
        let len = u32::try_from(value.len()).expect("signed fields are limited to u32 bytes");
        self.0.extend_from_slice(&len.to_be_bytes());
        self.0.extend_from_slice(value);
        self
    }

    pub(crate) fn finish(self) -> Vec<u8> {
        self.0
    }
}

pub fn transcript_hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
