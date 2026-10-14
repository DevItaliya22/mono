// SPDX-License-Identifier: MPL-2.0

//! [`RunName`] — the two words a run is called.

use super::*;

use crate::name::words::{FIRST, SECOND};

fn run(value: u128) -> RunId {
    RunId::from_uuid(Uuid::from_u128(value))
}

/// **The same id is the same name, every time.** This is the whole
/// justification for deriving it: a second device, a second window and a
/// second process all say *sunny-otter* about the same run with nothing
/// stored and nothing exchanged.
#[test]
fn one_id_always_has_the_same_name() {
    let id = run(0x51);
    let first = RunName::of(id);
    for _ in 0..8 {
        assert_eq!(RunName::of(id), first, "the derivation is a function");
    }
    // And a name is two words with one hyphen, drawn from the two lists.
    let (a, b) = first.words();
    assert!(
        FIRST.contains(&a),
        "the first word comes from the first list"
    );
    assert!(
        SECOND.contains(&b),
        "the second word comes from the second list"
    );
    assert_eq!(first.to_string(), format!("{a}-{b}"));
}

/// **Different runs get different names**, near-always — and in particular
/// the id fixtures the suite is full of, which differ only in their last
/// byte. An earlier shape that indexed the id's first two bytes gave every
/// one of these the same name, because a `Uuid::from_u128(small)` is fifteen
/// zero bytes and a payload.
#[test]
fn ids_that_differ_by_one_byte_get_different_names() {
    let names: BTreeSet<String> = (0u128..64)
        .map(|value| RunName::of(run(value)).to_string())
        .collect();
    assert!(
        names.len() >= 60,
        "64 consecutive ids should not crowd into a handful of names, got {}",
        names.len()
    );
}

/// **The collision odds the module documentation states.** Both list lengths
/// are asserted, because the sentence quotes them; and a thousand random ids
/// spread across the space rather than piling into a corner of it.
#[test]
fn the_name_space_is_the_size_the_docs_claim() {
    assert_eq!(
        (FIRST.len(), SECOND.len()),
        crate::name::DOCUMENTED,
        "the module doc quotes these two numbers and the collision odds computed from them"
    );
    assert_eq!(FIRST.len() * SECOND.len(), 82_944);

    // 1000 minted ids in 82,944 names: the birthday expectation is about six
    // collisions, and a derivation that had collapsed would show hundreds.
    let minted: Vec<RunId> = (0..1000).map(|_| RunId::mint()).collect();
    let distinct: BTreeSet<String> = minted
        .iter()
        .map(|run| RunName::of(*run).to_string())
        .collect();
    assert!(
        distinct.len() > 950,
        "1000 ids should land on more than 950 distinct names, got {}",
        distinct.len()
    );
}

/// **Both lists are strictly ascending**, which is how a duplicate — a word
/// that silently doubles its own odds — is caught, and how a reader finds a
/// word to check it. Everything is ASCII and lower case: the name is a
/// label, not a translated word, and it is drawn in eight locales.
#[test]
fn the_word_lists_are_sorted_ascii_and_free_of_duplicates() {
    for (name, list) in [("FIRST", FIRST), ("SECOND", SECOND)] {
        for pair in list.windows(2) {
            assert!(
                pair[0] < pair[1],
                "{name} is out of order (or repeats) at {:?} / {:?}",
                pair[0],
                pair[1]
            );
        }
        for word in list {
            assert!(
                !word.is_empty() && word.chars().all(|c| c.is_ascii_lowercase()),
                "{name} holds a word that is not plain lower-case ASCII: {word:?}"
            );
        }
    }
}

/// **The name is a label, and the id stays the identity.** There is no way
/// back: `RunName` carries no id, exposes no id, and no constructor takes a
/// name. This test is the reader's version of that — the compile-fail proof
/// is that no such function exists to call.
#[test]
fn a_name_names_nothing() {
    let a = run(1);
    let b = run(2);
    // Two runs may legitimately share a name; what may never happen is a
    // name being mistaken for the thing that tells them apart.
    assert_ne!(a, b, "the ids differ, whatever the labels do");
}
