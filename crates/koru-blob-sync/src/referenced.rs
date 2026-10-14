// SPDX-License-Identifier: MPL-2.0

//! The canonical referenced-blob walk.
//!
//! One pure walk over durable Workspace state, returning unique **live**
//! `BlobHash` values with the metadata a transfer needs. It is the answer to
//! "which blobs does this Workspace actually have", and it exists exactly once
//! so that Cloud coverage, a quota computation and a future collector cannot
//! disagree about it — the failure that costs a person a picture is three
//! walkers with three different ideas of "live".
//!
//! **`workspace.db.blobs` is not the answer.** That table lists what a
//! Workspace *may* use, including pictures no document references any more;
//! reconciling against it would upload bytes nobody can see and keep them
//! there forever.
//!
//! ## The rules, and where each comes from
//!
//! | Rule | Why |
//! | --- | --- |
//! | a document no longer in the durable set is skipped | permanently removed content does not count — vector 08's second document |
//! | a document in the Trash still counts | ADR-95 stamps the manifest entry and leaves the content intact; restoring it must hand back a whole picture — vector 12 |
//! | a node in the *document's* bin still counts | the same reason, one level down: the node is restorable, so its bytes are still owed |
//! | the listing's `kind` is not consulted | an image pasted into an ordinary page is a file this Workspace holds — vector 13 |
//! | a hash appears once, however many nodes name it | identity is content; two references are one object — vector 10 |
//! | a reference whose hash will not parse is counted, never guessed | an unreadable reference is news (vector 14's shape), and a malformed address cannot be transferred to anything |
//!
//! This walk is **pure**: it takes the state and returns the answer. The Store
//! supplies the input and the vectors under
//! `contracts/vectors/logical-usage-v1/` supply it too, which is what makes
//! the corpus a guard rather than a document.

use std::collections::{BTreeMap, BTreeSet};

use koru_store_core::model::BlobHash;

/// One document, as durable Workspace state presents it.
///
/// `durable` and `trashed` are two different facts and conflating them is the
/// bug this type exists to prevent: a trashed document is *present* and its
/// bytes are still owed, while a permanently removed one is gone and its bytes
/// are not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalkDocument {
    /// Still part of the Workspace's durable document set. A document the
    /// manifest no longer names is `false` and contributes nothing.
    pub durable: bool,
    /// The ADR-95 stamp on this document's manifest entry **or on an
    /// ancestor's**. Live is inherited through the document tree, so a child
    /// of a binned parent carries no stamp of its own and is still binned;
    /// whoever supplies this resolves that before it gets here.
    pub trashed: bool,
    pub nodes: Vec<WalkNode>,
}

/// One node's blob reference, and the metadata a transfer needs from it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WalkNode {
    /// The `BlobHash` in its canonical `b3:<hex>` spelling, as the node stores
    /// it. `None` for the overwhelming majority of nodes, which reference no
    /// file at all.
    pub blob: Option<String>,
    /// The media type the node recorded. Metadata: allocation declares it, and
    /// nothing is decided by it.
    pub mime: Option<String>,
}

/// One blob this Workspace references, once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferencedBlob {
    pub hash: BlobHash,
    /// The first non-empty media type any referencing node recorded. `None`
    /// when every reference left it blank, in which case a transfer declares
    /// the generic type rather than inventing a specific one.
    pub mime: Option<String>,
    /// How many live nodes name it. Not used to decide anything — a blob is
    /// referenced or it is not — but a count a person can read is the
    /// difference between "this picture is used twice" and a suspicion.
    pub references: u32,
}

/// What one walk found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Walk {
    /// Unique live hashes, ordered by their canonical spelling so two runs
    /// over the same state produce the same list — a reconciliation that
    /// reordered itself would be a queue nobody could read twice.
    pub blobs: Vec<ReferencedBlob>,
    /// References the walk could not turn into an address.
    ///
    /// **Counted rather than skipped silently.** A node that names something
    /// that is not a hash is a document this build does not fully understand,
    /// and a coverage number that quietly excluded it would claim
    /// completeness it has not got.
    pub unreadable: u32,
}

/// The one canonical walk.
pub fn referenced_blobs(documents: &[WalkDocument]) -> Walk {
    let mut found: BTreeMap<String, ReferencedBlob> = BTreeMap::new();
    let mut unreadable = 0u32;
    for document in documents {
        for reference in references(document) {
            let Ok((hash, mime)) = reference else {
                unreadable = unreadable.saturating_add(1);
                continue;
            };
            found
                .entry(hash.as_str().to_owned())
                .and_modify(|existing| {
                    existing.references = existing.references.saturating_add(1);
                    if existing.mime.is_none() {
                        existing.mime.clone_from(&mime);
                    }
                })
                .or_insert(ReferencedBlob {
                    hash,
                    mime,
                    references: 1,
                });
        }
    }
    Walk {
        blobs: found.into_values().collect(),
        unreadable,
    }
}

/// **The same walk, answered per document**: the live addresses one document
/// references, each once.
///
/// For a surface that has to say *which* document a file belongs to — the
/// question the Workspace-wide [`referenced_blobs`] deliberately flattens
/// away. It is the same rule by construction rather than by care: both read
/// [`references`], so a document this walk skips contributes nothing there
/// either, and an address one of them cannot parse the other cannot either.
pub fn document_blobs(document: &WalkDocument) -> BTreeSet<String> {
    references(document)
        .filter_map(Result::ok)
        .map(|(hash, _)| hash.as_str().to_owned())
        .collect()
}

/// **One document's references with their media types**, and how many it
/// names that will not parse.
///
/// The same walk again, for a surface that says *images* and *files* apart:
/// an address appears once, carrying the first non-empty media type any of
/// the document's nodes recorded for it — the Workspace walk's own rule for
/// the same question, so the two cannot disagree about what one file is.
pub fn document_references(document: &WalkDocument) -> DocumentReferences {
    let mut blobs: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut unreadable = 0u32;
    for reference in references(document) {
        let Ok((hash, mime)) = reference else {
            unreadable = unreadable.saturating_add(1);
            continue;
        };
        let entry = blobs.entry(hash.as_str().to_owned()).or_default();
        if entry.is_none() {
            *entry = mime;
        }
    }
    DocumentReferences { blobs, unreadable }
}

/// What [`document_references`] found in one document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocumentReferences {
    /// Each live address once, in canonical order, with its media type when
    /// any node recorded one.
    pub blobs: BTreeMap<String, Option<String>>,
    /// References that are not an address, counted for [`Walk::unreadable`]'s
    /// reason.
    pub unreadable: u32,
}

/// Every reference one document makes, under the rules in the module note:
/// nothing for a document no longer durable, blank references skipped, and a
/// reference that will not parse as `Err` so the caller can count it.
fn references(
    document: &WalkDocument,
) -> impl Iterator<Item = Result<(BlobHash, Option<String>), ()>> + '_ {
    // Permanently removed content does not count. Everything else does,
    // including the Trash: a stamp on a manifest entry is not a deletion.
    let nodes = match document.durable {
        true => document.nodes.as_slice(),
        false => &[],
    };
    nodes.iter().filter_map(|node| {
        let raw = node
            .blob
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())?;
        let Ok(hash) = BlobHash::parse(raw) else {
            return Some(Err(()));
        };
        let mime = node
            .mime
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        Some(Ok((hash, mime)))
    })
}

#[cfg(test)]
mod tests;
