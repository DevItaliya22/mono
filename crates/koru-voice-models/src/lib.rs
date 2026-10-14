// SPDX-License-Identifier: MPL-2.0

//! **Which models exist, where their verified bytes are on this machine, and
//! how they get there.**
//!
//! It knows **no engine and no audio**. It hands out a directory and a signed
//! descriptor; loading them into a recogniser is an engine adapter's job and
//! opening a microphone is the worker's. That exclusion is the charter rather
//! than a coincidence of what has been written so far — a crate that linked an
//! inference runtime could not be depended on by a settings page, and one that
//! knew about `cpal` could not be compiled into a test that must never open an
//! audio device.
//!
//! ## What Plan 1.1 put here, and what it deliberately did not
//!
//! This is the **seam** between Plan 1.2 (the downloader and the filesystem
//! store) and Plan 1.3 (the worker). Rev 1 of the roadmap called those parallel
//! while the worker depended on an API the store had not specified. They are
//! parallel now because the install, lease and load interfaces are *normative
//! Rust* rather than prose, and prose is what two agents interpret differently.
//!
//! | | |
//! | --- | --- |
//! | [`ModelTier`] | compatibility re-export of the contract crate's quality choice, never a model name |
//! | [`ModelDescriptor`] | every field that is **signed**, and the canonical bytes it signs as |
//! | [`CatalogEntry`] / [`Catalog`] | the descriptor, plus the unsigned facts about where to get it |
//! | [`CatalogSource`] | where that document comes from — `KORU_VOICE_CATALOG`, then [`DEFAULT_CATALOG_URL`] |
//! | [`TrustedKeys`] | the pinned model-signing key, and the three ways a signature can fail |
//! | [`TrustPolicy`] | how much a model has to prove — and the guarded, loud, MVP-only second answer |
//! | [`ModelStore`] / [`ModelLease`] / [`LoadedModel`] | install, lease, load — the interfaces the worker consumes |
//! | [`ModelSource`] | where bytes come from |
//! | [`anything_installed`] | whether a dictation could start — **derived from the disk, never from a flag** |
//! | [`ModelErrorCode`] | one closed list of stable tokens, and one total mapping into the session's |
//!
//! Plan 1.2 filled in the behaviour behind them, and added three things of its
//! own:
//!
//! | | |
//! | --- | --- |
//! | [`ModelPaths`] | the cache root — `KORU_VOICE_MODELS`, then `--root`, then the channel-independent default — and every path inside it |
//! | [`HttpSource`] | the resumable downloader: a digest-keyed `.part`, `If-Range` with `Content-Range` validation, explicit 200 / 206 / 416, coalesced progress |
//! | [`DiskModelStore`] | the store on a disk: atomic publish, a per-digest cross-process install lock, leases, prune, remove, and crash recovery for all three |
//!
//! ## Four things it is easy to get wrong
//!
//! **A digest is not a trust root.** It is delivered by the same catalog as the
//! URL, so whoever can serve a bad model can serve its matching digest. The
//! signature is over a canonical [`ModelDescriptor`] covering every
//! behaviour-bearing field — not over the archive, and not over the digest
//! alone — and
//! [ADR-250](../../../docs/decisions/ADR-226-250.md#adr-250-voicemodels--models-are-signed-by-one-pinned-model-signing-key-over-a-canonical-descriptor-and-the-cache-is-keyed-by-the-digest)
//! argues field by field why each one is inside the envelope.
//!
//! **Mirror URLs are outside the envelope, on purpose.** A host can be added
//! without re-signing every model, and the signed size and digest already
//! constrain what any mirror is allowed to return.
//!
//! **`MODEL_SIGNING_KEYS` is empty, and that is still the honest state of the
//! tree.** Minting the key is not a decision an implementation plan takes on
//! its own — it is a key somebody has to hold, rotate and keep offline — so
//! every install is refused with [`ModelErrorCode::NoTrustRoot`], which is the
//! correct behaviour for a build that cannot verify anything. The alternative,
//! a digest-only path while a key is pending, is exactly the degradation the
//! ADR refuses and would be invisible. Every verification vector in this crate
//! is built from a *generated* key pair for that reason, so the path is proven
//! without the shipped key existing; `docs/features/voice/archive/QUESTIONS.md` Q-V14 is
//! where that state is recorded.
//!
//! **Cache identity is the digest.** `<id>/<version>/<digest>/`. A catalog that
//! reuses an id and version with different bytes is
//! [`IdentityConflict`](ModelErrorCode::IdentityConflict) — a hard failure,
//! never a silent replace.
//!
//! ## What it deliberately does not do
//!
//! No inference, no audio, no engine loading. No channel: the model cache is
//! channel-independent, so a crate that knew which channel it was would be a
//! crate that could get that wrong. No UI and no translated string — the codes
//! here are branched on, and a surface looks up its own words in `koru-i18n`.

pub mod catalog;
pub mod descriptor;
pub mod disk;
pub mod error;
pub mod paths;
pub mod source;
pub mod store;
pub mod tier;
pub mod trust;

/// The cross-process locking, leasing and staging primitives. Private on
/// purpose: they are a *mechanism*, and the guarantees they exist for —
/// atomic publish, one install per digest, a lease a `prune` cannot ignore —
/// are [`DiskModelStore`]'s to make. A caller reaching for a lock directly is a
/// caller that will eventually take the wrong one.
mod lock;
/// Progress coalescing. Private for the same reason: the rule is a promise
/// [`InstallProgress`] makes on behalf of everything that emits one, not a
/// utility a caller opts into.
mod progress;

pub use catalog::{
    CATALOG_SCHEMA_VERSION, Catalog, CatalogEntry, CatalogSource, DEFAULT_CATALOG_URL,
};
pub use descriptor::{
    DESCRIPTOR_SCHEMA_VERSION, LicenceRef, ModelDescriptor, ModelSignature, ModelVersion,
    Sha256Digest,
};
pub use disk::{DiskModelStore, InstalledMetadata, METADATA_FILE, METADATA_SCHEMA_VERSION};
pub use error::{ModelError, ModelErrorCode};
pub use paths::ModelPaths;
pub use source::{DirectorySource, HttpSource, ModelSource, NoNetworkSource};
pub use store::{
    InstallProgress, InstallStage, InstalledModel, LoadedModel, ModelLease, ModelStore,
    anything_installed, choose_installed,
};
pub use tier::ModelTier;
pub use trust::{MODEL_SIGNING_KEYS, TrustPolicy, TrustedKeys};

#[cfg(test)]
mod fixtures;
/// A local fixture HTTP server. **No test in this crate reaches the network**,
/// and this is what makes that true of the resume suite as well as of the rest.
#[cfg(test)]
mod testserver;
