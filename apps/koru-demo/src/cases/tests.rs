// SPDX-License-Identifier: MPL-2.0

// The case-table tests are `cfg(windows)` (they compile registry chords
// into virtual-key codes); their helpers stay for them.
#![cfg_attr(not(windows), allow(dead_code))]
use super::*;

fn fixture() -> Fixture {
    Fixture {
        subject: LocatorPattern::parse("koru:w*/v*/n:00000000000000000000000000000001#text")
            .unwrap(),
        document: LocatorPattern::parse("koru:w*/v*/n:00000000000000000000000000000002#root")
            .unwrap(),
        entry: Some(
            LocatorPattern::parse("koru:w*/v*/n:00000000000000000000000000000003#text").unwrap(),
        ),
        reminder_date: Some("04/12/2099".into()),
        reminder_time: Some("3:30 PM".into()),
    }
}

// Compiles registry chords into Windows key codes; a Mac registry is ⌘-based.
#[cfg(windows)]
#[test]
fn every_case_is_explicit_and_step_identifiers_are_unique() {
    assert!(compile("typo", &fixture()).is_err());
    for name in names() {
        let case = compile(name, &fixture()).unwrap();
        let unique: std::collections::BTreeSet<_> = case.steps.iter().map(|s| s.id).collect();
        assert_eq!(unique.len(), case.steps.len());
    }
}

// Compiles registry chords into Windows key codes; a Mac registry is ⌘-based.
#[cfg(windows)]
#[test]
fn document_retention_is_after_durability_close_and_reopen() {
    let case = compile("document-edit", &fixture()).unwrap();
    let index = |id| case.steps.iter().position(|s| s.id == id).unwrap();
    assert!(index("document-durable") < index("close-document-tab"));
    assert!(index("close-document-tab") < index("reopen-document"));
    assert!(index("reopen-document") < index("reopened-edit-retained"));
    assert!(matches!(
        case.steps.last().unwrap().operation,
        Operation::Expect(Condition::Text(_))
    ));
}

// Compiles registry chords into Windows key codes; a Mac registry is ⌘-based.
#[cfg(windows)]
#[test]
fn saved_schedule_is_reopened_and_both_structured_fields_are_asserted() {
    let case = compile("task-reminder", &fixture()).unwrap();
    let index = |id| case.steps.iter().position(|s| s.id == id).unwrap();
    assert!(index("confirm-reminder") < index("saved-editor-open"));
    assert!(index("saved-editor-open") < index("saved-date"));
    assert!(index("saved-editor-open") < index("saved-time"));
    assert!(index("invalid-cannot-submit") < index("correct-date"));
}

// Compiles registry chords into Windows key codes; a Mac registry is ⌘-based.
#[cfg(windows)]
#[test]
fn slash_command_types_and_confirms_in_the_live_query_not_the_paragraph() {
    let case = compile("task-reminder", &fixture()).unwrap();
    let opener = case
        .steps
        .iter()
        .find(|step| step.id == "open-command-menu")
        .unwrap();
    assert_eq!(opener.operation, Operation::Input(Action::Text("/".into())));
    for id in ["task-command", "task-command-typed", "confirm-task-command"] {
        let step = case.steps.iter().find(|step| step.id == id).unwrap();
        let Target::Locator(target) = &step.target else {
            panic!("query locator");
        };
        assert_eq!(target.part, "query");
        assert!(
            matches!(&target.object, crate::locator::Object::Instance { kind, .. } if kind == "command")
        );
    }
}

// Compiles registry chords into Windows key codes; a Mac registry is ⌘-based.
#[cfg(windows)]
#[test]
fn cold_subject_text_is_checked_after_visible_navigation_and_before_edits() {
    let case = compile("document-edit", &fixture()).unwrap();
    let index = |id| case.steps.iter().position(|step| step.id == id).unwrap();
    assert!(index("subject-outside-document-viewport") < index("reach-last-paragraph"));
    assert_eq!(
        case.steps[index("focus-entry")].operation,
        Operation::ReachByPageUp
    );
    assert!(index("focus-entry") < index("fixture-entry"));
    assert!(index("fixture-entry") < index("reach-last-paragraph"));
    assert!(index("reach-last-paragraph") < index("fixture-original"));
    assert!(index("fixture-original") < index("move-caret"));
    assert!(index("reopen-document") < index("reach-reopened-paragraph"));
    assert!(index("reach-reopened-paragraph") < index("reopened-edit-retained"));
    assert!(
        !case
            .steps
            .iter()
            .any(|step| matches!(step.operation, Operation::Expect(Condition::Offscreen(_))))
    );
    let task = compile("task-reminder", &fixture()).unwrap();
    let index = |id| task.steps.iter().position(|step| step.id == id).unwrap();
    assert!(index("reach-item") < index("fixture-placeholder"));
    assert!(index("fixture-placeholder") < index("create-item"));
}
