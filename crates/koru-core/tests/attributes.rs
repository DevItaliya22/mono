// SPDX-License-Identifier: MPL-2.0

//! Attributes under concurrency — the Foundation §15 checklist, pinned.
//!
//! Two replicas, edited independently, must converge on their attributes the
//! way they do on text and structure ([ADR-176], Foundation invariant 20).

use koru_core::attributes::{KEY_COLOR, KEY_DUE, KEY_PRIORITY};
use koru_core::node::{AttrValue, Node};
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_core::time::{DateOnly, Temporal};
use koru_core::{NodeId, Priority, TagId, WorkspaceId};

fn forked_pair() -> (WorkspaceDoc, WorkspaceDoc, NodeId) {
    let mut a = WorkspaceDoc::create(WorkspaceId::new(), "acme").unwrap();
    let node = a
        .insert(Parent::Root, Node::task("Launch referral").unwrap())
        .unwrap();
    let snapshot = a.export_snapshot().unwrap();
    let b = WorkspaceDoc::from_snapshot(&snapshot).unwrap();
    (a, b, node)
}

fn sync(a: &mut WorkspaceDoc, b: &mut WorkspaceDoc) {
    let from_a = a.export_all_updates().unwrap();
    let from_b = b.export_all_updates().unwrap();
    a.import(&from_b).unwrap();
    b.import(&from_a).unwrap();
}

fn day(s: &str) -> DateOnly {
    DateOnly::parse(s).unwrap()
}

fn date(s: &str) -> AttrValue {
    AttrValue::Date(day(s))
}

fn on(s: &str) -> Temporal {
    Temporal::Date(day(s))
}

/// Two peers set the same attribute concurrently: both replicas end with the
/// same answer, and it is one of the two values written — never a third.
#[test]
fn concurrent_sets_of_one_attribute_converge() {
    let (mut a, mut b, node) = forked_pair();

    a.set_attribute(node, KEY_DUE, date("2026-08-30")).unwrap();
    b.set_attribute(node, KEY_DUE, date("2026-09-15")).unwrap();

    sync(&mut a, &mut b);

    let from_a = a.read(node).unwrap();
    let from_b = b.read(node).unwrap();
    assert_eq!(from_a, from_b, "replicas converged");
    let due = from_a.due().expect("a due survives");
    assert!(
        due == on("2026-08-30") || due == on("2026-09-15"),
        "one of the two writes won: {due:?}"
    );
}

/// Set against clear, concurrently — the delete-vs-update conflict of the
/// Foundation checklist. Whichever way it lands, both replicas land the same
/// way.
#[test]
fn a_concurrent_set_and_clear_converge() {
    let (mut a, mut b, node) = forked_pair();
    a.set_attribute(node, KEY_PRIORITY, AttrValue::Text("high".into()))
        .unwrap();
    sync(&mut a, &mut b);

    a.clear_attribute(node, KEY_PRIORITY).unwrap();
    b.set_attribute(node, KEY_PRIORITY, AttrValue::Text("urgent".into()))
        .unwrap();

    sync(&mut a, &mut b);

    assert_eq!(a.read(node).unwrap(), b.read(node).unwrap());
    let p = a.read(node).unwrap().priority();
    assert!(
        p.is_none() || p == Some(Priority::Urgent),
        "cleared, or the concurrent set — never the old value: {p:?}"
    );
}

/// Concurrent tag adds merge to the **union**: one attr per tag is what makes
/// a multi-value key merge-safe.
#[test]
fn concurrent_tag_adds_merge_to_the_union() {
    let (mut a, mut b, node) = forked_pair();
    let mvp = TagId::new();
    let research = TagId::new();

    a.add_tag(node, mvp).unwrap();
    b.add_tag(node, research).unwrap();

    sync(&mut a, &mut b);

    let mut from_a: Vec<TagId> = a.read(node).unwrap().tag_ids().collect();
    let mut from_b: Vec<TagId> = b.read(node).unwrap().tag_ids().collect();
    from_a.sort_unstable();
    from_b.sort_unstable();
    assert_eq!(from_a, from_b, "replicas converged");
    let mut expected = vec![mvp, research];
    expected.sort_unstable();
    assert_eq!(from_a, expected, "both tags survive");
}

/// Offline reconciliation: a peer that missed several attribute edits catches
/// up in one import and agrees.
#[test]
fn a_peer_reconciling_after_a_spell_offline_agrees() {
    let (mut a, mut b, node) = forked_pair();

    a.set_attribute(node, KEY_DUE, date("2026-08-30")).unwrap();
    a.set_attribute(node, KEY_COLOR, AttrValue::Text("teal".into()))
        .unwrap();
    a.add_tag(node, TagId::new()).unwrap();
    a.set_attribute(node, KEY_DUE, date("2026-09-15")).unwrap();

    b.import(&a.export_all_updates().unwrap()).unwrap();

    assert_eq!(a.read(node).unwrap(), b.read(node).unwrap());
    assert_eq!(b.read(node).unwrap().due(), Some(on("2026-09-15")));
}

/// Undo takes back exactly one attribute step, and what a remote peer wrote
/// stays untouched by it.
#[test]
fn undo_takes_back_exactly_one_step() {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "acme").unwrap();
    let node = ws
        .insert(Parent::Root, Node::task("Launch referral").unwrap())
        .unwrap();
    ws.enable_undo();

    ws.set_attribute(node, KEY_DUE, date("2026-08-30")).unwrap();
    let tag = TagId::new();
    ws.add_tag(node, tag).unwrap();

    assert!(ws.undo().unwrap());
    let read = ws.read(node).unwrap();
    assert_eq!(read.tag_ids().count(), 0, "the tag went");
    assert_eq!(read.due(), Some(on("2026-08-30")), "the due stayed");

    assert!(ws.undo().unwrap());
    assert_eq!(ws.read(node).unwrap().due(), None);
}

/// Deleting the node while metadata exists: the attributes go with their node
/// rather than leaking, and the other peer's concurrent attribute edit does
/// not resurrect it.
#[test]
fn deleting_a_node_takes_its_attributes_with_it() {
    let (mut a, mut b, node) = forked_pair();

    a.delete(node).unwrap();
    b.set_attribute(node, KEY_DUE, date("2026-08-30")).unwrap();

    sync(&mut a, &mut b);

    assert!(a.read(node).is_err(), "the delete won on a");
    assert!(b.read(node).is_err(), "and on b");
}

/// `duplicate` copies tags, color, due and priority: they are facts about the
/// content, and a copy of the content carries them (roadmap hazard note —
/// what must *clear* on copy is R1's TrackId, not these).
#[test]
fn a_duplicate_copies_the_shared_attributes() {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "acme").unwrap();
    let node = ws
        .insert(Parent::Root, Node::task("Launch referral").unwrap())
        .unwrap();
    let tag = TagId::new();
    ws.add_tag(node, tag).unwrap();
    ws.set_attribute(node, KEY_COLOR, AttrValue::Text("teal".into()))
        .unwrap();
    ws.set_attribute(node, KEY_DUE, date("2026-08-30")).unwrap();
    ws.set_attribute(node, KEY_PRIORITY, AttrValue::Text("high".into()))
        .unwrap();

    let copy = ws.duplicate(node, Parent::Root, "").unwrap();
    assert_ne!(copy, node, "a copy is a new identity");

    let read = ws.read(copy).unwrap();
    assert_eq!(read.tag_ids().collect::<Vec<_>>(), vec![tag]);
    assert_eq!(read.color_token(), Some("teal"));
    assert_eq!(read.due(), Some(on("2026-08-30")));
    assert_eq!(read.priority(), Some(Priority::High));

    let original = ws.read(node).unwrap();
    assert_eq!(original.tag_ids().collect::<Vec<_>>(), vec![tag]);
    assert_eq!(original.priority(), Some(Priority::High));
}

/// `plant_import` preserves everything — identities included — because a
/// migration re-homing content must not turn references dangling.
#[test]
fn an_import_planting_preserves_attributes_and_identity() {
    let mut source = WorkspaceDoc::create(WorkspaceId::new(), "old").unwrap();
    let node = source
        .insert(Parent::Root, Node::task("Launch referral").unwrap())
        .unwrap();
    let tag = TagId::new();
    source.add_tag(node, tag).unwrap();
    source
        .set_attribute(node, KEY_DUE, date("2026-08-30"))
        .unwrap();

    let cutting = source.cutting_for_import(node).unwrap();
    let mut target = WorkspaceDoc::create(WorkspaceId::new(), "new").unwrap();
    let planted = target.plant_import(&cutting, Parent::Root).unwrap();

    assert_eq!(planted, node, "the import keeps the original NodeId");
    let read = target.read(planted).unwrap();
    assert_eq!(read.tag_ids().collect::<Vec<_>>(), vec![tag]);
    assert_eq!(read.due(), Some(on("2026-08-30")));
}

/// Moving a node across parents keeps its attributes — they live on the node,
/// not on its position (Foundation invariant 1's point about stable refs).
#[test]
fn a_move_keeps_the_attributes() {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "acme").unwrap();
    let group = ws
        .insert(Parent::Root, Node::task("Phase 1").unwrap())
        .unwrap();
    let node = ws
        .insert(Parent::Root, Node::task("Launch referral").unwrap())
        .unwrap();
    ws.set_attribute(node, KEY_DUE, date("2026-08-30")).unwrap();

    ws.move_node(node, Parent::Node(group), 0).unwrap();

    assert_eq!(ws.read(node).unwrap().due(), Some(on("2026-08-30")));
}

/// **A zoned deadline converges as one whole value, instant and zone
/// together** ([ADR-183]).
///
/// This is the CRDT half of "never infer meaning from a missing zone": the
/// instant and the zone are two keys in one attribute record, so two peers
/// writing different zoned deadlines concurrently could in principle land one
/// peer's instant beside the other peer's zone — a deadline nobody set, in a
/// place nobody meant. `set_attribute` writes the record inside one
/// `atomic()`, and this pins that the survivor is one of the two values
/// written and never a splice of both.
///
/// [ADR-183]: ../../docs/decisions/ADR-201-225.md#adr-206-timemodel--koru-has-three-temporal-types-not-one-with-a-nullable-timezone
#[test]
fn a_concurrent_pair_of_zoned_deadlines_converges_without_splicing() {
    use koru_core::time::{TimeZoneName, ZonedDateTime};

    let zurich = ZonedDateTime::new(
        1_790_000_000_000,
        TimeZoneName::parse("Europe/Zurich").unwrap(),
    );
    let tokyo = ZonedDateTime::new(
        1_795_000_000_000,
        TimeZoneName::parse("Asia/Tokyo").unwrap(),
    );

    let (mut a, mut b, node) = forked_pair();
    a.set_attribute(node, KEY_DUE, AttrValue::ZonedDateTime(zurich.clone()))
        .unwrap();
    b.set_attribute(node, KEY_DUE, AttrValue::ZonedDateTime(tokyo.clone()))
        .unwrap();
    sync(&mut a, &mut b);

    let from_a = a.read(node).unwrap();
    assert_eq!(from_a, b.read(node).unwrap(), "replicas converged");
    let Some(Temporal::Zoned(due)) = from_a.due() else {
        panic!("a zoned due survives as a zoned due: {:?}", from_a.due());
    };
    assert!(
        due == zurich || due == tokyo,
        "one of the two writes won whole — never Zurich's instant in Tokyo"
    );
}

/// The three types survive a round trip through two replicas **as
/// themselves**: a floating plan does not come back zoned, and a zoned due
/// does not come back floating. Convergence is where a type token silently
/// widening would first show up.
#[test]
fn the_three_temporal_types_converge_as_themselves() {
    use koru_core::attributes::{AttributeDefinition, ValueType};
    use koru_core::time::{FloatingDateTime, TimeZoneName, ZonedDateTime};

    // A custom personal-shaped key, so a floating value has somewhere legal to
    // live on a node: `koru.plan` itself is personal storage's.
    let reminder =
        AttributeDefinition::custom("user.remind", "Remind", ValueType::FloatingDateTime);

    let (mut a, mut b, node) = forked_pair();
    a.set_attribute(node, KEY_DUE, date("2026-08-30")).unwrap();
    a.set_attribute(
        node,
        KEY_DUE,
        AttrValue::ZonedDateTime(ZonedDateTime::new(
            1_790_000_000_000,
            TimeZoneName::parse("Europe/Zurich").unwrap(),
        )),
    )
    .unwrap();
    a.set_attribute_defined(
        node,
        &reminder,
        AttrValue::FloatingDateTime(FloatingDateTime::parse("2026-08-30T08:00:00").unwrap()),
    )
    .unwrap();
    sync(&mut a, &mut b);

    let read = b.read(node).unwrap();
    assert!(
        matches!(read.due(), Some(Temporal::Zoned(_))),
        "the shared deadline is still zoned"
    );
    assert!(
        matches!(
            read.attr("user.remind").map(|a| &a.value),
            Some(AttrValue::FloatingDateTime(_))
        ),
        "the personal-shaped wall clock is still floating"
    );
}
