// SPDX-License-Identifier: MPL-2.0

//! What the field refuses, and how.
//!
//! The refusals are the point. Everything else here is `TextField`, which has
//! its own tests one crate down.

use super::*;

fn engine() -> TextEngine {
    TextEngine::new()
}

fn field(name: &str) -> (WorkspaceRename, TextEngine) {
    let mut e = engine();
    let f = WorkspaceRename::new(&mut e, WorkspaceId::new(), name);
    (f, e)
}

fn press(f: &mut WorkspaceRename, e: &mut TextEngine, key: &Key, taken: &[String]) -> Outcome {
    f.key(e, key, None, ModifiersState::empty(), taken)
}

fn typed(f: &mut WorkspaceRename, e: &mut TextEngine, text: &str) {
    for ch in text.chars() {
        let k = Key::Character(ch.to_string().into());
        f.key(e, &k, Some(&ch.to_string()), ModifiersState::empty(), &[]);
    }
}

const ENTER: Key = Key::Named(NamedKey::Enter);
const ESC: Key = Key::Named(NamedKey::Escape);

/// The ordinary path.
#[test]
fn enter_on_a_new_name_commits_it() {
    let (mut f, mut e) = field("Research");
    // The name arrives selected, so typing replaces it.
    typed(&mut f, &mut e, "Q3");
    match press(&mut f, &mut e, &ENTER, &[]) {
        Outcome::Commit(name) => assert_eq!(name, "Q3"),
        _ => panic!("expected a commit"),
    }
}

#[test]
fn escape_cancels_without_committing() {
    let (mut f, mut e) = field("Research");
    typed(&mut f, &mut e, "Q3");
    assert!(matches!(press(&mut f, &mut e, &ESC, &[]), Outcome::Cancel));
}

/// **Inline, not a dialog.** The field stays open, the text stays put, and the
/// complaint appears beside it — a modal here would take away the very thing
/// the person is being asked to change.
#[test]
fn an_empty_name_is_refused_in_the_field_and_keeps_the_text() {
    let (mut f, mut e) = field("");
    assert!(matches!(press(&mut f, &mut e, &ENTER, &[]), Outcome::Keep));
    assert!(f.hint().is_some(), "and it says why");
}

#[test]
fn an_empty_file_name_is_refused() {
    let mut e = engine();
    let mut f = WorkspaceRename::for_file(&mut e, DocumentId::new(), "");
    assert!(matches!(press(&mut f, &mut e, &ENTER, &[]), Outcome::Keep));
    assert_eq!(f.hint(), Some("A file needs a name"));
}

/// Files already share names (`Shared link.url` from every phone share), so
/// the uniqueness rule that workspaces need does not apply.
#[test]
fn a_file_may_reuse_another_files_name() {
    let mut e = engine();
    let mut f = WorkspaceRename::for_file(&mut e, DocumentId::new(), "Shared link.url");
    match press(&mut f, &mut e, &ENTER, &["Shared link.url".into()]) {
        Outcome::Commit(name) => assert_eq!(name, "Shared link.url"),
        _ => panic!("expected a commit"),
    }
}

#[test]
fn a_duplicate_name_is_refused_in_the_field() {
    let (mut f, mut e) = field("Research");
    let taken = vec!["Client work".to_string(), "Q3 Planning".to_string()];

    typed(&mut f, &mut e, "Q3 Planning");
    assert!(matches!(
        press(&mut f, &mut e, &ENTER, &taken),
        Outcome::Keep
    ));
    assert_eq!(
        f.hint(),
        Some("Another workspace already has that name"),
        "and it names the reason rather than just refusing"
    );
}

/// `Research` and `research` are the same row to anyone reading the list, which
/// is the only place either name is ever seen.
#[test]
fn a_duplicate_is_refused_whatever_its_case() {
    let (mut f, mut e) = field("x");
    typed(&mut f, &mut e, "rEsEaRcH");
    let taken = vec!["Research".to_string()];
    assert!(matches!(
        press(&mut f, &mut e, &ENTER, &taken),
        Outcome::Keep
    ));
}

/// Renaming a workspace to what it is already called is not a clash — the
/// caller passes every *other* workspace's name, so this is a commit.
#[test]
fn keeping_the_same_name_is_not_a_duplicate() {
    let (mut f, mut e) = field("Research");
    // `taken` never includes the workspace being renamed.
    match press(&mut f, &mut e, &ENTER, &["Client work".to_string()]) {
        Outcome::Commit(name) => assert_eq!(name, "Research"),
        _ => panic!("expected a commit"),
    }
}

/// The complaint is about the last Enter, not a standing state. Once you start
/// fixing it, it has done its job.
#[test]
fn typing_clears_the_complaint() {
    let (mut f, mut e) = field("");
    press(&mut f, &mut e, &ENTER, &[]);
    assert!(f.hint().is_some());
    typed(&mut f, &mut e, "a");
    assert_eq!(f.hint(), None);
}

/// Clicking away means "done" everywhere else in this app, so it means it here.
#[test]
fn a_press_outside_the_row_commits() {
    let (mut f, mut e) = field("Research");
    let row = Rect::new(100.0, 100.0, 200.0, 24.0);
    match f.mouse_down(&mut e, row, 500.0, 500.0, &[]) {
        Outcome::Commit(name) => assert_eq!(name, "Research"),
        _ => panic!("expected a commit"),
    }
}

/// **But not around the rules.** A name Enter would refuse must not be
/// committable by clicking elsewhere — otherwise the mouse is a way past the
/// check the keyboard enforces.
#[test]
fn a_press_outside_cannot_commit_a_name_enter_would_refuse() {
    let (mut f, mut e) = field("Research");
    let row = Rect::new(100.0, 100.0, 200.0, 24.0);
    let taken = vec!["Client work".to_string()];
    typed(&mut f, &mut e, "Client work");
    assert!(matches!(
        f.mouse_down(&mut e, row, 500.0, 500.0, &taken),
        Outcome::Cancel
    ));

    let (mut empty, mut e) = field("");
    assert!(matches!(
        empty.mouse_down(&mut e, row, 500.0, 500.0, &[]),
        Outcome::Cancel
    ));
}

/// A press inside is a caret move, not a commit.
#[test]
fn a_press_inside_the_row_keeps_editing() {
    let (mut f, mut e) = field("Research");
    let row = Rect::new(100.0, 100.0, 200.0, 24.0);
    assert!(matches!(
        f.mouse_down(&mut e, row, 150.0, 110.0, &[]),
        Outcome::Keep
    ));
}

#[test]
fn schedule_name_rules_are_refused_inline() {
    let mut e = engine();
    let schedule = ScheduleId::mint();
    let mut empty = WorkspaceRename::for_schedule(&mut e, schedule, "");
    assert!(matches!(
        press(&mut empty, &mut e, &ENTER, &[]),
        Outcome::Keep
    ));
    assert_eq!(
        empty.hint(),
        Some(koru_i18n::t(
            crate::format::lang(),
            koru_i18n::Key::DelegateScheduleNameNeeded
        ))
    );

    let mut long = WorkspaceRename::for_schedule(&mut e, schedule, "x");
    typed(&mut long, &mut e, &"a".repeat(129));
    assert!(matches!(
        press(&mut long, &mut e, &ENTER, &[]),
        Outcome::Keep
    ));
    assert_eq!(
        long.hint(),
        Some(koru_i18n::t(
            crate::format::lang(),
            koru_i18n::Key::DelegateScheduleNameTooLong
        ))
    );
}

#[test]
fn a_schedule_rename_cannot_submit_twice_while_sending() {
    let mut e = engine();
    let mut field = WorkspaceRename::for_schedule(&mut e, ScheduleId::mint(), "Research");
    field.set_sending(true);
    assert!(matches!(
        press(&mut field, &mut e, &ENTER, &[]),
        Outcome::Keep
    ));
}
