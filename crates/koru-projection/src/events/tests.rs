// SPDX-License-Identifier: MPL-2.0

//! The diff rules, on bare rows — the delivery path is proved in
//! `worker/tests.rs`, where a real projector runs.

use super::*;

fn ids() -> (WorkspaceId, DocumentId) {
    (WorkspaceId::new(), DocumentId::new())
}

fn attr(role: &str, kind: &str, value: &str) -> ProjectedAttr {
    ProjectedAttr {
        id: format!("id-{role}-{value}"),
        role: role.to_owned(),
        kind: kind.to_owned(),
        value: value.to_owned(),
        zone: None,
        label: None,
    }
}

/// A projected node carrying only what the diff reads.
fn node_with(id: &str, attrs: Vec<ProjectedAttr>) -> ProjectedNode {
    ProjectedNode {
        id: id.to_owned(),
        parent: None,
        ordinal: 0,
        depth: 0,
        kind: "paragraph".to_owned(),
        text: String::new(),
        heading: None,
        list_style: None,
        done: false,
        level_name: None,
        name: None,
        icon: None,
        colour: None,
        language: None,
        blob: None,
        mime: None,
        embed: None,
        in_trash: false,
        runs: Vec::new(),
        attrs,
    }
}

/// The same node, in the document's own bin.
fn binned(id: &str, attrs: Vec<ProjectedAttr>) -> ProjectedNode {
    ProjectedNode {
        in_trash: true,
        ..node_with(id, attrs)
    }
}

#[test]
fn first_sight_of_an_attribute_is_a_set() {
    let (ws, doc) = ids();
    let new = [node_with("n1", vec![attr(KEY_DUE, "date", "2026-08-30")])];

    let events = diff(ws, doc, &[], &new);

    assert_eq!(
        events,
        vec![AttributeEvent {
            workspace: ws,
            document: doc,
            node: "n1".into(),
            kind: AttributeEventKind::DueChanged {
                change: AttributeChange::Set {
                    value: AttributeEventValue {
                        kind: "date".into(),
                        value: "2026-08-30".into(),
                    }
                }
            },
        }]
    );
    assert_eq!(events[0].kind.name(), "due.changed");
    assert_eq!(events[0].kind.key(), KEY_DUE);
}

#[test]
fn a_due_date_that_moved_is_a_change_carrying_the_new_value() {
    let (ws, doc) = ids();
    let old = [("n1".to_owned(), attr(KEY_DUE, "date", "2026-08-30"))];
    let new = [node_with("n1", vec![attr(KEY_DUE, "date", "2026-09-02")])];

    let events = diff(ws, doc, &old, &new);

    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].kind,
        AttributeEventKind::DueChanged {
            change: AttributeChange::Changed {
                value: AttributeEventValue {
                    kind: "date".into(),
                    value: "2026-09-02".into(),
                }
            }
        }
    );
}

/// A date-only due becoming a timed one is a change even when the strings
/// could collide — the kind is part of the value.
#[test]
fn a_kind_change_alone_is_a_change() {
    let (ws, doc) = ids();
    let old = [("n1".to_owned(), attr(KEY_DUE, "date", "2026-08-30"))];
    let new = [node_with(
        "n1",
        vec![attr(KEY_DUE, "datetime", "1787133600000")],
    )];

    let events = diff(ws, doc, &old, &new);

    assert!(matches!(
        &events[0].kind,
        AttributeEventKind::DueChanged {
            change: AttributeChange::Changed { value }
        } if value.kind == "datetime"
    ));
}

#[test]
fn a_cleared_attribute_is_a_clear() {
    let (ws, doc) = ids();
    let old = [("n1".to_owned(), attr(KEY_PRIORITY, "text", "high"))];
    let new = [node_with("n1", vec![])];

    let events = diff(ws, doc, &old, &new);

    assert_eq!(
        events[0].kind,
        AttributeEventKind::PriorityChanged {
            change: AttributeChange::Cleared
        }
    );
    assert_eq!(events[0].kind.name(), "priority.changed");
}

/// Tags are a set: each new value is one `tag.added`, each vanished one a
/// `tag.removed`, and a tag on both sides says nothing.
#[test]
fn tags_diff_as_a_set() {
    let (ws, doc) = ids();
    let old = [
        ("n1".to_owned(), attr(KEY_TAG, "text", "tag-kept")),
        ("n1".to_owned(), attr(KEY_TAG, "text", "tag-gone")),
    ];
    let new = [node_with(
        "n1",
        vec![
            attr(KEY_TAG, "text", "tag-kept"),
            attr(KEY_TAG, "text", "tag-new"),
        ],
    )];

    let events = diff(ws, doc, &old, &new);

    assert_eq!(
        events.iter().map(|e| &e.kind).collect::<Vec<_>>(),
        vec![
            &AttributeEventKind::TagAdded {
                tag: "tag-new".into()
            },
            &AttributeEventKind::TagRemoved {
                tag: "tag-gone".into()
            },
        ]
    );
    assert_eq!(events[0].kind.name(), "tag.added");
    assert_eq!(events[1].kind.name(), "tag.removed");
    assert_eq!(events[0].kind.key(), KEY_TAG);
}

/// A key outside the system vocabulary is the generic `attribute.changed`,
/// and it carries which key.
#[test]
fn a_custom_key_is_the_generic_event() {
    let (ws, doc) = ids();
    let new = [node_with("n1", vec![attr("user.effort", "text", "xl")])];

    let events = diff(ws, doc, &[], &new);

    assert_eq!(events[0].kind.name(), "attribute.changed");
    assert_eq!(events[0].kind.key(), "user.effort");
    assert!(matches!(
        &events[0].kind,
        AttributeEventKind::AttributeChanged { key, change: AttributeChange::Set { .. } }
            if key == "user.effort"
    ));
}

#[test]
fn an_unchanged_document_emits_nothing() {
    let (ws, doc) = ids();
    let old = [
        ("n1".to_owned(), attr(KEY_DUE, "date", "2026-08-30")),
        ("n1".to_owned(), attr(KEY_TAG, "text", "tag-1")),
    ];
    let new = [node_with(
        "n1",
        vec![
            attr(KEY_DUE, "date", "2026-08-30"),
            attr(KEY_TAG, "text", "tag-1"),
        ],
    )];

    assert_eq!(diff(ws, doc, &old, &new), vec![]);
}

/// A label is display text; a label-only edit is not a semantic change.
#[test]
fn a_label_only_edit_emits_nothing() {
    let (ws, doc) = ids();
    let old = [("n1".to_owned(), attr("user.link", "url", "https://a"))];
    let mut relabelled = attr("user.link", "url", "https://a");
    relabelled.label = Some("Terms page".into());
    let new = [node_with("n1", vec![relabelled])];

    assert_eq!(diff(ws, doc, &old, &new), vec![]);
}

/// Convergence duplicates under a cardinality-one key: the first row is the
/// fold winner every reader sees, and a duplicate appearing behind it is not
/// a change.
#[test]
fn a_duplicate_behind_the_fold_winner_is_not_a_change() {
    let (ws, doc) = ids();
    let old = [("n1".to_owned(), attr(KEY_COLOR, "text", "teal"))];
    let new = [node_with(
        "n1",
        vec![
            attr(KEY_COLOR, "text", "teal"),
            attr(KEY_COLOR, "text", "red"),
        ],
    )];

    assert_eq!(diff(ws, doc, &old, &new), vec![]);
}

/// A dropped document clears everything it was known to carry, in the same
/// vocabulary an edit uses.
#[test]
fn a_removal_clears_every_known_attr() {
    let (ws, doc) = ids();
    let old = [
        ("n1".to_owned(), attr(KEY_DUE, "date", "2026-08-30")),
        ("n1".to_owned(), attr(KEY_TAG, "text", "tag-1")),
        ("n2".to_owned(), attr("user.effort", "text", "xl")),
    ];

    let events = cleared(ws, doc, &old);

    assert_eq!(
        events
            .iter()
            .map(|e| (e.node.as_str(), e.kind.name()))
            .collect::<Vec<_>>(),
        vec![
            ("n1", "due.changed"),
            ("n1", "tag.removed"),
            ("n2", "attribute.changed"),
        ]
    );
    assert!(events.iter().all(|e| matches!(
        &e.kind,
        AttributeEventKind::TagRemoved { .. }
            | AttributeEventKind::DueChanged {
                change: AttributeChange::Cleared
            }
            | AttributeEventKind::AttributeChanged {
                change: AttributeChange::Cleared,
                ..
            }
    )));
}

/// Two nodes trading the same attribute is two entities' events, never a
/// "move" — the entity is the triple, and each side tells its own story.
#[test]
fn each_node_diffs_on_its_own() {
    let (ws, doc) = ids();
    let old = [("n1".to_owned(), attr(KEY_DUE, "date", "2026-08-30"))];
    let new = [
        node_with("n1", vec![]),
        node_with("n2", vec![attr(KEY_DUE, "date", "2026-08-30")]),
    ];

    let events = diff(ws, doc, &old, &new);

    assert_eq!(
        events
            .iter()
            .map(|e| (e.node.as_str(), e.kind.name()))
            .collect::<Vec<_>>(),
        vec![("n1", "due.changed"), ("n2", "due.changed")]
    );
    assert!(matches!(
        &events[0].kind,
        AttributeEventKind::DueChanged {
            change: AttributeChange::Cleared
        }
    ));
    assert!(matches!(
        &events[1].kind,
        AttributeEventKind::DueChanged {
            change: AttributeChange::Set { .. }
        }
    ));
}

/// **Binning a node clears its attributes; restoring sets them again.** The
/// old side comes from `document_attrs`, which is live-only, so the new side
/// has to be too — otherwise a document with a bin in it would announce the
/// same due date on every projection forever, and a restore would say nothing.
#[test]
fn a_binned_node_holds_no_live_attribute() {
    let (ws, doc) = ids();
    let old = [("n1".to_owned(), attr(KEY_DUE, "date", "2026-08-30"))];

    let binned_away = diff(
        ws,
        doc,
        &old,
        &[binned("n1", vec![attr(KEY_DUE, "date", "2026-08-30")])],
    );
    assert_eq!(binned_away.len(), 1);
    assert!(matches!(
        &binned_away[0].kind,
        AttributeEventKind::DueChanged {
            change: AttributeChange::Cleared
        }
    ));

    let restored = diff(
        ws,
        doc,
        &[],
        &[node_with("n1", vec![attr(KEY_DUE, "date", "2026-08-30")])],
    );
    assert!(matches!(
        &restored[0].kind,
        AttributeEventKind::DueChanged {
            change: AttributeChange::Set { .. }
        }
    ));
}
