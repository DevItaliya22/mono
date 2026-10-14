// SPDX-License-Identifier: MPL-2.0

//! Where a copied tag lands — and a re-export of the tag **name** grammar,
//! which lives in [`koru_tagname`].
//!
//! **The grammar moved to a leaf crate** ([ADR-185], amended): the fold key,
//! the `/` path rules, `TagName` and the matcher are string logic, and four
//! things have to agree about them — this model, the editor's chooser, the
//! store's query parser and the search box's completion. Two of those four have
//! no node model and must not acquire one to answer a question about a string.
//! Everything it exports is re-exported here, so a caller with the node model
//! already sees no difference.
//!
//! What stays is the half that names a [`TagId`], because an id is not a name:
//! a `TagId` is document-scoped, so a tag arriving from another workspace is
//! resolved by name through the same fold key every other tag question uses.
//!
//! [ADR-185]: ../../../docs/decisions/ADR-201-225.md#adr-208-tagnames--a-tag-name-folds-for-identity-and-parses-for-hierarchy

pub use koru_tagname::{
    MAX_DEPTH, SEPARATOR, TagName, TagPathError, ancestors, covers, fold_key, fold_segment,
    fold_segments, hash_token_at, is_ancestor, is_tag_char, leaf, matches, matches_query, parent,
    segments, token_at,
};

use crate::ids::TagId;

/// Where a tag that arrived from somewhere else lands in the table it is being
/// applied against — a paste, and (later) an import.
///
/// A [`TagId`] is **document-scoped**: it names a record in one workspace's
/// system document and means nothing in another one's. So a tagged block
/// copied across workspaces has to be resolved by *name*, through the same
/// [`fold_key`] every other tag question folds through — which is what makes
/// `#Home` land on an existing `#home` rather than minting a second record.
///
/// There is deliberately no third "drop it" answer. Dropping happens further
/// out, and for one reason only: a [`Create`](Self::Create) request that the
/// host cannot answer because the system document is unreachable. That is a
/// degradation of the tag, never a failure of the paste.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TagLanding {
    /// Apply this id. Either the one it arrived with — the ordinary
    /// same-workspace paste, and also the case where no name travelled, which
    /// mirrors what duplicating a node does — or a *different* record in the
    /// destination whose fold key is the same name.
    Existing(TagId),
    /// Nothing here answers to that name: it has to be created, by
    /// [`SystemDoc::create_tag`](crate::system::SystemDoc::create_tag), which
    /// folds again on the way in and creates the parents of a `/`-path.
    Create(String),
}

/// Decide where a copied tag lands, given the destination's table.
///
/// `destination` is `(id, name)` for every tag the destination workspace
/// knows — an empty listing is an ordinary state (a fresh workspace), and the
/// answer there is [`TagLanding::Create`] rather than a dangling id.
///
/// The id wins over the name: the same workspace is the common case by far,
/// and a rename must not turn a paste into a second record.
pub fn resolve_copied_tag<'a>(
    id: TagId,
    name: Option<&str>,
    destination: impl IntoIterator<Item = (TagId, &'a str)>,
) -> TagLanding {
    let key = name.map(fold_key).filter(|k| !k.is_empty());
    let mut by_name: Option<TagId> = None;
    for (candidate, candidate_name) in destination {
        if candidate == id {
            return TagLanding::Existing(id);
        }
        if by_name.is_none()
            && let Some(key) = &key
            && fold_key(candidate_name) == *key
        {
            by_name = Some(candidate);
        }
    }
    match (by_name, name) {
        (Some(existing), _) => TagLanding::Existing(existing),
        // A name that folds to nothing is no name at all, so it takes the
        // no-name road: keep the id, the way a duplicate would.
        (None, Some(name)) if key.is_some() => TagLanding::Create(name.trim().to_owned()),
        _ => TagLanding::Existing(id),
    }
}

#[cfg(test)]
mod tests;
