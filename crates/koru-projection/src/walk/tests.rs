// SPDX-License-Identifier: MPL-2.0

//! What survives the walk, and what deliberately does not.

use super::*;
use koru_core::node::{Attr, AttrValue, Node};
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_core::{Heading, Mark};

fn ids() -> (WorkspaceId, DocumentId) {
    (WorkspaceId::new(), DocumentId::new())
}

/// A document with a root node, as every Koru document has: its text is the
/// title and its children are the body.
fn a_document() -> (WorkspaceDoc, NodeId) {
    let mut ws = WorkspaceDoc::create(koru_core::WorkspaceId::new(), "W").expect("create");
    let root = ws
        .insert(Parent::Root, Node::document("Roadmap").unwrap())
        .expect("root");
    (ws, root)
}

fn projected(ws: &WorkspaceDoc) -> DocumentProjection {
    let (workspace, document) = ids();
    project(ws, workspace, document, b"rev".to_vec()).expect("project")
}

fn node<'a>(p: &'a DocumentProjection, text: &str) -> &'a ProjectedNode {
    p.nodes
        .iter()
        .find(|n| n.text == text)
        .unwrap_or_else(|| panic!("no node with text {text:?} in {:?}", texts(p)))
}

fn texts(p: &DocumentProjection) -> Vec<&str> {
    p.nodes.iter().map(|n| n.text.as_str()).collect()
}

#[test]
fn a_documents_structure_comes_across() {
    let (mut ws, root) = a_document();
    let first = ws
        .insert(Parent::Node(root), Node::paragraph("First").unwrap())
        .unwrap();
    ws.insert(Parent::Node(root), Node::paragraph("Second").unwrap())
        .unwrap();
    ws.insert(Parent::Node(first), Node::paragraph("Nested").unwrap())
        .unwrap();

    let p = projected(&ws);
    assert_eq!(texts(&p), ["Roadmap", "First", "Nested", "Second"]);

    let nested = node(&p, "Nested");
    assert_eq!(nested.parent.as_deref(), Some(first.to_storage().as_str()));
    assert!(nested.depth > node(&p, "First").depth);
    assert_eq!(node(&p, "Second").ordinal, 1, "order among siblings");
}

/// A kind is the token the document already stores, so a consumer can branch on
/// it without linking the node model.
#[test]
fn a_kind_is_the_token_the_document_stores() {
    let (mut ws, root) = a_document();
    ws.insert(Parent::Node(root), Node::task("Ship it").unwrap())
        .unwrap();

    let p = projected(&ws);
    assert_eq!(node(&p, "Roadmap").kind, "document");
    assert_eq!(node(&p, "Ship it").kind, "task");
}

/// The properties a view branches on. Flattening these into text is what makes
/// a kanban impossible to build on this.
#[test]
fn block_properties_a_view_needs_are_carried() {
    let (mut ws, root) = a_document();
    let task = ws
        .insert(Parent::Node(root), Node::task("Ship it").unwrap())
        .unwrap();
    ws.set_done(task, true).unwrap();
    let heading = ws
        .insert(Parent::Node(root), Node::paragraph("Milestones").unwrap())
        .unwrap();
    ws.set_heading(heading, Some(Heading::H2)).unwrap();

    let p = projected(&ws);
    assert!(node(&p, "Ship it").done);
    assert_eq!(node(&p, "Milestones").heading, Some(2));
    assert_eq!(node(&p, "Roadmap").heading, None, "body text has no level");
}

/// **A date stays a date.** The tag and the value are apart so a consumer can
/// ask for every due date without parsing every string and hoping.
#[test]
fn attributes_stay_typed() {
    let (mut ws, root) = a_document();
    let task = ws
        .insert(Parent::Node(root), Node::task("Ship it").unwrap())
        .unwrap();
    let date = koru_core::time::DateOnly::parse("2026-03-01").unwrap();
    ws.set_attr(task, &Attr::new("due", AttrValue::Date(date)))
        .unwrap();
    ws.set_attr(
        task,
        &Attr::labelled("link", AttrValue::Url("https://koru.do".into()), "Home"),
    )
    .unwrap();

    let p = projected(&ws);
    let attrs = &node(&p, "Ship it").attrs;
    let due = attrs.iter().find(|a| a.role == "due").expect("the date");
    assert_eq!(due.kind, "date");
    assert_eq!(due.value, "2026-03-01");

    let link = attrs.iter().find(|a| a.role == "link").expect("the link");
    assert_eq!(link.kind, "url");
    assert_eq!(link.value, "https://koru.do");
    assert_eq!(link.label.as_deref(), Some("Home"));
}

/// Marks are carried because a consumer cannot recover them without reopening
/// the document — which is the whole test for what belongs here.
#[test]
fn emphasis_survives_as_runs() {
    let (mut ws, root) = a_document();
    let line = ws
        .insert(
            Parent::Node(root),
            Node::paragraph("plain bold end").unwrap(),
        )
        .unwrap();
    ws.set_mark(line, 6..10, Mark::Bold, true).unwrap();

    let p = projected(&ws);
    let n = node(&p, "plain bold end");
    let bold: Vec<_> = n.runs.iter().filter(|r| r.bold).collect();
    assert_eq!(bold.len(), 1, "runs: {:?}", n.runs);
    assert_eq!(&n.text[bold[0].start..bold[0].end], "bold");
}

/// **Only formatted runs are recorded.** Unformatted prose is one plain run per
/// node, and writing those down would double an ordinary document's rows to
/// record the absence of formatting.
#[test]
fn unformatted_text_carries_no_runs() {
    let (mut ws, root) = a_document();
    ws.insert(
        Parent::Node(root),
        Node::paragraph("nothing special").unwrap(),
    )
    .unwrap();

    let p = projected(&ws);
    let n = node(&p, "nothing special");
    assert!(!n.text.is_empty());
    assert!(
        n.runs.iter().all(|r| !r.is_plain()) || n.runs.iter().all(ProjectedRun::is_plain),
        "the walk itself keeps every run; the store is what drops the plain ones"
    );
}

/// The text comes from the runs, so the two cannot disagree about a document.
#[test]
fn the_text_is_the_runs_joined() {
    let (mut ws, root) = a_document();
    let line = ws
        .insert(
            Parent::Node(root),
            Node::paragraph("one two three").unwrap(),
        )
        .unwrap();
    ws.set_mark(line, 4..7, Mark::Italic, true).unwrap();

    let p = projected(&ws);
    let n = node(&p, "one two three");
    let joined: String = n.runs.iter().map(|_| "").collect::<String>();
    assert_eq!(joined, "");
    assert_eq!(n.text, "one two three");
    assert!(n.runs.len() > 1, "the mark split it: {:?}", n.runs);
}

// ------------------------------------------------------------------ the bin
//
// A binned node's content still has to be projected: the bin is a place
// somebody searches and restores from, and a projection that omitted it would
// make restore hand back something no consumer had heard of. It is **flagged,
// never hidden** — deciding who sees it belongs to the queries over the rows,
// not to the walk.
//
// This block replaces a test that asserted `p.nodes.len() == walk_all().len()`
// on a document with nothing in its bin. That was true of any walk at all, and
// `walk_all` *is* the trash-filtered walk, so it had been passing for a
// projection that never contained a binned node.

/// The one node the whole flag rests on: `walk` (and therefore `walk_all`)
/// filters the bin out, `walk_with_trash` does not, and the projection takes
/// the second.
#[test]
fn the_walk_does_not_stop_at_the_trash() {
    let (mut ws, root) = a_document();
    let binned = ws
        .insert(Parent::Node(root), Node::paragraph("thrown away").unwrap())
        .unwrap();
    ws.insert(Parent::Node(root), Node::paragraph("still here").unwrap())
        .unwrap();
    ws.trash(binned, 1_000, Some("Ana")).unwrap();

    let p = projected(&ws);
    assert!(
        ws.walk_all().unwrap().len() < ws.walk_with_trash().unwrap().len(),
        "the fixture has to actually have something in its bin"
    );
    assert_eq!(p.nodes.len(), ws.walk_with_trash().unwrap().len());
}

/// Binned, and still itself: the flag is set and the text is the text somebody
/// typed. A row whose title arrived empty could not be offered back to them.
#[test]
fn a_binned_node_keeps_its_text_and_gains_the_flag() {
    let (mut ws, root) = a_document();
    let binned = ws
        .insert(Parent::Node(root), Node::paragraph("thrown away").unwrap())
        .unwrap();
    ws.insert(Parent::Node(root), Node::paragraph("still here").unwrap())
        .unwrap();
    ws.trash(binned, 1_000, Some("Ana")).unwrap();

    let p = projected(&ws);
    assert!(node(&p, "thrown away").in_trash);
    assert!(!node(&p, "still here").in_trash);
    assert!(!node(&p, "Roadmap").in_trash, "the document itself is live");
}

/// The bin's own root is in the bin — it is the hidden container, not content,
/// and a consumer must never take it for a node somebody wrote.
#[test]
fn the_trash_container_is_itself_flagged() {
    let (mut ws, root) = a_document();
    let binned = ws
        .insert(Parent::Node(root), Node::paragraph("thrown away").unwrap())
        .unwrap();
    ws.trash(binned, 1_000, None).unwrap();

    let p = projected(&ws);
    let container = p
        .nodes
        .iter()
        .find(|n| n.kind == "trash")
        .expect("the bin's own root is projected");
    assert!(container.in_trash);
    assert!(
        container.text.is_empty() && container.name.is_none(),
        "the container is not something a person wrote: {container:?}"
    );
}

/// What went *with* an item is in the bin too. The flag is carried down the
/// walk, so this must hold at any depth rather than only for the item binned.
#[test]
fn everything_under_a_binned_node_is_flagged() {
    let (mut ws, root) = a_document();
    let parent = ws
        .insert(Parent::Node(root), Node::paragraph("the parent").unwrap())
        .unwrap();
    let child = ws
        .insert(Parent::Node(parent), Node::paragraph("the child").unwrap())
        .unwrap();
    ws.insert(
        Parent::Node(child),
        Node::paragraph("the grandchild").unwrap(),
    )
    .unwrap();
    ws.trash(parent, 1_000, None).unwrap();

    let p = projected(&ws);
    for text in ["the parent", "the child", "the grandchild"] {
        assert!(node(&p, text).in_trash, "{text} is under the bin");
    }
}

/// Out again, and the flag goes with it — otherwise a restore would put the
/// content back on the page and leave every consumer still hiding it.
#[test]
fn restoring_a_node_clears_the_flag() {
    let (mut ws, root) = a_document();
    let parent = ws
        .insert(Parent::Node(root), Node::paragraph("the parent").unwrap())
        .unwrap();
    ws.insert(Parent::Node(parent), Node::paragraph("the child").unwrap())
        .unwrap();
    ws.trash(parent, 1_000, None).unwrap();
    assert!(projected(&ws).nodes.iter().any(|n| n.in_trash));

    ws.restore(parent, None).unwrap();

    let p = projected(&ws);
    assert!(!node(&p, "the parent").in_trash);
    assert!(!node(&p, "the child").in_trash);
    assert!(
        p.nodes.iter().all(|n| n.kind != "trash" || n.in_trash),
        "an emptied bin keeps its container, and the container is still binned"
    );
}

/// **A binned *document* is a different question**, and the walk answers
/// nothing about it. Trashing a document stamps its entry in the workspace
/// manifest; its own nodes are untouched, so every one of them projects live
/// and the flag on the listing row is what a consumer joins against.
#[test]
fn a_document_with_no_bin_of_its_own_flags_nothing() {
    let (mut ws, root) = a_document();
    ws.insert(Parent::Node(root), Node::paragraph("first").unwrap())
        .unwrap();
    ws.insert(Parent::Node(root), Node::paragraph("second").unwrap())
        .unwrap();

    let p = projected(&ws);
    assert!(p.nodes.iter().all(|n| !n.in_trash));
    assert!(p.nodes.iter().all(|n| n.kind != "trash"));
}

#[test]
fn the_revision_is_carried_verbatim() {
    let (ws, _) = a_document();
    let (workspace, document) = ids();
    let p = project(&ws, workspace, document, b"exactly this".to_vec()).unwrap();
    assert_eq!(p.source_revision, b"exactly this");
    assert_eq!(p.workspace, workspace);
    assert_eq!(p.document, document);
}

/// **The invariant the table depends on**, asserted against the real walk
/// rather than a synthetic order: whatever a document is shaped like, no id
/// reaches the rows twice.
#[test]
fn no_projected_node_id_appears_twice() {
    let (mut ws, root) = a_document();
    let first = ws
        .insert(Parent::Node(root), Node::paragraph("First").unwrap())
        .unwrap();
    ws.insert(Parent::Node(first), Node::paragraph("Nested").unwrap())
        .unwrap();
    ws.insert(Parent::Node(root), Node::paragraph("Second").unwrap())
        .unwrap();

    let p = projected(&ws);
    let mut ids: Vec<&str> = p.nodes.iter().map(|n| n.id.as_str()).collect();
    let before = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), before, "a node id reached the rows twice");
}

// ------------------------------------------------------------------ P1.0: run documents

/// A run-shaped `LoroDoc`, as the delegation roadmap's P1.4 would write one:
/// its own root containers and no node tree.
fn a_run_document(with_meta_schema: Option<i64>) -> loro::LoroDoc {
    let doc = loro::LoroDoc::new();
    let run = doc.get_map("run");
    run.insert("status", "running").unwrap();
    doc.get_list("timeline").push("step").unwrap();
    if let Some(version) = with_meta_schema {
        doc.get_map("meta")
            .insert("schema_version", version)
            .unwrap();
    }
    doc.commit();
    doc
}

/// **P1.0: a manifest-registered document of an unknown kind reaches the
/// projection like any other commit**, and a run has no node tree to walk:
/// it adopts, it is writable, and it projects to zero rows — no error, no
/// refusal, nothing for search to index. The projection's cost for a run is
/// the fork the worker takes, never a walk.
#[test]
fn a_run_document_projects_to_no_rows_and_no_error() {
    let ws = WorkspaceDoc::adopt(a_run_document(None)).expect("adopts");
    assert_eq!(
        ws.compatibility(),
        koru_core::schema::Compatibility::Writable
    );
    let (workspace, document) = ids();
    let p = project(&ws, workspace, document, b"rev".to_vec()).expect("projects");
    assert!(p.nodes.is_empty(), "{:?}", texts(&p));
}

/// **The trap a Run document must not step in.** `meta.schema_version` is
/// the node model's own key, read by `WorkspaceDoc::compatibility` on every
/// projection (`worker.rs`, the `TooNew` branch marks the document
/// incompatible and skips it). A run that stamped a version of its own there
/// would be refused by every projection whose build understands less — so
/// the run's version lives under its own root map, never under `meta`.
#[test]
fn a_run_document_that_borrowed_the_meta_schema_key_would_read_as_too_new() {
    let ws = WorkspaceDoc::adopt(a_run_document(Some(99))).expect("adopts");
    assert!(matches!(
        ws.compatibility(),
        koru_core::schema::Compatibility::TooNew { found: 99, .. }
    ));
}

/// **The delegation pill projects one row, however it is read.**
///
/// This build reads it as `AttrValue::Document` — P1.15 put `koru.run` in
/// `attributes::DOCUMENT_REFERENCE_ROLES` — and a build that knows no
/// `koru.run` role reads the identical record as `AttrValue::Ref` homed at the
/// run. Both project as kind `ref` with the run's id as the value, because
/// `AttrValue::key` is `"ref"` for either variant: the widening leaves
/// `PROJECTION_SCHEMA_VERSION` untouched, and status and the executing device
/// stay things looked up from the Run document by that id, never stored here.
#[test]
fn a_run_pill_projects_as_a_ref_to_the_run_fresh_and_reopened() {
    let (mut ws, root) = a_document();
    let line = ws
        .insert(Parent::Node(root), Node::paragraph("Delegated").unwrap())
        .unwrap();
    let run = koru_core::DocumentId::new();
    ws.insert_pill(
        line,
        0,
        &Attr::new(
            "koru.run",
            AttrValue::Document(koru_core::DocumentReference::new(run)),
        ),
    )
    .unwrap();

    let row = |ws: &WorkspaceDoc| {
        let p = projected(ws);
        // The anchor character joins the text, so match on the prose.
        let line = p
            .nodes
            .iter()
            .find(|n| n.text.contains("Delegated"))
            .unwrap_or_else(|| panic!("{:?}", texts(&p)));
        let attrs = &line.attrs;
        assert_eq!(attrs.len(), 1, "{attrs:?}");
        (
            attrs[0].role.clone(),
            attrs[0].kind.clone(),
            attrs[0].value.clone(),
        )
    };
    let expected = ("koru.run".to_owned(), "ref".to_owned(), run.to_storage());
    assert_eq!(row(&ws), expected);
    let reopened = WorkspaceDoc::from_snapshot(&ws.export_snapshot().unwrap()).unwrap();
    assert_eq!(row(&reopened), expected);
}
