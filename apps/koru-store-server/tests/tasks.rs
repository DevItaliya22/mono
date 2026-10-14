// SPDX-License-Identifier: MPL-2.0

//! **The tasks vertical slice** — B-Gate's gate (roadmap rev6 §5).
//!
//! A task is made the way a person makes one: typed `koru-core` canonical
//! commands on a `WorkspaceDoc`, published over the ordinary sync session, and
//! then asked back through `POST /v1/tasks/query` — the same route a Tasks view
//! and `list_tasks` will call. Nothing here reads `projection.db` directly:
//! what is asserted is what a client can get, which is the same rule
//! `attribute_query.rs` states and for the same reason.
//!
//! **The division of labour is deliberate.** Where the question is *what does
//! the store answer*, the assertion is on an HTTP response. Where the question
//! is *what does the model do* — a refusal, a cascade, a reference that stopped
//! resolving — the assertion is in process on the document, because those
//! answers never cross the wire and inventing a route to observe them would be
//! testing a route that does not exist. Both halves are the same document.
//!
//! It lives in this package for the same reason `document_roundtrip.rs` does:
//! `CARGO_BIN_EXE_koru-store-server` is only set for tests of the package that
//! declares the binary.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use koru_core::attributes::{KEY_PRIORITY, LEGACY_DUE_ROLE, Lifecycle, TaskReason};
use koru_core::ids::EntityRef;
use koru_core::node::{Attr, AttrValue, BlobHash, Node};
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_core::tasks::{
    self, ActivityIntent, LifecycleReason, TaskFields, TaskRefusal,
    refs::{TaskRefResolution, resolve_task_ref},
};
use koru_core::time::{DateOnly, Temporal};
use koru_store_client::{Client, LifecycleFilter, TaskQuery, TaskQueryResponse};
use koru_workspace::store::StoreSession;

fn segment() -> &'static str {
    koru_channel::segment()
}

struct Store {
    child: Option<Child>,
    root: PathBuf,
}

impl Store {
    fn start(root: &Path) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_koru-store-server"));
        command
            .arg("serve")
            // A suite starting daemons on throwaway roots must never raise a
            // modal dialog: nothing dismisses it, so the process would never
            // exit and would go on holding its databases and this binary.
            .env("KORU_STORE_NO_DIALOG", "1")
            .arg("--console")
            .arg("--root")
            .arg(root);
        common::configure(&mut command, root);
        let child = command.spawn().expect("spawn koru-store-server");
        let deadline = Instant::now() + Duration::from_secs(30);
        while common::discovery(root).map(|d| Client::new(d).status().is_ok()) != Some(true) {
            assert!(
                Instant::now() < deadline,
                "the store never became reachable"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        Self {
            child: Some(child),
            root: root.to_path_buf(),
        }
    }

    fn client(&self) -> Client {
        Client::new(common::discovery(&self.root).expect("discover"))
    }

    fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        self.kill();
    }
}

fn root_for(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("koru-store-tasks-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp root");
    dir
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn on(s: &str) -> Temporal {
    Temporal::Date(DateOnly::parse(s).unwrap())
}

fn a_query() -> TaskQuery {
    TaskQuery::default()
}

/// One document, joined over the ordinary sync session, plus the session that
/// publishes it. The pair is what a window holds.
struct Editing {
    session: StoreSession,
    doc: WorkspaceDoc,
    root: PathBuf,
}

impl Editing {
    fn join(
        root: &Path,
        workspace: koru_store_core::ids::WorkspaceId,
        document: koru_store_core::ids::DocumentId,
        workspace_name: &str,
    ) -> Self {
        let (session, snapshot) =
            StoreSession::join(segment(), Some(root), workspace, document).expect("join");
        // **A document knows which workspace it is in, and that is stored in
        // the document.** Every canonical command reads it in preflight, so a
        // document whose CRDT never carried one cannot be worked on — which is
        // why the app and MCP both build a new document with
        // `WorkspaceDoc::create(workspace, …)` and publish *that*, rather than
        // seeding through the bare create route and writing into whatever comes
        // back. This does the same, and falls back for the same reason.
        let fresh = || {
            WorkspaceDoc::create(
                koru_core::WorkspaceId::from_uuid(workspace.as_uuid()),
                workspace_name,
            )
            .expect("create")
        };
        let mut doc = match WorkspaceDoc::from_snapshot(&snapshot) {
            Ok(doc) if doc.id().is_some() => doc,
            _ => fresh(),
        };
        // **And which document it is**, from the id this join was made with —
        // the same stamp `koru_workspace::Workspace::open_document_in` and
        // MCP's `join_for_edit` make at their own seams. It is the middle
        // segment of every `task:` reference minted in this document, and the
        // read model builds the same string from its own copy of this id: the
        // two agree only because both sides name the store's answer.
        doc.stamp_document_id(koru_core::DocumentId::from_uuid(document.as_uuid()))
            .expect("stamping the document id");
        Self {
            session,
            doc,
            root: root.to_path_buf(),
        }
    }

    /// Send everything written since the last publish, and wait for the store
    /// to call it durable. **Acknowledgement, never "what was sent"** — the
    /// vertical slice's rule, applied here too.
    fn publish(&mut self) {
        let Some(batch) = self.session.publish(&self.doc).expect("publish") else {
            return; // nothing to send
        };
        let session = &mut self.session;
        let doc = &mut self.doc;
        let deadline = Instant::now() + Duration::from_secs(30);
        while !session.is_durable(batch) {
            let _ = session.drain(doc);
            assert!(Instant::now() < deadline, "the write never became durable");
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = &self.root;
    }
}

// --------------------------------------------------------------- the slice

/// **The gate.** One document, one due date, and every semantic the release
/// owes — asserted through the public query where the store answers, and on the
/// document where the model answers.
#[test]
fn a_due_date_becomes_work_and_the_lifecycle_survives_the_round_trip() {
    let root = root_for("slice");
    let store = Store::start(&root);
    let client = store.client();

    let profile = client
        .create_profile("Tester", None, None, None)
        .expect("profile");
    let workspace = client
        .create_workspace(profile.id, "Acme")
        .expect("workspace");
    let document = client
        .create_document(workspace.id, "Tasks", "page")
        .expect("document");

    let mut editing = Editing::join(&root, workspace.id, document, "Acme");
    let page = editing
        .doc
        .insert(Parent::Root, Node::document("Tasks").unwrap())
        .unwrap();
    let ship = editing
        .doc
        .insert(Parent::Node(page), Node::task("Ship the report").unwrap())
        .unwrap();
    editing
        .doc
        .set_attribute(ship, KEY_PRIORITY, AttrValue::Text("high".into()))
        .expect("priority");

    // ---- create: a due date is one of the two ways a task exists.
    let created = tasks::set_due(&mut editing.doc, ship, on("2026-09-01")).expect("due");
    assert!(
        created
            .activity
            .iter()
            .any(|i| matches!(i, ActivityIntent::Created { .. })),
        "setting a due on a plain checklist item created the task"
    );
    editing.publish();

    // ---- the row, as a client sees it.
    let mut listed = TaskQueryResponse {
        rows: Vec::new(),
        definitions: Vec::new(),
        tags: Vec::new(),
        groups: Vec::new(),
        projected: false,
        updating_workspaces: Vec::new(),
        limit: 0,
        next_after: None,
        restarted: false,
        covered: None,
    };
    wait_for("the task to reach the read model", || {
        listed = client.query_tasks(&a_query()).expect("query");
        listed.projected && listed.rows.len() == 1
    });
    let row = &listed.rows[0];
    assert_eq!(row.title, "Ship the report", "the one title chain");
    assert_eq!(row.lifecycle, "open", "absence of a stored state is open");
    assert!(!row.document_trashed && !row.in_trash, "nothing is binned");
    assert!(
        row.breadcrumb.contains("Acme") && row.breadcrumb.contains("Tasks"),
        "the workspace and the document path: {}",
        row.breadcrumb
    );
    // **The bucket is not on the wire, and its inputs are** — a stored bucket
    // would be wrong by morning and wrong in another country, so the row
    // carries what a reader needs to compute one.
    let roles: Vec<&str> = row.attributes.iter().map(|a| a.role.as_str()).collect();
    assert!(
        roles.contains(&koru_core::attributes::KEY_DUE)
            && roles.contains(&KEY_PRIORITY)
            && roles.contains(&koru_core::attributes::KEY_TASK),
        "due, priority and the marker travel with the row: {roles:?}"
    );
    assert!(
        listed
            .definitions
            .iter()
            .any(|d| d.key == koru_core::attributes::KEY_DUE),
        "and the definitions a filter builds from"
    );

    // **The reference round-trips.** One string is the row's identity, its page
    // cursor and its activity subject; parsed back it is the same triple, and
    // it resolves in the document it names.
    let parsed = EntityRef::parse(&row.task).expect("the row's reference parses");
    assert!(matches!(parsed, EntityRef::Task { .. }), "kind-preserving");
    assert_eq!(
        parsed.workspace_id().to_storage(),
        workspace.id.to_storage()
    );
    assert_eq!(parsed.document_id().to_storage(), document.to_storage());
    assert_eq!(
        resolve_task_ref(&editing.doc, &parsed),
        TaskRefResolution::Found(ship)
    );
    // **And the two sides mint the same string.** The read model builds this
    // reference in SQL from the store's own columns; the domain built one in
    // the document when the command ran. They agree segment for segment,
    // including the middle one — which they did not while it was derived from
    // the node (TECHDEBT 72): every row of a page carried a different document
    // there, so one task had two spellings depending on who was asked.
    assert_eq!(
        created.task.to_storage(),
        row.task,
        "the command's subject and the row's identity are one string"
    );
    // A `block:` reference to the same node is a different subject and answers
    // Not Found — the kind is part of the identity (SPEC §3).
    assert_eq!(
        resolve_task_ref(
            &editing.doc,
            &EntityRef::block(
                koru_core::WorkspaceId::from_uuid(workspace.id.as_uuid()),
                koru_core::DocumentId::from_uuid(document.as_uuid()),
                ship,
            )
        ),
        TaskRefResolution::NotFound
    );

    // ---- complete, through the canonical command.
    tasks::complete_task(
        &mut editing.doc,
        ship,
        LifecycleReason::ExplicitComplete,
        koru_core::time::DateOnly::parse("2026-09-06").unwrap(),
    )
    .expect("done");
    editing.publish();
    wait_for("the completion to reach the read model", || {
        let open = client.query_tasks(&a_query()).expect("query");
        let done = client
            .query_tasks(&TaskQuery {
                lifecycle: LifecycleFilter::Completed,
                ..a_query()
            })
            .expect("query");
        open.projected && open.rows.is_empty() && done.rows.len() == 1
    });

    // ---- an unfinished child arrives under a completed task: it reopens.
    let sub = editing
        .doc
        .insert(Parent::Node(ship), Node::task("Draft the summary").unwrap())
        .unwrap();
    // **The command reopens it**, since D-2c. Marking a descendant of a
    // completed task is SPEC §5(d)'s "a descendant becoming an open task", and
    // that transition is answered inside `ensure_task` — in its own atomic
    // group, under its own causation id — rather than by a hook each of `/task`'s
    // surfaces has to remember. The classification this test used to ask
    // afterwards is asserted below, where it now correctly finds nothing left.
    let reopened =
        tasks::ensure_task(&mut editing.doc, sub, TaskReason::Explicit).expect("a child task");
    assert!(
        reopened.activity.iter().any(|i| matches!(
            i,
            ActivityIntent::Reopened {
                reason: LifecycleReason::UnfinishedChildAdded,
                ..
            }
        )),
        "and says why: {:?}",
        reopened.activity
    );
    assert_eq!(
        TaskFields::of(&editing.doc.read(ship).unwrap()).lifecycle,
        Lifecycle::Open
    );
    assert!(
        tasks::reclassify(&mut editing.doc, sub)
            .expect("classify")
            .is_none(),
        "and asking the classification afterwards finds nothing left to reopen"
    );

    // ---- and the two things that are *not* an unfinished child.
    tasks::complete_task(
        &mut editing.doc,
        sub,
        LifecycleReason::ExplicitComplete,
        koru_core::time::DateOnly::parse("2026-09-06").unwrap(),
    )
    .expect("tick");
    tasks::complete_task(
        &mut editing.doc,
        ship,
        LifecycleReason::ExplicitComplete,
        koru_core::time::DateOnly::parse("2026-09-06").unwrap(),
    )
    .expect("tick");
    assert!(
        tasks::reclassify(&mut editing.doc, sub)
            .expect("classify")
            .is_none(),
        "an already-completed child reopens nothing"
    );
    let note = editing
        .doc
        .insert(Parent::Node(ship), Node::paragraph("a note").unwrap())
        .unwrap();
    assert!(
        tasks::reclassify(&mut editing.doc, note)
            .expect("classify")
            .is_none(),
        "and neither does a passive note"
    );
    assert_eq!(
        TaskFields::of(&editing.doc.read(ship).unwrap()).lifecycle,
        Lifecycle::Completed,
        "the parent stayed completed through both"
    );

    // ---- Remove from Tasks: the task fields go, the content stays.
    let before = TaskFields::of(&editing.doc.read(ship).unwrap());
    assert!(before.done, "the checkbox was ticked by the completion");
    tasks::remove_from_tasks(&mut editing.doc, ship).expect("remove");
    let after = editing.doc.read(ship).unwrap();
    let fields = TaskFields::of(&after);
    assert!(!tasks::is_task(&fields), "it is no longer work");
    assert!(fields.due.is_none() && fields.marker.is_none());
    assert!(fields.done, "the box keeps its state — SPEC §6");
    assert_eq!(
        after.attr(KEY_PRIORITY).and_then(Attr::as_text),
        Some("high"),
        "and priority survives, because it is content"
    );
    // A removed task's reference answers **ordinary** Not Found: no "removed"
    // state, no fallback to the paragraph underneath.
    assert_eq!(
        resolve_task_ref(&editing.doc, &parsed),
        TaskRefResolution::NotFound
    );
    editing.publish();
    wait_for("the removal to reach the read model", || {
        let all = client
            .query_tasks(&TaskQuery {
                lifecycle: LifecycleFilter::All,
                ..a_query()
            })
            .expect("query");
        // The child is still a task; the parent is not.
        all.projected && all.rows.iter().all(|r| r.title != "Ship the report")
    });
}

/// **A refusal writes nothing.** Every canonical command validates before its
/// first write (owner decision 4, ADR-272's proof), so a command that refuses
/// leaves the document exactly as it found it — asserted on the exported bytes,
/// with a control that proves the export is stable in the first place.
///
/// The excluded kinds are refused at the *semantic door*, not merely hidden:
/// SPEC §4's closed set is five kinds, and a quote, a table and a code block
/// are none of them.
#[test]
fn an_excluded_kind_is_refused_and_the_document_is_untouched() {
    let mut doc = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "Acme").expect("create");
    let page = doc
        .insert(Parent::Root, Node::document("Notes").unwrap())
        .unwrap();
    let excluded = [
        (
            "quote",
            doc.insert(Parent::Node(page), Node::quote("said").unwrap()),
        ),
        (
            "code",
            doc.insert(Parent::Node(page), Node::code("fn main() {}").unwrap()),
        ),
        (
            "table",
            doc.insert(Parent::Node(page), Node::table("| a |").unwrap()),
        ),
    ];
    let excluded: Vec<(&str, koru_core::NodeId)> = excluded
        .into_iter()
        .map(|(name, id)| (name, id.expect("insert")))
        .collect();

    // The control: two exports of an unchanged document are the same bytes, so
    // the comparison below means something.
    let control = doc.export_snapshot().expect("export");
    assert_eq!(
        control,
        doc.export_snapshot().expect("export"),
        "an unchanged document exports identically, or the assertion below is empty"
    );

    for (name, node) in &excluded {
        for refused in [
            tasks::set_due(&mut doc, *node, on("2026-09-01")),
            tasks::ensure_task(&mut doc, *node, TaskReason::Explicit),
            tasks::complete_task(
                &mut doc,
                *node,
                LifecycleReason::ExplicitComplete,
                koru_core::time::DateOnly::parse("2026-09-06").unwrap(),
            ),
        ] {
            match refused {
                Err(koru_core::schema::StoreError::TaskRejected { why, .. }) => assert_eq!(
                    why,
                    TaskRefusal::KindNotCapable,
                    "{name} is outside SPEC §4's closed set"
                ),
                other => panic!("{name} was not refused: {other:?}"),
            }
        }
    }
    assert_eq!(
        control,
        doc.export_snapshot().expect("export"),
        "nine refusals wrote nothing at all"
    );

    // The same proof for the *other* preflight refusal: a command that does not
    // create a task, run on something that is not one.
    match tasks::complete_task(
        &mut doc,
        page,
        LifecycleReason::ExplicitComplete,
        koru_core::time::DateOnly::parse("2026-09-06").unwrap(),
    ) {
        Err(koru_core::schema::StoreError::TaskRejected { why, .. }) => {
            assert_eq!(why, TaskRefusal::NotATask)
        }
        other => panic!("a plain document root was not refused: {other:?}"),
    }
    assert_eq!(control, doc.export_snapshot().expect("export"));
}

/// **A leaf carries a due like anything else** (SPEC §4): an image is task
/// capable, its title is its filename through the one title chain, and it has
/// no owned content — so completion is explicit, never a checkbox.
///
/// Asserted through the query, because the *title* is the projection's answer
/// and an in-process check would be testing `koru-title` twice.
#[test]
fn a_due_on_an_image_is_a_task_titled_by_its_filename() {
    let root = root_for("image");
    let store = Store::start(&root);
    let client = store.client();

    let profile = client
        .create_profile("Tester", None, None, None)
        .expect("profile");
    let workspace = client
        .create_workspace(profile.id, "Acme")
        .expect("workspace");
    let document = client
        .create_document(workspace.id, "Gallery", "page")
        .expect("document");

    let mut editing = Editing::join(&root, workspace.id, document, "Acme");
    let page = editing
        .doc
        .insert(Parent::Root, Node::document("Gallery").unwrap())
        .unwrap();
    let image = editing
        .doc
        .insert(
            Parent::Node(page),
            Node::image(BlobHash("f00d".into()), "image/png", Some(8), Some(6)).unwrap(),
        )
        .unwrap();
    editing
        .doc
        .set_name(image, Some("chart.png"))
        .expect("filename");
    tasks::set_due(&mut editing.doc, image, on("2026-09-03")).expect("due");
    editing.publish();

    wait_for("the image task to reach the read model", || {
        let page = client.query_tasks(&a_query()).expect("query");
        page.projected && page.rows.len() == 1 && page.rows[0].title == "chart.png"
    });
    let listed = client.query_tasks(&a_query()).expect("query");
    assert_eq!(listed.rows[0].node_kind, "image");

    // **A leaf owns no content**, so there is nothing under it that completion
    // could be about — and completion is therefore the explicit command,
    // which is the whole of what SPEC §4 asks of an image task.
    let state = tasks::TaskState::of(&editing.doc, image).expect("state");
    assert!(state.is_task && !state.has_structural_children);
    tasks::complete_task(
        &mut editing.doc,
        image,
        LifecycleReason::ExplicitComplete,
        koru_core::time::DateOnly::parse("2026-09-06").unwrap(),
    )
    .expect("explicit complete");
    editing.publish();
    wait_for("the completion to reach the read model", || {
        let open = client.query_tasks(&a_query()).expect("query");
        open.projected && open.rows.is_empty()
    });
}

/// **A document written before any of this existed is already a task list.**
///
/// `is_task` is *marker OR due*, and the due is read through the pre-registry
/// bare `due` role — so a node written by an older build, with no marker
/// anywhere in the workspace, appears as work with nobody editing it. This
/// writes the legacy attribute at the raw door on purpose: that is what the old
/// build did.
#[test]
fn a_pre_existing_due_is_work_with_no_user_edit() {
    let root = root_for("legacy");
    let store = Store::start(&root);
    let client = store.client();

    let profile = client
        .create_profile("Tester", None, None, None)
        .expect("profile");
    let workspace = client
        .create_workspace(profile.id, "Acme")
        .expect("workspace");
    let document = client
        .create_document(workspace.id, "Old notes", "page")
        .expect("document");

    let mut editing = Editing::join(&root, workspace.id, document, "Acme");
    let page = editing
        .doc
        .insert(Parent::Root, Node::document("Old notes").unwrap())
        .unwrap();
    let legacy = editing
        .doc
        .insert(
            Parent::Node(page),
            Node::paragraph("Renew the lease").unwrap(),
        )
        .unwrap();
    editing
        .doc
        .set_attr(
            legacy,
            &Attr::new(
                LEGACY_DUE_ROLE,
                AttrValue::Date(DateOnly::parse("2026-09-04").unwrap()),
            ),
        )
        .expect("the attribute an older build wrote");
    assert!(
        editing
            .doc
            .read(legacy)
            .unwrap()
            .attr(koru_core::attributes::KEY_TASK)
            .is_none(),
        "and no marker anywhere"
    );
    editing.publish();

    wait_for("the legacy row to be answered as a task", || {
        let page = client.query_tasks(&a_query()).expect("query");
        page.projected && page.rows.len() == 1 && page.rows[0].title == "Renew the lease"
    });
    assert_eq!(
        client.query_tasks(&a_query()).expect("query").rows[0].lifecycle,
        "open",
        "absence of a stored lifecycle is open, in the data as well as the query"
    );
}

/// **Sweep first, never wait, and never open a document to answer.**
///
/// A workspace nobody has touched since the store restarted is cold. The first
/// query names it in `updating_workspaces` rather than blocking on it, asks for
/// a sweep, and converges. Then the counters do the other half: a *warm* query
/// projects nothing, and one edit projects exactly one document.
#[test]
fn a_cold_workspace_reports_updating_then_converges_without_opening_a_document() {
    let root = root_for("cold");
    let store = Store::start(&root);
    let client = store.client();

    let profile = client
        .create_profile("Tester", None, None, None)
        .expect("profile");
    let workspace = client
        .create_workspace(profile.id, "Acme")
        .expect("workspace");
    let document = client
        .create_document(workspace.id, "Tasks", "page")
        .expect("document");

    let mut editing = Editing::join(&root, workspace.id, document, "Acme");
    let page = editing
        .doc
        .insert(Parent::Root, Node::document("Tasks").unwrap())
        .unwrap();
    let first = editing
        .doc
        .insert(Parent::Node(page), Node::task("Book the room").unwrap())
        .unwrap();
    tasks::set_due(&mut editing.doc, first, on("2026-09-05")).expect("due");
    editing.publish();
    wait_for("the first task to be answered", || {
        let page = client.query_tasks(&a_query()).expect("query");
        page.projected && page.rows.len() == 1
    });

    // **The warm query opens nothing.** `projected` on `/v1/debug/projection`
    // counts documents the projector walked; a query that had to open one to
    // build a row would move it.
    let projected_before = counter(&client, "projected");
    for _ in 0..5 {
        let page = client.query_tasks(&a_query()).expect("query");
        assert_eq!(page.rows.len(), 1);
        assert!(page.projected);
    }
    assert_eq!(
        counter(&client, "projected"),
        projected_before,
        "five queries walked no documents"
    );
    // …and it did ask for a sweep, which is what makes a cold workspace
    // converge because somebody looked rather than because somebody typed.
    assert!(
        counter_at(&client, "tasks", "sweeps_requested") >= 5,
        "the port asked, every time"
    );

    // **One edit projects exactly one document.**
    let second = editing
        .doc
        .insert(Parent::Node(page), Node::task("Send the deck").unwrap())
        .unwrap();
    tasks::set_due(&mut editing.doc, second, on("2026-09-06")).expect("due");
    editing.publish();
    wait_for("the second task to be answered", || {
        let page = client.query_tasks(&a_query()).expect("query");
        page.projected && page.rows.len() == 2
    });
    assert_eq!(
        counter(&client, "projected"),
        projected_before + 1,
        "one edited document, one projection"
    );

    // **Cold, after a restart.** Nothing re-projects a workspace until it is
    // used, which is what makes start-up cost the same on one workspace and a
    // thousand — so the first query after a restart is the one that has to
    // report rather than wait.
    drop(store);
    let store = Store::start(&root);
    let client = store.client();
    let first_answer = client.query_tasks(&a_query()).expect("query");
    if !first_answer.projected {
        assert_eq!(
            first_answer.updating_workspaces,
            vec![workspace.id],
            "a workspace it may be behind on is named rather than waited for"
        );
    }
    wait_for("the restarted store to converge", || {
        let page = client.query_tasks(&a_query()).expect("query");
        page.projected && page.rows.len() == 2
    });
}

/// A number out of `/v1/debug/projection`.
fn counter(client: &Client, name: &str) -> u64 {
    counter_at(client, "projection", name)
}

fn counter_at(client: &Client, route: &str, name: &str) -> u64 {
    client
        .debug(route)
        .unwrap_or_default()
        .get(name)
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
}

// ------------------------------------------------------------------ events

/// **This suite opens a real events worker, so its `no-sink` guard has to be
/// armed** (roadmap §3.5). Without the feature in `[dev-dependencies]` the
/// worker would construct the configured telemetry sink and post these fixtures
/// into the shared analytics project — and `cfg(test)` cannot say so, because a
/// dependency is never compiled with it (the `no-reader` lesson).
#[test]
fn this_suite_sends_no_telemetry() {
    assert!(!koru_events_server::forward::sends_by_default());
    assert!(koru_events_server::forward::configured_sink().is_none());
}

/// **A task's activity is readable per subject, with its actor and one
/// correlation across a cascade.**
///
/// *Half of this is deferred, and here is which half.* Task events are emitted
/// by the **host wrappers** — `koru-workspace-host` and `koru-mcp-server` —
/// neither of which exists in a store-server process: the store derives
/// document lifecycle events from its own feed and has no opinion about tasks.
/// So the emission half (that each command's intent is actually attempted by
/// both wrappers) is **T-2's**, and is asserted in the host crates where the
/// wrappers live. What is asserted here is the other half, which is the one
/// B-Gate's slice needs and the one nothing else covers end to end: that the
/// twelve registered events, journaled as a real worker journals them, come
/// back under **one subject**, carrying their actor and a shared correlation id
/// across a cascade.
#[test]
fn a_tasks_activity_reads_back_per_subject_with_its_actor_and_correlation() {
    use koru_events::{
        Actor, EmitContext, EventId, SubjectKind, TaskCompletedReason, TaskCreatedReason,
        TaskNodeKind, TaskReopenedReason, TaskSubject,
    };

    let dir = tempfile::tempdir().unwrap();
    let handle = koru_events_server::EventsHandle::open(koru_events_server::WorkerConfig::quiet(
        dir.path().join("events"),
    ))
    .unwrap();
    handle.seed_baseline(&[]).unwrap();

    let workspace = uuid::Uuid::new_v4();
    let document = uuid::Uuid::new_v4();
    let mine = TaskSubject {
        workspace,
        document,
        node: uuid::Uuid::new_v4(),
    };
    let theirs = TaskSubject {
        workspace,
        document,
        node: uuid::Uuid::new_v4(),
    };
    // The subject spelling is the same `task:<workspace>:<document>:<node>` the
    // query row carries and the reference parses — one string, three uses.
    let subject = mine.subject().id;

    let ctx = EmitContext::user_session();
    // The cascade: a child arriving reopens its ancestor, and every row carries
    // the id the *command* minted — which is what makes desktop and MCP group
    // one cascade identically.
    let created: koru_events::Draft = koru_events::TaskCreated {
        task: mine,
        reason: TaskCreatedReason::Explicit,
        node_kind: TaskNodeKind::Task,
    }
    .into();
    let created = created.actor(Actor::user()).into_envelope(&ctx);
    let chain = created.event_id;

    let completed: koru_events::Draft = koru_events::TaskCompleted {
        task: mine,
        reason: TaskCompletedReason::ExplicitComplete,
    }
    .into();
    let completed = completed
        .actor(Actor::user())
        .in_chain(chain)
        .into_envelope(&ctx);

    let reopened: koru_events::Draft = koru_events::TaskReopened {
        task: mine,
        reason: TaskReopenedReason::UnfinishedChildAdded,
    }
    .into();
    let reopened = reopened
        .actor(Actor::user())
        .in_chain(chain)
        .into_envelope(&ctx);

    // A second task's row, so "one subject" means something.
    let elsewhere: koru_events::Draft = koru_events::TaskCreated {
        task: theirs,
        reason: TaskCreatedReason::Due,
        node_kind: TaskNodeKind::Paragraph,
    }
    .into();
    let elsewhere = elsewhere.actor(Actor::user()).into_envelope(&ctx);

    handle
        .state
        .append(&[created, completed, reopened, elsewhere])
        .expect("append");

    let page = handle
        .state
        .with_journal(
            |journal| journal.read_subject(SubjectKind::Task, &subject, 0, 100),
            || koru_events::wire::EventPage {
                events: Vec::new(),
                next_after: 0,
            },
        )
        .expect("read");
    let names: Vec<&str> = page
        .events
        .iter()
        .map(|e| e.envelope.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["task.created", "task.completed", "task.reopened"],
        "one subject's own rows, oldest first, and nobody else's"
    );
    for stored in &page.events {
        assert_eq!(
            stored.envelope.actor,
            Actor::user(),
            "attributed to the person who acted"
        );
    }
    let roots: Vec<EventId> = page
        .events
        .iter()
        .map(|e| e.envelope.chain_root())
        .collect();
    assert!(
        roots.iter().all(|root| *root == chain),
        "the cascade shares one correlation: {roots:?}"
    );
    // The reopen carries SPEC §5's reason verbatim, which is the one token a
    // reader has to be able to tell from an explicit reopen.
    let reason = page.events[2]
        .envelope
        .attrs
        .get("reason")
        .and_then(|v| match v {
            koru_events::AttrValue::Token(t) => Some(t.as_str()),
            _ => None,
        });
    assert_eq!(reason, Some("unfinished_child_added"));
}

// ---------------------------------------------------------- the two sweeps

/// **The golden cases are run by three adapters, and this names them.**
///
/// Adapter equality is proven by *shared data-only cases* executed in each
/// crate that owns a door — the reference run in the domain, the seam run in
/// the editor, the wrapper run in MCP. Re-importing those crates here to run
/// them a fourth time would prove nothing and would make this suite depend on
/// the whole product; what the gate owes is that all three runs exist and are
/// still called what the roadmap says. Checked by reading the sources, so a
/// renamed or deleted run fails *here* rather than being noticed by nobody.
#[test]
fn the_golden_cases_are_run_by_three_named_adapters() {
    for (what, source, name) in [
        (
            "the koru-core reference run",
            include_str!("../../../crates/koru-core/src/tasks/golden/tests.rs"),
            "fn the_golden_cases_are_what_the_commands_do",
        ),
        (
            "the koru-editor seam run",
            include_str!("../../../crates/koru-editor/src/tasks/tests.rs"),
            "fn the_golden_cases_survive_the_editors_seam",
        ),
        (
            "the koru-mcp-server wrapper run",
            include_str!("../../../crates/koru-mcp-server/src/tasks/tests.rs"),
            "fn the_golden_cases_agree_through_this_hosts_wrapper",
        ),
    ] {
        assert!(source.contains(name), "{what} is gone or renamed: {name}");
    }
}

/// **Mobile has no task-mutating surface this release** (mutation-door
/// inventory, §2). The host renames and syncs; it does not complete, reopen,
/// mark, unmark or set a due on anything.
///
/// The mechanism is a source-level sweep of the crate's two files, compiled in
/// with `include_str!` — so it is checked at build time against the sources
/// themselves rather than against a list somebody has to remember to update,
/// and adding the first task call to that crate fails this test.
#[test]
fn mobile_has_no_task_mutating_surface() {
    let sources = [
        (
            "lib.rs",
            include_str!("../../../crates/koru-mobile-host/src/lib.rs"),
        ),
        (
            "protobuf.rs",
            include_str!("../../../crates/koru-mobile-host/src/protobuf.rs"),
        ),
    ];
    // Every canonical command, the two raw doors they wrap, and the marker key
    // — a mutation would have to name one of these.
    for (file, source) in sources {
        for forbidden in [
            "tasks::ensure_task",
            "tasks::set_due",
            "tasks::clear_due",
            "tasks::complete_task",
            "tasks::reopen_task",
            "tasks::remove_from_tasks",
            "tasks::reclassify",
            "set_done",
            "KEY_TASK",
            "koru.task",
        ] {
            assert!(
                !source.contains(forbidden),
                "koru-mobile-host/{file} names {forbidden}; mobile has no task-mutating \
                 surface this release, and gaining one is a decision rather than an edit"
            );
        }
    }
}
