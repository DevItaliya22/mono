// SPDX-License-Identifier: MPL-2.0

//! **Where the bytes live, and the one rule that decides it.**
//!
//! ```text
//! KORU_VOICE_MODELS       <that path>
//! --root given            <root>/voice/models/<id>/<version>/<digest>/
//! otherwise               %LOCALAPPDATA%\Koru\models\<id>\<version>\<digest>\
//! ```
//!
//! **Channel-independent, which is the one deliberate divergence from
//! `UpdatePaths::updates_dir()`.** An update package is per-channel state; a
//! model is immutable content addressed by its digest, and five channels must
//! not each hold 650 MB of identical bytes. `KORU_VOICE_MODELS` exists so that
//! a `--root` test reuses one download rather than paying for isolation in
//! gigabytes — which is why it deliberately points *outside* the profile root
//! it overrides, and why a test asserts that every path this type answers with
//! still lands under whichever root won.
//!
//! ## Why the `<local data>` rule is written here rather than borrowed
//!
//! `koru_channel::channel_root` knows this rule already. It is not reachable:
//! every answer that crate gives carries a channel segment, and the charter in
//! `Cargo.toml` excludes `koru-channel` precisely so that a crate which must
//! never know which channel it is cannot find out. The duplicated body is four
//! lines and is recorded as a drift hazard in
//! `docs/features/voice/archive/QUESTIONS.md` Q-V26, rather than being hidden.
//!
//! ## Four directories nothing installs into
//!
//! `.staging`, `.parts`, `.locks` and `.leases` sit beside the model ids. They
//! begin with a dot, and [`koru_voice::ModelId`] refuses a leading dot — so an
//! id can never collide with one of them, and the walk in
//! [`crate::DiskModelStore::installed`] skips every dotted entry with one rule
//! rather than four names.

use crate::descriptor::{ModelDescriptor, ModelVersion, Sha256Digest};
use crate::error::{ModelError, ModelErrorCode};
use koru_voice::ModelId;
use std::path::{Path, PathBuf};

/// Where the model cache is, and every path inside it.
///
/// A resolved value rather than a function, so that a store which was pointed
/// somewhere cannot later ask the world again and get a different answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModelPaths {
    root: PathBuf,
}

/// The staging directory's name, and the three beside it. Dotted, so no
/// [`ModelId`] can name one.
const STAGING: &str = ".staging";
const PARTS: &str = ".parts";
const LOCKS: &str = ".locks";
const LEASES: &str = ".leases";

impl ModelPaths {
    /// **The whole rule**, and it reads the world: `KORU_VOICE_MODELS` first,
    /// then a `--root` if one was given, then the channel-independent default.
    ///
    /// Fails only when there is no default to fall back to — a machine with
    /// neither `%LOCALAPPDATA%` nor `$HOME` — because a store that guessed a
    /// directory is the failure `guide/contributing/PRINCIPLES.md` names by name.
    pub fn resolve(root: Option<&Path>) -> Result<ModelPaths, ModelError> {
        ModelPaths::decide(
            koru_env::value(koru_env::name::KORU_VOICE_MODELS).map(PathBuf::from),
            root,
            local_data_base(),
        )
    }

    /// The rule with the world passed in, so the three cases are testable
    /// without `set_var` — which is a process-global write and races every
    /// other test in the binary.
    pub fn decide(
        overridden: Option<PathBuf>,
        root: Option<&Path>,
        local_data: Option<PathBuf>,
    ) -> Result<ModelPaths, ModelError> {
        if let Some(path) = overridden {
            return Ok(ModelPaths { root: path });
        }
        if let Some(root) = root {
            return Ok(ModelPaths {
                root: root.join("voice").join("models"),
            });
        }
        let base = local_data.ok_or_else(|| {
            ModelErrorCode::Io.with(
                "no local data directory: set KORU_VOICE_MODELS or pass --root, \
                 because a model store may not guess where to write",
            )
        })?;
        Ok(ModelPaths {
            root: base.join("Koru").join("models"),
        })
    }

    /// A cache root somebody named exactly — a test's temporary directory, or
    /// the value `KORU_VOICE_MODELS` carried. No `voice/models` is appended:
    /// this is the directory itself.
    pub fn at(root: impl Into<PathBuf>) -> ModelPaths {
        ModelPaths { root: root.into() }
    }

    /// The cache root. Everything else this type answers is under it, and
    /// `every_resolved_path_lands_under_the_cache_root` is what says so.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn id_dir(&self, id: &ModelId) -> PathBuf {
        self.root.join(id.as_str())
    }

    pub fn version_dir(&self, id: &ModelId, version: &ModelVersion) -> PathBuf {
        self.id_dir(id).join(version.as_str())
    }

    /// `<cache>/<id>/<version>/<digest>/` — where a published model lives.
    pub fn model_dir(&self, descriptor: &ModelDescriptor) -> PathBuf {
        self.version_dir(&descriptor.id, &descriptor.version)
            .join(descriptor.digest.as_str())
    }

    /// Where a download is assembled and where a delete moves a directory
    /// before it removes it — so a concurrent reader never meets a model that
    /// is half written or half gone.
    pub fn staging_dir(&self) -> PathBuf {
        self.root.join(STAGING)
    }

    /// Where a `.part` survives a restart. **Keyed by digest**, not by URL or
    /// by process id: the same bytes resumed from a different mirror are still
    /// the same bytes.
    pub fn parts_dir(&self) -> PathBuf {
        self.root.join(PARTS)
    }

    /// The archive for one digest, once it is complete.
    pub fn archive_path(&self, digest: &Sha256Digest) -> PathBuf {
        self.parts_dir().join(format!("{digest}.archive"))
    }

    /// The per-digest install lock's file. One lock per set of bytes rather
    /// than one for the cache, because two people installing two different
    /// models have nothing to say to each other.
    pub fn install_lock(&self, digest: &Sha256Digest) -> PathBuf {
        self.root.join(LOCKS).join(format!("{digest}.install"))
    }

    /// The directory holding one digest's lease files, one per live holder.
    pub fn leases_dir(&self, digest: &Sha256Digest) -> PathBuf {
        self.root.join(LEASES).join(digest.as_str())
    }

    /// Whether a directory entry's name is one of ours rather than a model id.
    /// One rule, not four names — see the module note.
    pub fn is_internal(name: &str) -> bool {
        name.starts_with('.')
    }
}

/// `%LOCALAPPDATA%` on Windows, `~/Library/Application Support` on macOS,
/// `$XDG_DATA_HOME` (or `~/.local/share`) elsewhere.
///
/// The same rule `koru_channel`'s private `local_data_base` applies, restated
/// rather than borrowed — see the module note and Q-V26.
fn local_data_base() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
        })
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".local").join("share"))
            })
    }
}

#[cfg(test)]
mod tests;
