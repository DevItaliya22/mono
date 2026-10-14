// SPDX-License-Identifier: MPL-2.0

//! **The read model, against a real store process.**
//!
//! Stage 1's gate. Everything here spawns `koru-store-server.exe`, writes documents
//! through the ordinary API, and then asks the projection what it holds — the
//! same way the search index will in stage 2, and the same way retrieval and
//! views will after that.
//!
//! It reads `projection.db` directly, which is the one thing this suite does
//! that a client may not. That is deliberate: the daemon publishes counters, and
//! counters are what a component says about itself. What is asserted here is
//! what is actually on the disk.
//!
//! It lives in this package for the same reason `document_roundtrip.rs` does:
//! `CARGO_BIN_EXE_koru-store-server` is only set inside the tests of the package that
//! declares the binary.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use koru_projection::Projections;
use koru_store_client::Client;
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

    fn restart(&mut self) {
        self.kill();
        *self = Self::start(&self.root.clone());
    }

    /// The projection, opened read-only beside the store's own databases.
    fn projections(&self) -> Projections {
        Projections::open(&self.root).expect("open the projection")
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        self.kill();
    }
}

fn root_for(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "koru-store-projection-{name}-{}",
        std::process::id()
    ));
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

/// A workspace with `n` documents, each holding one line of text.
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

/// Wait until a document's projection contains `text`.
fn wait_for_text(store: &Store, document: DocumentId, text: &str) {
    wait_for(&format!("{text:?} to be projected"), || {
        store
            .projections()
            .read(document)
            .ok()
            .flatten()
            .is_some_and(|p| p.nodes.iter().any(|n| n.text == text))
    });
}

/// **The seam.** Text written through the ordinary API turns up in the read
/// model, without anything having asked for it.
#[test]
fn what_is_written_is_projected() {
    let root = root_for("seam");
    let store = Store::start(&root);
    let (workspace, documents) = a_workspace(&store, 1);

    write(&root, workspace, documents[0], "Ship the projection");
    wait_for_text(&store, documents[0], "Ship the projection");

    let projection = store.projections().read(documents[0]).unwrap().unwrap();
    assert_eq!(projection.workspace, workspace);
    assert!(
        !projection.source_revision.is_empty(),
        "a projection with no revision cannot be reconciled against"
    );
}

/// **The unit of work is one document.** Editing one must not re-project
/// another, or a workspace of a thousand documents pays for every keystroke.
#[test]
fn editing_one_document_leaves_the_others_projections_alone() {
    let root = root_for("one-document");
    let store = Store::start(&root);
    let (workspace, documents) = a_workspace(&store, 4);

    for (i, &document) in documents.iter().enumerate() {
        write(&root, workspace, document, &format!("line {i}"));
    }
    for (i, &document) in documents.iter().enumerate() {
        wait_for_text(&store, document, &format!("line {i}"));
    }

    let before = store.projections().revisions().unwrap();
    write(&root, workspace, documents[0], "changed");
    wait_for_text(&store, documents[0], "changed");

    let after = store.projections().revisions().unwrap();
    for &document in &documents[1..] {
        assert_eq!(
            before.get(&document),
            after.get(&document),
            "an untouched document was re-projected"
        );
    }
    assert_ne!(before.get(&documents[0]), after.get(&documents[0]));
}

/// **A document the projection is behind on is caught by comparison**, on the
/// next start, without anything having been told — and without dragging every
/// other document through a rebuild.
///
/// The staleness is induced by rolling one row's recorded revision back, which
/// is what a projector that died mid-write, or missed an event, leaves behind.
/// That is a fault this suite can create honestly; a lost in-memory event is
/// not, because the queue is drained inside the process that would have to be
/// killed to lose it.
#[test]
fn a_document_the_projection_is_behind_on_is_caught_by_comparison() {
    let root = root_for("catch-up");
    let mut store = Store::start(&root);
    let (workspace, documents) = a_workspace(&store, 2);
    for (i, &document) in documents.iter().enumerate() {
        write(&root, workspace, document, &format!("line {i}"));
        wait_for_text(&store, document, &format!("line {i}"));
    }
    let before = store.projections().revisions().unwrap();

    store.kill();
    {
        let db = rusqlite::Connection::open(root.join("projection.db")).unwrap();
        db.execute(
            "UPDATE documents SET source_revision = ?1 WHERE document_id = ?2",
            rusqlite::params![b"stale".to_vec(), documents[1].to_storage()],
        )
        .unwrap();
    }

    // **A restart sweeps nothing until a workspace shows life** — the store
    // opens lazily and the projection follows it rather than opening everything
    // to find out there was nothing to do. A reconnecting client is what
    // produces that life, so this is one.
    store.restart();
    write(&root, workspace, documents[0], "back again");
    wait_for("the stale document to be re-projected", || {
        store
            .projections()
            .revision(documents[1])
            .ok()
            .flatten()
            .is_some_and(|r| r != b"stale")
    });

    let after = store.projections().revisions().unwrap();
    assert_eq!(
        after.get(&documents[1]),
        before.get(&documents[1]),
        "it was re-projected, so it should be back where it was"
    );
}

/// **Deleting the read model rebuilds it**, and the store keeps working while
/// it does. That is what makes it disposable rather than a second source of
/// truth.
#[test]
fn a_deleted_projection_is_rebuilt_from_the_store() {
    let root = root_for("rebuild");
    let mut store = Store::start(&root);
    let (workspace, documents) = a_workspace(&store, 3);
    for (i, &document) in documents.iter().enumerate() {
        write(&root, workspace, document, &format!("line {i}"));
        wait_for_text(&store, document, &format!("line {i}"));
    }

    store.kill();
    for file in ["projection.db", "projection.db-wal", "projection.db-shm"] {
        let _ = std::fs::remove_file(root.join(file));
    }
    assert!(!root.join("projection.db").exists());

    // One commit is enough: it sweeps the whole workspace, so the other two
    // documents come back without anybody touching them.
    store.restart();
    write(&root, workspace, documents[0], "and back");
    for (i, &document) in documents.iter().enumerate().skip(1) {
        wait_for_text(&store, document, &format!("line {i}"));
    }
    wait_for("every document to be projected again", || {
        store.projections().documents().unwrap_or(0) == 3
    });
}

/// A schema this build does not understand is discarded and rebuilt, rather
/// than migrated. The projection is derived, so that is both simpler and safer.
#[test]
fn a_projection_from_another_schema_is_rebuilt() {
    let root = root_for("schema");
    let mut store = Store::start(&root);
    let (workspace, documents) = a_workspace(&store, 1);
    write(&root, workspace, documents[0], "still here");
    wait_for_text(&store, documents[0], "still here");

    store.kill();
    {
        let db = rusqlite::Connection::open(root.join("projection.db")).unwrap();
        db.pragma_update(
            None,
            "user_version",
            koru_projection::PROJECTION_SCHEMA_VERSION + 1,
        )
        .unwrap();
    }

    store.restart();
    write(&root, workspace, documents[0], "and after");
    wait_for_text(&store, documents[0], "still here");
    wait_for_text(&store, documents[0], "and after");
}

/// The counters the daemon publishes, through the ordinary authenticated route.
#[test]
fn the_projection_reports_what_it_is_doing() {
    let root = root_for("debug");
    let store = Store::start(&root);
    let (workspace, documents) = a_workspace(&store, 1);
    write(&root, workspace, documents[0], "counted");
    wait_for_text(&store, documents[0], "counted");

    let report = store.client().debug("projection").expect("the report");
    assert_eq!(
        report["schema_version"],
        koru_projection::PROJECTION_SCHEMA_VERSION
    );
    assert!(report["projected"].as_u64().unwrap() >= 1);
    assert_eq!(report["failed"].as_u64().unwrap(), 0);
    assert!(report["last_error"].is_null());
}
