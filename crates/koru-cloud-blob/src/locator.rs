// SPDX-License-Identifier: MPL-2.0

//! Where bytes live on the object volume.
//!
//! **Paths derive only from validated ids.** No customer filename and no
//! content hash is interpolated into a Cloud path: a filename is arbitrary
//! Unicode and is metadata, and the hash renders with a colon that NTFS forbids
//! (D248). The locator is minted, opaque and says nothing about the content.

use std::fmt;

use koru_account_core::workspace_authorization::Region;
use koru_device::cloud::CloudId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The immutable storage name of one published object. Opaque by construction:
/// two Workspaces holding identical bytes get different locators, because the
/// approved future format encrypts per Workspace and server-wide deduplication
/// would be either temporary or a privacy leak.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObjectLocator(Uuid);

impl ObjectLocator {
    pub fn mint() -> Self {
        Self(Uuid::new_v4())
    }
    pub fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }
    pub fn as_uuid(self) -> Uuid {
        self.0
    }
    /// Canonical lowercase hyphenated, and only that: a second spelling of one
    /// identifier is a second path for one object.
    pub fn parse(value: &str) -> Option<Self> {
        let parsed = Uuid::parse_str(value).ok()?;
        (parsed.hyphenated().to_string() == value).then_some(Self(parsed))
    }
}

impl fmt::Display for ObjectLocator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.hyphenated().fmt(f)
    }
}

const fn region_token(region: Region) -> &'static str {
    match region {
        Region::Us => "us",
        Region::Eu => "eu",
    }
}

/// `<region>/<workspace>/<incarnation>/<object-locator>`, relative to the
/// volume's `blob-objects` root. Every segment is a token this process minted
/// or validated, so the result is ASCII, has no `.` or `..` component, and
/// cannot be steered by anything a client sent.
pub fn object_key(
    region: Region,
    workspace: CloudId,
    incarnation: CloudId,
    locator: ObjectLocator,
) -> String {
    format!(
        "{}/{}/{}/{}",
        region_token(region),
        workspace.as_uuid().hyphenated(),
        incarnation.as_uuid().hyphenated(),
        locator
    )
}

/// `<upload-id>.part`, relative to the volume's `blob-staging` root. Flat, and
/// named by the session rather than by the object, because staging exists
/// before anybody knows whether the bytes are the object they claim to be.
pub fn staging_name(upload: CloudId) -> String {
    format!("{}.part", upload.as_uuid().hyphenated())
}

#[cfg(test)]
mod tests;
