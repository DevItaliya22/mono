// SPDX-License-Identifier: MPL-2.0

use super::*;

// ------------------------------------------------- a tag that arrived from
// somewhere else
//
// The name grammar itself lives in `koru-tagname` and is tested there. What is
// left here is the half that names a `TagId`, which is why it did not move.

/// The table as `resolve_copied_tag` takes it: id and name, borrowed.
fn table(entries: &[(TagId, String)]) -> impl Iterator<Item = (TagId, &str)> {
    entries.iter().map(|(id, name)| (*id, name.as_str()))
}

/// The everyday paste: the same workspace, so the id is already there and the
/// name is never consulted.
#[test]
fn a_tag_the_destination_already_knows_keeps_its_id() {
    let id = TagId::new();
    let entries = vec![(id, "home".to_owned()), (TagId::new(), "work".to_owned())];
    assert_eq!(
        resolve_copied_tag(id, Some("home"), table(&entries)),
        TagLanding::Existing(id)
    );
}

/// Across workspaces the id means nothing, so the name decides — and it decides
/// by fold key, which is what stops `#Home` becoming a second `#home`.
#[test]
fn a_stranger_id_with_a_known_name_reuses_the_existing_tag() {
    let theirs = TagId::new();
    let ours = TagId::new();
    let entries = vec![(ours, "Home".to_owned())];
    for spelling in ["home", "HOME", "  Home  "] {
        assert_eq!(
            resolve_copied_tag(theirs, Some(spelling), table(&entries)),
            TagLanding::Existing(ours),
            "{spelling} names the tag that is already here"
        );
    }
}

/// The fold is the one in the module docs, so the eight-language rules hold
/// here too: an accented and an unaccented spelling are one tag.
#[test]
fn reuse_folds_the_way_every_other_tag_question_does() {
    let ours = TagId::new();
    let entries = vec![(ours, "café".to_owned())];
    assert_eq!(
        resolve_copied_tag(TagId::new(), Some("CAFE"), table(&entries)),
        TagLanding::Existing(ours)
    );
}

/// Nothing answers to the name, so it has to be made — under the name as
/// typed, not the fold key, because display is case-preserving.
#[test]
fn an_unknown_name_is_created_under_its_own_spelling() {
    let entries = vec![(TagId::new(), "work".to_owned())];
    assert_eq!(
        resolve_copied_tag(TagId::new(), Some(" Fun/Home "), table(&entries)),
        TagLanding::Create("Fun/Home".to_owned())
    );
    // A fresh workspace knows nothing at all, and that is the same answer.
    assert_eq!(
        resolve_copied_tag(TagId::new(), Some("Fun/Home"), std::iter::empty()),
        TagLanding::Create("Fun/Home".to_owned())
    );
}

/// No name travelled — the source table had already forgotten the tag — so the
/// id is kept verbatim, which is what duplicating the node does.
#[test]
fn a_tag_with_no_name_keeps_its_id() {
    let id = TagId::new();
    assert_eq!(
        resolve_copied_tag(id, None, std::iter::empty()),
        TagLanding::Existing(id)
    );
    assert_eq!(
        resolve_copied_tag(id, Some("  /  "), std::iter::empty()),
        TagLanding::Existing(id),
        "a name that folds to nothing is no name"
    );
}
