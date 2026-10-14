// SPDX-License-Identifier: MPL-2.0

//! Publish the media a marketing project needs and git must not carry.
//!
//! The video project's inputs are two screen recordings, a soundtrack, a font
//! and a logo — about 9.6 MiB that would otherwise sit in every clone forever.
//! They live in the one origin instead, and the checkout keeps only their
//! digests. [`ASSETS_HOST`] is a second custom domain on the release bucket,
//! not a second bucket: Q-V78 settled that shape for the voice catalog and
//! nothing here earns a different one.
//!
//! **The digest is in the key.** `videos/<first 12 of sha256>/<path>` means
//! published bytes are immutable by construction — changing an asset writes a
//! new path rather than replacing one, so the one-year cache the zone rule
//! grants can never serve a stale copy and no purge is ever needed.
//!
//! The manifest is read by both sides. The video project's fetch script builds
//! the same URL from the same file, so a hash edited there changes what this
//! publishes in the same commit; there is no second table to drift. That
//! project is private, so its paths arrive as arguments (`--manifest`,
//! `--public-dir`) from `ops publish-assets`.

use crate::cloud::s3::sha256_hex;
use crate::cloud::{ASSETS_HOST, Cloud};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Safe only because the key carries the digest — see the module comment.
const ASSET_CACHE: &str = "public, max-age=31536000, immutable";
/// How much of the digest names the object. Long enough that a collision is not
/// a thing that happens, short enough to read in a URL. The fetch script slices
/// the same twelve characters; changing it here changes it there.
const KEY_DIGEST_CHARS: usize = 12;
/// **Where the media lives is the caller's to say.** The video project is
/// private (it is not in the open-source tree), so this public tool names no
/// path of it: `ops publish-assets` passes the manifest the fetch script also
/// reads, and the directory the manifest's paths are relative to.
const USAGE: &str = "usage: koru-release publish-assets --manifest <assets.manifest.json> \
                     --public-dir <dir> [--upload]";

#[derive(Deserialize)]
struct Manifest {
    prefix: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    path: String,
    sha256: String,
    size: u64,
    #[serde(rename = "contentType")]
    content_type: String,
}

/// `videos/1a2b3c4d5e6f/clips/tasks.mp4`.
pub fn object_key(prefix: &str, digest: &str, path: &str) -> String {
    format!("{prefix}/{}/{path}", &digest[..KEY_DIGEST_CHARS])
}

pub fn run(root: &Path, mut args: impl Iterator<Item = String>) -> Result<(), String> {
    let mut upload = false;
    let mut manifest_arg = None;
    let mut public_arg = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--upload" => upload = true,
            "--manifest" => manifest_arg = args.next(),
            "--public-dir" => public_arg = args.next(),
            other => return Err(format!("unrecognised argument {other}\n{USAGE}")),
        }
    }
    let (Some(manifest_arg), Some(public_arg)) = (manifest_arg, public_arg) else {
        return Err(format!("--manifest and --public-dir are required\n{USAGE}"));
    };
    // Relative to the checkout, like every other path the release tool reads.
    let manifest_path = root.join(&manifest_arg);
    let public_dir = root.join(&public_arg);
    let manifest_name = manifest_path.display().to_string();

    let manifest: Manifest =
        crate::read_json(&manifest_path).map_err(|error| format!("{manifest_name}: {error}"))?;
    if manifest.assets.is_empty() {
        return Err(format!("{manifest_name} lists no assets"));
    }

    // A dry run reads only what is already public, so it needs no token. Asking
    // for the token up front means a missing one fails before anything is read
    // off disk rather than halfway through publishing.
    let cloud = if upload {
        let paths = crate::release_paths()?;
        let config: crate::cloud::CloudConfig = crate::read_json(&paths.cloud).map_err(|_| {
            "release cloud configuration is missing; run `koru-release cloud-setup` first"
                .to_string()
        })?;
        Cloud::from_config(crate::cloudflare_token()?, config)?
    } else {
        Cloud::public_reader()?
    };

    let mut pending: Vec<(Asset, PathBuf, String)> = Vec::new();
    let mut warned_unreachable = false;
    for asset in manifest.assets {
        let source = public_dir.join(&asset.path);
        let bytes =
            std::fs::read(&source).map_err(|error| format!("{}: {error}", source.display()))?;

        // **Verify before publishing, not after.** The manifest is what every
        // fetch will check against; an upload of bytes that do not match it
        // publishes an asset nobody can ever successfully download.
        let digest = sha256_hex(&bytes);
        if digest != asset.sha256 {
            return Err(format!(
                "{} is sha256 {digest}, but {manifest_name} pins {}. Update the manifest \
                 deliberately if the asset really changed; do not publish the mismatch.",
                asset.path, asset.sha256
            ));
        }
        if bytes.len() as u64 != asset.size {
            return Err(format!(
                "{} is {} bytes, but {manifest_name} pins {}",
                asset.path,
                bytes.len(),
                asset.size
            ));
        }

        let key = object_key(&manifest.prefix, &digest, &asset.path);
        // **An origin that does not answer is not an error here.** Until
        // `cloud-setup` has attached the second custom domain the host does not
        // resolve at all, and a dry run that dies on that is a dry run nobody
        // can use to rehearse the first publication. Unknown means unpublished:
        // the worst outcome is re-uploading bytes that are already there, under
        // a key that by construction holds the same content.
        let published = match cloud.head_public_on(ASSETS_HOST, &key) {
            Ok(found) => found,
            Err(error) => {
                if !warned_unreachable {
                    println!("  ({ASSETS_HOST} did not answer: {error})");
                    println!("  Treating every asset as unpublished.");
                    warned_unreachable = true;
                }
                None
            }
        };
        match published {
            // The key carries the digest, so an object already at this path is
            // these exact bytes — unless it is a truncated earlier upload, which
            // the length catches.
            Some(Some(length)) if length == asset.size => {
                println!("  {key}\n    already published ({length} bytes)");
                continue;
            }
            Some(_) => println!("  {key}\n    present but the wrong length; replacing"),
            None => println!("  {key}\n    to publish ({} bytes)", asset.size),
        }
        pending.push((asset, source, key));
    }

    if pending.is_empty() {
        println!("\n[Koru] Every asset is already published to {ASSETS_HOST}.");
        return Ok(());
    }
    let total: u64 = pending.iter().map(|(asset, _, _)| asset.size).sum();
    if !upload {
        println!(
            "\n[Koru] {} asset(s), {total} bytes. Nothing was written. \
             Add --upload to publish this to {ASSETS_HOST}.",
            pending.len()
        );
        return Ok(());
    }

    for (asset, source, key) in &pending {
        cloud.put_file(key, source, &asset.content_type, ASSET_CACHE)?;
        println!("  published https://{ASSETS_HOST}/{key}");
    }
    println!(
        "\n[Koru] Published {} asset(s), {total} bytes to {ASSETS_HOST}.",
        pending.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_carries_twelve_digest_characters_before_the_path() {
        let digest = "fc6bcdae8b94ae1f1b2232a43833fabf3e22588d651e9c824a4fc47d39eefd60";
        assert_eq!(
            object_key("videos", digest, "clips/tasks.mp4"),
            "videos/fc6bcdae8b94/clips/tasks.mp4"
        );
    }

    /// The fetch script slices the same twelve characters out of a manifest of
    /// this shape. If this number or the shape moves, that one has to move
    /// with it. (The real manifest belongs to the private video project, so
    /// the public tool tests the shape, not that file.)
    #[test]
    fn a_manifest_in_the_fetch_scripts_shape_builds_its_keys() {
        let manifest: Manifest = serde_json::from_str(
            r#"{
                "note": "ignored",
                "prefix": "videos",
                "assets": [{
                    "path": "clips/tasks.mp4",
                    "sha256": "fc6bcdae8b94ae1f1b2232a43833fabf3e22588d651e9c824a4fc47d39eefd60",
                    "size": 1024,
                    "contentType": "video/mp4"
                }]
            }"#,
        )
        .expect("the fetch script's manifest shape parses");
        let asset = &manifest.assets[0];
        assert_eq!(asset.content_type, "video/mp4");
        assert_eq!(asset.size, 1024);
        assert_eq!(
            object_key(&manifest.prefix, &asset.sha256, &asset.path),
            "videos/fc6bcdae8b94/clips/tasks.mp4"
        );
    }

    /// The paths are the caller's: without them nothing is read.
    #[test]
    fn the_manifest_and_media_directory_are_required() {
        let root = Path::new("does-not-exist");
        for args in [
            vec![],
            vec!["--manifest", "m.json"],
            vec!["--public-dir", "public", "--upload"],
        ] {
            let error = run(root, args.into_iter().map(String::from)).unwrap_err();
            assert!(
                error.contains("--manifest and --public-dir are required"),
                "{error}"
            );
        }
        let error = run(root, ["--bogus".to_string()].into_iter()).unwrap_err();
        assert!(error.contains("unrecognised argument --bogus"), "{error}");
    }
}
