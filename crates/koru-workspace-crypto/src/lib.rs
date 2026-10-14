// SPDX-License-Identifier: MPL-2.0

//! Koru Cloud workspace cryptography — **two layers that do not touch**.
//!
//! Read [ADR-337](../../../docs/decisions/ADR-326-350.md) and
//! [the format document](../../../docs/cloud-platform/contracts/workspace-crypto-formats-v1.md)
//! before changing anything here. Customer ciphertext written in these formats
//! cannot be reinterpreted later; a change is a re-encryption.
//!
//! # The rule this crate exists to hold (D86)
//!
//! **Layer 1 — [`grant`] — is HPKE, and it encrypts nothing but keys.** A
//! [`grant::WekGrantV1`] carries one workspace encryption key to one recipient.
//! It is a few hundred bytes. Its suite is named in its own header.
//!
//! **Layer 2 — [`sealed`] — is `ContentCipherSuiteV1`, and it encrypts
//! everything else.** Blobs, segments, snapshots, checkpoints and small at-rest
//! records are sealed with [`sealed::seal`], which is
//! [c2sp.org/chunked-encryption](https://c2sp.org/chunked-encryption)
//! instantiated as Cobblestone-256 (SHA-512 + AES-256-GCM). It contains no
//! public-key cryptography at all.
//!
//! The test of the boundary, in the maintainers' words: **changing the HPKE suite must
//! mean reissuing grants for the same WEK, and re-encrypting nothing.** It holds
//! here because the only thing a grant ever carries is a
//! [`keys::WorkspaceEncryptionKey`], and every stored object names the *WEK
//! version* that opens it — never the suite that delivered it. Nothing under
//! [`sealed`], [`content`], [`checkpoint`], [`mapping`] or [`account`] links
//! `hpke`, and `crate::tests` fails if that stops being true.
//!
//! # What a persisted envelope always carries (D87)
//!
//! `format_version`, `suite_id`, a key id with its version, and a `purpose`.
//! Every one of those bytes is bound into the key derivation, so a ciphertext
//! valid in one semantic context is not valid in another. **The writer emits
//! exactly one approved suite; readers understand older ones; suites are never
//! negotiated between clients** — RFC 9180 provides no downgrade prevention, so
//! the policy is [`suites::WRITER_SUITES`] and lives here.
//!
//! # What this crate does not do
//!
//! It holds no key material at rest, opens no file, reaches no network and
//! makes no policy decision about *whether* a recipient should have a grant.
//! Custody is Plan 2.2, enrollment is Plan 2.4, the durability state machine is
//! Plan 4.2. This crate is the bytes and the invariants.

pub mod account;
pub mod checkpoint;
pub mod codec;
pub mod content;
pub mod envelope;
pub mod error;
pub mod grant;
pub mod identity;
pub mod keys;
pub mod mapping;
pub mod regional;
pub mod sealed;
pub mod suites;

pub use error::{CryptoError, Result};

#[cfg(test)]
mod testkit;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod vectors;
