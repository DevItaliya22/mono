// SPDX-License-Identifier: MPL-2.0

//! Resolving a tag id to the word somebody typed.
//!
//! **A node stores a `TagId`, never a name** ([ADR-176]). That is what makes a
//! rename one record rather than a rewrite of every document carrying the tag,
//! and it is the thing tags-as-entities exists to buy. It also means the value
//! the projection hands this crate — `0191d0f2-…` — is routing data rather than
//! text: indexed as-is it is searchable as a *field* and useless as a *word*,
//! so searching `amazing` never found a block tagged `#amazing`.
//!
//! So the **indexer** resolves it. The table lives in the workspace's
//! `koru-system` document; a host supplies it through [`TagNames`], the indexer
//! caches one [`TagTable`] per workspace, and a rename re-indexes the documents
//! carrying the ids whose *name* moved.
//!
//! ## Why the tokens are spelled out here
//!
//! `TAG_ROLE` is `koru_core::attributes::KEY_TAG` and [`KIND_SYSTEM`] is
//! `koru_core::explore::KIND_SYSTEM`. **This crate links no node model**
//! (`lib.rs`, and the manifest says it twice), and the projection is strings
//! and primitives precisely so a consumer does not acquire one. These are the
//! same stable storage tokens the kind strings in `fragments.rs` already are —
//! changing either orphans what is written with it, so copying them is as safe
//! as copying `"paragraph"`.
//!
//! [ADR-176]: ../../../docs/decisions/ADR-176-200.md#adr-199-attributes--attributes-are-typed-values-on-the-node-under-registered-keys-tags-are-entities-and-the-workspace-keeps-a-hidden-system-document

use std::collections::{HashMap, HashSet};

use koru_store_core::ids::WorkspaceId;

/// The attribute role a tag sits under. `koru_core::attributes::KEY_TAG`.
pub const TAG_ROLE: &str = "koru.tag";

/// The manifest kind token of a workspace's system document — where the tag
/// table lives. `koru_core::explore::KIND_SYSTEM`.
pub const KIND_SYSTEM: &str = "koru-system";

/// One workspace's tag table, flattened to the only thing an index wants: the
/// readable name behind an id.
///
/// **Alias tombstones are already resolved by whoever built this.** A merged
/// tag's record stays so that every node still carrying the old id remains
/// resolvable, and a reader follows the forwarding pointer — that is
/// `SystemDoc::resolve`'s job, on the side of the seam that can see the CRDT.
/// What arrives here is `id -> the name a person reads`, alias ids included.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct TagTable {
    names: HashMap<String, String>,
}

impl TagTable {
    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            // A tag with no name is not a word anybody can search for, and
            // indexing the empty string would put a blank fragment on every
            // node carrying it.
            names: pairs
                .into_iter()
                .filter(|(_, name)| !name.trim().is_empty())
                .collect(),
        }
    }

    /// The name behind an id, or `None` for one this table does not know —
    /// forgotten, or from a workspace whose table could not be read.
    pub fn name(&self, id: &str) -> Option<&str> {
        self.names.get(id).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Every id whose **readable name** differs between two tables: renamed,
    /// newly named, or gone.
    ///
    /// This is the whole of the re-index trigger, and what it deliberately does
    /// *not* answer for is a change that leaves every name alone — a new colour,
    /// a new attribute definition, a merge whose winner is spelled the same.
    /// The system document reprojects for all of those, and none of them
    /// changes one byte of any fragment.
    pub fn renamed_since(&self, previous: &Self) -> HashSet<String> {
        let mut moved = HashSet::new();
        for (id, name) in &self.names {
            if previous.names.get(id) != Some(name) {
                moved.insert(id.clone());
            }
        }
        for id in previous.names.keys() {
            if !self.names.contains_key(id) {
                moved.insert(id.clone());
            }
        }
        moved
    }
}

/// Where the indexer gets a workspace's tag table.
///
/// **A seam rather than a dependency.** Reading the table means opening the
/// system document's `LoroDoc`, which needs the store's lock and the node
/// model — two things this crate has, and must keep, no way of naming. The
/// daemon holds both and implements this.
///
/// `None` means *this workspace's table could not be read now*: no system
/// document yet, a fork that failed, a host that wired no source at all.
/// It is never an error — a tag falls back to being indexed as it was before,
/// and the next pass tries again.
pub trait TagNames: Send {
    /// `(TagId in storage form, the name a person reads)`, alias ids resolved
    /// to the winner's name.
    fn tag_names(&self, workspace: WorkspaceId) -> Option<Vec<(String, String)>>;
}

#[cfg(test)]
mod tests;
