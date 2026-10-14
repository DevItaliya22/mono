// SPDX-License-Identifier: MPL-2.0

use std::fmt;

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct KeyedDigest(pub Vec<u8>);

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SealedValue(pub Vec<u8>);

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OpaqueSecret(pub Vec<u8>);

impl OpaqueSecret {
    /// Borrow secret bytes only at the adapter boundary that must consume them.
    /// The wrapper's `Debug` remains redacted and owned bytes zeroize on drop.
    pub fn expose_secret(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for OpaqueSecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

macro_rules! redacted_debug {
    ($type:ty, $name:literal) => {
        impl fmt::Debug for $type {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!($name, "(<redacted>)"))
            }
        }
    };
}

redacted_debug!(KeyedDigest, "KeyedDigest");
redacted_debug!(SealedValue, "SealedValue");
redacted_debug!(OpaqueSecret, "OpaqueSecret");
