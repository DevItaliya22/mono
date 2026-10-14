// SPDX-License-Identifier: MPL-2.0

//! `document_state` as a table over what a listing can say.

use koru_delegate_core::DocumentId;
use koru_store_core::model::{DocumentEntry, Timestamp};

use super::*;

fn entry(id: DocumentId, parent: Option<DocumentId>, trashed: bool) -> DocumentEntry {
    DocumentEntry {
        id,
        parent,
        ordinal: 0,
        title: String::new(),
        kind: "page".into(),
        mime: None,
        note: None,
        trashed_at: trashed.then(|| Timestamp::from_millis(1)),
        trashed_by: None,
    }
}

#[test]
fn listed_untrashed_and_retained_is_live() {
    let id = DocumentId::new();
    assert_eq!(
        document_state(&[entry(id, None, false)], id, true),
        ContextState::Live
    );
}

#[test]
fn a_trashed_entry_is_in_trash_and_so_is_everything_under_it() {
    let parent = DocumentId::new();
    let child = DocumentId::new();
    let grandchild = DocumentId::new();
    let entries = [
        entry(parent, None, true),
        entry(child, Some(parent), false),
        entry(grandchild, Some(child), false),
    ];
    for id in [parent, child, grandchild] {
        assert_eq!(document_state(&entries, id, true), ContextState::InTrash);
    }
}

#[test]
fn an_unlisted_document_is_unavailable_whatever_its_version_row_says() {
    let id = DocumentId::new();
    assert_eq!(document_state(&[], id, true), ContextState::Unavailable);
    assert_eq!(document_state(&[], id, false), ContextState::Unavailable);
}

#[test]
fn a_listed_document_whose_version_is_gone_is_unavailable_even_if_trashed() {
    let id = DocumentId::new();
    assert_eq!(
        document_state(&[entry(id, None, true)], id, false),
        ContextState::Unavailable
    );
}

#[test]
fn an_unlisted_ancestor_does_not_make_a_listed_document_trashed() {
    let missing_parent = DocumentId::new();
    let id = DocumentId::new();
    assert_eq!(
        document_state(&[entry(id, Some(missing_parent), false)], id, true),
        ContextState::Live
    );
}

#[test]
fn a_parent_cycle_ends() {
    let a = DocumentId::new();
    let b = DocumentId::new();
    let entries = [entry(a, Some(b), false), entry(b, Some(a), false)];
    assert_eq!(document_state(&entries, a, true), ContextState::Live);
}

#[test]
fn a_blob_is_live_or_unavailable() {
    assert_eq!(blob_state(true), ContextState::Live);
    assert_eq!(blob_state(false), ContextState::Unavailable);
}

#[test]
fn states_serialize_as_stable_tokens() {
    for (state, token) in [
        (ContextState::Live, "\"live\""),
        (ContextState::InTrash, "\"in_trash\""),
        (ContextState::Unavailable, "\"unavailable\""),
    ] {
        assert_eq!(serde_json::to_string(&state).unwrap(), token);
        assert_eq!(state.as_str(), token.trim_matches('"'));
    }
}
