// SPDX-License-Identifier: MPL-2.0

//! MCP semantics against a real Store daemon.

mod common;

use std::{
    path::PathBuf,
    process::{Child, Command},
    time::{Duration, Instant},
};

use koru_mcp_server::{Backend, LocalBackend, RequestContext};
use koru_store_client::Client;
use serde_json::{Value, json};

struct Daemon {
    child: Child,
    root: PathBuf,
}

impl Daemon {
    fn start() -> (tempfile::TempDir, Self) {
        let dir = tempfile::tempdir().unwrap();
        let daemon = Self::start_at(dir.path());
        (dir, daemon)
    }

    fn start_at(root: &std::path::Path) -> Self {
        Self {
            child: Self::spawn_at(root),
            root: root.to_path_buf(),
        }
    }

    fn spawn_at(root: &std::path::Path) -> Child {
        let mut command = Command::new(env!("CARGO_BIN_EXE_koru-store-server"));
        command
            .arg("serve")
            .env("KORU_STORE_NO_DIALOG", "1")
            .arg("--console")
            .arg("--root")
            .arg(root);
        common::configure(&mut command, root);
        let child = command.spawn().expect("spawn koru-store-server");
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if common::discovery(root).is_some() {
                break;
            }
            assert!(Instant::now() < deadline, "the store never started");
            std::thread::sleep(Duration::from_millis(20));
        }
        child
    }

    /// Stop this store and start another on the same files, as a rebuild or a
    /// `stop-all` does. The new process binds a new port and mints new session
    /// credentials, so nothing a client held before is any use afterwards.
    fn restart(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.child = Self::spawn_at(&self.root);
    }
}

fn backend(root: &std::path::Path) -> LocalBackend {
    let root = root.to_path_buf();
    LocalBackend::with_client_factory(std::sync::Arc::new(move || {
        let discovery = common::discovery(&root).ok_or_else(|| {
            koru_store_core::errors::Error::new(
                koru_store_core::errors::Code::StoreUnreachable,
                "standalone Store is not ready",
            )
        })?;
        let renewal_root = root.clone();
        let renewal: koru_store_client::Renewal = std::sync::Arc::new(move || {
            common::discovery(&renewal_root).ok_or_else(|| {
                koru_store_core::errors::Error::new(
                    koru_store_core::errors::Code::StoreUnreachable,
                    "standalone Store is not ready",
                )
            })
        });
        Ok(koru_api_client::Client::new_with_renewal(
            discovery, renewal,
        ))
    }))
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn invoke(backend: &LocalBackend, tool: &str, arguments: Value) -> Value {
    backend
        .invoke(&RequestContext::local(), tool, arguments)
        .unwrap_or_else(|error| panic!("{tool}: {error}"))
}

#[test]
fn extraction_creates_a_real_document_and_leaves_a_routed_inline_reference() {
    let (_dir, daemon) = Daemon::start();
    let client = Client::new(common::discovery(&daemon.root).unwrap());
    let profile = client
        .create_profile("MCP test", None, None, Some("en"))
        .unwrap();
    let workspace = client.create_workspace(profile.id, "Smoke").unwrap();
    let backend = backend(&daemon.root);

    let created = invoke(
        &backend,
        "create_document",
        json!({"workspace_id":workspace.id.to_storage(), "title":"Source"}),
    );
    let source_id = created["document_id"].as_str().unwrap();
    let read = invoke(
        &backend,
        "read_document",
        json!({"workspace_id":workspace.id.to_storage(), "document_id":source_id}),
    );
    let paragraph = read["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["kind"] == "paragraph")
        .unwrap()["node_id"]
        .as_str()
        .unwrap();
    let edited = invoke(
        &backend,
        "edit_document",
        json!({
            "workspace_id":workspace.id.to_storage(),
            "document_id":source_id,
            "base_revision":read["revision"],
            "operations":[{
                "op":"replace_text", "node_id":paragraph,
                "expected":"", "text":"Move me with formatting-safe semantics"
            }]
        }),
    );
    let extracted = invoke(
        &backend,
        "extract_to_document",
        json!({
            "workspace_id":workspace.id.to_storage(),
            "source_document_id":source_id,
            "base_revision":edited["revision"],
            "source_node_id":paragraph,
            "expected":"Move me with formatting-safe semantics",
            "new_document_title":"Extracted"
        }),
    );
    let target_id = extracted["document_id"].as_str().unwrap();

    let source = invoke(
        &backend,
        "read_document",
        json!({"workspace_id":workspace.id.to_storage(), "document_id":source_id}),
    );
    let link = source["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["node_id"] == extracted["source_link_node_id"])
        .expect("replacement link paragraph");
    assert_eq!(link["content"]["text"], "\u{2060}");
    assert_eq!(link["content"]["attributes"][0]["role"], "document");
    // A `/document` object is `AttrValue::Document` now, not a routed `Ref`:
    // it names the stable document and nothing about where that document
    // currently lives (ADR-127). `node_id` is absent because extraction links
    // the whole document rather than one block inside it.
    let value = &link["content"]["attributes"][0]["value"];
    assert_eq!(value["type"], "document");
    assert_eq!(value["document_id"], target_id);
    assert!(value["node_id"].is_null());

    let target = invoke(
        &backend,
        "read_document",
        json!({"workspace_id":workspace.id.to_storage(), "document_id":target_id}),
    );
    assert!(
        target["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| { node["content"]["text"] == "Move me with formatting-safe semantics" })
    );
    assert_eq!(client.documents(workspace.id).unwrap().len(), 2);
}

/// **A task command lands on a document no window has ever opened.**
///
/// `WorkspaceDoc::create` is the only writer of the `C_META` workspace id, and
/// the store never calls it: a document the store made arrives here as an empty
/// `LoroDoc`. Step 0 of every canonical task command's preflight is that read,
/// so a `koru.due` on such a document refused with
/// `Malformed("the document carries no workspace id")` — and every unit suite
/// passed, because they all build their fixture with `create`.
///
/// The desktop seam was stamped first; this is the same gap at the *bridge's*
/// own seam, which no window is involved in at all.
///
/// **The fixture is built the way the gap is, not the way the suites are.**
/// Every other document in this file comes from `create_document`, which goes
/// through `WorkspaceDoc::create` and so hides the whole thing. This one is a
/// snapshot of a bare `LoroDoc` with nodes in it and no id — exactly what an
/// unstamped desktop build wrote, and what the store hands back to anything that
/// opens it. `edit_document` is then the only seam under test, so stamping any
/// *other* one would not rescue it.
#[test]
fn a_due_lands_on_a_document_no_window_has_ever_opened() {
    use koru_core::{Node, Parent, WorkspaceDoc};

    let (_dir, daemon) = Daemon::start();
    let client = Client::new(common::discovery(&daemon.root).unwrap());
    let profile = client
        .create_profile("MCP due", None, None, Some("en"))
        .unwrap();
    let workspace = client.create_workspace(profile.id, "Smoke").unwrap();
    let backend = backend(&daemon.root);
    let workspace_id = workspace.id.to_storage();

    let empty = loro::LoroDoc::new();
    let mut seed =
        WorkspaceDoc::from_snapshot(&empty.export(loro::ExportMode::Snapshot).unwrap()).unwrap();
    let page = seed
        .insert(Parent::Root, Node::document("Never opened").unwrap())
        .unwrap();
    let row = seed
        .insert(Parent::Node(page), Node::paragraph("Ship it").unwrap())
        .unwrap();
    assert!(
        seed.id().is_none(),
        "the fixture is only worth anything while it carries no id"
    );
    let document_id = client
        .import_document(
            workspace.id,
            "Never opened",
            koru_core::explore::KIND_DOCUMENT,
            seed.export_snapshot().unwrap(),
        )
        .unwrap();
    let document = document_id.to_storage();
    let row = row.to_storage();

    let read = invoke(
        &backend,
        "read_document",
        json!({"workspace_id":workspace_id, "document_id":document}),
    );

    // The refusal this test exists for is `invalid_request` carrying the
    // model's own words, so `invoke`'s panic would name it.
    invoke(
        &backend,
        "edit_document",
        json!({
            "workspace_id":workspace_id,
            "document_id":document,
            "base_revision":read["revision"],
            "operations":[{
                "op":"set_attribute", "node_id":row, "expected":"Ship it",
                "key":"koru.due", "value":{"type":"date", "value":"2026-09-04"}
            }]
        }),
    );

    let after = invoke(
        &backend,
        "read_document",
        json!({"workspace_id":workspace_id, "document_id":document}),
    );
    let node = after["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["node_id"] == row.as_str())
        .expect("the row survived");
    let attrs = node["content"]["attributes"]
        .as_array()
        .expect("attributes");
    let value = |key: &str| {
        attrs
            .iter()
            .find(|attr| attr["role"] == key)
            .map(|attr| attr["value"].clone())
    };
    assert_eq!(
        value("koru.due"),
        Some(json!({"type":"date", "value":"2026-09-04"})),
        "the due landed: {node}"
    );
    // And the row became a task, which is the other half of what a due means.
    assert!(
        value("koru.task").is_some(),
        "a due makes the row a task: {node}"
    );

    // The stamp reached the *store*, not only the bridge's copy — which is what
    // makes `capture`'s frontiers findable and what the next process to open
    // this document reads.
    let reopened =
        WorkspaceDoc::from_snapshot(&client.snapshot(workspace.id, document_id).unwrap()).unwrap();
    assert_eq!(
        reopened.id(),
        Some(koru_core::WorkspaceId::from_uuid(workspace.id.as_uuid())),
        "the write seam committed the workspace the request named"
    );
}

#[test]
fn public_mcp_ordering_uses_sibling_ids_and_rejects_numeric_ordinals() {
    let (_dir, daemon) = Daemon::start();
    let client = Client::new(common::discovery(&daemon.root).unwrap());
    let profile = client
        .create_profile("MCP ordering", None, None, Some("en"))
        .unwrap();
    let workspace = client.create_workspace(profile.id, "Smoke").unwrap();
    let backend = backend(&daemon.root);
    let workspace_id = workspace.id.to_storage();
    let created = invoke(
        &backend,
        "create_document",
        json!({"workspace_id":workspace_id, "title":"Ordering"}),
    );
    let document_id = created["document_id"].as_str().unwrap();
    let read = invoke(
        &backend,
        "read_document",
        json!({"workspace_id":workspace_id, "document_id":document_id}),
    );
    let root = read["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["kind"] == "document")
        .unwrap()["node_id"]
        .as_str()
        .unwrap();
    let a = read["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["kind"] == "paragraph")
        .unwrap()["node_id"]
        .as_str()
        .unwrap();
    let b = uuid::Uuid::new_v4().to_string();
    let c = uuid::Uuid::new_v4().to_string();
    let edited = invoke(
        &backend,
        "edit_document",
        json!({
            "workspace_id":workspace_id,
            "document_id":document_id,
            "base_revision":read["revision"],
            "operations":[
                {"op":"replace_text", "node_id":a, "expected":"", "text":"A"},
                {"op":"insert", "node_id":b, "parent_id":root, "after_node_id":a,
                 "kind":"paragraph", "text":"B"},
                {"op":"insert", "node_id":c, "parent_id":root, "after_node_id":b,
                 "kind":"paragraph", "text":"C"}
            ]
        }),
    );

    let ordinal_error = backend
        .invoke(
            &RequestContext::local(),
            "edit_document",
            json!({
                "workspace_id":workspace_id,
                "document_id":document_id,
                "base_revision":edited["revision"],
                "operations":[{"op":"move", "node_id":a, "expected":"A", "parent_id":root, "ordinal":2}]
            }),
        )
        .expect_err("ordinal must be rejected, never ignored");
    assert_eq!(ordinal_error.code, "invalid_request");
    assert!(ordinal_error.message.contains("unknown field `ordinal`"));

    invoke(
        &backend,
        "edit_document",
        json!({
            "workspace_id":workspace_id,
            "document_id":document_id,
            "base_revision":edited["revision"],
            "operations":[{
                "op":"move", "node_id":a, "expected":"A", "parent_id":root, "after_node_id":c
            }]
        }),
    );
    let reordered = invoke(
        &backend,
        "read_document",
        json!({"workspace_id":workspace_id, "document_id":document_id}),
    );
    let body: Vec<&str> = reordered["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["parent_id"] == root)
        .map(|node| node["content"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(body, vec!["B", "C", "A"]);
}

#[test]
fn one_call_creation_is_complete_exactly_readable_and_idempotent() {
    let (_dir, daemon) = Daemon::start();
    let client = Client::new(common::discovery(&daemon.root).unwrap());
    let profile = client
        .create_profile("MCP seeded create", None, None, Some("en"))
        .unwrap();
    let workspace = client.create_workspace(profile.id, "Smoke").unwrap();
    let backend = backend(&daemon.root);
    let workspace_id = workspace.id.to_storage();

    // The old, minimal spelling remains valid and now returns the exact same
    // projection as a read, including its root and natural first-edit target.
    let target = invoke(
        &backend,
        "create_document",
        json!({"workspace_id":workspace_id, "title":"Link target"}),
    );
    let target_read = invoke(
        &backend,
        "read_document",
        json!({
            "workspace_id":workspace_id,
            "document_id":target["document_id"]
        }),
    );
    assert_eq!(target, target_read);
    assert!(target["root_node_id"].is_string());
    assert!(
        target["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["kind"] == "paragraph" && node["content"]["text"] == "")
    );

    let root = uuid::Uuid::new_v4().to_string();
    let first = uuid::Uuid::new_v4().to_string();
    let second = uuid::Uuid::new_v4().to_string();
    let key = uuid::Uuid::new_v4().to_string();
    let request = json!({
        "workspace_id":workspace_id,
        "title":"Complete in one call",
        "idempotency_key":key,
        "root_node_id":root,
        "initial_operations":[
            {
                "op":"insert", "node_id":first, "parent_id":root,
                "after_node_id":null, "kind":"paragraph", "text":"First"
            },
            {
                "op":"insert", "node_id":second, "parent_id":root,
                "after_node_id":first, "kind":"paragraph", "text":"See target "
            },
            {
                "op":"insert_inline", "node_id":second, "expected":"See target ",
                "at_utf8":11,
                "inline":{"type":"document", "document_id":target["document_id"]}
            }
        ]
    });
    let created = invoke(&backend, "create_document", request.clone());
    assert_eq!(created["root_node_id"], root);
    assert_eq!(created["nodes"].as_array().unwrap().len(), 3);
    assert_eq!(created["nodes"][1]["node_id"], first);
    assert_eq!(created["nodes"][2]["node_id"], second);
    assert_eq!(
        created["nodes"][2]["content"]["text"],
        "See target \u{2060}"
    );
    assert_eq!(
        created["nodes"][2]["content"]["attributes"][0]["role"],
        "document"
    );

    let read = invoke(
        &backend,
        "read_document",
        json!({
            "workspace_id":workspace_id,
            "document_id":created["document_id"]
        }),
    );
    assert_eq!(created, read, "create must be the first read projection");

    let replay = invoke(&backend, "create_document", request.clone());
    assert_eq!(replay, created, "a retry must replay the original success");
    assert_eq!(client.documents(workspace.id).unwrap().len(), 2);

    let mut different = request;
    different["title"] = json!("Different request");
    let conflict = backend
        .invoke(&RequestContext::local(), "create_document", different)
        .expect_err("one key cannot create two different documents");
    assert_eq!(conflict.code, "idempotency_conflict");
}

/// **A bridge outlives the store it dialled.**
///
/// `koru-mcp` is started once by an MCP host and left running for a day. The
/// store under it is restarted routinely — a rebuild needs the exe unlocked,
/// `stop-all` takes it down, it can crash — and a new process binds a new port
/// and mints new session credentials, so the connection the bridge cached is
/// worth nothing afterwards. It used to keep that dead handle and answer
/// `store_unreachable` to everything until a person noticed and restarted the
/// bridge itself.
///
/// Documents already recovered on their own, because `StoreSession::join`
/// re-discovers per call. It was the management client — listing, history,
/// creation — that did not, which is why the failure looked partial.
#[test]
fn management_calls_survive_the_store_restarting_underneath_them() {
    let (_dir, mut daemon) = Daemon::start();
    let client = Client::new(common::discovery(&daemon.root).unwrap());
    let profile = client
        .create_profile("MCP restart", None, None, Some("en"))
        .unwrap();
    let workspace = client.create_workspace(profile.id, "Smoke").unwrap();
    let backend = backend(&daemon.root);
    let workspace_id = workspace.id.to_storage();

    let before = invoke(
        &backend,
        "create_document",
        json!({"workspace_id":workspace_id, "title":"Written before"}),
    );
    assert!(invoke(&backend, "list_workspaces", json!({}))["workspaces"].is_array());

    daemon.restart();

    // Every kind of call the bridge makes: a listing, a read through a joined
    // session, and a mutation that captures history.
    assert_eq!(
        invoke(&backend, "list_workspaces", json!({}))["workspaces"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let listed = invoke(
        &backend,
        "list_documents",
        json!({"workspace_id":workspace_id}),
    );
    assert_eq!(listed["documents"].as_array().unwrap().len(), 1);
    let read = invoke(
        &backend,
        "read_document",
        json!({"workspace_id":workspace_id, "document_id":before["document_id"]}),
    );
    assert_eq!(read["document_id"], before["document_id"]);
    let after = invoke(
        &backend,
        "create_document",
        json!({"workspace_id":workspace_id, "title":"Written after"}),
    );
    assert_ne!(after["document_id"], before["document_id"]);
    assert_eq!(
        invoke(
            &backend,
            "list_document_versions",
            json!({"workspace_id":workspace_id, "document_id":after["document_id"]})
        )["entries"]
            .as_array()
            .unwrap()
            .is_empty(),
        false,
        "the create after the restart still captured its history"
    );
}

/// A document made under another document, which is what *nest this under that*
/// means in Explore. The manifest carries the relationship; nothing is inserted
/// into the parent document's own tree.
#[test]
fn a_created_document_can_be_nested_under_another() {
    let (_dir, daemon) = Daemon::start();
    let client = Client::new(common::discovery(&daemon.root).unwrap());
    let profile = client
        .create_profile("MCP nesting", None, None, Some("en"))
        .unwrap();
    let workspace = client.create_workspace(profile.id, "Smoke").unwrap();
    let backend = backend(&daemon.root);
    let workspace_id = workspace.id.to_storage();

    let parent = invoke(
        &backend,
        "create_document",
        json!({"workspace_id":workspace_id, "title":"Parent"}),
    );
    let child = invoke(
        &backend,
        "create_document",
        json!({
            "workspace_id":workspace_id,
            "title":"Child",
            "parent_document_id":parent["document_id"]
        }),
    );

    let listed = invoke(
        &backend,
        "list_documents",
        json!({"workspace_id":workspace_id}),
    );
    let entry = listed["documents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == child["document_id"])
        .expect("the child is in the manifest");
    assert_eq!(entry["parent"], parent["document_id"]);

    // The parent's own document is untouched: nesting is a manifest fact, not
    // a node inside the parent's tree.
    let parent_read = invoke(
        &backend,
        "read_document",
        json!({"workspace_id":workspace_id, "document_id":parent["document_id"]}),
    );
    assert_eq!(parent_read["nodes"].as_array().unwrap().len(), 2);

    // A parent that does not exist is refused rather than quietly appended to
    // the workspace root.
    let orphan = backend
        .invoke(
            &RequestContext::local(),
            "create_document",
            json!({
                "workspace_id":workspace_id,
                "title":"Nowhere",
                "parent_document_id":uuid::Uuid::new_v4().to_string()
            }),
        )
        .expect_err("an unknown parent is not a workspace root");
    assert_eq!(orphan.code, "document_not_found");
}

/// **The edit door and the list door agree about one task.**
///
/// A `koru.due` written through `edit_document` makes the row a task; that fact
/// then has to travel a completely different path to be read back — through the
/// projector into `projection.db`, out through `POST /v1/tasks/query`, and into
/// `list_tasks`'s envelope. Every step is separately unit-tested and none of
/// those tests can see the seams between them: the row is written by
/// `koru-core`, indexed by `koru-projection`, paged by the daemon and mapped
/// by the bridge, and a name that drifts anywhere in that chain leaves each
/// suite green.
///
/// The two things worth asserting here rather than anywhere else are the
/// **reference** — `task:<workspace>:<document>:<node>`, built by the store out
/// of ids the bridge only sees separately — and the **due**, which has to come
/// back in the exact shape `set_attribute` took, or an agent that lists a task
/// and moves its deadline has to invent a second spelling of a date.
#[test]
fn a_task_created_through_the_edit_door_is_listed_with_its_reference_and_its_due() {
    let (_dir, daemon) = Daemon::start();
    let client = Client::new(common::discovery(&daemon.root).unwrap());
    let profile = client
        .create_profile("MCP tasks", None, None, Some("en"))
        .unwrap();
    let workspace = client.create_workspace(profile.id, "Acme").unwrap();
    let backend = backend(&daemon.root);
    let workspace_id = workspace.id.to_storage();

    let created = invoke(
        &backend,
        "create_document",
        json!({"workspace_id":workspace_id, "title":"Plans"}),
    );
    let document_id = created["document_id"].as_str().unwrap().to_owned();
    let row = created["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["kind"] == "paragraph")
        .expect("the blank row a new document opens with")["node_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let named = invoke(
        &backend,
        "edit_document",
        json!({
            "workspace_id":workspace_id,
            "document_id":document_id,
            "base_revision":created["revision"],
            "operations":[{
                "op":"replace_text", "node_id":row,
                "expected":"", "text":"Ship the report"
            }]
        }),
    );
    // The due is what makes it a task: there is no `create_task` here, and
    // nothing above asked for one.
    invoke(
        &backend,
        "edit_document",
        json!({
            "workspace_id":workspace_id,
            "document_id":document_id,
            "base_revision":named["revision"],
            "operations":[{
                "op":"set_attribute", "node_id":row, "expected":"Ship the report",
                "key":"koru.due", "value":{"type":"date", "value":"2026-09-04"}
            }, {
                "op":"set_attribute", "node_id":row, "expected":"Ship the report",
                "key":"koru.priority", "value":{"type":"text", "value":"high"}
            }]
        }),
    );

    // The read model is debounced, so the row is waited for rather than
    // assumed — `updating_workspaces` is the store saying the same thing.
    let deadline = Instant::now() + Duration::from_secs(30);
    let listed = loop {
        let page = invoke(&backend, "list_tasks", json!({}));
        if page["tasks"].as_array().is_some_and(|rows| rows.len() == 1) {
            break page;
        }
        assert!(
            Instant::now() < deadline,
            "the task never reached the read model: {page}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };

    let task = &listed["tasks"][0];
    assert_eq!(
        task["task"],
        json!(format!("task:{workspace_id}:{document_id}:{row}")),
        "the reference is the triple, spelled by the store: {task}"
    );
    assert_eq!(task["workspace_id"], json!(workspace_id));
    assert_eq!(task["document_id"], json!(document_id));
    assert_eq!(task["node_id"], json!(row));
    assert_eq!(task["title"], "Ship the report", "the one title chain");
    assert_eq!(task["node_kind"], "paragraph");
    assert_eq!(
        task["lifecycle"], "open",
        "absence of a stored state is open"
    );
    assert_eq!(
        task["due"],
        json!({"type":"date", "value":"2026-09-04"}),
        "the shape `set_attribute` took, unchanged: {task}"
    );
    assert_eq!(task["priority"], "high");
    assert!(task["list"].is_null(), "it is in no list: {task}");
    assert_eq!(task["document_trashed"], json!(false));
    assert_eq!(task["in_trash"], json!(false));
    assert!(
        task["breadcrumb"]
            .as_str()
            .is_some_and(|crumb| crumb.contains("Acme") && crumb.contains("Plans")),
        "workspace, then the document path: {task}"
    );
    assert!(listed["next_after"].is_null(), "one row is the whole list");
    assert_eq!(listed["restarted"], json!(false));

    // And the filters reach the store rather than being decoration: this task
    // is open, so asking for completed ones answers nothing.
    let completed = invoke(&backend, "list_tasks", json!({"lifecycle":"completed"}));
    assert_eq!(completed["tasks"], json!([]), "{completed}");

    // A deadline window stated over instants alone does not answer for a due
    // stored as a calendar date — the same rule a search attribute filter has.
    let by_instant = invoke(
        &backend,
        "list_tasks",
        json!({"due":{"instants":{"before_ms":1_788_998_400_000_i64}}}),
    );
    assert_eq!(by_instant["tasks"], json!([]), "{by_instant}");
    let by_date = invoke(
        &backend,
        "list_tasks",
        json!({"due":{"dates":{"before":"2026-09-05"}}}),
    );
    assert_eq!(
        by_date["tasks"].as_array().map(Vec::len),
        Some(1),
        "{by_date}"
    );

    // The house rule holds at this seam too: a misspelled filter is refused
    // rather than dropped, which would answer a different question.
    let refused = backend
        .invoke(
            &RequestContext::local(),
            "list_tasks",
            json!({"minimum_priority":"high"}),
        )
        .expect_err("an unknown filter is not silently discarded");
    assert_eq!(refused.code, "invalid_request");
}
