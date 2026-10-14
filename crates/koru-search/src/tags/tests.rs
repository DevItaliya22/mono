// SPDX-License-Identifier: MPL-2.0

//! What the table answers, and what it calls a change.

use super::*;

fn table(pairs: &[(&str, &str)]) -> TagTable {
    TagTable::from_pairs(
        pairs
            .iter()
            .map(|(id, name)| ((*id).to_owned(), (*name).to_owned())),
    )
}

#[test]
fn an_unknown_id_has_no_name_rather_than_a_blank_one() {
    let table = table(&[("t1", "amazing")]);
    assert_eq!(table.name("t1"), Some("amazing"));
    assert_eq!(table.name("t2"), None);
    assert_eq!(TagTable::default().name("t1"), None);
}

/// A tag whose name is whitespace is not a word anybody can search for, and a
/// fragment holding one would be a blank row on every node carrying it.
#[test]
fn a_nameless_tag_is_not_in_the_table() {
    let table = table(&[("t1", "   "), ("t2", "mvp")]);
    assert_eq!(table.len(), 1);
    assert_eq!(table.name("t1"), None);
}

/// **The re-index trigger.** Only a name that moved is one, because only a name
/// is in a fragment.
#[test]
fn only_a_changed_name_counts_as_a_change() {
    let before = table(&[("t1", "amazing"), ("t2", "mvp")]);

    let unchanged = table(&[("t1", "amazing"), ("t2", "mvp")]);
    assert!(unchanged.renamed_since(&before).is_empty());

    let renamed = table(&[("t1", "astonishing"), ("t2", "mvp")]);
    assert_eq!(
        renamed.renamed_since(&before),
        ["t1".to_owned()].into_iter().collect()
    );

    let forgotten = table(&[("t2", "mvp")]);
    assert_eq!(
        forgotten.renamed_since(&before),
        ["t1".to_owned()].into_iter().collect()
    );

    let added = table(&[("t1", "amazing"), ("t2", "mvp"), ("t3", "later")]);
    assert_eq!(
        added.renamed_since(&before),
        ["t3".to_owned()].into_iter().collect()
    );
}

/// A merge is a rename only when the winner is spelled differently: the loser's
/// tombstone resolves to the winner's name on the far side of the seam, so an
/// id that arrives here spelled the same is not work.
#[test]
fn a_merge_onto_the_same_spelling_is_not_a_rename() {
    let before = table(&[("loser", "mvp"), ("winner", "mvp")]);
    let after = table(&[("loser", "mvp"), ("winner", "mvp")]);
    assert!(after.renamed_since(&before).is_empty());
}
