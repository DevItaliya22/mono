// SPDX-License-Identifier: MPL-2.0

//! Canonical encoding — fixed width, big-endian, no floats, no options.
//!
//! Every header in this crate is a **fixed-width struct**. Not "length-prefixed
//! where it needs to be": fixed width, every field, always. That is stronger
//! than the approved bootstrap contract's §11 requirement and it buys the thing
//! the requirement is for — *no two distinct headers can produce the same
//! bytes* — without a reader ever having to trust a length it read.
//!
//! Variable-length data appears only inside an encrypted body, and there it is
//! length-prefixed with a `u32` that is checked against what is actually
//! present before a single byte is consumed. A declared length is never used to
//! reserve, index or seek.
//!
//! **No arithmetic on a parsed length is done in `usize`.** Every bound is
//! computed in `u64` with `checked_*`, and converted once, at the end, through
//! a fallible cast. A 32-bit target and a 4 GiB declared length is the case
//! D91 asks for and it is the case this module exists to make boring.

use crate::error::{CryptoError, Result};

/// Writes a canonical byte string. Every method is infallible except the ones
/// that take a caller-supplied length, and there are none of those.
#[derive(Debug, Default)]
pub struct Writer(Vec<u8>);

impl Writer {
    pub fn with_capacity(capacity: usize) -> Self {
        Self(Vec::with_capacity(capacity))
    }

    pub fn u8(&mut self, value: u8) -> &mut Self {
        self.0.push(value);
        self
    }

    pub fn u16(&mut self, value: u16) -> &mut Self {
        self.0.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub fn u32(&mut self, value: u32) -> &mut Self {
        self.0.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub fn u64(&mut self, value: u64) -> &mut Self {
        self.0.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub fn bytes(&mut self, value: &[u8]) -> &mut Self {
        self.0.extend_from_slice(value);
        self
    }

    /// A UUID as its 16 raw bytes — the one binary spelling, matching
    /// `koru_store_core::ids` and the bootstrap contract's §11.
    pub fn uuid(&mut self, value: uuid::Uuid) -> &mut Self {
        self.0.extend_from_slice(value.as_bytes());
        self
    }

    /// A variable-length field inside an encrypted body: `u32` length then the
    /// bytes. Refuses anything a `u32` cannot describe rather than truncating.
    pub fn var(&mut self, value: &[u8]) -> Result<&mut Self> {
        let len = u32::try_from(value.len()).map_err(|_| CryptoError::TooLarge)?;
        self.u32(len);
        self.0.extend_from_slice(value);
        Ok(self)
    }

    pub fn finish(self) -> Vec<u8> {
        self.0
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Reads a canonical byte string. Every read is bounds-checked against what is
/// actually present; nothing is ever reserved on the strength of a declared
/// length.
#[derive(Debug)]
pub struct Reader<'a> {
    input: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(input: &'a [u8]) -> Self {
        Self { input, at: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.input.len() - self.at
    }

    /// The bytes consumed so far. Used by every header parser to recover the
    /// exact bytes it read, which then become the key-derivation context — so
    /// a modified header cannot decrypt, and no re-serialization step exists to
    /// disagree with the parse.
    pub fn consumed(&self) -> &'a [u8] {
        &self.input[..self.at]
    }

    pub fn rest(&self) -> &'a [u8] {
        &self.input[self.at..]
    }

    pub fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(len).ok_or(CryptoError::Malformed)?;
        if end > self.input.len() {
            return Err(CryptoError::Malformed);
        }
        let out = &self.input[self.at..end];
        self.at = end;
        Ok(out)
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let slice = self.take(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(slice);
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.array::<2>()?))
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.array::<4>()?))
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.array::<8>()?))
    }

    pub fn uuid(&mut self) -> Result<uuid::Uuid> {
        Ok(uuid::Uuid::from_bytes(self.array::<16>()?))
    }

    /// A `u32`-prefixed field. **The length is validated against the bytes that
    /// are actually there before anything is taken**, in `u64`, so a declared
    /// 4 GiB in a 40-byte buffer is a `Malformed` and never an allocation.
    pub fn var(&mut self) -> Result<&'a [u8]> {
        let declared = u64::from(self.u32()?);
        if declared > self.remaining() as u64 {
            return Err(CryptoError::Malformed);
        }
        // The comparison above is what makes this cast safe; it is written as a
        // fallible one anyway because a cast that is safe *because of an
        // invariant three lines up* is a cast that stops being safe when
        // somebody moves the check.
        let len = usize::try_from(declared).map_err(|_| CryptoError::Malformed)?;
        self.take(len)
    }

    /// Fails unless every byte has been consumed. Every parser in this crate
    /// ends with this: trailing bytes are a malformed record, not something to
    /// ignore, because "ignore what you do not understand" is how a second
    /// interpretation of the same object becomes possible.
    pub fn finish(self) -> Result<()> {
        if self.at == self.input.len() {
            Ok(())
        } else {
            Err(CryptoError::Malformed)
        }
    }
}

/// `a` and `b` are equal, in constant time. Used for commitments and key ids.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

#[cfg(test)]
mod tests;
