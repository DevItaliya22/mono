// SPDX-License-Identifier: MPL-2.0

use koru_account_core::{
    Clock, CryptoError, Entropy, KeyedDigest, OpaqueSecret, SealedValue, TransientEncryption,
    UnixMillis,
};
use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey},
    hmac,
    rand::{SecureRandom, SystemRandom},
};

const FORMAT_VERSION: u8 = 1;
const NONCE_BYTES: usize = 12;
const KEY_BYTES: usize = 32;

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_millis(&self) -> UnixMillis {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
            .unwrap_or(0)
    }
}

#[derive(Debug)]
pub struct SystemEntropy(SystemRandom);

impl Default for SystemEntropy {
    fn default() -> Self {
        Self(SystemRandom::new())
    }
}

impl Entropy for SystemEntropy {
    fn fill(&self, output: &mut [u8]) -> Result<(), CryptoError> {
        self.0.fill(output).map_err(|_| CryptoError::KeyUnavailable)
    }
}

/// Restart-stable authenticated encryption and keyed digests for temporary
/// account artifacts. Hosts own and persist both independent 256-bit keys;
/// ciphertext stores only a version, random nonce and AEAD output.
pub struct AeadTransientEncryption<R> {
    encryption_key: LessSafeKey,
    digest_key: hmac::Key,
    entropy: R,
}

impl<R> std::fmt::Debug for AeadTransientEncryption<R> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AeadTransientEncryption(<redacted>)")
    }
}

impl<R> AeadTransientEncryption<R> {
    pub fn new(
        encryption_key: OpaqueSecret,
        digest_key: OpaqueSecret,
        entropy: R,
    ) -> Result<Self, CryptoError> {
        if encryption_key.expose_secret().len() != KEY_BYTES
            || digest_key.expose_secret().len() != KEY_BYTES
            || encryption_key.expose_secret() == digest_key.expose_secret()
        {
            return Err(CryptoError::InvalidInput);
        }
        let encryption_key = UnboundKey::new(&aead::AES_256_GCM, encryption_key.expose_secret())
            .map_err(|_| CryptoError::InvalidInput)?;
        Ok(Self {
            encryption_key: LessSafeKey::new(encryption_key),
            digest_key: hmac::Key::new(hmac::HMAC_SHA256, digest_key.expose_secret()),
            entropy,
        })
    }
}

impl AeadTransientEncryption<SystemEntropy> {
    pub fn system(
        encryption_key: OpaqueSecret,
        digest_key: OpaqueSecret,
    ) -> Result<Self, CryptoError> {
        Self::new(encryption_key, digest_key, SystemEntropy::default())
    }
}

impl<R: Entropy> TransientEncryption for AeadTransientEncryption<R> {
    fn seal(&self, purpose: &'static str, plaintext: &[u8]) -> Result<SealedValue, CryptoError> {
        if purpose.is_empty() {
            return Err(CryptoError::InvalidInput);
        }
        let mut nonce_bytes = [0_u8; NONCE_BYTES];
        self.entropy.fill(&mut nonce_bytes)?;
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let capacity = 1_usize
            .checked_add(NONCE_BYTES)
            .and_then(|size| size.checked_add(plaintext.len()))
            .and_then(|size| size.checked_add(aead::AES_256_GCM.tag_len()))
            .ok_or(CryptoError::InvalidInput)?;
        let mut ciphertext = plaintext.to_vec();
        self.encryption_key
            .seal_in_place_append_tag(nonce, Aad::from(purpose.as_bytes()), &mut ciphertext)
            .map_err(|_| CryptoError::KeyUnavailable)?;
        let mut output = Vec::with_capacity(capacity);
        output.push(FORMAT_VERSION);
        output.extend_from_slice(&nonce_bytes);
        output.extend_from_slice(&ciphertext);
        Ok(SealedValue(output))
    }

    fn open(&self, purpose: &'static str, sealed: &SealedValue) -> Result<Vec<u8>, CryptoError> {
        let minimum = 1 + NONCE_BYTES + aead::AES_256_GCM.tag_len();
        if purpose.is_empty()
            || sealed.0.len() < minimum
            || sealed.0.first() != Some(&FORMAT_VERSION)
        {
            return Err(CryptoError::VerificationFailed);
        }
        let mut nonce_bytes = [0_u8; NONCE_BYTES];
        nonce_bytes.copy_from_slice(&sealed.0[1..1 + NONCE_BYTES]);
        let mut ciphertext = sealed.0[1 + NONCE_BYTES..].to_vec();
        let plaintext = self
            .encryption_key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::from(purpose.as_bytes()),
                &mut ciphertext,
            )
            .map_err(|_| CryptoError::VerificationFailed)?;
        let length = plaintext.len();
        ciphertext.truncate(length);
        Ok(ciphertext)
    }

    fn digest(&self, purpose: &'static str, value: &[u8]) -> Result<KeyedDigest, CryptoError> {
        if purpose.is_empty() {
            return Err(CryptoError::InvalidInput);
        }
        let mut context = hmac::Context::with_key(&self.digest_key);
        context.update(purpose.as_bytes());
        context.update(&[0]);
        context.update(value);
        Ok(KeyedDigest(context.sign().as_ref().to_vec()))
    }
}
