// SPDX-License-Identifier: MPL-2.0

//! The read API: "every task assigned to Kim due before August" answered
//! without parsing a single line of text.

use chrono::NaiveDate;
use koru_core::node::{Attr, AttrValue, Content, Kind, Node, NodeMeta};
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_core::{DateRange, NodeId, Ref, TaskQuery, WorkspaceId};

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

/// The same day as the **calendar-date type** an attribute stores. Two
/// helpers rather than one conversion, because a `DateRange` bound and a
/// stored value are different things and the types say so.
fn day(y: i32, m: u32, dd: u32) -> koru_core::time::DateOnly {
    koru_core::time::DateOnly::new(d(y, m, dd))
}

struct Fixture {
    ws: WorkspaceDoc,
    kim: NodeId,
    ben: NodeId,
    phase1: NodeId,
    reward: NodeId,
    invite: NodeId,
    legal: NodeId,
}

/// ```text
/// Phase 1                (level "Phase")
///   Reward payout flow   due Aug 2, kim
///   Invite email         due Sep 9, ben, done
/// Legal review           due Aug 20, kim
/// ```
fn fixture() -> Fixture {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "acme").unwrap();

    let kim = ws
        .insert(
            Parent::Root,
            Node::new(Kind::Contact, Content::text("Kim")).unwrap(),
        )
        .unwrap();
    let ben = ws
        .insert(
            Parent::Root,
            Node::new(Kind::Contact, Content::text("Ben")).unwrap(),
        )
        .unwrap();

    let phase1 = ws
        .insert(
            Parent::Root,
            Node::task("Phase 1").unwrap().with_meta(NodeMeta {
                level_name: Some("Phase".into()),
                ..Default::default()
            }),
        )
        .unwrap();

    let reward = ws
        .insert(
            Parent::Node(phase1),
            Node::task("Reward payout flow").unwrap(),
        )
        .unwrap();
    ws.set_attr(reward, &Attr::new("due", AttrValue::Date(day(2026, 8, 2))))
        .unwrap();
    ws.set_attr(
        reward,
        &Attr::new("assignee", AttrValue::Ref(Ref::new(kim))),
    )
    .unwrap();

    let invite = ws
        .insert(Parent::Node(phase1), Node::task("Invite email").unwrap())
        .unwrap();
    ws.set_attr(invite, &Attr::new("due", AttrValue::Date(day(2026, 9, 9))))
        .unwrap();
    ws.set_attr(
        invite,
        &Attr::new("assignee", AttrValue::Ref(Ref::new(ben))),
    )
    .unwrap();
    ws.set_done(invite, true).unwrap();

    let legal = ws
        .insert(Parent::Root, Node::task("Legal review").unwrap())
        .unwrap();
    ws.set_attr(legal, &Attr::new("due", AttrValue::Date(day(2026, 8, 20))))
        .unwrap();
    ws.set_attr(legal, &Attr::new("assignee", AttrValue::Ref(Ref::new(kim))))
        .unwrap();

    Fixture {
        ws,
        kim,
        ben,
        phase1,
        reward,
        invite,
        legal,
    }
}

fn ids(rows: &[koru_core::TaskSummary]) -> Vec<NodeId> {
    rows.iter().map(|r| r.id).collect()
}

#[test]
fn attributes_survive_a_round_trip_through_the_crdt() {
    let f = fixture();
    let node = f.ws.read(f.reward).unwrap();
    assert_eq!(
        node.due(),
        Some(koru_core::time::Temporal::Date(day(2026, 8, 2)))
    );
    assert_eq!(
        node.assignees().map(|r| r.node).collect::<Vec<_>>(),
        vec![f.kim]
    );
}

/// The query from the brief: `GET /tasks?assignee=kim&due_lt=2026-09-01`.
#[test]
fn assignee_and_due_combine() {
    let f = fixture();
    let rows =
        f.ws.query(
            &TaskQuery::new()
                .assignee(f.kim)
                .due(DateRange::before(d(2026, 9, 1))),
        )
        .unwrap();
    assert_eq!(ids(&rows), vec![f.reward, f.legal]);
}

#[test]
fn due_before_excludes_the_boundary() {
    let f = fixture();
    let rows =
        f.ws.query(&TaskQuery::new().due(DateRange::before(d(2026, 8, 2))))
            .unwrap();
    assert!(ids(&rows).is_empty(), "Aug 2 is not before Aug 2");

    let rows =
        f.ws.query(&TaskQuery::new().due(DateRange::before(d(2026, 8, 3))))
            .unwrap();
    assert_eq!(ids(&rows), vec![f.reward]);
}

#[test]
fn under_restricts_to_a_subtree() {
    let f = fixture();
    let rows = f.ws.query(&TaskQuery::new().under(f.phase1)).unwrap();
    assert_eq!(
        ids(&rows),
        vec![f.reward, f.invite],
        "the group itself is excluded"
    );

    // Without it, the whole workspace is in scope.
    let all = f.ws.query(&TaskQuery::new()).unwrap();
    assert_eq!(all.len(), 4, "3 tasks + the group");
}

#[test]
fn level_finds_user_named_groupings() {
    let f = fixture();
    let rows = f.ws.query(&TaskQuery::new().level("Phase")).unwrap();
    assert_eq!(ids(&rows), vec![f.phase1]);
    assert!(
        f.ws.query(&TaskQuery::new().level("Project"))
            .unwrap()
            .is_empty(),
        "levels are the user's own words, not a fixed taxonomy"
    );
}

#[test]
fn leaf_separates_groups_from_tasks() {
    let f = fixture();
    let leaves = f.ws.query(&TaskQuery::new().leaf(true)).unwrap();
    assert_eq!(ids(&leaves), vec![f.reward, f.invite, f.legal]);

    let groups = f.ws.query(&TaskQuery::new().leaf(false)).unwrap();
    assert_eq!(ids(&groups), vec![f.phase1]);
}

#[test]
fn done_filters_both_ways() {
    let f = fixture();
    assert_eq!(
        ids(&f.ws.query(&TaskQuery::new().done(true)).unwrap()),
        vec![f.invite]
    );
    assert_eq!(
        ids(&f.ws.query(&TaskQuery::new().done(false)).unwrap()),
        vec![f.phase1, f.reward, f.legal]
    );
}

#[test]
fn text_search_is_case_insensitive() {
    let f = fixture();
    let rows = f.ws.query(&TaskQuery::new().text("PAYOUT")).unwrap();
    assert_eq!(ids(&rows), vec![f.reward]);
}

/// Contacts are nodes too, but they are not tasks and must never appear in a
/// task list.
#[test]
fn non_task_kinds_are_never_returned() {
    let f = fixture();
    let all = f.ws.query(&TaskQuery::new()).unwrap();
    assert!(!ids(&all).contains(&f.kim));
    assert!(!ids(&all).contains(&f.ben));
}

/// A kanban card needs the person's *name*, not an id — one call, no N+1.
#[test]
fn summaries_resolve_assignees_to_names() {
    let f = fixture();
    let rows = f.ws.query(&TaskQuery::new().assignee(f.kim)).unwrap();
    let card = &rows[0];
    assert_eq!(card.assignees.len(), 1);
    assert_eq!(card.assignees[0].label(), "Kim");
    assert!(!card.assignees[0].is_dangling());
}

#[test]
fn a_reference_to_a_deleted_contact_dangles_rather_than_erroring() {
    let mut f = fixture();
    f.ws.delete(f.kim).unwrap();

    let rows = f.ws.query(&TaskQuery::new().text("Reward")).unwrap();
    let card = &rows[0];
    assert!(card.assignees[0].is_dangling(), "target is gone");
    assert_eq!(card.assignees[0].label(), "unavailable", "still renders");
    assert_eq!(
        card.assignees[0].id(),
        f.kim,
        "and still knows what it pointed at"
    );
}

#[test]
fn groups_report_progress_over_their_children() {
    let f = fixture();
    let rows = f.ws.query(&TaskQuery::new().level("Phase")).unwrap();
    let group = &rows[0];
    assert_eq!(group.progress, (1, 2), "one of two children ticked");
    assert!(group.is_group());
    assert!(!group.children_complete());

    let leaf = &f.ws.query(&TaskQuery::new().text("Legal")).unwrap()[0];
    assert_eq!(leaf.progress, (0, 0));
    assert!(!leaf.is_group());
}

#[test]
fn summaries_carry_depth_for_indented_rendering() {
    let f = fixture();
    let all = f.ws.query(&TaskQuery::new()).unwrap();
    let depth_of = |id: NodeId| all.iter().find(|r| r.id == id).unwrap().depth;
    assert_eq!(depth_of(f.phase1), 0);
    assert_eq!(depth_of(f.reward), 1);
    assert_eq!(depth_of(f.legal), 0);
}

/// Two dates on one task are told apart by role, not by guessing.
#[test]
fn a_start_date_is_not_a_due_date() {
    let mut f = fixture();
    f.ws.set_attr(
        f.legal,
        &Attr::new("start", AttrValue::Date(day(2026, 1, 1))),
    )
    .unwrap();

    let rows =
        f.ws.query(&TaskQuery::new().due(DateRange::before(d(2026, 2, 1))))
            .unwrap();
    assert!(
        !ids(&rows).contains(&f.legal),
        "the January start date must not match a due-date filter"
    );
    assert_eq!(
        f.ws.read(f.legal).unwrap().due(),
        Some(koru_core::time::Temporal::Date(day(2026, 8, 20)))
    );
}

#[test]
fn removing_an_attribute_takes_it_out_of_results() {
    let mut f = fixture();
    let attr = f.ws.read(f.reward).unwrap().attr("assignee").unwrap().id;
    f.ws.remove_attr(f.reward, attr).unwrap();

    let rows = f.ws.query(&TaskQuery::new().assignee(f.kim)).unwrap();
    assert_eq!(
        ids(&rows),
        vec![f.legal],
        "reward no longer assigned to kim"
    );
}

/// Attributes must merge like everything else.
#[test]
fn attributes_converge_across_replicas() {
    let f = fixture();
    let mut a = f.ws;
    let mut b = WorkspaceDoc::from_snapshot(&a.export_snapshot().unwrap()).unwrap();

    // A sets a due date on the group; B assigns it to Ben. Different
    // attributes on the same node, concurrently.
    a.set_attr(
        f.phase1,
        &Attr::new("due", AttrValue::Date(day(2026, 12, 1))),
    )
    .unwrap();
    b.set_attr(
        f.phase1,
        &Attr::new("assignee", AttrValue::Ref(Ref::new(f.ben))),
    )
    .unwrap();

    let from_a = a.export_all_updates().unwrap();
    let from_b = b.export_all_updates().unwrap();
    a.import(&from_b).unwrap();
    b.import(&from_a).unwrap();

    let merged = a.read(f.phase1).unwrap();
    assert_eq!(
        merged.due(),
        Some(koru_core::time::Temporal::Date(day(2026, 12, 1))),
        "A's date survived"
    );
    assert_eq!(
        merged.assignees().map(|r| r.node).collect::<Vec<_>>(),
        vec![f.ben],
        "B's assignee survived"
    );
    assert_eq!(
        b.read(f.phase1).unwrap().due(),
        merged.due(),
        "replicas agree"
    );
}

// --------------------------------------------------------------- backlinks

/// "Every task related to this document" — the question a knowledge view asks.
/// Derived by walking rather than read from a stored list, so it cannot go
/// stale; see `docs/deferred/`.
#[test]
fn backlinks_find_every_node_referencing_a_target() {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "acme").unwrap();
    let spec = ws
        .insert(Parent::Root, Node::document("spec.md").unwrap())
        .unwrap();
    let other = ws
        .insert(Parent::Root, Node::document("notes.md").unwrap())
        .unwrap();

    let a = ws
        .insert(Parent::Root, Node::task("Write it").unwrap())
        .unwrap();
    let b = ws
        .insert(Parent::Root, Node::task("Review it").unwrap())
        .unwrap();
    let unrelated = ws
        .insert(Parent::Root, Node::task("Unrelated").unwrap())
        .unwrap();

    ws.set_attr(a, &Attr::new("document", AttrValue::Ref(Ref::new(spec))))
        .unwrap();
    // A different role still counts: "related to" is not "assigned to".
    ws.set_attr(b, &Attr::new("attachment", AttrValue::Ref(Ref::new(spec))))
        .unwrap();
    ws.set_attr(
        unrelated,
        &Attr::new("document", AttrValue::Ref(Ref::new(other))),
    )
    .unwrap();

    let mut found = ws.backlinks(spec).unwrap();
    found.sort();
    let mut want = vec![a, b];
    want.sort();
    assert_eq!(found, want);

    assert!(
        ws.backlinks(unrelated).unwrap().is_empty(),
        "nothing points at it"
    );
}

/// The same question through the query surface, so a view gets summaries it can
/// draw rather than bare ids.
#[test]
fn links_to_returns_summaries_a_view_can_render() {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "acme").unwrap();
    let spec = ws
        .insert(Parent::Root, Node::document("spec.md").unwrap())
        .unwrap();
    let task = ws
        .insert(Parent::Root, Node::task("Write the spec").unwrap())
        .unwrap();
    ws.set_attr(task, &Attr::new("document", AttrValue::Ref(Ref::new(spec))))
        .unwrap();

    let hits = ws.query(&TaskQuery::new().links_to(spec)).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].text, "Write the spec");
    assert_eq!(hits[0].id, task);
}

/// Removing the reference removes the backlink — the property a stored list
/// would have to be maintained to keep, and that walking gets for free.
#[test]
fn a_backlink_disappears_with_the_reference() {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "acme").unwrap();
    let spec = ws
        .insert(Parent::Root, Node::document("spec.md").unwrap())
        .unwrap();
    let task = ws
        .insert(Parent::Root, Node::task("Write it").unwrap())
        .unwrap();
    let attr = Attr::new("document", AttrValue::Ref(Ref::new(spec)));
    let attr_id = ws.set_attr(task, &attr).unwrap();
    assert_eq!(ws.backlinks(spec).unwrap(), vec![task]);

    ws.remove_attr(task, attr_id).unwrap();
    assert!(ws.backlinks(spec).unwrap().is_empty());
}
