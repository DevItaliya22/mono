// SPDX-License-Identifier: MPL-2.0

use std::fmt;

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use rand_core::OsRng;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::Zeroize;

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DevicePublicKey(pub(crate) [u8; 32]);

impl DevicePublicKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_storage(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(64);
        for byte in self.0 {
            output.push(HEX[(byte >> 4) as usize] as char);
            output.push(HEX[(byte & 0x0f) as usize] as char);
        }
        output
    }

    pub fn parse(value: &str) -> Option<Self> {
        if value.len() != 64 {
            return None;
        }
        let mut bytes = [0u8; 32];
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            bytes[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
        }
        Some(Self(bytes))
    }

    pub(crate) fn verifying_key(self) -> Result<VerifyingKey, ed25519_dalek::SignatureError> {
        VerifyingKey::from_bytes(&self.0)
    }

    pub fn verify_message(&self, message: &[u8], signature: SignatureBytes) -> bool {
        use ed25519_dalek::Verifier as _;
        self.verifying_key()
            .and_then(|key| {
                key.verify(
                    message,
                    &ed25519_dalek::Signature::from_bytes(signature.as_bytes()),
                )
            })
            .is_ok()
    }
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

impl fmt::Debug for DevicePublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DevicePublicKey({:02x}{:02x}{:02x}{:02x}…)",
            self.0[0], self.0[1], self.0[2], self.0[3]
        )
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReplicationEndpointId(pub(crate) [u8; 32]);

impl ReplicationEndpointId {
    /// Cloud admission requires a decodable, nonweak Ed25519 endpoint key.
    /// This explicit check does not change existing device/peer decoding.
    pub fn validate_cloud_key(self) -> Result<(), &'static str> {
        let key = VerifyingKey::from_bytes(&self.0).map_err(|_| "invalid endpoint public key")?;
        if key.is_weak() {
            return Err("weak endpoint public key");
        }
        Ok(())
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_storage(self) -> String {
        DevicePublicKey(self.0).to_storage()
    }

    pub fn parse(value: &str) -> Option<Self> {
        DevicePublicKey::parse(value).map(|key| Self(key.0))
    }
}

impl fmt::Debug for ReplicationEndpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "EndpointId({:02x}{:02x}{:02x}{:02x}…)",
            self.0[0], self.0[1], self.0[2], self.0[3]
        )
    }
}

pub struct DeviceSigningKey(SigningKey);

impl DeviceSigningKey {
    pub fn generate() -> Self {
        Self(SigningKey::generate(&mut OsRng))
    }

    pub fn from_secret_bytes(bytes: [u8; 32]) -> Self {
        Self(SigningKey::from_bytes(&bytes))
    }

    pub fn public_key(&self) -> DevicePublicKey {
        DevicePublicKey(self.0.verifying_key().to_bytes())
    }

    pub fn secret_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    pub(crate) fn sign(&self, bytes: &[u8]) -> SignatureBytes {
        SignatureBytes(self.0.sign(bytes).to_bytes())
    }

    pub fn sign_message(&self, message: &[u8]) -> SignatureBytes {
        self.sign(message)
    }
}

impl fmt::Debug for DeviceSigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeviceSigningKey([REDACTED])")
    }
}

impl Drop for DeviceSigningKey {
    fn drop(&mut self) {
        let mut bytes = self.0.to_bytes();
        bytes.zeroize();
    }
}

pub struct ReplicationEndpointSecret(SigningKey);

impl ReplicationEndpointSecret {
    pub fn generate() -> Self {
        Self(SigningKey::generate(&mut OsRng))
    }

    pub fn from_secret_bytes(bytes: [u8; 32]) -> Self {
        Self(SigningKey::from_bytes(&bytes))
    }

    pub fn endpoint_id(&self) -> ReplicationEndpointId {
        ReplicationEndpointId(self.0.verifying_key().to_bytes())
    }

    pub fn secret_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }
}

impl fmt::Debug for ReplicationEndpointSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReplicationEndpointSecret([REDACTED])")
    }
}

impl Drop for ReplicationEndpointSecret {
    fn drop(&mut self) {
        let mut bytes = self.0.to_bytes();
        bytes.zeroize();
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub struct SignatureBytes(pub(crate) [u8; 64]);

impl SignatureBytes {
    pub fn from_bytes(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }
}

impl fmt::Debug for SignatureBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SignatureBytes([64 bytes])")
    }
}

impl Serialize for SignatureBytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for SignatureBytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let bytes = Vec::<u8>::deserialize(deserializer)?;
        let bytes: [u8; 64] = bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("signature must be 64 bytes"))?;
        Ok(Self(bytes))
    }
}
