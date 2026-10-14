// SPDX-License-Identifier: MPL-2.0

//! The canvas model: the object rule, the writers, the orders.
//!
//! No disk and no store — every document here is an in-memory `WorkspaceDoc`,
//! and "through storage" means an exported snapshot read back. The two-device
//! cases live in `tests/convergence.rs`, beside the rest of the CRDT pins.

use std::collections::HashMap;

use loro::{LoroMap, LoroValue};

use super::*;
use crate::WorkspaceId;
use crate::node::CANVAS_LIMIT;
use crate::schema::{at_from_loro, extent_from_loro, read_node_id};

// ------------------------------------------------------------------ fixtures

/// A fresh canvas document: a `Document` root, as the store's opener makes.
fn canvas() -> (WorkspaceDoc, NodeId) {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "canvas").unwrap();
    let root = ws
        .insert(Parent::Root, Node::document("Board").unwrap())
        .unwrap();
    (ws, root)
}

fn at(x: i32, y: i32) -> CanvasPoint {
    CanvasPoint::new(x, y)
}

fn card(ws: &mut WorkspaceDoc, root: NodeId, x: i32, y: i32, text: &str) -> NodeId {
    add_object(ws, root, ObjectKind::Card, at(x, y), text).unwrap()
}

fn image(ws: &mut WorkspaceDoc, root: NodeId, x: i32, y: i32) -> NodeId {
    add_image(
        ws,
        root,
        at(x, y),
        BlobHash("b3:feed".into()),
        "image/png",
        Some(640),
        Some(480),
        320,
    )
    .unwrap()
}

fn snapshot(ws: &WorkspaceDoc, root: NodeId) -> CanvasSnapshot {
    read(ws, root).unwrap()
}

fn object(ws: &WorkspaceDoc, root: NodeId, id: NodeId) -> CanvasObject {
    snapshot(ws, root)
        .get(id)
        .cloned()
        .unwrap_or_else(|| panic!("{id:?} is not on the canvas"))
}

fn ids(objects: &[CanvasObject]) -> Vec<NodeId> {
    objects.iter().map(|o| o.id).collect()
}

fn refusal(result: Result<impl std::fmt::Debug>) -> CanvasRefusal {
    match result {
        Err(StoreError::CanvasRejected { why, .. }) => why,
        other => panic!("expected a canvas refusal, got {other:?}"),
    }
}

/// The meta map behind a node, found the way a peer's raw write would reach
/// it — for the tests that store what no writer here would.
fn raw_meta(ws: &WorkspaceDoc, id: NodeId) -> LoroMap {
    let tree = ws.doc().get_tree("nodes");
    tree.nodes()
        .into_iter()
        .filter(|tid| !tree.is_node_deleted(tid).unwrap_or(true))
        .find_map(|tid| {
            let meta = tree.get_meta(tid).ok()?;
            (read_node_id(&meta) == Some(id)).then_some(meta)
        })
        .expect("the node is in the tree")
}

fn map(fields: &[(&str, LoroValue)]) -> LoroValue {
    LoroValue::from(
        fields
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect::<HashMap<_, _>>(),
    )
}

fn stored(ws: &WorkspaceDoc, id: NodeId, key: &str, value: LoroValue) {
    raw_meta(ws, id).insert(key, value).unwrap();
    ws.commit();
}

// ------------------------------------------------------------------ storage

/// **Through a copy, and through storage.** `cutting` carries the whole
/// `NodeMeta` and `plant` writes it with `write_node`, so every value that
/// function forgets is a value every copy drops. `prose_language` was one of
/// them — on every build, not only an older one's copy.
#[test]
fn write_cutting_plant_read_round_trips_at_extent_and_prose_language() {
    let (mut ws, root) = canvas();
    ws.set_prose_language(root, Some("de-DE")).unwrap();
    let a = card(&mut ws, root, -120, 40, "Erste\nzweite Zeile");
    let pic = image(&mut ws, root, 500, -3);
    set_width(&mut ws, a, 310).unwrap();

    let cutting = ws.cutting(root).unwrap();
    let mut other = WorkspaceDoc::create(WorkspaceId::new(), "copy").unwrap();
    let copy = other.plant(&cutting, Parent::Root, "").unwrap();
    let other = WorkspaceDoc::from_snapshot(&other.export_snapshot().unwrap()).unwrap();

    assert_eq!(
        other.read(copy).unwrap().meta.prose_language.as_deref(),
        Some("de-DE"),
        "the copy keeps its spelling language"
    );
    let original = snapshot(&ws, root);
    let copied = snapshot(&other, copy);
    assert_eq!(copied.objects.len(), 2);
    for (was, now) in original.objects.iter().zip(&copied.objects) {
        assert_ne!(was.id, now.id, "a copy mints fresh ids");
        assert_eq!(
            (was.kind, was.at, was.extent, &was.text, &was.lines),
            (now.kind, now.at, now.extent, &now.text, &now.lines)
        );
    }
    assert_eq!(copied.objects[0].at, Some(at(-120, 40)));
    assert_eq!(copied.objects[0].extent, Some(CanvasExtent::new(310, None)));
    assert_eq!(copied.objects[1].extent, Some(CanvasExtent::new(320, None)));
    let _ = pic;
}

/// A page node carries neither key, and reading a document written before
/// canvases existed writes nothing — the whole compatibility story of an
/// additive meta value.
#[test]
fn a_page_node_has_no_position_and_reading_one_writes_nothing() {
    let (mut ws, root) = canvas();
    let line = ws
        .insert(Parent::Node(root), Node::paragraph("prose").unwrap())
        .unwrap();
    let node = ws.read(line).unwrap();
    assert_eq!((node.meta.at, node.meta.extent), (None, None));
    let meta = raw_meta(&ws, line);
    assert!(meta.get("at").is_none() && meta.get("extent").is_none());

    let before = ws.export_snapshot().unwrap();
    let reopened = WorkspaceDoc::from_snapshot(&before).unwrap();
    read(&reopened, root).unwrap();
    assert_eq!(reopened.export_snapshot().unwrap(), before);
}

/// Each value is **one** map in the meta, never split keys — the property
/// that makes two concurrent moves resolve to one whole position.
#[test]
fn a_position_is_one_whole_map_value() {
    let (mut ws, root) = canvas();
    let a = card(&mut ws, root, 7, -9, "x");
    let meta = raw_meta(&ws, a);
    assert_eq!(
        meta.get("at").and_then(|v| v.into_value().ok()),
        Some(map(&[("x", LoroValue::I64(7)), ("y", LoroValue::I64(-9))]))
    );
    assert_eq!(
        meta.get("extent").and_then(|v| v.into_value().ok()),
        Some(map(&[("w", LoroValue::I64(i64::from(DEFAULT_WIDTH)))])),
        "a card's height is derived, so none is stored"
    );
}

// ------------------------------------------------------------------ lenient readers

/// Every malformed shape a peer could store reads as *no position* — the
/// object becomes an arrival — and never fails the node.
#[test]
fn a_malformed_position_reads_as_none_and_makes_an_arrival() {
    let huge = i64::from(i32::MAX) + 1;
    let beyond = i64::from(CANVAS_LIMIT) + 1;
    let cases = [
        ("a number, not a map", LoroValue::I64(3)),
        ("a string", LoroValue::from("10,20")),
        ("null", LoroValue::Null),
        ("no y", map(&[("x", LoroValue::I64(1))])),
        ("no x", map(&[("y", LoroValue::I64(1))])),
        (
            "x a string",
            map(&[("x", "1".into()), ("y", LoroValue::I64(1))]),
        ),
        (
            "x a float",
            map(&[("x", LoroValue::Double(1.0)), ("y", LoroValue::I64(1))]),
        ),
        (
            "x a bool",
            map(&[("x", true.into()), ("y", LoroValue::I64(1))]),
        ),
        (
            "x beyond i32",
            map(&[("x", LoroValue::I64(huge)), ("y", LoroValue::I64(1))]),
        ),
        (
            "y below i32",
            map(&[("x", LoroValue::I64(1)), ("y", LoroValue::I64(-huge))]),
        ),
        (
            "x beyond the limit",
            map(&[("x", LoroValue::I64(beyond)), ("y", LoroValue::I64(1))]),
        ),
        ("an empty map", map(&[])),
        (
            "a list",
            LoroValue::from(vec![LoroValue::I64(1), LoroValue::I64(2)]),
        ),
    ];
    for (what, value) in cases {
        let (mut ws, root) = canvas();
        let a = card(&mut ws, root, 0, 0, "the card");
        stored(&ws, a, "at", value);
        let node = ws
            .read(a)
            .unwrap_or_else(|e| panic!("{what}: the node failed: {e}"));
        assert_eq!(node.meta.at, None, "{what}");
        let canvas = snapshot(&ws, root);
        assert!(canvas.objects.is_empty(), "{what}");
        assert_eq!(
            ids(&canvas.arrivals),
            vec![a],
            "{what}: shown as an arrival"
        );
    }
}

/// A field this build does not know is ignored, so a later build can add one
/// (`z`, a rotation) without this one reading the whole position as broken.
#[test]
fn unknown_fields_are_ignored() {
    let (mut ws, root) = canvas();
    let a = card(&mut ws, root, 0, 0, "x");
    stored(
        &ws,
        a,
        "at",
        map(&[
            ("x", LoroValue::I64(5)),
            ("y", LoroValue::I64(6)),
            ("z", LoroValue::I64(2)),
            ("turn", LoroValue::Double(0.5)),
        ]),
    );
    stored(
        &ws,
        a,
        "extent",
        map(&[("w", LoroValue::I64(90)), ("depth", "deep".into())]),
    );
    let read = ws.read(a).unwrap().meta;
    assert_eq!(read.at, Some(at(5, 6)));
    assert_eq!(read.extent, Some(CanvasExtent::new(90, None)));
}

/// `w` is required and a size is at least one pixel; a malformed `h` costs
/// the height, never the width a person set.
#[test]
fn a_malformed_extent_reads_leniently() {
    let (mut ws, root) = canvas();
    let a = card(&mut ws, root, 0, 0, "x");
    let read_back = |ws: &WorkspaceDoc, value: LoroValue| {
        stored(ws, a, "extent", value);
        ws.read(a).unwrap().meta.extent
    };
    assert_eq!(read_back(&ws, map(&[("h", LoroValue::I64(5))])), None);
    assert_eq!(read_back(&ws, map(&[("w", LoroValue::I64(0))])), None);
    assert_eq!(read_back(&ws, map(&[("w", LoroValue::I64(-40))])), None);
    assert_eq!(read_back(&ws, map(&[("w", "wide".into())])), None);
    assert_eq!(read_back(&ws, LoroValue::I64(200)), None);
    assert_eq!(
        read_back(
            &ws,
            map(&[("w", LoroValue::I64(200)), ("h", "tall".into())])
        ),
        Some(CanvasExtent::new(200, None))
    );
    assert_eq!(
        read_back(
            &ws,
            map(&[("w", LoroValue::I64(200)), ("h", LoroValue::I64(80))])
        ),
        Some(CanvasExtent::new(200, Some(80)))
    );
    // Still an object: a missing extent is a default width, not a lost card.
    assert_eq!(ids(&snapshot(&ws, root).objects), vec![a]);
}

/// A position stored as a **container** rather than a value — what a
/// careless writer using `insert_container` would leave — reads as none too.
#[test]
fn a_position_stored_as_a_container_reads_as_none() {
    let (mut ws, root) = canvas();
    let a = card(&mut ws, root, 0, 0, "x");
    let meta = raw_meta(&ws, a);
    let inner = meta.insert_container("at", LoroMap::new()).unwrap();
    inner.insert("x", 1).unwrap();
    inner.insert("y", 2).unwrap();
    ws.commit();
    assert_eq!(ws.read(a).unwrap().meta.at, None);
}

/// A small property check over the decoders: nothing panics, and anything
/// decoded lies inside the bounds every writer keeps.
#[test]
fn the_decoders_never_panic_and_never_leave_the_bounds() {
    let numbers = [
        LoroValue::I64(0),
        LoroValue::I64(1),
        LoroValue::I64(-1),
        LoroValue::I64(i64::from(CANVAS_LIMIT)),
        LoroValue::I64(-i64::from(CANVAS_LIMIT)),
        LoroValue::I64(i64::from(CANVAS_LIMIT) + 1),
        LoroValue::I64(i64::MAX),
        LoroValue::I64(i64::MIN),
        LoroValue::Double(f64::NAN),
        LoroValue::Double(f64::INFINITY),
        LoroValue::Double(3.5),
        LoroValue::Null,
        LoroValue::Bool(false),
        LoroValue::from("7"),
    ];
    for x in &numbers {
        for y in &numbers {
            let value = map(&[
                ("x", x.clone()),
                ("y", y.clone()),
                ("w", x.clone()),
                ("h", y.clone()),
            ]);
            if let Some(point) = at_from_loro(&value) {
                assert!(point.x.abs() <= CANVAS_LIMIT && point.y.abs() <= CANVAS_LIMIT);
            }
            if let Some(extent) = extent_from_loro(&value) {
                assert!((1..=CANVAS_LIMIT).contains(&extent.w));
                assert!(extent.h.is_none_or(|h| (1..=CANVAS_LIMIT).contains(&h)));
            }
            assert_eq!(at_from_loro(x), None, "a bare scalar is never a position");
        }
    }
}

/// Every writer clamps: a position to ±`CANVAS_LIMIT`, a size to at least one
/// pixel.
#[test]
fn writers_clamp_what_they_store() {
    let (mut ws, root) = canvas();
    let a = card(&mut ws, root, i32::MAX, i32::MIN, "x");
    assert_eq!(
        object(&ws, root, a).at,
        Some(at(CANVAS_LIMIT, -CANVAS_LIMIT))
    );
    move_object(&mut ws, a, at(i32::MIN, 12)).unwrap();
    assert_eq!(object(&ws, root, a).at, Some(at(-CANVAS_LIMIT, 12)));
    for (asked, got) in [(0, 1), (-5, 1), (i32::MAX, CANVAS_LIMIT), (480, 480)] {
        set_width(&mut ws, a, asked).unwrap();
        assert_eq!(
            object(&ws, root, a).extent,
            Some(CanvasExtent::new(got, None))
        );
    }
    ws.set_extent(a, Some(CanvasExtent::new(10, Some(-3))))
        .unwrap();
    assert_eq!(
        object(&ws, root, a).extent,
        Some(CanvasExtent::new(10, Some(1)))
    );
    ws.set_at(a, None).unwrap();
    assert_eq!(
        ids(&snapshot(&ws, root).arrivals),
        vec![a],
        "no position, an arrival"
    );
}

// ------------------------------------------------------------------ the object rule

#[test]
fn read_classifies_each_kind() {
    let (mut ws, root) = canvas();
    let plain_card = card(&mut ws, root, 0, 0, "Idea\nmore\nand more");
    let pic = image(&mut ws, root, 300, 0);
    let start = add_object(&mut ws, root, ObjectKind::Start, at(0, 200), "Go").unwrap();
    let agent = add_object(
        &mut ws,
        root,
        ObjectKind::Agent,
        at(300, 200),
        "Summarize\nbriefly",
    )
    .unwrap();
    let output = add_object(&mut ws, root, ObjectKind::Output, at(600, 200), "Report").unwrap();
    // What a writer with no notion of place leaves: a typed line, an empty
    // trailing line, and a child page.
    let arrival = ws
        .insert(
            Parent::Node(root),
            Node::paragraph("typed on the phone").unwrap(),
        )
        .unwrap();
    ws.insert(Parent::Node(root), Node::paragraph("").unwrap())
        .unwrap();
    let page = ws
        .insert(Parent::Node(root), Node::document("A child page").unwrap())
        .unwrap();
    ws.set_at(page, Some(at(5, 5))).unwrap();
    let arrow = add_arrow(&mut ws, root, start, agent).unwrap();

    let canvas = snapshot(&ws, root);
    let kinds: Vec<(NodeId, ObjectKind)> = canvas.objects.iter().map(|o| (o.id, o.kind)).collect();
    assert_eq!(
        kinds,
        vec![
            (plain_card, ObjectKind::Card),
            (pic, ObjectKind::Image),
            (start, ObjectKind::Start),
            (agent, ObjectKind::Agent),
            (output, ObjectKind::Output),
        ],
        "sibling order, and a document is never an object even with a position"
    );
    assert_eq!(
        ids(&canvas.arrivals),
        vec![arrival],
        "the empty line is not an arrival"
    );
    assert_eq!(canvas.arrivals[0].kind, ObjectKind::Card);
    assert_eq!(
        canvas.arrows,
        vec![CanvasArrow {
            id: arrow,
            from: start,
            to: agent,
            label: String::new()
        }]
    );

    let card = canvas.get(plain_card).unwrap();
    assert_eq!(card.text, "Idea");
    assert_eq!(card.lines, vec!["more", "and more"]);
    assert_eq!(card.extent, Some(CanvasExtent::new(DEFAULT_WIDTH, None)));
    let img = canvas.get(pic).unwrap();
    assert_eq!(
        img.image,
        Some(CanvasImage {
            hash: BlobHash("b3:feed".into()),
            mime: "image/png".into(),
            width: Some(640),
            height: Some(480),
        })
    );
    assert_eq!((img.text.as_str(), img.lines.len()), ("", 0));
    let agent_node = canvas.get(agent).unwrap();
    assert_eq!(
        agent_node.lines,
        vec!["briefly"],
        "its lines are its instruction"
    );
    assert_eq!(agent_node.agent, None, "no agent chosen yet");
    assert_eq!(
        ws.read(agent)
            .unwrap()
            .attr(ROLE_AGENT)
            .and_then(Attr::as_text),
        Some(""),
        "stored as the empty key"
    );
    assert_eq!(
        ws.read(start)
            .unwrap()
            .attr(ROLE_START)
            .and_then(Attr::as_text),
        Some(START_MANUAL)
    );
    assert_eq!(
        ws.read(output)
            .unwrap()
            .attr(ROLE_OUTPUT)
            .and_then(Attr::as_text),
        Some(OUTPUT_PAGE)
    );
}

/// A card's lines are its unpositioned descendants, depth first: a list adds
/// its items and no line of its own, a code block one line per line, and a
/// positioned descendant (an object inside a frame, later) is not a line.
#[test]
fn a_cards_lines_are_its_unpositioned_descendants() {
    let (mut ws, root) = canvas();
    let a = card(&mut ws, root, 0, 0, "Head");
    let list = ws
        .insert(Parent::Node(a), Node::list_block().unwrap())
        .unwrap();
    ws.insert(Parent::Node(list), Node::paragraph("one").unwrap())
        .unwrap();
    ws.insert(Parent::Node(list), Node::paragraph("two").unwrap())
        .unwrap();
    ws.insert(Parent::Node(a), Node::code("let x;\nlet y;").unwrap())
        .unwrap();
    let inside = ws
        .insert(Parent::Node(a), Node::paragraph("inside a frame").unwrap())
        .unwrap();
    ws.set_at(inside, Some(at(10, 10))).unwrap();
    ws.insert(Parent::Node(a), Node::divider().unwrap())
        .unwrap();

    assert_eq!(
        object(&ws, root, a).lines,
        vec!["one", "two", "let x;", "let y;"]
    );
}

/// Pill anchors are not text: a card's line reads as the words around them.
#[test]
fn text_is_plain() {
    let (mut ws, root) = canvas();
    let a = card(&mut ws, root, 0, 0, "Call  today");
    ws.insert_pill(a, 5, &Attr::new("koru.due", AttrValue::Text("x".into())))
        .unwrap();
    assert!(
        ws.read(a)
            .unwrap()
            .content
            .as_text()
            .unwrap()
            .contains(ANCHOR)
    );
    assert_eq!(object(&ws, root, a).text, "Call  today");
}

/// An arrow is listed only while both of its ends are on the canvas — and a
/// node carrying only one end is still an arrow, never an arrival.
#[test]
fn arrows_with_a_missing_end_are_hidden() {
    let (mut ws, root) = canvas();
    let a = card(&mut ws, root, 0, 0, "a");
    let b = card(&mut ws, root, 300, 0, "b");
    let line = ws
        .insert(Parent::Node(a), Node::paragraph("a line of a").unwrap())
        .unwrap();
    let raw_arrow = |ws: &mut WorkspaceDoc, attrs: Vec<Attr>| {
        ws.insert(
            Parent::Node(root),
            Node::new(
                Kind::Paragraph,
                Content::Text {
                    text: "label".into(),
                    attrs,
                },
            )
            .unwrap(),
        )
        .unwrap()
    };
    let from = |n| Attr::new(ROLE_ARROW_FROM, AttrValue::Ref(Ref::new(n)));
    let to = |n| Attr::new(ROLE_ARROW_TO, AttrValue::Ref(Ref::new(n)));
    raw_arrow(&mut ws, vec![from(a)]);
    raw_arrow(&mut ws, vec![to(b)]);
    raw_arrow(&mut ws, vec![from(a), to(a)]);
    raw_arrow(&mut ws, vec![from(a), to(line)]);
    raw_arrow(&mut ws, vec![from(a), to(NodeId::new())]);
    let shown = raw_arrow(&mut ws, vec![from(a), to(b)]);

    let canvas = snapshot(&ws, root);
    assert_eq!(canvas.arrows.len(), 1);
    assert_eq!(canvas.arrows[0].id, shown);
    assert_eq!(canvas.arrows[0].label, "label");
    assert!(
        canvas.arrivals.is_empty(),
        "a one-ended arrow is not an arrival"
    );
}

/// Deleting an object leaves the arrow that names it alone — it dangles and
/// is hidden — so one Ctrl+Z brings both back.
#[test]
fn a_deleted_objects_arrows_come_back_with_undo() {
    let (mut ws, root) = canvas();
    let a = card(&mut ws, root, 0, 0, "a\nwith a line");
    let b = card(&mut ws, root, 300, 0, "b");
    let arrow = add_arrow(&mut ws, root, a, b).unwrap();
    ws.enable_undo();
    let before = snapshot(&ws, root);

    delete_object(&mut ws, a).unwrap();
    let after = snapshot(&ws, root);
    assert_eq!(ids(&after.objects), vec![b]);
    assert!(after.arrows.is_empty());
    assert!(ws.read(arrow).is_ok(), "the arrow itself was not touched");

    assert!(ws.undo().unwrap());
    assert_eq!(snapshot(&ws, root), before);
    assert!(!ws.can_undo(), "one step");
}

// ------------------------------------------------------------------ undo

/// Run `write` on a canvas with undo on, and prove it was **one** step: it
/// changed the canvas, one Ctrl+Z restores exactly what was there, and there
/// is no second step behind it.
fn one_undo_step(
    setup: impl FnOnce(&mut WorkspaceDoc, NodeId) -> Vec<NodeId>,
    write: impl FnOnce(&mut WorkspaceDoc, NodeId, &[NodeId]),
) {
    let (mut ws, root) = canvas();
    let made = setup(&mut ws, root);
    ws.enable_undo();
    let before = snapshot(&ws, root);
    let doc_before = ws.doc().get_deep_value();

    write(&mut ws, root, &made);
    assert_ne!(
        ws.doc().get_deep_value(),
        doc_before,
        "the write changed something"
    );
    assert!(ws.undo().unwrap(), "and it can be undone");
    assert_eq!(
        snapshot(&ws, root),
        before,
        "one Ctrl+Z restores the canvas"
    );
    assert!(!ws.can_undo(), "the write was one step, not several");
    assert!(ws.redo().unwrap(), "and redo puts it back");
}

#[test]
fn every_writer_is_one_undo_step() {
    let two_cards = |ws: &mut WorkspaceDoc, root: NodeId| {
        vec![
            card(ws, root, 0, 0, "a\nline one\nline two"),
            card(ws, root, 300, 0, "b"),
        ]
    };
    let an_agent = |ws: &mut WorkspaceDoc, root: NodeId| {
        vec![add_object(ws, root, ObjectKind::Agent, at(0, 0), "Do it").unwrap()]
    };

    one_undo_step(
        |_, _| vec![],
        |ws, root, _| {
            card(ws, root, 1, 2, "multi\nline\ncard");
        },
    );
    one_undo_step(
        |_, _| vec![],
        |ws, root, _| {
            add_object(ws, root, ObjectKind::Agent, at(1, 2), "agent").unwrap();
        },
    );
    one_undo_step(
        |_, _| vec![],
        |ws, root, _| {
            image(ws, root, 1, 2);
        },
    );
    one_undo_step(two_cards, |ws, _, made| {
        move_object(ws, made[0], at(50, 60)).unwrap()
    });
    one_undo_step(two_cards, |ws, _, made| {
        set_width(ws, made[0], 500).unwrap()
    });
    one_undo_step(two_cards, |ws, _, made| {
        set_text(ws, made[0], "A\nline one\nchanged\nadded").unwrap()
    });
    one_undo_step(two_cards, |ws, _, made| {
        set_text(ws, made[0], "just one").unwrap()
    });
    one_undo_step(two_cards, |ws, root, made| {
        add_arrow(ws, root, made[0], made[1]).unwrap();
    });
    one_undo_step(
        |ws, root| {
            let mut made = two_cards(ws, root);
            made.push(add_arrow(ws, root, made[0], made[1]).unwrap());
            made
        },
        |ws, _, made| delete_arrow(ws, made[2]).unwrap(),
    );
    one_undo_step(two_cards, |ws, _, made| delete_object(ws, made[0]).unwrap());
    one_undo_step(an_agent, |ws, _, made| {
        set_agent(ws, made[0], "claude", Some("opus")).unwrap()
    });
    one_undo_step(an_agent, |ws, _, made| {
        set_run(ws, made[0], DocumentId::new()).unwrap()
    });
    one_undo_step(an_agent, |ws, _, made| {
        set_result(
            ws,
            made[0],
            &["first".into(), "second".into(), "third".into()],
        )
        .unwrap()
    });
    one_undo_step(
        |ws, root| {
            let made = an_agent(ws, root);
            set_result(ws, made[0], &["old".into(), "lines".into()]).unwrap();
            made
        },
        |ws, _, made| set_result(ws, made[0], &["new".into()]).unwrap(),
    );
    one_undo_step(
        |ws, root| vec![add_object(ws, root, ObjectKind::Output, at(0, 0), "out").unwrap()],
        |ws, _, made| set_output_page(ws, made[0], DocumentId::new()).unwrap(),
    );
    one_undo_step(
        |ws, root| vec![add_object(ws, root, ObjectKind::Start, at(0, 0), "go").unwrap()],
        |ws, _, made| set_execution(ws, made[0], "run", "device").unwrap(),
    );
    one_undo_step(an_agent, |ws, _, made| {
        set_step_execution(ws, made[0], "run").unwrap()
    });
    one_undo_step(two_cards, |ws, _, made| {
        bring_to_front(ws, made[0]).unwrap()
    });
    one_undo_step(two_cards, |ws, _, made| send_to_back(ws, made[1]).unwrap());
}

/// A write that changes nothing leaves no step behind: Ctrl+Z after it undoes
/// the gesture before, not a phantom.
#[test]
fn a_write_that_changes_nothing_is_no_step() {
    let (mut ws, root) = canvas();
    let a = card(&mut ws, root, 10, 20, "a\nline");
    let b = card(&mut ws, root, 300, 0, "b");
    ws.enable_undo();
    move_object(&mut ws, a, at(10, 20)).unwrap();
    set_width(&mut ws, a, DEFAULT_WIDTH).unwrap();
    set_text(&mut ws, a, "a\nline").unwrap();
    bring_to_front(&mut ws, b).unwrap();
    send_to_back(&mut ws, a).unwrap();
    assert!(!ws.can_undo(), "nothing was written");
}

/// A refusal is checked before anything is written.
#[test]
fn refusals_are_tokens_and_write_nothing() {
    let (mut ws, root) = canvas();
    let a = card(&mut ws, root, 0, 0, "a");
    let b = card(&mut ws, root, 300, 0, "b");
    let pic = image(&mut ws, root, 0, 300);
    let arrow = add_arrow(&mut ws, root, a, b).unwrap();
    let line = ws
        .insert(Parent::Node(a), Node::paragraph("a line").unwrap())
        .unwrap();
    ws.enable_undo();

    assert_eq!(
        refusal(add_object(&mut ws, root, ObjectKind::Image, at(0, 0), "")),
        CanvasRefusal::ImageNeedsAddImage
    );
    assert_eq!(
        refusal(add_arrow(&mut ws, root, a, a)),
        CanvasRefusal::SelfArrow
    );
    assert_eq!(
        refusal(add_arrow(&mut ws, root, a, b)),
        CanvasRefusal::DuplicateArrow
    );
    assert_eq!(
        refusal(add_arrow(&mut ws, root, a, line)),
        CanvasRefusal::NotOnCanvas
    );
    assert_eq!(
        refusal(move_object(&mut ws, arrow, at(1, 1))),
        CanvasRefusal::NotAnObject
    );
    assert_eq!(
        refusal(move_object(&mut ws, root, at(1, 1))),
        CanvasRefusal::NotAnObject
    );
    assert_eq!(
        refusal(delete_object(&mut ws, arrow)),
        CanvasRefusal::NotAnObject
    );
    assert_eq!(refusal(delete_arrow(&mut ws, a)), CanvasRefusal::NotAnArrow);
    assert_eq!(
        refusal(set_agent(&mut ws, a, "claude", None)),
        CanvasRefusal::NotAnAgent
    );
    assert_eq!(
        refusal(set_run(&mut ws, a, DocumentId::new())),
        CanvasRefusal::NotAnAgent
    );
    assert_eq!(
        refusal(set_result(&mut ws, a, &["x".into()])),
        CanvasRefusal::NotAnAgent
    );
    assert_eq!(
        refusal(set_output_page(&mut ws, a, DocumentId::new())),
        CanvasRefusal::NotAnOutput
    );
    assert_eq!(
        refusal(set_text(&mut ws, pic, "words")),
        CanvasRefusal::NoText
    );
    assert!(!ws.can_undo(), "no refusal wrote anything");
    // The reverse direction is a different arrow, and allowed.
    add_arrow(&mut ws, root, b, a).unwrap();
    assert_eq!(CanvasRefusal::DuplicateArrow.as_str(), "duplicate_arrow");
}

// ------------------------------------------------------------------ text

/// `set_text` is diffed, not rebuilt: an unchanged line keeps its node, a
/// changed plain line is edited in place, and surplus lines go.
#[test]
fn set_text_edits_lines_in_place() {
    let (mut ws, root) = canvas();
    let a = card(&mut ws, root, 0, 0, "Title\none\ntwo\nthree");
    let lines_before = ws.children(Parent::Node(a)).unwrap();
    assert_eq!(lines_before.len(), 3);

    set_text(&mut ws, a, "Title\none\nTWO\nthree\nfour").unwrap();
    let lines_after = ws.children(Parent::Node(a)).unwrap();
    assert_eq!(
        &lines_after[..3],
        &lines_before[..],
        "the three lines kept their nodes"
    );
    let card = object(&ws, root, a);
    assert_eq!(
        (card.text.as_str(), card.lines.clone()),
        (
            "Title",
            vec![
                "one".to_string(),
                "TWO".into(),
                "three".into(),
                "four".into()
            ]
        )
    );

    set_text(&mut ws, a, "Renamed").unwrap();
    let card = object(&ws, root, a);
    assert_eq!((card.text.as_str(), card.lines.len()), ("Renamed", 0));
    assert!(ws.children(Parent::Node(a)).unwrap().is_empty());

    set_text(&mut ws, a, "Crlf\r\nline\r\n").unwrap();
    assert_eq!(object(&ws, root, a).lines, vec!["line", ""]);
}

/// A line whose plain text did not change is not rewritten, so its pill
/// survives a read-then-write round trip.
#[test]
fn set_text_keeps_the_pills_of_unchanged_lines() {
    let (mut ws, root) = canvas();
    let a = card(&mut ws, root, 0, 0, "Head\nCall  today");
    let line = ws.children(Parent::Node(a)).unwrap()[0];
    let pill = ws
        .insert_pill(line, 5, &Attr::new("koru.due", AttrValue::Text("x".into())))
        .unwrap();
    let card = object(&ws, root, a);
    let round_trip = std::iter::once(card.text.clone())
        .chain(card.lines.clone())
        .collect::<Vec<_>>()
        .join("\n");

    set_text(&mut ws, a, &round_trip).unwrap();
    let node = ws.read(line).unwrap();
    assert!(node.content.attrs().iter().any(|attr| attr.id == pill));
    assert!(
        node.content.as_text().unwrap().contains(ANCHOR),
        "the anchor is still in the text"
    );
}

/// Anything that is not a plain line — a nested list — is replaced by a
/// paragraph, so what `set_text` wrote is exactly what `read` gives back.
#[test]
fn set_text_replaces_what_is_not_a_plain_line() {
    let (mut ws, root) = canvas();
    let a = card(&mut ws, root, 0, 0, "Head");
    let list = ws
        .insert(Parent::Node(a), Node::list_block().unwrap())
        .unwrap();
    ws.insert(Parent::Node(list), Node::paragraph("item").unwrap())
        .unwrap();
    set_text(&mut ws, a, "Head\nitem\nnext").unwrap();
    assert!(ws.read(list).is_err(), "the list went");
    assert_eq!(object(&ws, root, a).lines, vec!["item", "next"]);
}

/// A card whose first line was turned into a list is a positioned list
/// container: it has no line of its own, and its items are its lines, in both
/// directions.
#[test]
fn a_list_card_reads_and_writes_its_items() {
    let (mut ws, root) = canvas();
    let list = ws
        .insert(Parent::Node(root), Node::list_block().unwrap())
        .unwrap();
    ws.set_list_style(list, Some(crate::node::ListStyle::Bullet))
        .unwrap();
    ws.set_at(list, Some(at(0, 0))).unwrap();
    for item in ["milk", "eggs"] {
        ws.insert(Parent::Node(list), Node::paragraph(item).unwrap())
            .unwrap();
    }
    let card = object(&ws, root, list);
    assert_eq!((card.kind, card.text.as_str()), (ObjectKind::Card, ""));
    assert_eq!(card.lines, vec!["milk", "eggs"]);

    // What `read` reported, written back: the empty first line is dropped.
    set_text(&mut ws, list, "\nmilk\neggs\nbread").unwrap();
    assert_eq!(object(&ws, root, list).lines, vec!["milk", "eggs", "bread"]);
    set_text(&mut ws, list, "tea\ncoffee").unwrap();
    let card = object(&ws, root, list);
    assert_eq!(
        (card.text.as_str(), card.lines.clone()),
        ("", vec!["tea".to_string(), "coffee".into()])
    );
}

/// An arrow's text is its label.
#[test]
fn set_text_on_an_arrow_sets_its_label() {
    let (mut ws, root) = canvas();
    let a = card(&mut ws, root, 0, 0, "a");
    let b = card(&mut ws, root, 300, 0, "b");
    let arrow = add_arrow(&mut ws, root, a, b).unwrap();
    set_text(&mut ws, arrow, "then").unwrap();
    assert_eq!(snapshot(&ws, root).arrows[0].label, "then");
    assert!(set_text(&mut ws, arrow, "two\nlines").is_err());
}

// ------------------------------------------------------------------ workflow nodes

#[test]
fn an_agent_node_keeps_its_agent_run_and_result() {
    let (mut ws, root) = canvas();
    let agent = add_object(
        &mut ws,
        root,
        ObjectKind::Agent,
        at(0, 0),
        "Summarize\nthe notes",
    )
    .unwrap();

    set_agent(&mut ws, agent, "claude", Some("opus")).unwrap();
    let node = object(&ws, root, agent);
    assert_eq!(
        (node.agent.as_deref(), node.agent_model.as_deref()),
        (Some("claude"), Some("opus"))
    );
    set_agent(&mut ws, agent, "codex", None).unwrap();
    let node = object(&ws, root, agent);
    assert_eq!(
        (node.agent.as_deref(), node.agent_model),
        (Some("codex"), None)
    );
    assert_eq!(
        ws.read(agent).unwrap().attrs_with_role(ROLE_AGENT).count(),
        1,
        "rewritten, not added"
    );

    let (first, second) = (DocumentId::new(), DocumentId::new());
    set_run(&mut ws, agent, first).unwrap();
    set_run(&mut ws, agent, second).unwrap();
    assert_eq!(object(&ws, root, agent).run, Some(second));
    assert_eq!(ws.read(agent).unwrap().attrs_with_role(ROLE_RUN).count(), 1);

    set_result(&mut ws, agent, &["Three points:".into(), "one\ntwo".into()]).unwrap();
    let node = object(&ws, root, agent);
    assert_eq!(node.result, vec!["Three points:", "one", "two"]);
    assert_eq!(
        node.lines,
        vec!["the notes"],
        "the result is not an instruction line"
    );
    let holder = result_children(&ws, agent).unwrap()[0];

    set_result(&mut ws, agent, &["Two points:".into(), "one".into()]).unwrap();
    assert_eq!(object(&ws, root, agent).result, vec!["Two points:", "one"]);
    assert_eq!(
        result_children(&ws, agent).unwrap(),
        vec![holder],
        "the holder is kept"
    );

    // An instruction line added after the result exists goes above it.
    set_text(&mut ws, agent, "Summarize\nthe notes\ncarefully").unwrap();
    let node = object(&ws, root, agent);
    assert_eq!(node.lines, vec!["the notes", "carefully"]);
    assert_eq!(
        ws.children(Parent::Node(agent)).unwrap().last(),
        Some(&holder)
    );

    set_result(&mut ws, agent, &[]).unwrap();
    assert!(object(&ws, root, agent).result.is_empty());
    assert!(ws.read(holder).is_err());

    // Through storage, all of it.
    set_result(&mut ws, agent, &["back".into()]).unwrap();
    let reopened = WorkspaceDoc::from_snapshot(&ws.export_snapshot().unwrap()).unwrap();
    assert_eq!(snapshot(&reopened, root), snapshot(&ws, root));
}

/// Two peers that each wrote a first result left two holders. `read` shows
/// the first; the next `set_result` folds them into one.
#[test]
fn two_result_holders_fold_into_one() {
    let (mut ws, root) = canvas();
    let agent = add_object(&mut ws, root, ObjectKind::Agent, at(0, 0), "Do").unwrap();
    for text in ["from A", "from B"] {
        ws.insert(
            Parent::Node(agent),
            Node::new(
                Kind::Paragraph,
                Content::Text {
                    text: text.into(),
                    attrs: vec![Attr::new(ROLE_RESULT, AttrValue::Bool(true))],
                },
            )
            .unwrap(),
        )
        .unwrap();
    }
    let node = object(&ws, root, agent);
    assert_eq!(
        (node.result.clone(), node.lines.len()),
        (vec!["from A".to_string()], 0)
    );

    set_result(&mut ws, agent, &["merged".into()]).unwrap();
    assert_eq!(result_children(&ws, agent).unwrap().len(), 1);
    assert_eq!(object(&ws, root, agent).result, vec!["merged"]);
}

/// The output page reads back as a document id even though its role is not
/// one the attribute reader normalizes: the record is a document reference's.
#[test]
fn an_output_node_links_its_page() {
    let (mut ws, root) = canvas();
    let output = add_object(&mut ws, root, ObjectKind::Output, at(0, 0), "Report").unwrap();
    let page = DocumentId::new();
    set_output_page(&mut ws, output, page).unwrap();
    let reopened = WorkspaceDoc::from_snapshot(&ws.export_snapshot().unwrap()).unwrap();
    assert_eq!(object(&reopened, root, output).output_page, Some(page));
    // Rewriting it keeps one attribute.
    let later = DocumentId::new();
    set_output_page(&mut ws, output, later).unwrap();
    assert_eq!(object(&ws, root, output).output_page, Some(later));
    assert_eq!(
        ws.read(output)
            .unwrap()
            .attrs_with_role(ROLE_OUTPUT_PAGE)
            .count(),
        1
    );
}

/// A chain's run lives on its nodes: the start names the run and the device
/// that began it, each step the run it last ran in. Rewritten in place, and
/// through storage.
#[test]
fn a_chain_run_is_recorded_on_its_nodes() {
    let (mut ws, root) = canvas();
    let start = add_object(&mut ws, root, ObjectKind::Start, at(0, 0), "Go").unwrap();
    let agent = add_object(&mut ws, root, ObjectKind::Agent, at(300, 0), "Do").unwrap();
    let output = add_object(&mut ws, root, ObjectKind::Output, at(600, 0), "Page").unwrap();
    assert_eq!(execution(&ws, start).unwrap(), None);
    assert_eq!(step_execution(&ws, agent).unwrap(), None);

    set_execution(&mut ws, start, "run-1", "device-a").unwrap();
    set_execution(&mut ws, start, "run-2", "device-b").unwrap();
    set_step_execution(&mut ws, agent, "run-2").unwrap();
    set_step_execution(&mut ws, output, "run-1").unwrap();
    set_step_execution(&mut ws, output, "run-2").unwrap();

    let reopened = WorkspaceDoc::from_snapshot(&ws.export_snapshot().unwrap()).unwrap();
    assert_eq!(
        execution(&reopened, start).unwrap(),
        Some(CanvasExecution {
            id: "run-2".into(),
            device: "device-b".into()
        })
    );
    assert_eq!(
        step_execution(&reopened, agent).unwrap().as_deref(),
        Some("run-2")
    );
    assert_eq!(
        step_execution(&reopened, output).unwrap().as_deref(),
        Some("run-2")
    );
    let start_node = reopened.read(start).unwrap();
    assert_eq!(start_node.attrs_with_role(ROLE_START_EXECUTION).count(), 1);
    assert_eq!(start_node.attrs_with_role(ROLE_START_DEVICE).count(), 1);
    assert_eq!(
        reopened
            .read(output)
            .unwrap()
            .attrs_with_role(ROLE_STEP_EXECUTION)
            .count(),
        1
    );
    // An empty id ends the run: nothing is in progress, and one record stays.
    set_execution(&mut ws, start, "", "device-b").unwrap();
    assert_eq!(execution(&ws, start).unwrap(), None);
    assert_eq!(
        ws.read(start)
            .unwrap()
            .attrs_with_role(ROLE_START_EXECUTION)
            .count(),
        1
    );

    // None of it is a line or a result of the node it marks.
    assert_eq!(snapshot(&reopened, root).get(start).unwrap().text, "Go");
    assert!(
        snapshot(&reopened, root)
            .get(agent)
            .unwrap()
            .lines
            .is_empty()
    );

    // Each refuses the wrong node, before writing anything.
    let card = card(&mut ws, root, 0, 300, "a card");
    ws.enable_undo();
    assert_eq!(
        refusal(set_execution(&mut ws, agent, "x", "y")),
        CanvasRefusal::NotAStart
    );
    assert_eq!(
        refusal(set_step_execution(&mut ws, start, "x")),
        CanvasRefusal::NotAStep
    );
    assert_eq!(
        refusal(set_step_execution(&mut ws, card, "x")),
        CanvasRefusal::NotAStep
    );
    assert!(!ws.can_undo(), "no refusal wrote anything");
    assert_eq!(CanvasRefusal::NotAStep.as_str(), "not_a_step");
}

// ------------------------------------------------------------------ stacking

#[test]
fn bring_to_front_and_send_to_back_reorder_siblings() {
    let (mut ws, root) = canvas();
    let a = card(&mut ws, root, 0, 0, "a");
    let b = card(&mut ws, root, 0, 0, "b");
    let c = card(&mut ws, root, 0, 0, "c");
    bring_to_front(&mut ws, a).unwrap();
    assert_eq!(ids(&snapshot(&ws, root).objects), vec![b, c, a]);
    send_to_back(&mut ws, c).unwrap();
    assert_eq!(ids(&snapshot(&ws, root).objects), vec![c, b, a]);
    bring_to_front(&mut ws, b).unwrap();
    assert_eq!(ids(&snapshot(&ws, root).objects), vec![c, a, b]);
}

// ------------------------------------------------------------------ reading order

fn placed(points: &[(i32, i32)]) -> Vec<CanvasObject> {
    points
        .iter()
        .map(|&(x, y)| CanvasObject {
            id: NodeId::new(),
            kind: ObjectKind::Card,
            at: Some(at(x, y)),
            extent: None,
            text: String::new(),
            lines: Vec::new(),
            image: None,
            agent: None,
            agent_model: None,
            run: None,
            result: Vec::new(),
            output_page: None,
        })
        .collect()
}

fn order_of(objects: &[CanvasObject], rtl: bool) -> Vec<usize> {
    reading_order(objects, rtl)
        .into_iter()
        .map(|id| objects.iter().position(|o| o.id == id).unwrap())
        .collect()
}

/// Two cards side by side, one a pixel lower, still read left then right.
#[test]
fn a_one_pixel_offset_is_still_one_row() {
    let objects = placed(&[(300, 0), (0, 1)]);
    assert_eq!(order_of(&objects, false), vec![1, 0]);
    assert_eq!(order_of(&objects, true), vec![0, 1]);
}

/// Two columns read row by row, interleaved, as a page's table would.
#[test]
fn two_columns_read_row_by_row() {
    let objects = placed(&[
        (0, 0),
        (0, 200),
        (0, 400),
        (400, 10),
        (400, 205),
        (400, 390),
    ]);
    assert_eq!(order_of(&objects, false), vec![0, 3, 1, 4, 2, 5]);
    assert_eq!(order_of(&objects, true), vec![3, 0, 4, 1, 5, 2]);
}

/// The band is measured from the row's first object, not from the last one
/// added, so a staircase does not become one endless row.
#[test]
fn the_row_band_is_measured_from_the_rows_first_object() {
    let objects = placed(&[(0, 0), (100, 30), (200, 40), (300, 41), (400, 70)]);
    assert_eq!(order_of(&objects, false), vec![0, 1, 2, 3, 4]);
    let rows = placed(&[(500, 0), (400, 30), (300, 60), (200, 90)]);
    // Rows: {0, 30} then {60, 90}; each reads left to right.
    assert_eq!(order_of(&rows, false), vec![1, 0, 3, 2]);
}

#[test]
fn objects_without_a_position_have_no_reading_place() {
    let mut objects = placed(&[(0, 0), (10, 10)]);
    objects[0].at = None;
    assert_eq!(order_of(&objects, false), vec![1]);
    // Identical positions fall back to stacking order.
    let same = placed(&[(5, 5), (5, 5), (5, 5)]);
    assert_eq!(order_of(&same, false), vec![0, 1, 2]);
    assert_eq!(order_of(&same, true), vec![0, 1, 2]);
}

#[test]
fn reading_order_is_safe_at_the_extremes() {
    let objects = placed(&[(i32::MAX, i32::MAX), (i32::MIN, i32::MIN), (0, 0)]);
    assert_eq!(order_of(&objects, false), vec![1, 2, 0]);
}

// ------------------------------------------------------------------ direction

fn with_text(texts: &[&str]) -> CanvasSnapshot {
    let mut objects = placed(&vec![(0, 0); texts.len()]);
    for (object, text) in objects.iter_mut().zip(texts) {
        let (first, rest) = split_lines(text);
        object.text = first.to_owned();
        object.lines = rest.into_iter().map(str::to_owned).collect();
    }
    CanvasSnapshot {
        root: NodeId::new(),
        objects,
        arrivals: Vec::new(),
        arrows: Vec::new(),
    }
}

#[test]
fn dominant_direction_counts_strong_letters() {
    assert!(dominant_direction(&with_text(&[
        "مرحبا بالعالم",
        "خطة العمل\nالخطوة الأولى"
    ])));
    assert!(!dominant_direction(&with_text(&[
        "Hello world",
        "Plan\nfirst step"
    ])));
    assert!(
        dominant_direction(&with_text(&["שלום עולם", "OK"])),
        "Hebrew outweighs two letters"
    );
    assert!(
        !dominant_direction(&with_text(&["Quarterly plan", "خطة"])),
        "English outweighs one word"
    );
    assert!(
        !dominant_direction(&with_text(&["١٢٣٤٥٦٧٨٩ 2026", "a"])),
        "digits are not letters, in either script"
    );
    assert!(
        !dominant_direction(&with_text(&[])),
        "no text reads left to right"
    );
    assert!(
        !dominant_direction(&with_text(&["ab", "אב"])),
        "a tie reads left to right"
    );
    assert!(
        !dominant_direction(&with_text(&["日本語のメモ", "خطة"])),
        "CJK is left to right"
    );
}

/// The direction is read from a real canvas's arrivals too.
#[test]
fn dominant_direction_reads_arrivals() {
    let (mut ws, root) = canvas();
    card(&mut ws, root, 0, 0, "ok");
    ws.insert(
        Parent::Node(root),
        Node::paragraph("مرحبا بالعالم").unwrap(),
    )
    .unwrap();
    assert!(dominant_direction(&snapshot(&ws, root)));
}

// ------------------------------------------------------------------ the tree

/// **Z-order is sibling order, and it rests on Loro's fractional index.**
/// `mov_to` refuses to run without it (`FiIfNotConfigured::Throw`), and the
/// repo relies on Loro 1.13.9 enabling it by default
/// (loro-internal `tree_state.rs:665-667`) while Loro's own doc comment says
/// the opposite — so an upgrade that flips the default fails here rather than
/// in every `move_node`.
#[test]
fn the_node_tree_has_fractional_indexing_enabled() {
    let (mut ws, root) = canvas();
    assert!(ws.doc().get_tree("nodes").is_fractional_index_enabled());
    let a = card(&mut ws, root, 0, 0, "a");
    card(&mut ws, root, 0, 0, "b");
    let reopened = WorkspaceDoc::from_snapshot(&ws.export_snapshot().unwrap()).unwrap();
    assert!(
        reopened
            .doc()
            .get_tree("nodes")
            .is_fractional_index_enabled()
    );
    let mut reopened = reopened;
    bring_to_front(&mut reopened, a).unwrap();
    assert_eq!(
        snapshot(&reopened, root).objects.last().map(|o| o.id),
        Some(a)
    );
}
