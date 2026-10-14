// SPDX-License-Identifier: MPL-2.0

//! A `task:` reference from one end of the system to the other.
//!
//! The single-function behaviour is beside the module, in `src/tasks/tests.rs`.
//! What is pinned here is the part **no single crate can see**: the storage
//! string is the whole contract between `koru_core::EntityRef`, the store's
//! `koru_store_core::personal_attributes::EntityRef` and
//! `koru_events::Subject::task`, and none of those three depends on the others
//! ([ADR-285]). So each side pins its half against the *same literal spelling*,
//! and this file is the domain's half: it re-implements the store's parser
//! grammar rather than calling it, because calling it would be the dependency
//! edge the decision exists to avoid.
//!
//! [ADR-285]: ../../../docs/decisions/ADR-276-300.md#adr-285-taskmodel--a-task-is-a-marked-node-and-the-marker-or-a-due-date-is-the-whole-derivation

use koru_core::attributes::TaskReason;
use koru_core::node::Node;
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_core::tasks::refs::{TaskRefResolution, resolve_task_ref};
use koru_core::tasks::{ensure_task, remove_from_tasks};
use koru_core::{DocumentId, EntityRef, NodeId, WorkspaceId};

fn doc() -> WorkspaceDoc {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "tasks").unwrap();
    ws.enable_undo();
    ws
}

fn row(ws: &mut WorkspaceDoc, parent: Parent, text: &str) -> NodeId {
    ws.insert(parent, Node::paragraph(text).unwrap()).unwrap()
}

/// The store's address grammar, restated: a bounded lowercase ASCII kind token
/// and three UUIDs, re-emitted in the one canonical spelling both sides write.
/// `None` is exactly where `koru_store_core::personal_attributes::EntityRef`
/// answers `None`, and the canonicalisation is its `to_storage`.
fn store_side_round_trip(spelled: &str) -> Option<String> {
    let mut parts = spelled.split(':');
    let kind = parts.next()?;
    let workspace = WorkspaceId::parse(parts.next()?)?;
    let document = DocumentId::parse(parts.next()?)?;
    let node = NodeId::parse(parts.next()?)?;
    let token = !kind.is_empty()
        && kind.len() <= 32
        && kind
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if parts.next().is_some() || !token {
        return None;
    }
    Some(format!(
        "{kind}:{}:{}:{}",
        workspace.to_storage(),
        document.to_storage(),
        node.to_storage()
    ))
}

/// The whole journey, in the order a task reference actually makes it: minted
/// by a command, written to storage, read by the store as a personal-attribute
/// key, spelled as an event subject id, and resolved back to the node.
///
/// Every hop is the same 4-segment string, byte for byte. That is the point —
/// the three crates share no type, so if the spelling drifts anywhere the
/// reference silently stops naming the thing it named.
#[test]
fn a_task_reference_survives_domain_store_event_and_back() {
    let mut ws = doc();
    let page = ws
        .insert(Parent::Root, Node::document("Plans").unwrap())
        .unwrap();
    let node = row(&mut ws, Parent::Node(page), "Launch referral");
    let reference = ensure_task(&mut ws, node, TaskReason::Explicit)
        .unwrap()
        .task;

    // 1. Domain → storage. The kind is the first segment and is never dropped,
    //    and the middle segment is the **document**, not the row: it is the id
    //    the manifest lists this page under, so every task on the page carries
    //    the same one and a reader may group by it.
    let spelled = reference.to_storage();
    assert_eq!(reference.kind(), "task");
    assert_eq!(spelled.split(':').count(), 4);
    assert!(spelled.starts_with("task:"));
    assert_eq!(reference.document_id(), ws.document_id(page));
    assert_eq!(ws.own_document_id(), Some(reference.document_id()));

    // 2. Storage → the store's parser and back, byte-identical. The store
    //    validates the *shape* of a kind token and never a membership in a
    //    list, which is why `task` needed no change on that side at all.
    assert_eq!(
        store_side_round_trip(&spelled).as_deref(),
        Some(spelled.as_str()),
        "the store re-emits the same bytes it was handed"
    );

    // 3. That string is the personal-attribute key. A `block:` key over the
    //    same node is a different key, so nothing filed under one is readable
    //    under the other (pinned against the real map in
    //    `koru-store-core`'s `a_task_key_and_a_block_key_for_one_triple_do_not_alias`).
    let block = EntityRef::block(reference.workspace_id(), reference.document_id(), node);
    assert_ne!(spelled, block.to_storage());
    assert!(reference.same_target(&block), "one node, two subjects");

    // 4. The event subject id, as `koru_events::Subject::task` formats it.
    //    Compared as a string rather than by calling it: `koru-events` is a
    //    leaf and may not acquire the node model, so the spelling is all the
    //    two share.
    assert_eq!(
        spelled,
        format!(
            "task:{}:{}:{}",
            reference.workspace_id(),
            reference.document_id(),
            node
        )
    );

    // 5. And back: parsed out of storage, it resolves to the node it named.
    let parsed = EntityRef::parse(&spelled).expect("the storage form parses");
    assert_eq!(parsed, reference, "the kind survived the round trip");
    assert_eq!(
        resolve_task_ref(&ws, &parsed),
        TaskRefResolution::Found(node)
    );

    // 6. The task is removed. The same string now answers ordinary Not Found —
    //    no removed state, and no fallback to the paragraph underneath.
    remove_from_tasks(&mut ws, node).unwrap();
    assert_eq!(
        resolve_task_ref(&ws, &EntityRef::parse(&spelled).unwrap()),
        TaskRefResolution::NotFound
    );
}

/// **A `task:` string reaching a build that has never heard of the kind is
/// skipped, never a panic and never an error.** That is the existing
/// unknown-kind behaviour, and it is why ADR-272 could add a kind without a
/// schema bump. An older build cannot be compiled here, so what is pinned is
/// the mechanism it uses: `parse` answers `None` for a kind outside this
/// build's closed set, and a reader filtering a stored list keeps what it can
/// address and walks past the rest.
#[test]
fn a_reference_of_an_unknown_kind_is_skipped_rather_than_fatal() {
    let (ws, doc, node) = (WorkspaceId::new(), DocumentId::new(), NodeId::new());
    let stored = [
        format!("task:{ws}:{doc}:{node}"),
        format!("block:{ws}:{doc}:{node}"),
        format!("document:{ws}:{doc}:{node}"),
        // What a `task:` reference looks like to a build that predates it, and
        // what a kind a *newer* build invents will look like to this one.
        format!("sprint:{ws}:{doc}:{node}"),
        format!("milestone:{ws}:{doc}:{node}"),
        // Not addresses at all — same answer, for the same reason.
        format!("task:{ws}:{doc}"),
        format!("task:{ws}:{doc}:{node}:extra"),
        format!("Task:{ws}:{doc}:{node}"),
        format!("task:{ws}:not-a-uuid:{node}"),
        String::new(),
    ];

    let addressable: Vec<EntityRef> = stored.iter().filter_map(|s| EntityRef::parse(s)).collect();
    assert_eq!(
        addressable.len(),
        3,
        "the reader kept the three kinds it knows and skipped the rest: {addressable:?}"
    );
    assert_eq!(
        addressable.iter().map(EntityRef::kind).collect::<Vec<_>>(),
        ["task", "block", "document"]
    );

    // The store side is laxer on purpose — it stores the token opaquely — so a
    // kind this build skips is still a well-formed address there and still
    // round-trips. The closed set lives here, and only here.
    for unknown in ["sprint", "milestone"] {
        let spelled = format!("{unknown}:{ws}:{doc}:{node}");
        assert_eq!(EntityRef::parse(&spelled), None, "{spelled} is not ours");
        assert_eq!(
            store_side_round_trip(&spelled).as_deref(),
            Some(spelled.as_str()),
            "{spelled} is still an address the store can file under"
        );
    }
}

/// The three ways a reference fails to resolve, all one answer.
///
/// The third is the one worth writing down: the *document* segment is
/// deliberately **not** compared, and it stayed that way when the segment
/// became truthful. A `NodeId` is document-scoped and unique, so the node is
/// what decides — a reference carrying another document's id resolves by its
/// node, and a node that is not in this document is Not Found for the ordinary
/// reason rather than a special one. Comparing the segment would buy nothing
/// and would newly refuse every reference minted before the convention changed.
#[test]
fn every_way_a_task_reference_misses_is_ordinary_not_found() {
    let mut here = doc();
    let workspace = here.id().unwrap();
    let page = here
        .insert(Parent::Root, Node::document("Plans").unwrap())
        .unwrap();
    let node = row(&mut here, Parent::Node(page), "Launch referral");
    ensure_task(&mut here, node, TaskReason::Explicit).unwrap();
    let document = here.document_id(page);

    // (a) A node that is no longer a task.
    let mut elsewhere = doc();
    let other = row(&mut elsewhere, Parent::Root, "Ship the copy");
    ensure_task(&mut elsewhere, other, TaskReason::Explicit).unwrap();
    remove_from_tasks(&mut elsewhere, other).unwrap();
    assert_eq!(
        resolve_task_ref(
            &elsewhere,
            &EntityRef::task(elsewhere.id().unwrap(), DocumentId::UNIDENTIFIED, other)
        ),
        TaskRefResolution::NotFound,
        "a node that stopped being a task"
    );

    // (b) A node that does not exist in this document.
    assert_eq!(
        resolve_task_ref(&here, &EntityRef::task(workspace, document, NodeId::new())),
        TaskRefResolution::NotFound,
        "a node that never existed"
    );
    assert_eq!(
        resolve_task_ref(&here, &EntityRef::task(workspace, document, other)),
        TaskRefResolution::NotFound,
        "a task, but another document's"
    );

    // (c) A document segment that names something else. The node still
    //     decides — this is the documented non-comparison, pinned so a later
    //     change to it is a deliberate one.
    assert_eq!(
        resolve_task_ref(&here, &EntityRef::task(workspace, DocumentId::new(), node)),
        TaskRefResolution::Found(node),
        "the node is the identity; the document segment is context"
    );

    // And a workspace that is not this one, which this document genuinely
    // cannot answer for.
    assert_eq!(
        resolve_task_ref(&here, &EntityRef::task(WorkspaceId::new(), document, node)),
        TaskRefResolution::NotFound
    );
}
