// SPDX-License-Identifier: MPL-2.0

//! What a session says about how the workspace opened, and what it refuses to
//! do about it.
//!
//! The store's own tests cover recovery mechanics. These cover the seam above
//! it — the one that used to be an `eprintln!` and an empty window.

use std::fs;
use std::path::PathBuf;

use koru_core::node::Node;
use koru_core::schema::{Parent, SCHEMA_VERSION, WorkspaceDoc};
use koru_workspace::session::{self, Session};

/// The dev workspace, which is what these tests are about opening.
///
/// `Session::open` takes a workspace id and a name now — the profile's list
/// supplies them in the app. These tests predate that and are about *how a
/// workspace opens*, not which one, so they keep asking for the same one.
fn open(dir: &PathBuf, peer: &str) -> (Session, WorkspaceDoc, koru_workspace::session::Report) {
    Session::open(dir, session::dev_workspace_id(), "Koru", peer).unwrap()
}

fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("koru-open-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// The one case where an empty workspace is the right answer, and it says so.
/// Every other route to an empty window is an error.
#[test]
fn an_empty_directory_opens_fresh_and_says_so() {
    let dir = temp("fresh");
    let (_session, doc, report) = open(&dir, "A");
    assert!(report.fresh, "nothing was on disk");
    assert!(report.is_quiet(), "and nothing to complain about");
    assert!(report.read_only.is_none());
    assert_eq!(doc.walk().unwrap().len(), 0);
    let _ = fs::remove_dir_all(&dir);
}

/// Reopening is not a first run. If this ever answers `fresh` again, a real
/// workspace is one keystroke from being presented as a new one.
#[test]
fn reopening_is_not_a_first_run() {
    let dir = temp("reopen");
    let (mut session, mut doc, _) = open(&dir, "A");
    doc.insert(Parent::Root, Node::task("written").unwrap())
        .unwrap();
    session.save(&doc);
    drop(session);

    let (_session, doc, report) = open(&dir, "A");
    assert!(!report.fresh);
    assert_eq!(doc.walk().unwrap().len(), 1);
    let _ = fs::remove_dir_all(&dir);
}

/// A document from a newer build opens, and is not written to. Both halves
/// matter: refusing to open would lock you out of your own content, and
/// writing would let a build that cannot see the newer data re-export over it.
#[test]
fn a_workspace_from_a_newer_build_opens_read_only() {
    let dir = temp("readonly");

    // Write a snapshot claiming a schema this build does not understand. Poked
    // in through the raw document, since there is deliberately no API for
    // setting the version to anything but the current one.
    let (mut session, mut doc, _) = open(&dir, "A");
    let task = doc
        .insert(Parent::Root, Node::task("from the future").unwrap())
        .unwrap();
    doc.doc()
        .get_map("meta")
        .insert("schema_version", SCHEMA_VERSION + 1)
        .unwrap();
    doc.commit();
    session.save(&doc);
    drop(session);

    let (mut session, doc, report) = open(&dir, "A");
    assert!(!report.is_quiet(), "and the user is told");
    let why = report.read_only.clone().expect("should refuse to write");
    assert!(why.contains(&(SCHEMA_VERSION + 1).to_string()), "{why}");
    assert_eq!(
        doc.read(task).unwrap().content.as_text(),
        Some("from the future"),
        "it still opens - read-only is not locked out"
    );
    assert!(
        session.label().contains("read-only"),
        "and it stays visible: {}",
        session.label()
    );

    // The load-bearing half: a save must write nothing at all.
    let snapshot = dir
        .join("workspaces")
        .join("00000000-0000-4000-8000-000000000001")
        .join("snapshot.loro");
    let before = fs::metadata(&snapshot).unwrap().len();
    let stamp = fs::metadata(&snapshot).unwrap().modified().unwrap();
    session.save(&doc);
    assert_eq!(fs::metadata(&snapshot).unwrap().len(), before);
    assert_eq!(fs::metadata(&snapshot).unwrap().modified().unwrap(), stamp);

    let _ = fs::remove_dir_all(&dir);
}

/// Two data directories are two workspaces. This is the whole of the
/// separation between a build being worked on and content being written in, so
/// it is worth an assertion of its own.
#[test]
fn two_directories_are_two_workspaces() {
    let root = temp("split");
    let (dev, real) = (root.join("dev"), root.join("real"));

    let (mut a, mut doc_a, _) = open(&dev, "A");
    doc_a
        .insert(Parent::Root, Node::task("dev scratch").unwrap())
        .unwrap();
    a.save(&doc_a);

    let (_b, doc_b, report) = open(&real, "A");
    assert!(report.fresh, "the other directory is untouched by it");
    assert_eq!(doc_b.walk().unwrap().len(), 0);

    let _ = fs::remove_dir_all(&root);
}

/// A save with nothing on disk yet has nothing to preserve; the session after
/// it copies the snapshot aside before replacing it. Asserted here as well as
/// in the store because this is the path the app actually takes.
#[test]
fn the_second_session_leaves_a_backup_of_the_first() {
    let dir = temp("backup");
    let (mut first, mut doc, _) = open(&dir, "A");
    doc.insert(Parent::Root, Node::task("session one").unwrap())
        .unwrap();
    first.save(&doc);
    assert!(first.backups().is_empty(), "nothing existed to copy");
    drop(first);

    let (mut second, doc, _) = open(&dir, "A");
    second.save(&doc);
    let kept = second.backups();
    assert_eq!(kept.len(), 1, "the state session two opened on");

    let restored = WorkspaceDoc::from_snapshot(&fs::read(&kept[0]).unwrap()).unwrap();
    assert_eq!(
        restored.walk().unwrap().len(),
        1,
        "and it is a real document, not an empty file"
    );

    let _ = fs::remove_dir_all(&dir);
}
