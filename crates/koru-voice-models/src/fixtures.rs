// SPDX-License-Identifier: MPL-2.0

//! **What every suite in this crate builds its vectors out of.**
//!
//! One descriptor and one signer, shared, so that *"it verifies"* means the same
//! thing in five files — the lesson `koru_store_core::fixtures` already carries
//! for the eight languages.
//!
//! **The key pair is generated, not checked in.** A fixture signature stored as
//! a string would go stale the first time the canonical form changed, and the
//! repair — regenerating it — is exactly the step somebody skips by relaxing the
//! assertion instead. Generating it means the vectors are recomputed on every
//! run and a change to [`crate::ModelDescriptor::canonical_bytes`] shows up as a
//! failure in the one test that pins the bytes, not as ten failures nobody can
//! read.

use crate::catalog::{CATALOG_SCHEMA_VERSION, Catalog, CatalogEntry};
use crate::descriptor::{ModelDescriptor, ModelSignature, ModelVersion, Sha256Digest};
use crate::tier::ModelTier;
use crate::trust::TrustedKeys;
use koru_voice::{EngineId, ModelId, VoiceLanguage};
use minisign::{KeyPair, SecretKey, sign};
use std::io::Cursor;
use url::Url;

/// The digest of the three bytes `abc`, which is also what
/// [`archive_bytes`] contains — so a fixture archive and a fixture descriptor
/// agree without either restating the other.
pub const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

pub fn archive_bytes() -> &'static [u8] {
    b"abc"
}

/// A descriptor that passes every rule, for a test to then break one field of.
pub fn descriptor() -> ModelDescriptor {
    ModelDescriptor {
        schema_version: crate::DESCRIPTOR_SCHEMA_VERSION,
        id: ModelId::parse("parakeet-tdt-v3").unwrap(),
        version: ModelVersion::parse("1.2.0").unwrap(),
        engine: EngineId::parse("sherpa").unwrap(),
        tier: ModelTier::Compact,
        languages: vec![VoiceLanguage::En, VoiceLanguage::ZhHans],
        size: archive_bytes().len() as u64,
        digest: Sha256Digest::parse(ABC_SHA256).unwrap(),
        working_set_bytes: 1_073_741_824,
        licence: crate::LicenceRef::parse("Apache-2.0").unwrap(),
        attribution: "k2-fsa".to_string(),
    }
}

/// A key pair, and the [`TrustedKeys`] that pins its public half.
pub struct Signer {
    pub trusted: TrustedKeys,
    secret: SecretKey,
    public: minisign::PublicKey,
}

impl Signer {
    pub fn new() -> Signer {
        let KeyPair { pk, sk } = KeyPair::generate_unencrypted_keypair().unwrap();
        let trusted = TrustedKeys::from_public_keys(&[pk.to_box().unwrap().to_string()]).unwrap();
        Signer {
            trusted,
            secret: sk,
            public: pk,
        }
    }

    /// The public half, as a `minisign.pub` block — what would be added to
    /// [`crate::MODEL_SIGNING_KEYS`].
    pub fn public_key_text(&self) -> String {
        self.public.to_box().unwrap().to_string()
    }

    /// Sign a descriptor's canonical bytes, the way the publisher will.
    pub fn sign(&self, descriptor: &ModelDescriptor) -> ModelSignature {
        self.sign_bytes(&descriptor.canonical_bytes().unwrap())
    }

    /// Sign arbitrary bytes — which is how a test builds *a real signature over
    /// something else*, the case a digest check alone would never catch.
    pub fn sign_bytes(&self, bytes: &[u8]) -> ModelSignature {
        let signature = sign(
            Some(&self.public),
            &self.secret,
            Cursor::new(bytes.to_vec()),
            Some("koru model fixture"),
            Some("koru model fixture"),
        )
        .unwrap()
        .to_string();
        ModelSignature::parse(signature).unwrap()
    }
}

/// An entry the given signer vouches for.
pub fn entry(signer: &Signer, descriptor: ModelDescriptor) -> CatalogEntry {
    let signature = signer.sign(&descriptor);
    CatalogEntry {
        descriptor,
        signature,
        url: Url::parse("https://models.koru.do/v1/parakeet-1.2.0.tar.gz").unwrap(),
        mirrors: Vec::new(),
    }
}

pub fn catalog(entries: Vec<CatalogEntry>) -> Catalog {
    Catalog {
        schema_version: CATALOG_SCHEMA_VERSION,
        entries,
    }
}
