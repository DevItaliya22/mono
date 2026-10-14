// SPDX-License-Identifier: MPL-2.0

use super::Refusal;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use koru_device::cloud::CloudReplicaPrincipalV1;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const MAX_HANDSHAKE_BYTES: usize = 32_768;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version1;
impl Serialize for Version1 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u8(1)
    }
}
impl<'de> Deserialize<'de> for Version1 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        if u32::deserialize(d)? != 1 {
            return Err(serde::de::Error::custom("unknown handshake version"));
        }
        Ok(Self)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Bytes<const N: usize>(pub [u8; N]);
impl<const N: usize> Serialize for Bytes<N> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&URL_SAFE_NO_PAD.encode(self.0))
    }
}
impl<'de, const N: usize> Deserialize<'de> for Bytes<N> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = String::deserialize(d)?;
        if value.len() != N.div_ceil(3) * 4 - (3 - N % 3) % 3 {
            return Err(serde::de::Error::custom("invalid evidence length"));
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(&value)
            .map_err(serde::de::Error::custom)?;
        if URL_SAFE_NO_PAD.encode(&bytes) != value {
            return Err(serde::de::Error::custom("noncanonical evidence"));
        }
        bytes
            .try_into()
            .map(Self)
            .map_err(|_| serde::de::Error::custom("wrong evidence size"))
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HandshakeV1 {
    ClientHello {
        version: Version1,
        dataset: CloudReplicaPrincipalV1,
        client_nonce: Bytes<32>,
        credential: String,
    },
    WorkerChallenge {
        version: Version1,
        server_nonce: Bytes<32>,
    },
    ClientFinish {
        version: Version1,
        signature: Bytes<64>,
    },
    SessionAccepted {
        version: Version1,
        session_id: Bytes<32>,
        generation: u64,
        expires_at_seconds: i64,
    },
    Refused {
        version: Version1,
        code: String,
    },
}

impl std::fmt::Debug for HandshakeV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HandshakeV1(<redacted>)")
    }
}

pub fn decode(bytes: &[u8]) -> Result<HandshakeV1, Refusal> {
    if bytes.is_empty() || bytes.len() > MAX_HANDSHAKE_BYTES {
        // Length only, never content: an empty read is a closed stream,
        // and that is a different fault from a message that would not parse.
        tracing::warn!(
            len = bytes.len(),
            "cloud handshake: message length out of bounds"
        );
        return Err(Refusal::Malformed);
    }
    let message: HandshakeV1 = serde_json::from_slice(bytes).map_err(|_| {
        tracing::warn!(len = bytes.len(), "cloud handshake: message did not parse");
        Refusal::Malformed
    })?;
    match &message {
        HandshakeV1::ClientHello {
            credential: token, ..
        } => {
            if token.is_empty() || token.len() > 10_240 || !token.is_ascii() {
                return Err(Refusal::Malformed);
            }
        }
        HandshakeV1::SessionAccepted {
            generation,
            expires_at_seconds,
            ..
        } => {
            if *generation == 0 || *expires_at_seconds < 0 {
                return Err(Refusal::Malformed);
            }
        }
        HandshakeV1::Refused { code, .. } => {
            if code.is_empty()
                || code.len() > 64
                || !code
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            {
                return Err(Refusal::Malformed);
            }
        }
        _ => {}
    }
    Ok(message)
}

pub fn encode(message: &HandshakeV1) -> Result<Vec<u8>, Refusal> {
    let bytes = serde_json::to_vec(message).map_err(|_| Refusal::Malformed)?;
    decode(&bytes)?;
    let mut framed = Vec::with_capacity(bytes.len() + 4);
    framed.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    framed.extend(bytes);
    Ok(framed)
}
