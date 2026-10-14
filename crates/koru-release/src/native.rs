// SPDX-License-Identifier: MPL-2.0

//! **Which sherpa-onnx archive a release actually linked, and whether it is the
//! canonical one.**
//!
//! Every shipped target links upstream's TTS-free (`…-no-tts-lib`) archive,
//! which `fetch-voice-runtime` downloads and refuses unless it matches its pin.
//! The build script will still take whatever sits under the right name in
//! `SHERPA_ONNX_ARCHIVE_DIR`, or an extracted directory in
//! `SHERPA_ONNX_LIB_DIR`, so the bytes a build linked are checked again here.
//! **An official release must link the canonical archive on every platform**:
//! `dist-cloud` refuses to package unless the binaries were linked against the
//! archive whose hash `koru_voice_sherpa::artifacts::NAMES` pins. `dist-local`
//! builds anyway and says what it linked.
//!
//! **The witness is the build script's own cache, not the provisioning
//! directory.** `sherpa-onnx-sys` copies the exactly-named archive from
//! `SHERPA_ONNX_ARCHIVE_DIR` into `<target>/sherpa-onnx-prebuilt/` and extracts
//! it there; once the extracted `lib/` exists it never looks at
//! `SHERPA_ONNX_ARCHIVE_DIR` again. So the file in the provisioning directory
//! can be replaced after the fact and say nothing about the binary, while the
//! cached copy is the one the libraries were unpacked from. Checking it after
//! the release build is the honest question — "what did these binaries link?" —
//! rather than "what is lying around for the next build?".
//!
//! Two answers are refusals for an official release because they cannot be
//! verified at all: `SHERPA_ONNX_LIB_DIR` (an already-extracted directory the
//! build script uses as-is, with no archive behind it), and a cache that holds
//! an extracted `lib/` but no archive.

use std::path::{Path, PathBuf};

use koru_voice_sherpa::artifacts::{self, NativeArtifact};

/// What one release build linked.
#[derive(Debug, PartialEq, Eq)]
pub enum Linked {
    /// The pinned archive, byte for byte.
    Canonical,
    /// An archive with the right name and a different hash — a stale or
    /// locally built one, usually.
    NotCanonical {
        path: PathBuf,
        sha256: String,
        size: u64,
    },
    /// `SHERPA_ONNX_LIB_DIR` was set: an extracted directory with no archive to
    /// hash.
    LibDirOverride(PathBuf),
    /// No archive in the build script's cache to compare.
    Missing(PathBuf),
}

/// Where `sherpa-onnx-sys` caches the archive it extracted, by its own rule:
/// `CARGO_TARGET_DIR` when set, otherwise the `target` directory.
pub fn cache_root(root: &Path, cargo_target_dir: Option<&Path>) -> PathBuf {
    cargo_target_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.join("target"))
        .join("sherpa-onnx-prebuilt")
}

/// What was linked, given the build script's cache and whether
/// `SHERPA_ONNX_LIB_DIR` overrode it.
pub fn linked(
    artifact: &NativeArtifact,
    cache_root: &Path,
    lib_dir_override: Option<&Path>,
) -> Result<Linked, String> {
    if let Some(lib_dir) = lib_dir_override {
        return Ok(Linked::LibDirOverride(lib_dir.to_path_buf()));
    }
    let path = cache_root.join(artifact.file_name());
    if !path.is_file() {
        return Ok(Linked::Missing(path));
    }
    let (size, sha256) =
        koru_update::sha256_file(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    if artifact.is_pinned() && size == artifact.size && sha256 == artifact.sha256 {
        Ok(Linked::Canonical)
    } else {
        Ok(Linked::NotCanonical { path, sha256, size })
    }
}

/// The official-release rule: canonical, or a refusal that says what to do.
pub fn require_canonical(artifact: &NativeArtifact, linked: &Linked) -> Result<(), String> {
    let name = artifact.file_name();
    match linked {
        Linked::Canonical => Ok(()),
        Linked::NotCanonical { path, sha256, size } => Err(format!(
            "an official release must link the canonical {name}, and this build linked a \
             different one ({}: sha256 {sha256}, {size} bytes; the pin is {}, {} bytes). \
             Run scripts/setup/fetch-voice-runtime to place the canonical archive in \
             third-party/sherpa-onnx/{}/, delete {} so the build script re-extracts it, \
             and publish again",
            path.display(),
            artifact.sha256,
            artifact.size,
            artifacts::SHERPA_ONNX_VERSION,
            path.parent().unwrap_or(path).display(),
        )),
        Linked::LibDirOverride(dir) => Err(format!(
            "an official release cannot verify which sherpa-onnx it linked: \
             {} is set ({}), and the build script uses that directory as-is. \
             Unset it and let SHERPA_ONNX_ARCHIVE_DIR provide the canonical {name}",
            artifacts::env::LIB_DIR,
            dir.display()
        )),
        Linked::Missing(path) => Err(format!(
            "an official release cannot verify which sherpa-onnx it linked: {} is not there. \
             Delete {} so the next build copies and extracts the canonical {name} from \
             SHERPA_ONNX_ARCHIVE_DIR, and publish again",
            path.display(),
            path.parent().unwrap_or(path).display(),
        )),
    }
}

/// One line for a build that is allowed to link a non-canonical archive, so a
/// local bundle still says what it is.
pub fn notice(artifact: &NativeArtifact, linked: &Linked) -> Option<String> {
    match linked {
        Linked::Canonical => None,
        Linked::NotCanonical { sha256, .. } => Some(format!(
            "this bundle links a non-canonical {} (sha256 {sha256}); dist-local allows it, \
             and dist-cloud would refuse it",
            artifact.file_name()
        )),
        Linked::LibDirOverride(dir) => Some(format!(
            "this bundle links sherpa-onnx from {} = {}; dist-cloud would refuse it",
            artifacts::env::LIB_DIR,
            dir.display()
        )),
        Linked::Missing(path) => Some(format!(
            "could not tell which sherpa-onnx this bundle links ({} is not there); dist-cloud would refuse it",
            path.display()
        )),
    }
}

/// Which rule a package is built under.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rule {
    /// `dist-cloud`: the canonical archive, or no package.
    Official,
    /// `dist-local`: any archive the build accepted, said out loud when it is
    /// not the canonical one.
    Local,
}

/// Check what the host's release build just linked, reading the environment
/// that build inherited. Run after the channel binaries are built and before
/// anything is staged, signed or uploaded.
pub fn check_this_build(root: &Path, rule: Rule) -> Result<(), String> {
    let Some(artifact) = artifacts::artifact_for_this_build() else {
        // `package::build` refuses an unsupported host before this; an
        // official release that reaches here unverified is still refused.
        return match rule {
            Rule::Official => Err("no pinned sherpa-onnx archive for this host".into()),
            Rule::Local => Ok(()),
        };
    };
    let target_dir = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from);
    let lib_dir = std::env::var_os(artifacts::env::LIB_DIR).map(PathBuf::from);
    let linked = linked(
        artifact,
        &cache_root(root, target_dir.as_deref()),
        lib_dir.as_deref(),
    )?;
    match rule {
        Rule::Official => {
            require_canonical(artifact, &linked)?;
            println!(
                "[Koru] sherpa-onnx: linked the canonical {} (sha256 {})",
                artifact.file_name(),
                artifact.sha256
            );
        }
        Rule::Local => {
            if let Some(notice) = notice(artifact, &linked) {
                println!("[Koru] note: {notice}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
