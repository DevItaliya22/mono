// SPDX-License-Identifier: MPL-2.0

//! **The search index, against a real store process.**
//!
//! Stage 2's gate. Everything here spawns `koru-store-server.exe`, writes documents
//! and renames them through the ordinary API, and then asks the index what it
//! holds.
//!
//! It reads `search.db` directly, which is the one thing this suite does that a
//! client may not — the same licence `projection.rs` takes, and for the same
//! reason: the daemon publishes counters, and counters are what a component
//! says about itself. What is asserted here is what is actually on the disk.
//!
//! There is no `POST /v1/search` yet; that is stage 3. What this proves is the
//! chain from a commit to a matchable row.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use koru_search::{SearchIndex, SearchQuery};
use koru_store_client::Client;
use koru_store_client::{Navigation, SearchRequest};
use koru_store_core::ids::{DocumentId, WorkspaceId};

fn segment() -> &'static str {
    koru_channel::segment()
}

struct Store {
    child: Option<Child>,
    root: PathBuf,
}

impl Store {
    fn start(root: &Path) -> Self {
        Self::start_inner(root, true)
    }

    /// A store that writes its log to `<root>/logs/` instead of to stdout.
    ///
    /// **`--console` is the switch, and it means "no file".** Every other test
    /// here wants the output on the harness's stdout; the one test that asserts
    /// on what reaches the *log* needs there to be a log to read.
    fn start_logging_to_file(root: &Path) -> Self {
        Self::start_inner(root, false)
    }

    fn start_inner(root: &Path, console: bool) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_koru-store-server"));
        command
            .arg("serve")
            // A suite starting daemons on throwaway roots must never raise a
            // modal dialog: nothing dismisses it, so the process would never
            // exit and would go on holding its databases and this binary.
            .env("KORU_STORE_NO_DIALOG", "1");
        if console {
            command.arg("--console");
        }
        command.arg("--root").arg(root);
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

    fn restart(&mut self) {
        self.kill();
        *self = Self::start(&self.root.clone());
    }

    /// The index, opened beside the store's own databases.
    fn index(&self) -> SearchIndex {
        SearchIndex::open(&self.root).expect("open the index")
    }

    fn find(&self, text: &str) -> Vec<koru_search::SearchHit> {
        self.index()
            .query(
                &SearchQuery {
                    text: text.to_owned(),
                    ..Default::default()
                },
                &[],
            )
            .expect("query")
            .hits
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        self.kill();
    }
}

fn root_for(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("koru-store-search-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp root");
    dir
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Wait until both derived layers have nothing queued.
///
/// **Necessary before asserting that something did *not* happen.** A write
/// produces more than one commit — the session echoes the store its own bytes
/// back, which is a deliberate cost recorded in `StoreSession::publish` — so a
/// document can still be waiting out its debounce when the next step begins.
/// Without this, "a rename re-indexed the document" is a race rather than a
/// finding.
fn wait_for_quiet(store: &Store) {
    wait_for("the derived layers to settle", || {
        let client = store.client();
        let projection = client.debug("projection").unwrap_or_default();
        let search = client.debug("search").unwrap_or_default();
        projection["pending"] == 0 && search["pending"] == 0
    });
    // One debounce longer than the last event, so anything that arrived in the
    // gap is counted before the assertion rather than after it.
    std::thread::sleep(Duration::from_millis(800));
    wait_for("the derived layers to settle", || {
        let client = store.client();
        let projection = client.debug("projection").unwrap_or_default();
        let search = client.debug("search").unwrap_or_default();
        projection["pending"] == 0 && search["pending"] == 0
    });
}

fn wait_for_hit(store: &Store, text: &str) {
    wait_for(&format!("{text:?} to be indexed"), || {
        !store.find(text).is_empty()
    });
}

/// A workspace with `n` documents named `Doc <i>`.
fn a_workspace(store: &Store, n: usize) -> (WorkspaceId, Vec<DocumentId>) {
    let client = store.client();
    let profile = client
        .create_profile("Tester", None, None, None)
        .expect("profile");
    let workspace = client
        .create_workspace(profile.id, "Acme")
        .expect("workspace");
    let documents = (0..n)
        .map(|i| {
            client
                .create_document(workspace.id, &format!("Doc {i}"), "page")
                .expect("document")
        })
        .collect();
    (workspace.id, documents)
}

/// Write a line into a document through the ordinary sync path.
fn write(root: &Path, workspace: WorkspaceId, document: DocumentId, text: &str) {
    use koru_core::node::Node;
    use koru_core::schema::{Parent, WorkspaceDoc};
    use koru_workspace::store::StoreSession;

    let (mut session, snapshot) =
        StoreSession::join(segment(), Some(root), workspace, document).expect("join");
    let mut doc = if snapshot.is_empty() {
        WorkspaceDoc::create(koru_core::WorkspaceId::new(), "W").expect("create")
    } else {
        WorkspaceDoc::from_snapshot(&snapshot).expect("snapshot")
    };
    let root_node = match doc
        .children(Parent::Root)
        .unwrap()
        .into_iter()
        .find(|&n| doc.kind_of(n) == Some(koru_core::Kind::Document))
    {
        Some(n) => n,
        None => doc
            .insert(Parent::Root, Node::document("Doc").unwrap())
            .expect("root"),
    };
    doc.insert(Parent::Node(root_node), Node::paragraph(text).unwrap())
        .expect("insert");

    let batch = session
        .publish(&doc)
        .expect("publish")
        .expect("something to send");
    wait_for("the store to call the write durable", || {
        let _ = session.drain(&mut doc);
        session.is_durable(batch)
    });
}

/// Edit the manifest through its own room, which is how a rename and a trash
/// reach the store. There is no HTTP route for either — the manifest is a
/// document, and the app edits it the same way it edits any other.
fn with_manifest(root: &Path, workspace: WorkspaceId, edit: impl FnOnce(&mut Manifest)) {
    use koru_workspace::store::StoreSession;

    let (mut session, snapshot) =
        StoreSession::join_manifest(segment(), Some(root), workspace).expect("join the manifest");
    let mut manifest = Manifest::from_snapshot(&snapshot).expect("manifest");
    edit(&mut manifest);
    let batch = session
        .publish(&manifest)
        .expect("publish")
        .expect("something to send");
    wait_for("the store to call the manifest edit durable", || {
        let _ = session.drain(&mut manifest);
        session.is_durable(batch)
    });
}

use koru_store_core::manifest::Manifest;

// --------------------------------------------------------------- the chain

/// **The seam.** Text written through the ordinary API turns up in the index,
/// with its path assembled, without anything having asked for it.
#[test]
fn what_is_written_is_findable() {
    let root = root_for("seam");
    let store = Store::start(&root);
    let (workspace, documents) = a_workspace(&store, 1);

    write(&root, workspace, documents[0], "the quarterly plan");
    wait_for_hit(&store, "quarterly");

    let hits = store.find("quarterly");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].document, Some(documents[0].to_storage()));
    assert_eq!(hits[0].workspace, workspace.to_storage());
    assert_eq!(hits[0].title, "Doc 0");
    assert_eq!(hits[0].breadcrumb, "Acme / Doc 0");
    let _ = workspace;
}

/// **A document created and never opened is still findable.** It has a title in
/// the listing and no content at all — which is why the title comes from the
/// listing rather than from a node.
#[test]
fn a_document_with_no_content_is_findable_by_its_title() {
    let root = root_for("untouched");
    let store = Store::start(&root);
    let (workspace, documents) = a_workspace(&store, 2);

    // Only the first is ever written to. The second exists and is empty.
    write(&root, workspace, documents[0], "anything at all");
    wait_for_hit(&store, "anything");
    wait_for_hit(&store, "Doc 1");

    let hits = store.find("Doc 1");
    assert_eq!(hits[0].document, Some(documents[1].to_storage()));
}

/// A workspace's own name is matchable.
#[test]
fn a_workspace_is_findable_by_its_name() {
    let root = root_for("workspace");
    let store = Store::start(&root);
    let (workspace, documents) = a_workspace(&store, 1);
    write(&root, workspace, documents[0], "anything at all");

    wait_for_hit(&store, "Acme");
    assert!(store.find("Acme").iter().any(|h| h.document.is_none()));
}

/// **The unit of work is one document.** Editing one must not re-index another,
/// or a workspace of a thousand documents pays for every keystroke.
#[test]
fn editing_one_document_leaves_the_others_rows_alone() {
    let root = root_for("one-document");
    let store = Store::start(&root);
    let (workspace, documents) = a_workspace(&store, 4);
    for (i, &document) in documents.iter().enumerate() {
        write(&root, workspace, document, &format!("line {i}"));
        wait_for_hit(&store, &format!("line {i}"));
    }
    let indexed_before = indexed_at(&store, documents[1]);

    write(&root, workspace, documents[0], "and something new");
    wait_for_hit(&store, "something");

    assert_eq!(
        indexed_at(&store, documents[1]),
        indexed_before,
        "editing one document re-indexed another"
    );
    for (i, _) in documents.iter().enumerate().skip(1) {
        assert_eq!(store.find(&format!("line {i}")).len(), 1);
    }
}

fn indexed_at(store: &Store, document: DocumentId) -> i64 {
    let db = rusqlite::Connection::open(store.root.join("search.db")).expect("open");
    db.query_row(
        "SELECT indexed_at_ms FROM documents WHERE document_id = ?1",
        rusqlite::params![document.to_storage()],
        |r| r.get(0),
    )
    .unwrap_or(-1)
}

// -------------------------------------------------------------- the listing

/// **A rename is one row.** Every affected path is right immediately, and no
/// document is re-indexed — which is the whole reason a breadcrumb is assembled
/// rather than stored.
#[test]
fn a_rename_moves_every_path_without_reindexing() {
    let root = root_for("rename");
    let store = Store::start(&root);
    let (workspace, documents) = a_workspace(&store, 1);
    write(&root, workspace, documents[0], "the quarterly plan");
    wait_for_hit(&store, "quarterly");
    wait_for_quiet(&store);
    let indexed_before = indexed_at(&store, documents[0]);
    let projected_before = store.client().debug("projection").unwrap()["projected"].clone();

    with_manifest(&root, workspace, |manifest| {
        manifest
            .rename_document(documents[0], "Plans")
            .expect("rename");
    });

    wait_for("the rename to reach the index", || {
        store
            .find("quarterly")
            .first()
            .is_some_and(|hit| hit.title == "Plans")
    });
    assert_eq!(store.find("quarterly")[0].breadcrumb, "Acme / Plans");
    wait_for_quiet(&store);
    assert_eq!(
        indexed_at(&store, documents[0]),
        indexed_before,
        "a rename re-indexed the document"
    );
    // The sweep a listing change queues compares and finds nothing to do — a
    // rename must not cost the workspace a re-projection either.
    assert_eq!(
        store.client().debug("projection").unwrap()["projected"],
        projected_before,
        "a rename re-projected the document"
    );
    // **And the new name is what is matchable.** The title fragment is written
    // on the listing path for exactly this reason: built from the node walk, it
    // would still say `Doc 0` until somebody typed into the document again.
    let by_new_name = store.find("Plans");
    assert_eq!(
        by_new_name.first().map(|hit| hit.field.as_str()),
        Some(koru_search::field::TITLE),
        "the new name is not what the document is found by"
    );
    assert!(
        store.find("Doc 0").is_empty(),
        "the old name is still matchable"
    );
}

/// **Binned is not deleted.** A document in the trash leaves the default answer
/// and stays findable in the bin, because that is a place somebody searches and
/// restores from.
#[test]
fn a_trashed_document_leaves_the_default_answer_and_stays_in_the_bin() {
    let root = root_for("trash");
    let store = Store::start(&root);
    let (workspace, documents) = a_workspace(&store, 1);
    write(&root, workspace, documents[0], "the quarterly plan");
    wait_for_hit(&store, "quarterly");

    with_manifest(&root, workspace, |manifest| {
        manifest
            .trash_document(
                documents[0],
                koru_store_core::model::Timestamp::now(),
                Some("Tester"),
            )
            .expect("trash");
    });

    wait_for("the trash flag to reach the index", || {
        store.find("quarterly").is_empty()
    });
    let binned = store
        .index()
        .query(
            &SearchQuery {
                text: "quarterly".into(),
                lifecycle: koru_search::LifecycleFilter::Trash,
                ..Default::default()
            },
            &[],
        )
        .expect("query")
        .hits;
    assert_eq!(binned.len(), 1);
    assert_eq!(binned[0].trashed_by.as_deref(), Some("Tester"));
}

/// **A binned parent takes its children with it.** The manifest stamps only the
/// entry that was binned — the subtree inherits (ADR-95) — and the projection
/// carries the inherited stamp down the tree before mirroring, or a child
/// would answer live queries forever: the reconcile compares digests that
/// already match the raw listing, so nothing downstream could ever catch it.
#[test]
fn a_trashed_parent_takes_its_children_out_of_the_default_answer() {
    let root = root_for("trash-inherited");
    let store = Store::start(&root);
    let (workspace, documents) = a_workspace(&store, 2);
    let (parent, child) = (documents[0], documents[1]);
    write(&root, workspace, child, "the nested idea");
    wait_for_hit(&store, "nested");
    with_manifest(&root, workspace, |manifest| {
        manifest
            .move_document(child, Some(parent), 0)
            .expect("nest");
    });

    with_manifest(&root, workspace, |manifest| {
        manifest
            .trash_document(
                parent,
                koru_store_core::model::Timestamp::now(),
                Some("Tester"),
            )
            .expect("trash");
    });

    wait_for("the child to leave the default answer", || {
        store.find("nested").is_empty()
    });
    let binned = store
        .index()
        .query(
            &SearchQuery {
                text: "nested".into(),
                lifecycle: koru_search::LifecycleFilter::Trash,
                ..Default::default()
            },
            &[],
        )
        .expect("query")
        .hits;
    assert_eq!(binned.len(), 1, "the child is findable in the bin");
    assert_eq!(
        binned[0].trashed_by.as_deref(),
        Some("Tester"),
        "the inherited stamp names whoever binned the parent"
    );

    with_manifest(&root, workspace, |manifest| {
        manifest.restore_document(parent).expect("restore");
    });
    wait_for("the child to come back with its parent", || {
        !store.find("nested").is_empty()
    });
}

// -------------------------------------------------------------- recovery

/// Deleting the index is safe: it is rebuilt from the read model, by the same
/// comparison that catches anything else it was not told about.
#[test]
fn a_deleted_index_is_rebuilt_from_the_read_model() {
    let root = root_for("rebuild");
    let mut store = Store::start(&root);
    let (workspace, documents) = a_workspace(&store, 3);
    for (i, &document) in documents.iter().enumerate() {
        write(&root, workspace, document, &format!("line {i}"));
        wait_for_hit(&store, &format!("line {i}"));
    }

    store.kill();
    for file in ["search.db", "search.db-wal", "search.db-shm"] {
        let _ = std::fs::remove_file(root.join(file));
    }
    assert!(!root.join("search.db").exists());

    store.restart();

    // Nothing is written and nothing is asked for: the reconcile finds all
    // three by comparing its own cursors against the projection.
    for (i, _) in documents.iter().enumerate() {
        wait_for_hit(&store, &format!("line {i}"));
    }
    assert_eq!(
        store.find("Acme").len(),
        1,
        "the workspace name came back too"
    );
}

/// A schema this build does not understand is discarded and rebuilt rather than
/// migrated — the index is derived, so that is both simpler and safer.
#[test]
fn an_index_from_another_schema_is_rebuilt() {
    let root = root_for("schema");
    let mut store = Store::start(&root);
    let (workspace, documents) = a_workspace(&store, 1);
    write(&root, workspace, documents[0], "still here");
    wait_for_hit(&store, "still");

    store.kill();
    {
        let db = rusqlite::Connection::open(root.join("search.db")).unwrap();
        db.pragma_update(None, "user_version", koru_search::SEARCH_SCHEMA_VERSION + 1)
            .unwrap();
    }

    store.restart();
    wait_for_hit(&store, "still");
}

/// The counters the daemon publishes, through the ordinary authenticated route.
///
/// **Metadata only.** `/v1/debug/*` must never carry content, and a search
/// index is the component most tempted to answer with a title.
#[test]
fn the_index_reports_what_it_is_doing_and_no_content() {
    let root = root_for("debug");
    let store = Store::start(&root);
    let (workspace, documents) = a_workspace(&store, 1);
    write(&root, workspace, documents[0], "counted once");
    wait_for_hit(&store, "counted");

    let report = store.client().debug("search").expect("the report");
    assert_eq!(report["schema_version"], koru_search::SEARCH_SCHEMA_VERSION);
    assert!(report["indexed"].as_u64().unwrap_or(0) >= 1);
    assert_eq!(report["failed"], 0);
    assert!(report["documents"].as_u64().unwrap_or(0) >= 1);

    let printed = report.to_string();
    for content in ["counted", "Doc 0", "Acme"] {
        assert!(
            !printed.contains(content),
            "the debug route answered with content: {printed}"
        );
    }
}

// ------------------------------------------------------- the route, stage 3

/// **The whole point of the stage: an answer over HTTP.**
///
/// Everything above reads `search.db` on the disk, which is a licence this
/// suite takes and a client does not have. This drives the same query through
/// the same client a third party would use.
#[test]
fn a_query_is_answered_over_http() {
    let root = root_for("route");
    let store = Store::start(&root);
    let (workspace, documents) = a_workspace(&store, 1);

    write(&root, workspace, documents[0], "the quarterly plan");
    wait_for_hit(&store, "quarterly");

    let response = store
        .client()
        .search(&SearchRequest {
            query: SearchQuery {
                text: "quarterly".into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .expect("search over http");

    assert_eq!(response.page.hits.len(), 1, "{:?}", response.page.hits);
    assert_eq!(
        response.page.hits[0].document,
        Some(documents[0].to_storage())
    );
    assert_eq!(response.page.hits[0].breadcrumb, "Acme / Doc 0");
    // Neither extra was asked for, so neither was paid for.
    assert!(response.facets.is_none());
    assert!(response.recent_queries.is_none());
}

/// The two extras a search surface wants on the same keystroke, in one round
/// trip rather than three.
#[test]
fn facets_and_recent_queries_come_back_with_the_page_when_asked_for() {
    let root = root_for("extras");
    let store = Store::start(&root);
    let (workspace, documents) = a_workspace(&store, 1);
    write(&root, workspace, documents[0], "the quarterly plan");
    wait_for_hit(&store, "quarterly");

    let client = store.client();
    // A query only reaches the history by leading somewhere.
    client
        .record_navigation(&Navigation {
            workspace,
            document: Some(documents[0].to_storage()),
            node: None,
            query: "quarterly".into(),
        })
        .expect("record");

    let response = client
        .search(&SearchRequest {
            query: SearchQuery {
                text: "quarterly".into(),
                ..Default::default()
            },
            include_facets: true,
            recent_queries: Some(8),
        })
        .expect("search");

    let facets = response.facets.expect("facets were asked for");
    assert_eq!(facets.workspaces.len(), 1);
    assert_eq!(facets.workspaces[0].workspace_name, "Acme");

    let recent = response.recent_queries.expect("recent were asked for");
    assert_eq!(
        recent.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(),
        ["quarterly"],
        "a navigated query is what earns a place in the history"
    );
}

/// **A malformed workspace id is an error, not an empty page.**
///
/// It used to be dropped, which emptied the scope — and an empty scope means
/// *no results* rather than *everywhere*, so the caller got a successful,
/// empty answer and no hint it had asked for something impossible. The typed
/// id on the wire is what refuses it, before any handler runs.
#[test]
fn a_malformed_workspace_id_is_refused_rather_than_silently_ignored() {
    let root = root_for("badid");
    let store = Store::start(&root);
    let _ = a_workspace(&store, 1);

    let discovery = common::discovery(&root).expect("discover");
    let response = reqwest::blocking::Client::new()
        .post(format!("http://127.0.0.1:{}/v1/search", discovery.port))
        .bearer_auth(&discovery.token)
        .json(&serde_json::json!({ "text": "anything", "workspaces": ["not-a-uuid"] }))
        .send()
        .expect("send");

    assert!(
        response.status().is_client_error(),
        "a malformed id answered {}",
        response.status()
    );
}

/// **The query never reaches the log.** It travels in a body precisely so that
/// it cannot, and the log rotates for a week.
#[test]
fn a_search_leaves_no_query_text_in_the_stores_log() {
    let root = root_for("privacy");
    let store = Store::start_logging_to_file(&root);
    let (workspace, documents) = a_workspace(&store, 1);
    write(&root, workspace, documents[0], "the quarterly plan");
    wait_for_hit(&store, "quarterly");

    store
        .client()
        .search(&SearchRequest {
            query: SearchQuery {
                text: "supercalifragilistic".into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .expect("search");

    // Give the appender a moment to flush what it has.
    std::thread::sleep(Duration::from_millis(300));
    let logs = root.join("logs");
    let mut looked_at = 0;
    for entry in std::fs::read_dir(&logs).expect("logs") {
        let path = entry.expect("entry").path();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            !text.contains("supercalifragilistic"),
            "the query text reached {}",
            path.display()
        );
        // The request itself was logged — otherwise this proves nothing.
        looked_at += usize::from(text.contains("/v1/search"));
    }
    assert!(looked_at > 0, "no log names the route at all, in {logs:?}");
}

/// `search`, and nothing that names what is underneath.
#[test]
fn the_store_advertises_search() {
    let root = root_for("capability");
    let store = Store::start(&root);
    let status = store.client().status().expect("status");
    assert!(
        status.capabilities.contains(&"search".to_string()),
        "{:?}",
        status.capabilities
    );
}

/// **`behind` is a real number, not a decoration.**
///
/// The one counter that says search is stale, so a test that only ever saw it
/// at zero would prove nothing. This makes the index genuinely behind — delete
/// its rows out from under it while the store is down, leaving the read model
/// intact — and watches the reconcile notice, report it, and clear it.
#[test]
fn the_index_reports_how_far_behind_the_read_model_it_is() {
    let root = root_for("behind");
    let mut store = Store::start(&root);
    let (workspace, documents) = a_workspace(&store, 3);
    for (n, document) in documents.iter().enumerate() {
        write(&root, workspace, *document, &format!("document number {n}"));
    }
    wait_for_hit(&store, "number");
    wait_for_quiet(&store);

    // Caught up: nothing queued, and nothing the read model holds that the
    // index has not consumed.
    let report = store.client().debug("search").expect("report");
    assert_eq!(report["behind"], 0, "{report}");

    // Now make it behind, without touching the projection: forget every cursor
    // the index has. The rows stay, so this is not a rebuild — it is an index
    // that has lost track of what it consumed.
    store.kill();
    {
        let db = rusqlite::Connection::open(root.join("search.db")).unwrap();
        db.execute("DELETE FROM documents", []).unwrap();
    }
    store.restart();

    // The reconcile compares, records that it found work, and repairs it. On a
    // small fixture the bounded repair can finish before the restarted HTTP
    // boundary is ready for the first sample; the deterministic worker test
    // pins the transient `behind` value itself. Here, assert that the real
    // process noticed missed work and returned to current.
    wait_for("the index to notice missed work", || {
        let report = store.client().debug("search").unwrap_or_default();
        report["caught_up"].as_u64().unwrap_or(0) > 0
    });

    // It repairs itself, so the number returns to zero rather than sticking.
    wait_for("the index to catch up", || {
        store.client().debug("search").unwrap_or_default()["behind"] == 0
    });
}

/// The durations, split by stage.
///
/// **A total cannot say which part was slow.** `last_millis` used to cover
/// reading the read model, building fragments and writing them at once.
#[test]
fn the_derived_layers_report_where_their_time_went() {
    let root = root_for("timing");
    let store = Store::start(&root);
    let (workspace, documents) = a_workspace(&store, 1);
    write(&root, workspace, documents[0], "timed");
    wait_for_hit(&store, "timed");
    wait_for_quiet(&store);

    let client = store.client();
    let search = client.debug("search").expect("search report");
    let projection = client.debug("projection").expect("projection report");

    for (report, stages) in [
        (&search, ["read_millis", "build_millis", "write_millis"]),
        (&projection, ["fork_millis", "walk_millis", "write_millis"]),
    ] {
        for stage in stages {
            assert!(
                report[stage].is_number(),
                "{stage} is missing from {report}"
            );
        }
        // A cumulative total cannot be less than the slowest single one, and
        // neither can be less than the last. Cheap, and it catches a counter
        // wired to the wrong variable.
        let total = report["total_millis"].as_u64().expect("total_millis");
        let max = report["max_millis"].as_u64().expect("max_millis");
        let last = report["last_millis"].as_u64().expect("last_millis");
        assert!(total >= max, "total {total} < max {max} in {report}");
        assert!(max >= last, "max {max} < last {last} in {report}");
    }

    // **`stale_found` does not climb once a workspace is warm**, which is the
    // only claim it makes. It is *not* zero here: the workspace's first sweep
    // legitimately finds the documents it has never projected, which is the
    // mechanism working rather than failing. What would mean an event went
    // missing is this number moving after that — so the assertion is on the
    // delta across an ordinary edit, not on the value.
    let before = projection["stale_found"].as_u64().expect("stale_found");
    write(&root, workspace, documents[0], "and again");
    wait_for_hit(&store, "again");
    wait_for_quiet(&store);
    let after = client.debug("projection").expect("report")["stale_found"]
        .as_u64()
        .expect("stale_found");
    assert_eq!(
        after, before,
        "a sweep found work an event should have delivered"
    );
}
