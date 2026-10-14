// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;
use url::Url;

use crate::MAX_PACKAGE_SIZE;
use koru_channel::ReleaseChannel;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateTarget {
    pub channel: ReleaseChannel,
    pub platform: String,
    pub architecture: String,
}

impl UpdateTarget {
    pub fn manifest_path(&self) -> String {
        format!(
            "channels/{}/{}-{}.json",
            self.channel, self.platform, self.architecture
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct UpdateManifest {
    pub schema_version: u32,
    pub channel: ReleaseChannel,
    pub platform: String,
    pub architecture: String,
    pub version: String,
    pub build_number: u64,
    pub published_at: String,
    #[serde(default)]
    pub commit: Option<String>,
    /// Where this release's source code is: the public repository at the
    /// full commit it was built from (MPL-2.0 §3.2). Optional and omitted
    /// when absent, so manifests written before it existed still parse and
    /// an older reader ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default)]
    pub minimum_updater_version: Option<u32>,
    #[serde(default)]
    pub minimum_supported_build: Option<u64>,
    #[serde(default)]
    pub required: bool,
    pub release_notes: ReleaseNotes,
    pub package: UpdatePackage,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReleaseNotes {
    pub title: String,
    pub summary: Vec<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct UpdatePackage {
    pub url: String,
    #[serde(default)]
    pub file_name: String,
    pub size: u64,
    pub sha256: String,
    pub signature: String,
    pub format: PackageFormat,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PackageFormat {
    Zip,
    #[serde(rename = "tar.gz")]
    TarGz,
    AppImage,
    /// An Apple disk image: `Koru.app` plus an Applications symlink, the
    /// drag-install every mac user already knows.
    Dmg,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestError(pub String);

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ManifestError {}

impl UpdateManifest {
    pub fn parse_and_validate(bytes: &[u8], target: &UpdateTarget) -> Result<Self, ManifestError> {
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|e| ManifestError(format!("manifest JSON is invalid: {e}")))?;
        manifest.validate(target)?;
        Ok(manifest)
    }

    pub fn validate(&self, target: &UpdateTarget) -> Result<(), ManifestError> {
        reject(self.schema_version != 1, "unsupported manifest schema")?;
        reject(
            self.channel != target.channel,
            "manifest channel does not match request",
        )?;
        reject(
            self.platform != target.platform,
            "manifest platform does not match this build",
        )?;
        reject(
            self.architecture != target.architecture,
            "manifest architecture does not match this build",
        )?;
        reject(self.version.trim().is_empty(), "version is empty")?;
        reject(
            self.build_number == 0,
            "build number must be greater than zero",
        )?;
        reject(self.published_at.trim().is_empty(), "published_at is empty")?;
        reject(
            self.release_notes.title.trim().is_empty(),
            "release-note title is empty",
        )?;
        reject(
            self.release_notes.summary.is_empty(),
            "release-note summary is empty",
        )?;
        reject(
            self.release_notes
                .summary
                .iter()
                .any(|line| line.trim().is_empty()),
            "release-note summary contains an empty item",
        )?;
        if let Some(url) = &self.release_notes.url {
            require_https(url, "release-note URL")?;
        }
        require_https(&self.package.url, "package URL")?;
        reject(
            self.package.file_name.trim().is_empty(),
            "package file name is empty",
        )?;
        reject(
            Path::new(&self.package.file_name)
                .file_name()
                .and_then(|v| v.to_str())
                != Some(self.package.file_name.as_str()),
            "package file name must not contain a path",
        )?;
        reject(self.package.size == 0, "package size is zero")?;
        reject(
            self.package.size > MAX_PACKAGE_SIZE,
            "package exceeds Koru's size limit",
        )?;
        reject(
            !valid_sha256(&self.package.sha256),
            "package SHA-256 is malformed",
        )?;
        reject(
            self.package.signature.trim().is_empty(),
            "package signature is missing",
        )?;
        // The format follows the platform: Windows installs from the zip the
        // updater already extracts; macOS ships a disk image (zip is still
        // legal there — it is what pre-dmg releases published).
        let format_fits = match self.platform.as_str() {
            "macos" => matches!(self.package.format, PackageFormat::Dmg | PackageFormat::Zip),
            _ => matches!(self.package.format, PackageFormat::Zip),
        };
        reject(
            !format_fits,
            "package format is unsupported on this platform",
        )?;
        Ok(())
    }
}

fn require_https(value: &str, field: &str) -> Result<(), ManifestError> {
    let url = Url::parse(value).map_err(|e| ManifestError(format!("{field} is invalid: {e}")))?;
    reject(url.scheme() != "https", &format!("{field} must use HTTPS"))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn reject(condition: bool, message: &str) -> Result<(), ManifestError> {
    if condition {
        Err(ManifestError(message.to_string()))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> UpdateTarget {
        UpdateTarget {
            channel: ReleaseChannel::Stable,
            platform: "windows".into(),
            architecture: "x86_64".into(),
        }
    }

    fn manifest() -> UpdateManifest {
        UpdateManifest {
            schema_version: 1,
            channel: ReleaseChannel::Stable,
            platform: "windows".into(),
            architecture: "x86_64".into(),
            version: "1.2.3".into(),
            build_number: 42,
            published_at: "2026-08-04T20:00:00Z".into(),
            commit: Some("abc1234".into()),
            source: None,
            minimum_updater_version: Some(1),
            minimum_supported_build: None,
            required: false,
            release_notes: ReleaseNotes {
                title: "Better".into(),
                summary: vec!["Safer updates".into()],
                url: Some("https://updates.koru.do/releases/1.2.3/release-notes.html".into()),
            },
            package: UpdatePackage {
                url: "https://updates.koru.do/releases/1.2.3/koru-windows-x86_64.zip".into(),
                file_name: "koru-windows-x86_64.zip".into(),
                size: 100,
                sha256: "a".repeat(64),
                signature: "signature".into(),
                format: PackageFormat::Zip,
            },
        }
    }

    /// `source` is additive: absent from an old manifest, omitted when unset,
    /// and carried through when a release names its commit.
    #[test]
    fn the_source_link_is_optional_in_both_directions() {
        let old = serde_json::to_value(manifest()).unwrap();
        assert!(old.get("source").is_none(), "unset is omitted, not null");
        let parsed: UpdateManifest = serde_json::from_value(old).unwrap();
        assert_eq!(parsed.source, None);

        let mut named = manifest();
        named.source = Some(
            "https://github.com/koruworks/mono/tree/0123456789abcdef0123456789abcdef01234567"
                .into(),
        );
        let json = serde_json::to_string(&named).unwrap();
        assert_eq!(
            serde_json::from_str::<UpdateManifest>(&json).unwrap(),
            named
        );
    }

    #[test]
    fn of_this_build_maps_flavor_strings() {
        assert_eq!(
            ReleaseChannel::from_build_channel("nightly"),
            ReleaseChannel::Nightly
        );
        assert_eq!(
            ReleaseChannel::from_build_channel("beta"),
            ReleaseChannel::Beta
        );
        assert_eq!(
            ReleaseChannel::from_build_channel("stable"),
            ReleaseChannel::Stable
        );
        assert_eq!(
            ReleaseChannel::from_build_channel("dev"),
            ReleaseChannel::Internal
        );
        assert_eq!(
            ReleaseChannel::from_build_channel("internal"),
            ReleaseChannel::Internal
        );
        assert_eq!(
            ReleaseChannel::from_build_channel("Nightly"),
            ReleaseChannel::Nightly
        );
    }

    #[test]
    fn internal_is_only_choosable_on_internal_binaries() {
        // This test binary is internal (no KORU_RELEASE_CHANNEL at compile).
        assert!(ReleaseChannel::this_build_is_internal());
        assert!(ReleaseChannel::choosable().contains(&ReleaseChannel::Internal));
        assert_eq!(ReleaseChannel::default(), ReleaseChannel::Internal);
    }

    #[test]
    fn a_complete_matching_manifest_is_valid() {
        manifest().validate(&target()).unwrap();
    }

    #[test]
    fn wrong_target_and_insecure_urls_are_rejected() {
        let mut m = manifest();
        m.architecture = "aarch64".into();
        assert!(m.validate(&target()).is_err());
        m.architecture = "x86_64".into();
        m.package.url = "http://updates.koru.do/file.zip".into();
        assert!(m.validate(&target()).is_err());
    }

    #[test]
    fn malformed_hash_size_signature_and_format_are_rejected() {
        let mut m = manifest();
        m.package.sha256 = "ABC".into();
        assert!(m.validate(&target()).is_err());
        m.package.sha256 = "a".repeat(64);
        m.package.size = 0;
        assert!(m.validate(&target()).is_err());
        m.package.size = 1;
        m.package.signature.clear();
        assert!(m.validate(&target()).is_err());
        m.package.signature = "sig".into();
        m.package.format = PackageFormat::TarGz;
        assert!(m.validate(&target()).is_err());
    }

    /// The format follows the platform: a mac manifest may carry a disk
    /// image (or the zip pre-dmg releases published); Windows takes only the
    /// zip its updater extracts.
    #[test]
    fn the_package_format_follows_the_platform() {
        let mut m = manifest();
        m.platform = "macos".into();
        m.package.format = PackageFormat::Dmg;
        let mac = UpdateTarget {
            channel: ReleaseChannel::Stable,
            platform: "macos".into(),
            architecture: "x86_64".into(),
        };
        m.validate(&mac).unwrap();
        m.package.format = PackageFormat::Zip;
        m.validate(&mac).unwrap();
        m.package.format = PackageFormat::Dmg;
        m.platform = "windows".into();
        assert!(m.validate(&target()).is_err());
    }
}
