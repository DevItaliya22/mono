// SPDX-License-Identifier: MPL-2.0

//! **What an assistive technology may ask of a paragraph, and what it may
//! not.**
//!
//! Against the real [`Editor`] over a real in-memory document, laid out for
//! real through parley, because the subject is what the *editor* does with a
//! request and a stubbed body would agree with anything.

use accesskit::{Action, ActionData, TextPosition, TextSelection};
use koru_core::WorkspaceId;
use koru_core::node::Node as CoreNode;
use koru_core::schema::{Parent, WorkspaceDoc};

use super::*;
use crate::accessibility::projection::{DocumentAccessibility, Publish, ViewGeometry};
use crate::{Editing, Editor};

const VIEWPORT_W: f32 = 760.0;
const VIEWPORT_H: f32 = 620.0;

const FIRST: &str = "The first paragraph, which is long enough to wrap onto more than one \
                     visual line at the width this fixture uses.";
const SECOND: &str = "The second paragraph.";

fn document(texts: &[&str]) -> (WorkspaceDoc, Editor) {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "actions fixture")
        .expect("an in-memory workspace");
    let doc = ws
        .insert(
            Parent::Root,
            CoreNode::document("Fixture").expect("a document"),
        )
        .expect("the document node");
    for text in texts {
        ws.insert(
            Parent::Node(doc),
            CoreNode::paragraph(*text).expect("a paragraph"),
        )
        .expect("a paragraph node");
    }
    ws.enable_undo();
    let mut editor = Editor::rooted_at(&ws, doc, true);
    editor.set_viewport(VIEWPORT_W, VIEWPORT_H);
    editor.view.active = true;
    editor.reflow();
    (ws, editor)
}

fn base_of(id: koru_core::NodeId) -> accesskit::NodeId {
    let (high, low) = id.as_uuid().as_u64_pair();
    let mixed = high ^ low.rotate_left(17);
    accesskit::NodeId(if mixed == 0 { 1 } else { mixed })
}

/// Publish the whole body once, so the view has run ids a request can name.
fn published(editor: &Editor) -> DocumentAccessibility {
    let mut access = DocumentAccessibility::default();
    access.body(
        &editor.body.rows,
        ViewGeometry {
            origin: (0.0, 0.0),
            content_x: editor.view.content_x,
            scroll: editor.view.scroll,
            viewport_h: editor.view.viewport.1,
        },
        Some(editor.body.focus),
        Publish::Whole,
        base_of,
    );
    access
}

/// A request for the whole of one row, in the run ids it was published with.
fn whole_row(access: &DocumentAccessibility, editor: &Editor, index: usize) -> TextSelection {
    let row = &editor.body.rows[index];
    let runs = access.published_run_ids(row.id);
    let first = *runs.first().expect("a first run");
    let last = *runs.last().expect("a last run");
    TextSelection {
        anchor: TextPosition {
            node: first,
            character_index: 0,
        },
        focus: TextPosition {
            node: last,
            character_index: 0,
        },
    }
}

fn selection_request(
    anchor: accesskit::NodeId,
    at: usize,
    focus: accesskit::NodeId,
    to: usize,
) -> ActionData {
    ActionData::SetTextSelection(TextSelection {
        anchor: TextPosition {
            node: anchor,
            character_index: at,
        },
        focus: TextPosition {
            node: focus,
            character_index: to,
        },
    })
}

// ------------------------------------------------------------------- focus

#[test]
fn focus_moves_the_keyboard_and_leaves_the_caret_where_it_was() {
    let (_ws, mut editor) = document(&[FIRST, SECOND]);
    let mut access = published(&editor);
    // A selection in the first row, the way a person would have one.
    editor.body.rows[0].select_range_now(4, 9);
    editor.reflow();
    let before = editor.body.rows[0].directed_range();
    let second = editor.body.rows[1].id;

    let outcome = perform(&mut editor, &mut access, second, Action::Focus, None);

    assert_eq!(outcome, Ok(Performed::Focused));
    assert_eq!(editor.body.focus, 1);
    assert!(editor.view.active);
    // **The whole reason focus does nothing else.** Windows sends a focus
    // request before every invoke; one that placed a caret would eat this.
    assert_eq!(
        editor.body.rows[0].directed_range(),
        before,
        "a focus request disturbed a selection in another paragraph"
    );
}

#[test]
fn a_request_naming_a_row_that_is_not_in_this_body_is_refused() {
    let (_ws, mut editor) = document(&[FIRST]);
    let mut access = published(&editor);
    let stranger = koru_core::NodeId::from_uuid(uuid::Uuid::new_v4());

    for action in [
        Action::Focus,
        Action::SetTextSelection,
        Action::ScrollIntoView,
    ] {
        assert_eq!(
            perform(&mut editor, &mut access, stranger, action, None),
            Err(Refused::NoSuchRow),
            "{action:?} on a row that does not exist"
        );
    }
    assert_eq!(access.counters().requests_refused, 3);
    assert_eq!(access.counters().requests_performed, 0);
}

// --------------------------------------------------------------- selection

#[test]
fn a_set_text_selection_lands_on_exactly_the_bytes_it_named() {
    let (_ws, mut editor) = document(&[FIRST, SECOND]);
    let mut access = published(&editor);
    let row = editor.body.rows[1].id;
    let runs = access.published_run_ids(row);
    let first = runs[0];

    let data = selection_request(first, 4, first, 10);
    let outcome = perform(
        &mut editor,
        &mut access,
        row,
        Action::SetTextSelection,
        Some(&data),
    );

    assert_eq!(
        outcome,
        Ok(Performed::Selected {
            anchor: 4,
            focus: 10
        })
    );
    assert_eq!(editor.body.focus, 1);
    assert_eq!(editor.body.rows[1].directed_range(), Some((4, 10)));
    assert_eq!(&SECOND[4..10], "second");
}

#[test]
fn a_range_naming_runs_this_view_never_published_is_refused_rather_than_clamped() {
    let (_ws, mut editor) = document(&[FIRST, SECOND]);
    let mut access = published(&editor);
    let row = editor.body.rows[1].id;
    editor.body.rows[1].select_range_now(2, 5);
    editor.reflow();
    let before = editor.body.rows[1].directed_range();

    // An id no run was ever published under.
    let invented = accesskit::NodeId(0xDEAD_BEEF_1234_5678);
    let data = selection_request(invented, 0, invented, 3);
    let outcome = perform(
        &mut editor,
        &mut access,
        row,
        Action::SetTextSelection,
        Some(&data),
    );

    assert_eq!(outcome, Err(Refused::StaleRange));
    assert_eq!(
        editor.body.rows[1].directed_range(),
        before,
        "a stale range was clamped onto whatever was nearest instead of refused"
    );
}

#[test]
fn a_range_belonging_to_another_paragraph_cannot_truncate_this_one() {
    let (_ws, mut editor) = document(&[FIRST, SECOND]);
    let mut access = published(&editor);
    let second = editor.body.rows[1].id;
    let elsewhere = whole_row(&access, &editor, 0);
    let before = editor.body.rows[1].directed_range();

    let data = ActionData::SetTextSelection(elsewhere);
    let outcome = perform(
        &mut editor,
        &mut access,
        second,
        Action::SetTextSelection,
        Some(&data),
    );

    assert_eq!(outcome, Err(Refused::StaleRange));
    assert_eq!(editor.body.rows[1].directed_range(), before);
}

#[test]
fn a_selection_request_with_no_data_is_malformed_rather_than_a_guess() {
    let (_ws, mut editor) = document(&[FIRST]);
    let mut access = published(&editor);
    let row = editor.body.rows[0].id;

    assert_eq!(
        perform(
            &mut editor,
            &mut access,
            row,
            Action::SetTextSelection,
            None
        ),
        Err(Refused::Malformed)
    );
    assert_eq!(
        perform(
            &mut editor,
            &mut access,
            row,
            Action::SetTextSelection,
            Some(&ActionData::NumericValue(3.0))
        ),
        Err(Refused::Malformed)
    );
}

// ------------------------------------------------------------- no mutation

/// **The read-only gate, and the shape of it.**
///
/// Not "read-only refuses these actions" — *no* accessibility request writes a
/// Koru document, read-only or not — so the assertion is about the document's
/// bytes rather than about a flag.
#[test]
fn no_assistive_technology_request_can_change_a_single_byte() {
    let (ws, mut editor) = document(&[FIRST, SECOND]);
    let mut access = published(&editor);
    let row = editor.body.rows[1].id;
    let before: Vec<String> = editor.body.rows.iter().map(|r| r.text.clone()).collect();
    let version = ws.version();

    let selection = ActionData::SetTextSelection(whole_row(&access, &editor, 1));
    let value = ActionData::Value("this should never arrive in the document".into());
    let asked: [(Action, Option<&ActionData>); 8] = [
        (Action::Focus, None),
        (Action::SetTextSelection, Some(&selection)),
        (Action::ScrollIntoView, None),
        (Action::SetValue, Some(&value)),
        (Action::ReplaceSelectedText, Some(&value)),
        (Action::Click, None),
        (Action::Increment, None),
        (Action::ShowContextMenu, None),
    ];
    for (action, data) in asked {
        let _ = perform(&mut editor, &mut access, row, action, data);
    }

    let after: Vec<String> = editor.body.rows.iter().map(|r| r.text.clone()).collect();
    assert_eq!(after, before, "an accessibility request changed the text");
    assert_eq!(
        ws.version(),
        version,
        "an accessibility request wrote an operation into the document"
    );
}

#[test]
fn the_two_actions_that_would_write_are_refused_by_name() {
    let (_ws, mut editor) = document(&[FIRST]);
    let mut access = published(&editor);
    let row = editor.body.rows[0].id;
    let value = ActionData::Value("no".into());

    for action in [Action::SetValue, Action::ReplaceSelectedText] {
        assert_eq!(
            perform(&mut editor, &mut access, row, action, Some(&value)),
            Err(Refused::WouldWrite),
            "{action:?}"
        );
    }
}

#[test]
fn an_action_the_editor_does_not_perform_is_refused_and_counted() {
    let (_ws, mut editor) = document(&[FIRST]);
    let mut access = published(&editor);
    let row = editor.body.rows[0].id;

    for action in [
        Action::Click,
        Action::Blur,
        Action::Expand,
        Action::Collapse,
        Action::Increment,
        Action::Decrement,
        Action::ShowContextMenu,
        Action::ScrollUp,
    ] {
        assert_eq!(
            perform(&mut editor, &mut access, row, action, None),
            Err(Refused::Unsupported),
            "{action:?}"
        );
    }
    assert_eq!(access.counters().requests_refused, 8);
}

// ------------------------------------------------------------------ reveal

#[test]
fn scroll_into_view_uses_the_views_own_reveal() {
    let texts: Vec<String> = (0..200)
        .map(|i| format!("Paragraph {i}, with enough words in it to occupy a line."))
        .collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let (_ws, mut editor) = document(&refs);
    let mut access = published(&editor);
    let far = editor.body.rows[150].id;
    assert_eq!(editor.view.scroll, 0.0, "the fixture starts at the top");

    let outcome = perform(&mut editor, &mut access, far, Action::ScrollIntoView, None);

    assert_eq!(outcome, Ok(Performed::Revealed));
    assert!(
        editor.view.scroll > 0.0,
        "revealing a paragraph 150 rows down did not scroll"
    );
    assert_eq!(editor.body.focus, 150);
    assert_eq!(access.counters().requests_performed, 1);
}
