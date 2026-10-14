// SPDX-License-Identifier: MPL-2.0

//! Native tasks: where they land, and what happens when two peers each decide
//! a workspace needs a tasks document.
//!
//! In memory throughout — no store is dialled and no directory is touched. The
//! store-facing half (`ensure_task_list`'s join and register) is exercised by
//! the workspace suites that own a temp root; what is pinned here is the part
//! that decides *which* document and *what* is written into it, which is where
//! a mistake would silently strand somebody's tasks.

use super::*;
use koru_core::WorkspaceId;
use koru_core::attributes::Lifecycle;
use koru_core::tasks::TaskFields;
use koru_core::time::DateOnly;
use koru_store_core::manifest::Manifest;
use koru_store_core::model::Timestamp;

// ------------------------------------------------------------------ fixtures

/// A document shaped like one `ensure_task_list` would have created: stamped
/// with the workspace id, seeded with a root and a line, undo on.
fn task_list(workspace: WorkspaceId) -> WorkspaceDoc {
    let mut doc = WorkspaceDoc::create(workspace, TASK_LIST_TITLE).unwrap();
    seed_task_list(&mut doc).unwrap();
    doc.enable_undo();
    doc
}

fn manifest(workspace: WorkspaceId) -> Manifest {
    Manifest::create(
        explore::store_workspace(workspace),
        "w",
        Timestamp::from_millis(1),
    )
    .unwrap()
}

fn register(manifest: &mut Manifest, id: DocumentId, title: &str, kind: &str) {
    manifest
        .add_document(explore::store_document(id), title, kind, None)
        .unwrap();
}

fn on(s: &str) -> Temporal {
    Temporal::Date(DateOnly::parse(s).unwrap())
}

/// Every task in one document, as the projection would see them: a marked node,
/// named by its text. The read model asks `is_task` over the node's attributes
/// and never which document holds it, so this is the same question at the shape
/// a unit test can ask.
fn task_titles(doc: &WorkspaceDoc) -> Vec<String> {
    let mut out = Vec::new();
    for (node, _) in doc.walk().unwrap() {
        let read = doc.read(node).unwrap();
        if koru_core::tasks::is_task(&TaskFields::of(&read)) {
            out.push(read.content.as_text().unwrap_or("").to_owned());
        }
    }
    out
}

// ------------------------------------------------------------------- the pick

/// Every replica answers "where does the next task go" identically, from the
/// list alone and without talking — which is what makes two peers stop
/// diverging the moment they have seen each other. Order-independent, because
/// two manifests can list the same entries in different orders before they
/// converge.
#[test]
fn the_lowest_id_is_where_the_next_task_goes() {
    let mut ids: Vec<DocumentId> = (0..4).map(|_| DocumentId::new()).collect();
    let expected = *ids.iter().min().unwrap();
    assert_eq!(pick_task_list(&ids), Some(expected));
    ids.reverse();
    assert_eq!(pick_task_list(&ids), Some(expected));
    assert_eq!(pick_task_list(&[]), None, "a workspace may have none yet");
}

/// The manifest scan sees `task-list` entries and nothing else — a page and the
/// system document are not homes for a native task.
#[test]
fn only_task_list_entries_are_candidates() {
    let workspace = WorkspaceId::new();
    let mut m = manifest(workspace);
    let page = DocumentId::new();
    let tasks = DocumentId::new();
    let system = DocumentId::new();
    register(&mut m, page, "A page", koru_core::explore::KIND_DOCUMENT);
    register(&mut m, tasks, TASK_LIST_TITLE, KIND_TASK_LIST);
    register(&mut m, system, "", koru_core::explore::KIND_SYSTEM);

    let mut w = Workspace::new(WorkspaceDoc::create(workspace, "w").unwrap());
    w.manifest = Some(m);
    assert_eq!(w.task_list_documents(), vec![tasks]);
}

/// A workspace with no manifest at all — an in-memory one, which is what every
/// view's unit suite runs as — lists nothing and creates nothing. The store is
/// never dialled to find that out.
#[test]
fn without_a_store_creating_a_native_task_declines() {
    let mut w = Workspace::new(WorkspaceDoc::create(WorkspaceId::new(), "w").unwrap());
    assert!(w.task_list_documents().is_empty());
    assert!(
        w.create_native_task("write it down", None, None, None)
            .is_err()
    );
    assert!(!w.take_entries_changed(), "and nothing was registered");
}

// -------------------------------------------------------------- the two peers

/// **SPEC §8's promise, and the reason this module reconciles nothing.**
///
/// Two peers offline at the same time each decide the workspace needs a tasks
/// document and each write a task into their own. When their manifests meet,
/// both entries are still listed on both sides and both tasks are still there —
/// nothing is stranded, nothing is merged, and from then on both peers agree
/// about where the *next* task goes.
///
/// The failure this pins is the tempting one: repairing the "duplicate" the way
/// the system document does, which for a document full of somebody's tasks
/// would mean either dropping a manifest entry that still has content behind
/// it, or copying nodes between two Loro trees to look tidy.
#[test]
fn two_peers_each_creating_a_tasks_document_strand_nothing() {
    let workspace = WorkspaceId::new();

    let mut a_manifest = manifest(workspace);
    let mut b_manifest = manifest(workspace);
    // The shared history, so the two are replicas of one manifest rather than
    // two unrelated documents.
    b_manifest
        .import(&a_manifest.export_all_updates().unwrap())
        .unwrap();

    let a_id = DocumentId::new();
    let b_id = DocumentId::new();
    register(&mut a_manifest, a_id, TASK_LIST_TITLE, KIND_TASK_LIST);
    register(&mut b_manifest, b_id, TASK_LIST_TITLE, KIND_TASK_LIST);

    let mut a_doc = task_list(workspace);
    let mut b_doc = task_list(workspace);
    plant_native_task(&mut a_doc, "buy milk", None, None, None).unwrap();
    plant_native_task(
        &mut b_doc,
        "call the vet",
        Some(on("2026-09-04")),
        None,
        None,
    )
    .unwrap();

    // They meet.
    let a_updates = a_manifest.export_all_updates().unwrap();
    let b_updates = b_manifest.export_all_updates().unwrap();
    a_manifest.import(&b_updates).unwrap();
    b_manifest.import(&a_updates).unwrap();

    for (who, m) in [("A", &a_manifest), ("B", &b_manifest)] {
        let mut w = Workspace::new(WorkspaceDoc::create(workspace, "w").unwrap());
        w.manifest = Some(Manifest::from_snapshot(&m.export_snapshot().unwrap()).unwrap());
        let mut found = w.task_list_documents();
        found.sort();
        let mut both = vec![a_id, b_id];
        both.sort();
        assert_eq!(found, both, "{who} still lists both tasks documents");
        assert_eq!(
            pick_task_list(&found),
            Some(both[0]),
            "{who} agrees about where the next task goes"
        );
    }

    // And both peers' tasks are still tasks, in the documents that hold them.
    assert_eq!(task_titles(&a_doc), vec!["buy milk".to_string()]);
    assert_eq!(task_titles(&b_doc), vec!["call the vet".to_string()]);
    let listed: Vec<String> = task_titles(&a_doc)
        .into_iter()
        .chain(task_titles(&b_doc))
        .collect();
    assert_eq!(
        listed,
        vec!["buy milk".to_string(), "call the vet".into()],
        "a listing over the workspace's tasks documents sees both"
    );
}

// -------------------------------------------------------------- what is written

/// A native task records how it came to exist: `tasks_view`, not `explicit` and
/// not `due` — the difference is the whole reason [`TaskReason`] is stored
/// rather than inferred.
#[test]
fn a_native_task_carries_the_tasks_view_reason() {
    let mut doc = task_list(WorkspaceId::new());
    let (node, created, dated) = plant_native_task(&mut doc, "buy milk", None, None, None).unwrap();
    assert_eq!(created.created_with_reason, Some(TaskReason::TasksView));
    assert!(dated.is_none());
    let fields = TaskFields::of(&doc.read(node).unwrap());
    assert_eq!(fields.marker, Some(TaskReason::TasksView));
    assert_eq!(fields.lifecycle, Lifecycle::Open, "a new task is open");
    assert_eq!(doc.read(node).unwrap().content.as_text(), Some("buy milk"));
}

/// A task with no date is an ordinary task, not a task with an empty date. It
/// carries no `koru.due` at all — which is what puts it in Later rather than
/// making the bucket rules answer for a value that is not there.
#[test]
fn an_undated_task_has_no_due() {
    let mut doc = task_list(WorkspaceId::new());
    let (node, _, _) = plant_native_task(&mut doc, "someday", None, None, None).unwrap();
    assert_eq!(TaskFields::of(&doc.read(node).unwrap()).due, None);
}

/// The date the capture row carried is set through the canonical `set_due`, so
/// the marker, the value and the validation are the ones every other surface
/// gets — and the reason stays `tasks_view`, because `ensure_task` ran first
/// and the node was already a task by the time the due arrived.
#[test]
fn a_dated_task_is_dated_through_the_canonical_command() {
    let mut doc = task_list(WorkspaceId::new());
    let (node, created, dated) =
        plant_native_task(&mut doc, "call the vet", Some(on("2026-09-04")), None, None).unwrap();
    assert_eq!(created.created_with_reason, Some(TaskReason::TasksView));
    let dated = dated.expect("a due was given");
    assert_eq!(dated.effect, koru_core::tasks::TaskEffect::DueChanged);
    assert!(
        dated.created_with_reason.is_none(),
        "the task already existed, so the due did not create it"
    );
    let fields = TaskFields::of(&doc.read(node).unwrap());
    assert_eq!(fields.due, Some(on("2026-09-04")));
    assert_eq!(
        fields.marker,
        Some(TaskReason::TasksView),
        "the due did not rewrite how the task came to exist"
    );
}

/// **One Ctrl+Z takes the whole capture back.** Three writes happen — the row,
/// the marker, the due — and a person typed one thing. The row is inserted
/// inside the `compose` bracket for exactly this: an undo that left a blank
/// unmarked paragraph behind would be a half-undone gesture.
#[test]
fn a_dated_task_is_one_undo_step() {
    let mut doc = task_list(WorkspaceId::new());
    let before = doc.walk().unwrap().len();
    plant_native_task(&mut doc, "call the vet", Some(on("2026-09-04")), None, None).unwrap();
    assert_eq!(task_titles(&doc).len(), 1);

    assert!(doc.undo().unwrap());
    assert!(task_titles(&doc).is_empty(), "the task is gone");
    assert_eq!(
        doc.walk().unwrap().len(),
        before,
        "and so is the row it was written on"
    );
}

/// The undated capture is one step too — the same bracket, one command fewer.
#[test]
fn an_undated_task_is_one_undo_step() {
    let mut doc = task_list(WorkspaceId::new());
    let before = doc.walk().unwrap().len();
    plant_native_task(&mut doc, "buy milk", None, None, None).unwrap();
    assert!(doc.undo().unwrap());
    assert!(task_titles(&doc).is_empty());
    assert_eq!(doc.walk().unwrap().len(), before);
}

/// The seed is what every Koru document has — a `Kind::Document` root whose
/// text is the title, and a line to type into — and it is idempotent, because
/// a peer that won the race may already have written one into the room this
/// replica just joined.
#[test]
fn the_seed_is_a_document_and_runs_once() {
    let mut doc = WorkspaceDoc::create(WorkspaceId::new(), TASK_LIST_TITLE).unwrap();
    seed_task_list(&mut doc).unwrap();
    let root = document_root(&doc).expect("a root");
    assert_eq!(doc.read(root).unwrap().content.as_text(), Some("Tasks"));
    let rows = doc.children(Parent::Node(root)).unwrap().len();
    seed_task_list(&mut doc).unwrap();
    assert_eq!(document_root(&doc), Some(root), "no second root");
    assert_eq!(doc.children(Parent::Node(root)).unwrap().len(), rows);
}

/// A tasks document is stamped with its workspace by construction. Step 0 of
/// every canonical command's preflight is that read, and a document without one
/// refuses `/due` silently — which is the bug this stamp exists to have already
/// fixed.
#[test]
fn a_tasks_document_carries_its_workspace_id() {
    let workspace = WorkspaceId::new();
    let doc = task_list(workspace);
    assert_eq!(doc.id(), Some(workspace));
}

// ------------------------------------------------------------------- the list

/// A task typed straight into a list carries that list — one `koru.list` value
/// on the task, set in the same bracket, so the capture is still one Ctrl+Z.
/// Membership is a filter and not a place: the row is in the tasks document
/// like every other native task.
#[test]
fn a_task_typed_into_a_list_carries_it() {
    let mut doc = task_list(WorkspaceId::new());
    let list = ListId::new();
    let (node, _, _) =
        plant_native_task(&mut doc, "fit new brake pads", None, Some(list), None).unwrap();
    assert_eq!(doc.read(node).unwrap().list_id(), Some(list));
    assert_eq!(
        TaskFields::of(&doc.read(node).unwrap()).marker,
        Some(TaskReason::TasksView),
        "still a native task"
    );
    assert!(doc.undo().unwrap());
    assert!(
        task_titles(&doc).is_empty(),
        "and the membership was part of the one step"
    );
}

/// A task with no list is in no list — not in an empty one. Dateless *and*
/// listless is the ordinary capture.
#[test]
fn a_task_with_no_list_is_in_none() {
    let mut doc = task_list(WorkspaceId::new());
    let (node, _, _) = plant_native_task(&mut doc, "buy milk", None, None, None).unwrap();
    assert_eq!(doc.read(node).unwrap().list_id(), None);
}

// -------------------------------------------------------------- what refuses

/// **A refused capture leaves nothing behind.** `koru.due` is shared, so it
/// refuses a floating wall clock — and by then the row exists and is already
/// marked. Returning the error there would leave a task the caller was told it
/// had not created, and a retry would make a second one; the group is undone
/// instead, and the document is where it was.
#[test]
fn a_refused_date_leaves_the_document_alone() {
    use koru_core::time::FloatingDateTime;

    let mut doc = task_list(WorkspaceId::new());
    let before = doc.walk().unwrap().len();
    let floating = Temporal::Floating(FloatingDateTime::new(
        DateOnly::parse("2026-09-04")
            .unwrap()
            .date()
            .and_hms_opt(9, 0, 0)
            .unwrap(),
    ));
    let refused = plant_native_task(&mut doc, "call the vet", Some(floating), None, None);
    assert!(refused.is_err(), "a shared deadline carries its zone");
    assert_eq!(
        refused.unwrap_err().code,
        koru_store_core::errors::Code::InvalidRequest,
        "a caller's mistake, not a broken workspace"
    );
    assert!(task_titles(&doc).is_empty(), "no half-made task");
    assert_eq!(doc.walk().unwrap().len(), before, "and no stray row");
}

// ------------------------------------------------------------ membership (V-4)

/// **Membership travels the door a tick travels**, and the round trip leaves
/// the task itself alone.
///
/// `run_task_command` is what the Tasks surface's `SetList` runs through — it
/// exists because a task lives wherever it was typed, so a list chosen on a
/// list page is a write into a document nobody is looking at. What it runs is
/// the `set_list`/`clear_list` pair rather than a bare `set_attribute`: one
/// value, cardinality one, validated as a [`ListId`], and each call its own
/// undo step.
///
/// It is deliberately **not** `set_attribute_on_all`, which is the *in-group*
/// form `plant_native_task` above needs inside its `compose` bracket — a
/// checkpoint there would close the bracket and split one capture into two
/// Ctrl+Zs. Here there is no bracket, and the checkpoint is what makes
/// changing a membership one step.
#[test]
fn a_membership_set_and_cleared_round_trips_through_the_command_door() {
    let workspace = WorkspaceId::new();
    let mut w = Workspace::new(task_list(workspace));
    // The document under the caret, which is the arm `run_task_command` takes
    // when the task's document is the one already open. The parked arm is the
    // store's and is exercised by the suites that own a temp root.
    let document = DocumentId::new();
    w.active = Some(document);
    let (node, _, _) = plant_native_task(&mut w.ws, "Order the groupset", None, None, None)
        .expect("a planted task");
    assert_eq!(w.ws.read(node).unwrap().list_id(), None, "in no list yet");

    let wedding = ListId::new();
    w.run_task_command(document, |doc| doc.set_list(node, wedding).map(|_| ()))
        .expect("the membership lands");
    assert_eq!(w.ws.read(node).unwrap().list_id(), Some(wedding));

    // Another list replaces the first rather than being added beside it.
    let reading = ListId::new();
    w.run_task_command(document, |doc| doc.set_list(node, reading).map(|_| ()))
        .expect("the second membership lands");
    assert_eq!(w.ws.read(node).unwrap().list_id(), Some(reading));

    w.run_task_command(document, |doc| doc.clear_list(node).map(|_| ()))
        .expect("and it comes off");
    assert_eq!(w.ws.read(node).unwrap().list_id(), None);

    // None of it touched the task. A list is a filter, not a place, so a
    // membership that comes and goes cannot take the task with it.
    let fields = TaskFields::of(&w.ws.read(node).unwrap());
    assert!(koru_core::tasks::is_task(&fields), "still a task");
    assert_eq!(fields.lifecycle, Lifecycle::Open, "and still open");
    assert_eq!(task_titles(&w.ws), vec!["Order the groupset".to_owned()]);
}

/// A membership whose list the workspace has since forgotten is still an
/// ordinary value on the node: the document cannot see the list table, so
/// nothing here refuses, and the surface resolves it to no list rather than to
/// a broken one.
#[test]
fn a_membership_naming_no_list_is_still_a_value() {
    let workspace = WorkspaceId::new();
    let mut w = Workspace::new(task_list(workspace));
    let document = DocumentId::new();
    w.active = Some(document);
    let (node, _, _) = plant_native_task(&mut w.ws, "Loose end", None, Some(ListId::new()), None)
        .expect("a planted task");
    assert!(
        w.ws.read(node).unwrap().list_id().is_some(),
        "the id the caller named, whether or not a list wears it"
    );
}

// ------------------------------------------------------------- the receipt

/// A window with one document open and a workspace named, and no store — the
/// shape every receipt test wants, because a receipt is about *what was
/// written where*, and the store's part of it is one `bool`.
fn open_on(workspace: WorkspaceId, doc: WorkspaceDoc) -> (Workspace, DocumentId) {
    let id = DocumentId::new();
    let mut w = Workspace::new(doc);
    w.active = Some(id);
    w.current = Some(workspace);
    (w, id)
}

/// One canonical command on a fresh row — the shape every command the road
/// carries has, refused or not, so the receipt tests drive the real closure
/// type rather than a stand-in.
fn write_a_task(doc: &mut WorkspaceDoc, title: &str) -> Result<(), koru_core::StoreError> {
    let root = document_root(doc).expect("a seeded tasks document has a root");
    let row = doc.insert(Parent::Node(root), Node::paragraph(title).unwrap())?;
    tasks::ensure_task(doc, row, TaskReason::TasksView).map(|_| ())
}

/// **The receipt names the document that was written, at the version it was
/// written**, read from that document the moment the command returned — not
/// from an intent, and not later.
#[test]
fn a_command_on_the_active_document_answers_with_its_receipt() {
    let workspace = WorkspaceId::new();
    let (mut w, id) = open_on(workspace, task_list(workspace));
    let before = w.ws.version();

    let ((), landed) = w
        .run_task_command(id, |doc| write_a_task(doc, "buy milk"))
        .unwrap();

    assert_eq!(landed.document, id);
    assert_eq!(landed.workspace, Some(workspace));
    let peer = w.ws.peer_id();
    let receipt = landed.receipt.expect("a write answers with a receipt");
    assert_eq!(receipt.peer, peer, "the session that wrote");
    assert_eq!(
        receipt.counter,
        i64::from(w.ws.version().get(&peer).copied().unwrap_or(0)),
        "read after the write"
    );
    assert!(
        receipt.counter > i64::from(before.get(&peer).copied().unwrap_or(0)),
        "and past where this session's own operations stood before it"
    );
    assert!(
        !landed.published,
        "no session took the publish, and the receipt says so rather than guessing"
    );
}

/// A write into a document this window holds parked — the ordinary case for a
/// Tasks surface — names *that* document, not the active one.
#[test]
fn a_command_on_a_parked_document_answers_with_that_documents_receipt() {
    let workspace = WorkspaceId::new();
    let (mut w, active) = open_on(workspace, task_list(workspace));
    let parked = DocumentId::new();
    w.others.insert(parked, task_list(workspace));

    let ((), landed) = w
        .run_task_command(parked, |doc| write_a_task(doc, "call the vet"))
        .unwrap();

    assert_eq!(landed.document, parked);
    assert_ne!(landed.document, active);
    let receipt = landed.receipt.expect("a write answers with a receipt");
    let peer = w.others[&parked].peer_id();
    assert_eq!(receipt.peer, peer);
    assert_eq!(
        receipt.counter,
        i64::from(w.others[&parked].version().get(&peer).copied().unwrap_or(0))
    );
    assert_eq!(
        task_titles(&w.others[&parked]),
        vec!["call the vet".to_string()]
    );
    assert!(
        task_titles(&w.ws).is_empty(),
        "the active document was not touched"
    );
}

/// A native task's receipt names the document `ensure_task_list` chose — the
/// one fact `Create` does not carry in its input.
#[test]
fn a_native_task_names_the_tasks_document_it_landed_in() {
    let workspace = WorkspaceId::new();
    let (mut w, _active) = open_on(workspace, WorkspaceDoc::create(workspace, "w").unwrap());
    let list = DocumentId::new();
    let mut m = manifest(workspace);
    register(&mut m, list, TASK_LIST_TITLE, KIND_TASK_LIST);
    w.manifest = Some(m);
    w.others.insert(list, task_list(workspace));

    let created = w.create_native_task("ship it", None, None, None).unwrap();

    assert_eq!(created.document, list);
    assert_eq!(
        created.landed.document, list,
        "the receipt agrees with the answer"
    );
    assert_eq!(created.landed.workspace, Some(workspace));
    let receipt = created
        .landed
        .receipt
        .expect("a write answers with a receipt");
    let peer = w.others[&list].peer_id();
    assert_eq!(receipt.peer, peer);
    assert_eq!(
        receipt.counter,
        i64::from(w.others[&list].version().get(&peer).copied().unwrap_or(0))
    );
    assert_eq!(task_titles(&w.others[&list]), vec!["ship it".to_string()]);
}

/// A command that refuses produces no receipt — the caller already tells the
/// two apart, and a receipt for a write that did not happen would be waited on
/// for the whole budget.
#[test]
fn a_refused_command_answers_no_receipt() {
    let workspace = WorkspaceId::new();
    let (mut w, id) = open_on(workspace, task_list(workspace));
    let before = w.ws.version();
    let refused = w.run_task_command(id, |doc| {
        koru_core::tasks::complete_task(
            doc,
            NodeId::new(),
            koru_core::tasks::LifecycleReason::ExplicitComplete,
            koru_core::time::DateOnly::parse("2026-09-06").unwrap(),
        )
    });
    assert!(refused.is_err());
    assert_eq!(w.ws.version(), before, "and nothing was written");
}

/// A document that carries no workspace id, in a window that has none open,
/// gets a receipt with no workspace: a surface then invalidates without a
/// requirement, because it could not address one.
#[test]
fn a_receipt_without_a_workspace_says_so() {
    let mut w = Workspace::new(task_list(WorkspaceId::new()));
    let id = DocumentId::new();
    w.active = Some(id);
    // `current` stays `None`, and the document's own stamp is what answers.
    let ((), landed) = w
        .run_task_command(id, |doc| write_a_task(doc, "x"))
        .unwrap();
    assert_eq!(
        landed.workspace,
        w.ws.id(),
        "the document's stamp, when the window has none"
    );
    assert!(landed.workspace.is_some());
}

/// A level captured with the task is written in the same bracket as the
/// marker: one undo step takes the row, the marker and the level together.
#[test]
fn a_native_task_carries_its_priority_in_one_step() {
    use koru_core::attributes::Priority;
    let mut doc = task_list(WorkspaceId::new());
    doc.enable_undo();
    let (node, _, _) =
        plant_native_task(&mut doc, "buy milk", None, None, Some(Priority::Urgent)).unwrap();
    assert_eq!(doc.read(node).unwrap().priority(), Some(Priority::Urgent));
    assert!(doc.undo().unwrap());
    assert!(doc.read(node).is_err(), "one step: the row itself is gone");
}

/// **A command that wrote nothing answers with no receipt**, and that is a
/// real answer rather than a missing one.
///
/// A guarded no-op — the six canonical commands all have one — commits
/// nothing, so this session's own operations do not move. There is no write
/// for a read model to catch up to, so a surface must not arm a wait on it and
/// must not leave an optimistic row standing for an event that will never
/// happen.
#[test]
fn a_command_that_wrote_nothing_answers_with_no_receipt() {
    let workspace = WorkspaceId::new();
    let (mut w, id) = open_on(workspace, task_list(workspace));

    let ((), wrote) = w
        .run_task_command(id, |doc| write_a_task(doc, "buy milk"))
        .unwrap();
    assert!(wrote.receipt.is_some(), "the write has one");

    // The same command again, guarded into a no-op by reading nothing and
    // writing nothing.
    let ((), idle) = w.run_task_command(id, |_doc| Ok(())).unwrap();

    assert!(
        idle.receipt.is_none(),
        "nothing committed, so there is nothing to wait for"
    );
    assert_eq!(idle.document, id, "and it still says which document");
}

/// **A command writing several operations answers with one receipt**, at the
/// end of them — one peer's operations are contiguous, so covering the end
/// covers all of them and no list is needed.
#[test]
fn a_multi_operation_command_answers_with_one_receipt_at_the_end() {
    let workspace = WorkspaceId::new();
    let (mut w, id) = open_on(workspace, task_list(workspace));
    let peer = w.ws.peer_id();
    let before = i64::from(w.ws.version().get(&peer).copied().unwrap_or(0));

    let ((), landed) = w
        .run_task_command(id, |doc| {
            write_a_task(doc, "one")?;
            write_a_task(doc, "two")?;
            write_a_task(doc, "three")
        })
        .unwrap();

    let receipt = landed.receipt.expect("a receipt");
    assert_eq!(receipt.peer, peer);
    assert_eq!(
        receipt.counter,
        i64::from(w.ws.version().get(&peer).copied().unwrap_or(0)),
        "the end of the whole command, not of its first operation"
    );
    assert!(
        receipt.counter > before + 1,
        "and it really did write more than one operation"
    );
}
