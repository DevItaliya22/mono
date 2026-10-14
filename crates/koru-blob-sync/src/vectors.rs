// SPDX-License-Identifier: MPL-2.0

//! The `logical-usage-v1` corpus, typed for the referenced-blob walk.
//!
//! **This crate is a consumer of somebody else's vectors**, so the shapes
//! below are deliberately *tolerant*: unknown fields are ignored and absent
//! ones take their documented defaults. `koru-cloud-blob`'s own vectors use
//! `deny_unknown_fields` because adding a field there is adding a sentence to
//! a contract this repository owns; adding a field to the usage corpus is a
//! sentence in the *billing* contract, and failing this crate's build for it
//! would be this crate claiming an ownership it has not got.
//!
//! ## Why a billing corpus guards a transfer walk
//!
//! Because they are the same question asked twice. Coverage asks "which blobs
//! must exist in Cloud", quota asks "which blobs does this account pay for",
//! and a collector asks "which blobs may be removed". Three answers is a
//! picture that got uploaded, was never counted, and then got swept — so
//! there is one walk, and the corpus that fixes the billing answer fixes it.
//!
//! ## The one translation
//!
//! The corpus names a blob by its `dedup_identity` — *"within-workspace
//! identities derived from the workspace's `WorkspaceBlobIdentityKey`, never a
//! plaintext `BlobHash`"* — because the hosted side never sees a plaintext
//! address. A client does: on this side of the boundary the identity of the
//! bytes **is** the `BlobHash`. So each distinct identity in a workspace is
//! mapped to one deterministic `b3:<hex>` address, one-to-one and back again.
//! The mapping changes no case: the corpus is about which references are live,
//! and the spelling of an identity is a fact about hosted encryption.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::referenced::{WalkDocument, WalkNode};

/// Every case file, named once, so a case added to the corpus and not to this
/// list is a case this crate can be shown to have skipped.
macro_rules! corpus {
    ($($name:literal),* $(,)?) => {
        /// `(case name, its JSON)`, in the index's order. Compiled in so a
        /// test never reads the working tree.
        pub const CASE_FILES: &[(&str, &str)] = &[
            $((
                $name,
                include_str!(concat!(
                    "../../../contracts/vectors/logical-usage-v1/",
                    $name,
                    ".json"
                )),
            ),)*
        ];
    };
}

corpus!(
    "01-empty-workspace",
    "02-single-plain-text-document",
    "03-non-latin-script-document",
    "04-bidi-mixed-direction-document",
    "05-typed-attributes-and-marks",
    "06-document-in-trash",
    "07-nodes-in-the-document-trash",
    "08-permanently-deleted-document",
    "09-blob-referenced-once",
    "10-blob-referenced-twice-in-one-workspace",
    "11-same-blob-in-two-workspaces",
    "12-blob-referenced-only-from-trash",
    "13-image-inside-a-page-not-a-file-entry",
    "14-unreadable-node",
    "15-too-new-schema",
    "16-composed-vs-decomposed",
    "17-mixed-algorithm-version-account",
    "18-retained-workspaces-keep-counting",
    "19-blob-not-yet-durable",
    "20-structure-heavy-almost-no-text",
    "21-pathological-empty-cardinality",
    "22-grandfathered-threshold-after-migration",
);

#[derive(Clone, Debug, Deserialize)]
pub struct Case {
    #[serde(rename = "case")]
    pub id: String,
    #[serde(default)]
    pub description: String,
    pub input: Input,
    #[serde(default)]
    pub expected: Expected,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Input {
    /// Absent in the two cases whose input is an account-level migration
    /// rather than workspace content (17 and 22).
    #[serde(default)]
    pub workspaces: Vec<InputWorkspace>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct InputWorkspace {
    pub workspace: String,
    /// Absent in case 21, whose documents are parametric rather than
    /// enumerated. A case with no enumerated documents is not a walk case.
    #[serde(default)]
    pub documents: Option<Vec<InputDocument>>,
    #[serde(default)]
    pub blobs: Vec<InputBlob>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct InputDocument {
    #[serde(default)]
    pub document: String,
    #[serde(default)]
    pub listing: Listing,
    /// `false` means the document is no longer in the durable document set —
    /// permanently removed, history still in Loro.
    #[serde(default)]
    pub durable: bool,
    #[serde(default)]
    pub nodes: Vec<InputNode>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Listing {
    #[serde(default)]
    pub kind: String,
    /// The ADR-95 stamp. Present means binned; the content is intact.
    #[serde(default)]
    pub trashed_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct InputNode {
    /// The blob's within-workspace dedup identity, translated below.
    #[serde(default)]
    pub blob: Option<String>,
    #[serde(default)]
    pub mime: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct InputBlob {
    pub dedup_identity: String,
    /// `null` for a blob whose upload never finalized: Cloud holds no bytes
    /// and there is no verified length (case 19).
    #[serde(default)]
    pub verified_plaintext_length_bytes: Option<u64>,
    #[serde(default)]
    pub durable: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Expected {
    #[serde(default)]
    pub workspaces: Option<Vec<ExpectedWorkspace>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ExpectedWorkspace {
    pub workspace: String,
    /// `null` when the workspace's usage could not be computed at all.
    #[serde(default)]
    pub files_bytes: Option<u64>,
}

impl InputWorkspace {
    /// The walk's input, with each dedup identity translated to the plaintext
    /// address a client would hold.
    pub fn walk_documents(&self) -> Option<Vec<WalkDocument>> {
        let documents = self.documents.as_ref()?;
        Some(
            documents
                .iter()
                .map(|document| WalkDocument {
                    durable: document.durable,
                    trashed: document.listing.trashed_at.is_some(),
                    nodes: document
                        .nodes
                        .iter()
                        .map(|node| WalkNode {
                            blob: node.blob.as_deref().map(address_of),
                            mime: node.mime.clone(),
                        })
                        .collect(),
                })
                .collect(),
        )
    }

    /// Address → the corpus's own facts about that blob.
    pub fn blobs_by_address(&self) -> BTreeMap<String, InputBlob> {
        self.blobs
            .iter()
            .map(|blob| (address_of(&blob.dedup_identity), blob.clone()))
            .collect()
    }
}

/// One dedup identity, one plaintext address. BLAKE3 of the identity, which is
/// deterministic, one-to-one, and spelled exactly as a real address is.
pub fn address_of(identity: &str) -> String {
    format!("b3:{}", blake3::hash(identity.as_bytes()).to_hex())
}

/// Parse every case. Returns them in corpus order.
pub fn cases() -> Vec<Case> {
    CASE_FILES
        .iter()
        .map(|(name, json)| {
            serde_json::from_str::<Case>(json)
                .unwrap_or_else(|error| panic!("{name} is not a logical-usage case: {error}"))
        })
        .collect()
}
