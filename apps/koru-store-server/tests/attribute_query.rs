// SPDX-License-Identifier: MPL-2.0

//! The attribute query service, against a real store process (roadmap A1c,
//! decision D9).
//!
//! Attributes are set the way the editor sets them — typed `koru-core`
//! operations on a `WorkspaceDoc`, published over the ordinary sync session —
//! and then asked back through `POST /v1/attributes/query`, the same route a
//! Tracked view or a due list will call in R1. Nothing here reads
//! `projection.db` directly: what is asserted is what a client can get.
//!
//! It lives in this package for the same reason `document_roundtrip.rs` does:
//! `CARGO_BIN_EXE_koru-store-server` is only set for tests of the package
//! that declares the binary.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use koru_core::attributes::{KEY_DUE, KEY_PRIORITY};
use koru_core::node::{AttrValue, Node};
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_core::system::SystemDoc;
use koru_store_client::{
    AttributeQuery, AttributeQueryResponse, Client, DateWindow, DueFilter, InstantWindow,
};
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

    fn restart(&mut self) {
        self.kill();
        *self = Self::start(&self.root.clone());
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        self.kill();
    }
}

fn root_for(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("koru-store-attrq-{name}-{}", std::process::id()));
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

fn date(y: i32, m: u32, d: u32) -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// The same day as the **calendar-date type** an attribute stores.
fn day(y: i32, m: u32, d: u32) -> koru_core::time::DateOnly {
    koru_core::time::DateOnly::new(date(y, m, d))
}

/// A **zoned** deadline — an instant and the zone it was meant in. There is
/// no way to build one without the zone, which is the point (ADR-183).
fn zoned(instant_ms: i64, zone: &str) -> AttrValue {
    AttrValue::ZonedDateTime(koru_core::time::ZonedDateTime::new(
        instant_ms,
        koru_core::time::TimeZoneName::parse(zone).unwrap(),
    ))
}

/// Epoch milliseconds for a UTC wall-clock moment, computed from day
/// arithmetic so the test does not depend on chrono's timezone API.
fn utc_ms(y: i32, m: u32, d: u32, hour: i64) -> i64 {
    let days = (date(y, m, d) - date(1970, 1, 1)).num_days();
    days * 86_400_000 + hour * 3_600_000
}

fn a_query() -> AttributeQuery {
    AttributeQuery::default()
}

/// A **calendar** due window. It answers for date-only and floating dues and
/// deliberately not for zoned ones — a caller that wants those sends an
/// instant window as well, in a zone it chose (ADR-183, rule 5).
fn calendar_window(after: &str, before: &str) -> Option<DueFilter> {
    Some(DueFilter {
        dates: Some(DateWindow {
            after: Some(after.to_owned()),
            before: Some(before.to_owned()),
        }),
        instants: None,
    })
}

/// An **instant** due window, `[after, before)` in epoch-ms UTC.
fn instant_window(after_ms: i64, before_ms: i64) -> Option<DueFilter> {
    Some(DueFilter {
        dates: None,
        instants: Some(InstantWindow {
            after_ms: Some(after_ms),
            before_ms: Some(before_ms),
        }),
    })
}

/// Node ids are opaque; rows are recognised by their excerpt.
fn excerpts(response: &AttributeQueryResponse) -> Vec<&str> {
    response.rows.iter().map(|r| r.excerpt.as_str()).collect()
}

/// The whole slice: attributes set via document edits over the sync session,
/// answered as typed rows through the public query route — filters, sorting,
/// paging, trash flags, definitions and tags — and still answered after the
/// process is killed and restarted.
#[test]
fn attributes_set_by_document_edits_answer_the_query_service() {
    let root = root_for("slice");
    let mut store = Store::start(&root);
    let client = store.client();

    let profile = client
        .create_profile("Tester", None, None, None)
        .expect("profile");
    let workspace = client
        .create_workspace(profile.id, "Acme")
        .expect("workspace");

    // The workspace system document, with one tag — planted the way A2's
    // reconciler will plant it, as an ordinary manifest entry of the
    // `koru-system` kind carrying a `SystemDoc` snapshot.
    let mut system = SystemDoc::create().expect("system document");
    let tag = system
        .create_tag("mvp", Some("teal"), 1_756_000_000_000)
        .expect("tag");
    client
        .import_document(
            workspace.id,
            "System",
            koru_core::explore::KIND_SYSTEM,
            system.export_snapshot().expect("snapshot"),
        )
        .expect("plant the system document");

    // ---- "Tasks": three attributed paragraphs, edited over the session.
    let tasks = client
        .create_document(workspace.id, "Tasks", "page")
        .expect("document");
    let (mut session, snapshot) =
        StoreSession::join(segment(), Some(&root), workspace.id, tasks).expect("join");
    let mut doc = if snapshot.is_empty() {
        WorkspaceDoc::create(koru_core::WorkspaceId::new(), "Acme").expect("create")
    } else {
        WorkspaceDoc::from_snapshot(&snapshot).expect("open")
    };
    let page = doc
        .insert(Parent::Root, Node::document("Tasks").unwrap())
        .unwrap();
    let report = doc
        .insert(
            Parent::Node(page),
            Node::paragraph("Ship the report").unwrap(),
        )
        .unwrap();
    doc.set_attribute(report, KEY_DUE, AttrValue::Date(day(2026, 9, 1)))
        .expect("a calendar-date due");
    doc.set_attribute(report, KEY_PRIORITY, AttrValue::Text("high".into()))
        .expect("priority");
    let launch = doc
        .insert(
            Parent::Node(page),
            Node::paragraph("Plan the launch").unwrap(),
        )
        .unwrap();
    doc.set_attribute(launch, KEY_DUE, zoned(utc_ms(2026, 9, 2, 15), "Asia/Tokyo"))
        .expect("a timed due");
    doc.set_attribute(launch, KEY_PRIORITY, AttrValue::Text("medium".into()))
        .expect("priority");
    let tagged = doc
        .insert(
            Parent::Node(page),
            Node::paragraph("Tag the pitch").unwrap(),
        )
        .unwrap();
    doc.add_tag(tagged, tag).expect("tag");
    let batch = session
        .publish(&doc)
        .expect("publish")
        .expect("work to send");
    wait_for("the Tasks write to be durable", || {
        let _ = session.drain(&mut doc);
        session.is_durable(batch)
    });

    // ---- "Old": one attributed paragraph, then the whole document binned.
    let old = client
        .create_document(workspace.id, "Old", "page")
        .expect("document");
    let (mut old_session, _) =
        StoreSession::join(segment(), Some(&root), workspace.id, old).expect("join");
    let mut old_doc = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "Acme").expect("create");
    let old_page = old_doc
        .insert(Parent::Root, Node::document("Old").unwrap())
        .unwrap();
    let archived = old_doc
        .insert(
            Parent::Node(old_page),
            Node::paragraph("Archived thing").unwrap(),
        )
        .unwrap();
    old_doc
        .set_attribute(archived, KEY_DUE, AttrValue::Date(day(2026, 12, 31)))
        .expect("due");
    let batch = old_session
        .publish(&old_doc)
        .expect("publish")
        .expect("work");
    wait_for("the Old write to be durable", || {
        let _ = old_session.drain(&mut old_doc);
        old_session.is_durable(batch)
    });

    let (mut manifest_session, manifest_snapshot) =
        StoreSession::join_manifest(segment(), Some(&root), workspace.id).expect("join manifest");
    let mut manifest = koru_store_core::manifest::Manifest::from_snapshot(&manifest_snapshot)
        .expect("open the manifest");
    manifest
        .trash_document(old, koru_store_core::model::Timestamp::now(), Some("test"))
        .expect("bin it");
    let batch = manifest_session
        .publish(&manifest)
        .expect("publish")
        .expect("work");
    wait_for("the trash stamp to be durable", || {
        let _ = manifest_session.drain(&mut manifest);
        manifest_session.is_durable(batch)
    });

    // ---- Has-attribute, once the debounced projection converges. The three
    // due rows arrive typed: the calendar date as a `date`, the instant as a
    // `datetime` in decimal epoch-ms — never one folded into the other.
    let due_rows = |client: &Client| {
        client
            .query_attributes(&AttributeQuery {
                has: Some(KEY_DUE.into()),
                sort: Some("due".into()),
                ..a_query()
            })
            .expect("query")
    };
    // The trash flag is part of the wait: rows and the listing refresh are
    // separate projector work, and the flag can trail the rows by a tick.
    wait_for("the projection to answer all three due rows", || {
        let response = due_rows(&client);
        response.projected
            && response.rows.len() == 3
            && response.rows.iter().any(|r| r.document == old && r.trashed)
    });
    let response = due_rows(&client);
    assert_eq!(
        excerpts(&response),
        ["Ship the report", "Plan the launch", "Archived thing"],
        "due sort is soonest first, across both temporal domains"
    );
    let row = &response.rows[0];
    assert_eq!(row.workspace, workspace.id);
    assert_eq!(row.document, tasks);
    assert!(!row.node.is_empty(), "the node travels with its document");
    assert_eq!(row.node_kind, "paragraph");
    assert_eq!(row.document_title, "Tasks");
    assert!(!row.trashed);
    let due = row.attributes.iter().find(|a| a.role == KEY_DUE).unwrap();
    assert_eq!(
        (due.kind.as_str(), due.value.as_str()),
        ("date", "2026-09-01")
    );
    let timed = response.rows[1]
        .attributes
        .iter()
        .find(|a| a.role == KEY_DUE)
        .unwrap();
    assert_eq!(timed.kind, "datetime_zoned");
    assert_eq!(timed.value, utc_ms(2026, 9, 2, 15).to_string());
    assert_eq!(
        timed.zone.as_deref(),
        Some("Asia/Tokyo"),
        "the zone rides with the instant, or the row is unreadable"
    );

    // The system registry rides along, custom-or-not, so a filter UI builds
    // from types; the workspace's tag table resolves the id to a name.
    assert!(
        response
            .definitions
            .iter()
            .any(|d| d.key == KEY_DUE && d.system && d.value_type == "datetime_zoned")
    );
    assert!(response.tags.iter().any(|t| t.id == tag.to_storage()
        && t.name == "mvp"
        && t.color.as_deref() == Some("teal")
        && t.workspace == workspace.id));

    // ---- The due window, **each temporal type in its own terms**
    // (ADR-183, rule 5). A calendar window answers for calendar days and
    // never silently folds a zoned deadline in — the launch's instant is
    // 15:00Z on Sep 2, which is already Sep 3 in the Tokyo zone it was set
    // in, so "the calendar Sep 2" is a different question and gets a
    // different answer.
    let calendar = |after: &str, before: &str| {
        client
            .query_attributes(&AttributeQuery {
                due: calendar_window(after, before),
                ..a_query()
            })
            .expect("query")
    };
    assert_eq!(
        excerpts(&calendar("2026-09-01", "2026-09-02")),
        ["Ship the report"]
    );
    assert!(
        excerpts(&calendar("2026-09-02", "2026-09-03")).is_empty(),
        "a calendar window does not reach a zoned deadline"
    );

    // The zoned deadline answers to an **instant** window, exactly.
    let instants = client
        .query_attributes(&AttributeQuery {
            due: instant_window(utc_ms(2026, 9, 2, 15), utc_ms(2026, 9, 2, 16)),
            ..a_query()
        })
        .expect("query");
    assert_eq!(excerpts(&instants), ["Plan the launch"]);

    // "That day, however it was set" is two questions, and a caller asks both.
    let both = client
        .query_attributes(&AttributeQuery {
            due: Some(DueFilter {
                dates: Some(DateWindow {
                    after: Some("2026-09-01".into()),
                    before: Some("2026-09-03".into()),
                }),
                instants: Some(InstantWindow {
                    after_ms: Some(utc_ms(2026, 9, 1, 0)),
                    before_ms: Some(utc_ms(2026, 9, 3, 0)),
                }),
            }),
            ..a_query()
        })
        .expect("query");
    let mut rows = excerpts(&both);
    rows.sort_unstable();
    assert_eq!(rows, ["Plan the launch", "Ship the report"]);

    // ---- Priority ≥ high excludes medium: the semantic order, under which
    // "medium" is below "high" however the two spell alphabetically.
    let important = client
        .query_attributes(&AttributeQuery {
            min_priority: Some("high".into()),
            ..a_query()
        })
        .expect("query");
    assert_eq!(excerpts(&important), ["Ship the report"]);

    // ---- The tag filter matches by id, and the base rule keeps every other
    // attributed node out.
    let tagged_rows = client
        .query_attributes(&AttributeQuery {
            tags: vec![tag.to_storage()],
            ..a_query()
        })
        .expect("query");
    assert_eq!(excerpts(&tagged_rows), ["Tag the pitch"]);

    // ---- The binned document's row is flagged, never hidden.
    let archived_row = response
        .rows
        .iter()
        .find(|r| r.document == old)
        .expect("the trashed document still answers");
    assert!(archived_row.trashed, "flagged, not hidden");
    assert_eq!(archived_row.document_title, "Old");

    // ---- Paging: two pages of the same sorted set, no overlap, no gap.
    let page = |offset: usize| {
        client
            .query_attributes(&AttributeQuery {
                has: Some(KEY_DUE.into()),
                sort: Some("due".into()),
                limit: Some(2),
                offset: Some(offset),
                ..a_query()
            })
            .expect("query")
    };
    let (first, second) = (page(0), page(2));
    assert_eq!(first.limit, 2);
    assert_eq!(excerpts(&first), ["Ship the report", "Plan the launch"]);
    assert_eq!(excerpts(&second), ["Archived thing"]);

    // ---- Kill and restart: the answer survives the process, because both
    // the documents and the read model are on the disk.
    store.restart();
    let client = store.client();
    wait_for("the restarted store to answer the same rows", || {
        let response = due_rows(&client);
        response.projected && response.rows.len() == 3
    });
}

/// A workspace nobody has touched since the read model was lost converges
/// because the query asked — the sweep-first rule — and names itself in
/// `updating_workspaces` until it has.
#[test]
fn a_cold_workspace_converges_after_the_sweep() {
    let root = root_for("cold");
    let mut store = Store::start(&root);
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
    let (mut session, _) =
        StoreSession::join(segment(), Some(&root), workspace.id, document).expect("join");
    let mut doc = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "Acme").expect("create");
    let page = doc
        .insert(Parent::Root, Node::document("Tasks").unwrap())
        .unwrap();
    let node = doc
        .insert(Parent::Node(page), Node::paragraph("Due later").unwrap())
        .unwrap();
    doc.set_attribute(node, KEY_DUE, AttrValue::Date(day(2026, 9, 1)))
        .expect("due");
    let batch = session.publish(&doc).expect("publish").expect("work");
    wait_for("the write to be durable", || {
        let _ = session.drain(&mut doc);
        session.is_durable(batch)
    });

    // Lose the read model while the store is down. The authoritative data is
    // untouched; the projection is derived and rebuildable, and rebuilding it
    // for this workspace is exactly what the query's sweep request triggers.
    store.kill();
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(root.join(format!("projection.db{suffix}")));
    }
    store.restart();
    let client = store.client();

    let ask = |client: &Client| {
        client
            .query_attributes(&AttributeQuery {
                workspaces: vec![workspace.id],
                has: Some(KEY_DUE.into()),
                ..a_query()
            })
            .expect("query")
    };
    let first = ask(&client);
    if !first.projected {
        assert_eq!(
            first.updating_workspaces,
            vec![workspace.id],
            "a cold workspace is named, not silently omitted"
        );
    }
    wait_for("the cold workspace to converge after the sweep", || {
        let response = ask(&client);
        response.projected && response.rows.len() == 1
    });
    let settled = ask(&client);
    assert_eq!(excerpts(&settled), ["Due later"]);
    assert!(settled.updating_workspaces.is_empty());
}

/// **Group-by-tag: counts over the whole corpus, binned rows flagged, and a
/// merged tag folded into the tag it was merged into.**
///
/// The three things a page cannot answer for itself. A client can count the
/// rows it fetched; what it cannot do is count the ones it did not, tell a
/// binned document from a live one without the listing, or know that a node is
/// still carrying an id that was merged away three months ago — `merge_tags`
/// re-points no node, so grouping on the raw stored value splits one group
/// into a named half and a nameless one.
#[test]
fn grouping_by_tag_counts_the_corpus_and_folds_a_merged_tag() {
    let root = root_for("groups");
    let store = Store::start(&root);
    let client = store.client();

    let profile = client
        .create_profile("Tester", None, None, None)
        .expect("profile");
    let workspace = client
        .create_workspace(profile.id, "Acme")
        .expect("workspace");

    // Three tags, and `dupe` merged into `mvp` — the shape a person makes by
    // typing the same tag twice and then tidying up.
    let mut system = SystemDoc::create().expect("system document");
    let mvp = system
        .create_tag("mvp", Some("teal"), 1_756_000_000_000)
        .expect("tag");
    let later = system
        .create_tag("later", None, 1_756_000_000_000)
        .expect("tag");
    let dupe = system
        .create_tag("MVP again", None, 1_756_000_000_000)
        .expect("tag");
    assert!(system.merge_tags(dupe, mvp).expect("merge"));
    client
        .import_document(
            workspace.id,
            "System",
            koru_core::explore::KIND_SYSTEM,
            system.export_snapshot().expect("snapshot"),
        )
        .expect("plant the system document");

    // ---- "Tasks": four tagged paragraphs, two of them carrying the
    // merged-away id, one of them carrying both.
    let tasks = client
        .create_document(workspace.id, "Tasks", "page")
        .expect("document");
    let (mut session, _) =
        StoreSession::join(segment(), Some(&root), workspace.id, tasks).expect("join");
    let mut doc = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "Acme").expect("create");
    let page = doc
        .insert(Parent::Root, Node::document("Tasks").unwrap())
        .unwrap();
    for (text, tags) in [
        ("Ship it", vec![mvp]),
        ("Still the old id", vec![dupe]),
        ("Both ids on one line", vec![mvp, dupe]),
        ("Someday", vec![later]),
    ] {
        let node = doc
            .insert(Parent::Node(page), Node::paragraph(text).unwrap())
            .unwrap();
        for tag in tags {
            doc.add_tag(node, tag).expect("tag");
        }
    }
    let batch = session
        .publish(&doc)
        .expect("publish")
        .expect("work to send");
    wait_for("the Tasks write to be durable", || {
        let _ = session.drain(&mut doc);
        session.is_durable(batch)
    });

    // ---- "Old": one `mvp` paragraph, then the whole document binned.
    let old = client
        .create_document(workspace.id, "Old", "page")
        .expect("document");
    let (mut old_session, _) =
        StoreSession::join(segment(), Some(&root), workspace.id, old).expect("join");
    let mut old_doc = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "Acme").expect("create");
    let old_page = old_doc
        .insert(Parent::Root, Node::document("Old").unwrap())
        .unwrap();
    let archived = old_doc
        .insert(
            Parent::Node(old_page),
            Node::paragraph("Archived thing").unwrap(),
        )
        .unwrap();
    old_doc.add_tag(archived, mvp).expect("tag");
    let batch = old_session
        .publish(&old_doc)
        .expect("publish")
        .expect("work");
    wait_for("the Old write to be durable", || {
        let _ = old_session.drain(&mut old_doc);
        old_session.is_durable(batch)
    });

    let (mut manifest_session, manifest_snapshot) =
        StoreSession::join_manifest(segment(), Some(&root), workspace.id).expect("join manifest");
    let mut manifest = koru_store_core::manifest::Manifest::from_snapshot(&manifest_snapshot)
        .expect("open the manifest");
    manifest
        .trash_document(old, koru_store_core::model::Timestamp::now(), Some("test"))
        .expect("bin it");
    let batch = manifest_session
        .publish(&manifest)
        .expect("publish")
        .expect("work");
    wait_for("the trash stamp to be durable", || {
        let _ = manifest_session.drain(&mut manifest);
        manifest_session.is_durable(batch)
    });

    // A page of one, so the counts cannot have come from the rows.
    let grouped = |client: &Client| {
        client
            .query_attributes(&AttributeQuery {
                workspaces: vec![workspace.id],
                group_by: Some("tag".into()),
                limit: Some(1),
                ..a_query()
            })
            .expect("query")
    };
    // **The trashed flag has to be waited for as well as the counts**, because
    // they come from different writes: the counts from the workspace
    // documents, the flag from the *manifest's* trash stamp. Waiting for that
    // stamp to be durable is not waiting for the projector to have applied it,
    // and `count == 4` is satisfied the moment the binned node is projected —
    // which happens before the manifest is. Asserting on the flag after
    // waiting only for the counts is asserting on state this test never armed.
    wait_for("the projection to answer the groups", || {
        let response = grouped(&client);
        response.projected
            && response
                .groups
                .iter()
                .any(|g| g.tag == mvp.to_storage() && g.count == 4 && g.trashed == 1)
            && response.groups.iter().any(|g| g.tag == later.to_storage())
    });

    let response = grouped(&client);
    assert_eq!(response.rows.len(), 1, "one row, and the counts are not it");
    let group = |tag: &str| {
        response
            .groups
            .iter()
            .find(|g| g.tag == tag)
            .unwrap_or_else(|| panic!("no group for {tag}"))
    };
    let mvp_group = group(&mvp.to_storage());
    assert_eq!(
        mvp_group.count, 4,
        "four distinct nodes: two spelled mvp, one spelled with the merged-away id, \
         one carrying both — and the binned one"
    );
    assert_eq!(
        mvp_group.trashed, 1,
        "the binned row is counted and flagged, exactly as the rows are"
    );
    assert_eq!(mvp_group.workspace, workspace.id);
    assert_eq!(group(&later.to_storage()).count, 1);
    assert!(
        !response.groups.iter().any(|g| g.tag == dupe.to_storage()),
        "the merged-away id is folded away, not left as a group nothing can name"
    );

    // Every group names a tag the response's own table can spell. That is the
    // whole contract between `groups` and `tags`: the id, and nothing else.
    for group in &response.groups {
        assert!(
            response
                .tags
                .iter()
                .any(|t| t.id == group.tag && t.workspace == group.workspace),
            "a group named a tag the live table cannot name"
        );
    }

    // ---- A filter narrows the counts with the rows, never independently.
    let narrowed = client
        .query_attributes(&AttributeQuery {
            workspaces: vec![workspace.id],
            group_by: Some("tag".into()),
            tags: vec![later.to_storage()],
            ..a_query()
        })
        .expect("query");
    assert_eq!(narrowed.groups.len(), 1);
    assert_eq!(narrowed.groups[0].tag, later.to_storage());
    assert_eq!(narrowed.groups[0].count, 1);

    // ---- Asking for no grouping gets no groups, and an unknown token is the
    // caller's mistake rather than a silently ungrouped answer.
    assert!(
        client
            .query_attributes(&AttributeQuery {
                workspaces: vec![workspace.id],
                ..a_query()
            })
            .expect("query")
            .groups
            .is_empty()
    );
    assert!(
        client
            .query_attributes(&AttributeQuery {
                workspaces: vec![workspace.id],
                group_by: Some("priority".into()),
                ..a_query()
            })
            .is_err()
    );
}
