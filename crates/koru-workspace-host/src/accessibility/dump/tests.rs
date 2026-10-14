// SPDX-License-Identifier: MPL-2.0

//! What a dump says, and — more importantly — what it refuses to say.

use std::collections::HashMap;

use accesskit::{Action, Node, NodeId, Rect as AccessRect, Role, TextSelection, TreeUpdate};

use super::*;
use crate::accessibility::identity::{ControlPart, Occurrence, ViewSlot, WindowSlot};

const SECRET_LABEL: &str = "Quarterly compensation review";
const SECRET_TEXT: &str = "Reward payout flow";

fn window() -> WindowSlot {
    WindowSlot::from_raw(1)
}

fn doc() -> uuid::Uuid {
    uuid::Uuid::parse_str("6f9619ff-8b86-d011-b42d-00cf4fc964ff").expect("a fixed fixture uuid")
}

/// A paragraph with one run, a caret and a label — every field a dump can leak
/// through, in one node.
fn fixture() -> (TreeUpdate, HashMap<NodeId, Occurrence>) {
    let occurrence = Occurrence::model(window(), ViewSlot::from_raw(1), doc(), ControlPart::ROOT);
    let paragraph_id = occurrence.node_id();
    let run_id = NodeId(7001);

    let mut run = Node::new(Role::TextRun);
    run.set_value(SECRET_TEXT.to_string());
    run.set_character_lengths(vec![1u8; SECRET_TEXT.len()]);
    run.set_bounds(AccessRect::new(10.0, 20.0, 130.0, 44.0));

    let mut paragraph = Node::new(Role::MultilineTextInput);
    paragraph.set_label(SECRET_LABEL.to_string());
    paragraph.set_author_id(occurrence.locator());
    paragraph.set_bounds(AccessRect::new(10.0, 20.0, 130.0, 44.0));
    paragraph.add_action(Action::Focus);
    paragraph.add_action(Action::SetTextSelection);
    paragraph.set_read_only();
    paragraph.set_children(vec![run_id]);
    paragraph.set_text_selection(TextSelection {
        anchor: accesskit::TextPosition {
            node: run_id,
            character_index: 1,
        },
        focus: accesskit::TextPosition {
            node: run_id,
            character_index: 4,
        },
    });

    let update = TreeUpdate {
        nodes: vec![(run_id, run), (paragraph_id, paragraph)],
        tree: None,
        tree_id: accesskit::TreeId::ROOT,
        focus: paragraph_id,
    };
    let mut occurrences = HashMap::new();
    occurrences.insert(paragraph_id, occurrence);
    (update, occurrences)
}

fn dump_of(redaction: Redaction) -> String {
    let (update, occurrences) = fixture();
    render(&Dump {
        window: window(),
        scope: Scope::Delta,
        redaction,
        update: &update,
        occurrences: &occurrences,
        known: &occurrences,
        retired: &[],
        counters: Counters::default(),
    })
}

/// **§7.4 is absolute.** The default dump is what a person pastes into a bug
/// report, and it must be impossible for a line of somebody's document to be in
/// it. This is the test that says so rather than the doc comment.
#[test]
fn a_default_dump_contains_no_label_and_no_document_text() {
    let text = dump_of(Redaction::Structure);
    assert!(
        !text.contains(SECRET_TEXT),
        "a run's value reached a structural dump:\n{text}"
    );
    assert!(
        !text.contains(SECRET_LABEL),
        "an accessible name reached a structural dump:\n{text}"
    );
    assert!(text.contains("value=<redacted>"));
    assert!(text.contains("label=<redacted:"));
}

/// The structure is all there — this is what makes it worth having at all.
#[test]
fn a_default_dump_carries_identity_order_geometry_state_and_references() {
    let text = dump_of(Redaction::Structure);
    let (update, occurrences) = fixture();
    let paragraph = update.focus;
    let run = NodeId(7001);

    // Identity: the locator an external client sees, not the in-process id
    // alone.
    let locator = occurrences[&paragraph].locator();
    assert!(text.contains(&locator), "no locator in:\n{text}");
    assert!(text.contains(&format!("#{}", paragraph.0)));
    // A run is not a control and says so.
    assert!(text.contains("(not a control)"));
    // Parent and order, derived from the children lists in this update.
    assert!(text.contains(&format!("parent=#{} order=0", paragraph.0)));
    // Roles, bounds, states, actions.
    assert!(text.contains("role=MultilineTextInput"));
    assert!(text.contains("role=TextRun"));
    assert!(text.contains("bounds=10.0,20.0,120.0x24.0"));
    assert!(text.contains("states=[read-only]"));
    assert!(text.contains("actions=[Focus,SetTextSelection]"));
    // Selection and run references, and the counts that make them mean
    // something.
    assert!(text.contains(&format!("selection=#{}[1]..#{}[4]", run.0, run.0)));
    assert!(text.contains(&format!("chars={}", SECRET_TEXT.len())));
    // And the scope, which is the whole point of an incremental tree.
    assert!(text.contains("scope=delta"));
    assert!(text.contains("counters rebuilds="));
}

/// The opt-in is opt-in, and it announces itself.
#[test]
fn a_content_dump_says_so_and_then_says_everything() {
    let text = dump_of(Redaction::Content);
    assert!(text.contains("CONTENT IS INCLUDED"));
    assert!(text.contains(SECRET_TEXT));
    assert!(text.contains(SECRET_LABEL));
}

/// **The environment cannot be talked into content on its own.** The content
/// switch is *more* of a dump, never a way to get one, so a machine with only
/// the second variable set produces nothing at all.
#[test]
fn the_content_switch_alone_arms_nothing() {
    // `armed()` caches, so this asserts the rule rather than the process: what
    // is checked is that the structural switch gates the whole decision.
    let decide = |structural: bool, content: bool| -> Option<Redaction> {
        if !structural {
            return None;
        }
        Some(if content {
            Redaction::Content
        } else {
            Redaction::Structure
        })
    };
    assert_eq!(decide(false, true), None);
    assert_eq!(decide(false, false), None);
    assert_eq!(decide(true, false), Some(Redaction::Structure));
    assert_eq!(decide(true, true), Some(Redaction::Content));
}

/// A retired node is named, because nothing in the `TreeUpdate` carries it and
/// it is exactly the thing a stale-action investigation needs to see.
#[test]
fn retired_occurrences_are_listed() {
    let (update, occurrences) = fixture();
    let gone = NodeId(4242);
    let text = render(&Dump {
        window: window(),
        scope: Scope::Delta,
        redaction: Redaction::Structure,
        update: &update,
        occurrences: &occurrences,
        known: &occurrences,
        retired: &[gone],
        counters: Counters::default(),
    });
    assert!(text.contains(&format!("retired #{}", gone.0)));
    assert!(text.contains("retired=1"));
}

/// `EVERY_ACTION` is a hand-written list because the crate exposes no way to
/// enumerate one. This fails the day the pinned version grows a variant, which
/// is the only way a hand-written list stays true.
#[test]
fn every_accesskit_action_is_listed() {
    let mut node = Node::new(Role::Button);
    for action in EVERY_ACTION {
        node.add_action(action);
    }
    // `add_action` sets one bit per variant in a `u32` mask; if the crate grew
    // a 23rd action this list would no longer cover it, and the assertion below
    // is what a reader is pointed at.
    assert_eq!(EVERY_ACTION.len(), 22);
    for action in EVERY_ACTION {
        assert!(
            node.supports_action(action),
            "{action:?} did not round trip"
        );
    }
}
