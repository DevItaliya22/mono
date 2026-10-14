// SPDX-License-Identifier: MPL-2.0

//! Which channel this binary was built for.

use std::fmt;

use crate::build_info;

/// A release channel.
///
/// `serde` is behind a feature because only the update client and the release
/// tool put one in JSON. A crate that merely asks what channel it is should not
/// link a serialization framework to find out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "lowercase")
)]
pub enum ReleaseChannel {
    Stable,
    Beta,
    Nightly,
    /// Local debug/release builds. Lower than nightly; never published. Only
    /// offered in the UI when this binary itself is internal.
    Internal,
}

impl ReleaseChannel {
    /// Public channels every install can pick.
    pub const PUBLIC: [Self; 3] = [Self::Stable, Self::Beta, Self::Nightly];

    /// Public channels plus Internal — only for internal binaries.
    pub const ALL: [Self; 4] = [Self::Stable, Self::Beta, Self::Nightly, Self::Internal];

    /// The channel this binary was built for. Used when the user has not saved
    /// a preference: a nightly install must not default to Stable and then
    /// complain that the installed build is newer than the channel.
    pub fn of_this_build() -> Self {
        Self::from_build_channel(build_info().channel)
    }

    /// Pure mapping so tests can cover every flavor without baking env vars.
    pub fn from_build_channel(channel: &str) -> Self {
        match channel.trim().to_ascii_lowercase().as_str() {
            "nightly" => Self::Nightly,
            "beta" => Self::Beta,
            "internal" | "dev" => Self::Internal,
            _ => Self::Stable,
        }
    }

    /// Whether this binary is a developer/internal build (`internal` / `dev`).
    pub fn this_build_is_internal() -> bool {
        matches!(Self::of_this_build(), Self::Internal)
    }

    /// Channels the welcome segment control may offer on this binary.
    pub fn choosable() -> &'static [Self] {
        if Self::this_build_is_internal() {
            &Self::ALL
        } else {
            &Self::PUBLIC
        }
    }

    /// Drop Internal when this binary is not internal (stale prefs / hand-edit).
    pub fn clamp_to_choosable(self) -> Self {
        if self == Self::Internal && !Self::this_build_is_internal() {
            Self::of_this_build()
        } else {
            self
        }
    }

    /// Internal has no CDN channel — checks and downloads do not apply.
    pub const fn has_update_feed(self) -> bool {
        !matches!(self, Self::Internal)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Nightly => "nightly",
            Self::Internal => "internal",
        }
    }
}

impl Default for ReleaseChannel {
    fn default() -> Self {
        Self::of_this_build()
    }
}

impl fmt::Display for ReleaseChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ReleaseChannel {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "stable" => Ok(Self::Stable),
            "beta" => Ok(Self::Beta),
            "nightly" => Ok(Self::Nightly),
            "internal" | "dev" => Ok(Self::Internal),
            _ => Err(format!("unsupported release channel: {value}")),
        }
    }
}

#[cfg(test)]
mod tests;
