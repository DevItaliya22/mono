// SPDX-License-Identifier: MPL-2.0

//! Key material and the ids that name it.
//!
//! Every secret here zeroizes on drop and refuses to print itself. Every one of
//! them is **uniformly random** — none is derived from an identifier, and the
//! reason is D90's: `KDF(regional_key, account_id)` would make one key's
//! destruction meaningless, because the key could be recomputed from public
//! inputs. Deriving a per-tenant key from a tenant id defeats individual
//! crypto-erasure, and that is not a subtlety to be rediscovered later.
//!
//! The one derivation in this crate is [`crate::identity`]'s purpose subkey,
//! which is derived *from a secret root* rather than from an identifier, and
//! exists precisely so that a root is never used raw for two jobs.

use uuid::Uuid;
use zeroize::Zeroize;

use crate::sealed::{InputKey, KEY_LEN};

/// A 128-bit key identifier. Opaque, minted where the key is minted, and never
/// derived from anything about the key it names.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyId([u8; 16]);

impl KeyId {
    /// The all-zero id. **It is not a key id**: it is the value a
    /// fixed-width field carries when the field does not apply. See
    /// [`crate::grant::WekGrantV1`] for the one place that matters.
    pub const NONE: Self = Self([0u8; 16]);

    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub fn from_uuid(id: Uuid) -> Self {
        Self(*id.as_bytes())
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    pub fn as_uuid(&self) -> Uuid {
        Uuid::from_bytes(self.0)
    }

    pub fn is_none(&self) -> bool {
        *self == Self::NONE
    }
}

impl std::fmt::Debug for KeyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_none() {
            return f.write_str("KeyId(NONE)");
        }
        write!(
            f,
            "KeyId({:02x}{:02x}{:02x}{:02x}…)",
            self.0[0], self.0[1], self.0[2], self.0[3]
        )
    }
}

/// The version of a workspace encryption key.
///
/// **Its axis is independent of the blob identity key's** (D14/D17): rotating
/// the WEK changes no `CloudObjectId` and re-uploads nothing. Historical
/// objects are not rewritten on rotation, so old versions stay referenced for
/// as long as an object they open is referenced.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WekVersion(pub u32);

/// The version of a [`WorkspaceBlobIdentityKey`]. Its own axis, deliberately.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobIdentityKeyVersion(pub u32);

macro_rules! secret_key {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        pub struct $name([u8; KEY_LEN]);

        impl $name {
            pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
                Self(bytes)
            }

            /// Draws a fresh key from the caller's CSPRNG. The RNG is a
            /// parameter rather than a call to the OS, so a golden vector can
            /// be reproduced and a test never depends on entropy it cannot
            /// pin — the same reason RFC 9180's own vectors publish `ikmE`.
            pub fn generate(rng: &mut impl rand_core::CryptoRng) -> Self {
                let mut bytes = [0u8; KEY_LEN];
                rng.fill_bytes(&mut bytes);
                Self(bytes)
            }

            pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
                &self.0
            }
        }

        impl Drop for $name {
            fn drop(&mut self) {
                self.0.zeroize();
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }
    };
}

secret_key!(
    /// The **workspace encryption key**. The only thing a grant ever carries,
    /// and therefore the whole of the interface between the two layers: layer 1
    /// delivers 32 bytes, layer 2 consumes 32 bytes, and neither knows anything
    /// else about the other.
    WorkspaceEncryptionKey
);

secret_key!(
    /// A **data encryption key**: random, one per object, wrapped under the WEK
    /// version that governs the object (D86).
    ///
    /// What it buys is **per-object crypto-erasure**: destroying one object's
    /// 104-byte wrap makes that object unrecoverable even to a holder of the
    /// WEK, which is what Plan 5.4's purge needs. It is deliberately not a
    /// cheap-re-key mechanism — see `crate::envelope` for why there is nothing
    /// there to buy.
    ContentEncryptionKey
);

secret_key!(
    /// The **per-account metadata key** (D59). Random, never
    /// `KDF(regional_key, account_id)`, and stored only as
    /// `Wrap(regional_metadata_KEK, account_metadata_key)` — in **one** place,
    /// so that destroying that one record is a crypto-erasure rather than a
    /// hunt through every envelope that referenced it.
    AccountMetadataKey
);

secret_key!(
    /// The **regional metadata KEK** (D59). Lives in the regional KMS; this
    /// type exists so the format and its vectors can be exercised, not so the
    /// key can be carried around in application memory.
    RegionalMetadataKek
);

secret_key!(
    /// The **workspace blob identity key** root (D90).
    ///
    /// **Never an encryption key.** It has one job — deriving `CloudObjectId`
    /// through [`crate::identity`] — and that job goes through a purpose
    /// subkey, so if the root is ever given a second job the two cannot
    /// collide. There is deliberately no `as_input_key`-shaped path from this
    /// type into [`crate::sealed`]; the only way to reach the root's bytes is
    /// [`WorkspaceBlobIdentityKey::as_bytes`], which `identity` uses to derive
    /// and nothing else uses at all.
    WorkspaceBlobIdentityKey
);

/// The keys that may seal something, and the one that may not.
///
/// `as_input_key` is written out four times rather than generated by the macro
/// above, because the fifth key — [`WorkspaceBlobIdentityKey`] — must not have
/// it, and a macro that gave it to everything and then had it removed by hand
/// is a macro that will give it back the next time somebody adds a key. D90 is
/// a rule about what a key is *for*; the type system is where it is cheapest to
/// state.
macro_rules! sealing_key {
    ($name:ident) => {
        impl $name {
            /// The key as an input key for [`crate::sealed`].
            pub fn as_input_key(&self) -> InputKey {
                InputKey::from_bytes(self.0)
            }
        }
    };
}

sealing_key!(WorkspaceEncryptionKey);
sealing_key!(ContentEncryptionKey);
sealing_key!(AccountMetadataKey);
sealing_key!(RegionalMetadataKek);

#[cfg(test)]
mod tests;
