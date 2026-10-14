// SPDX-License-Identifier: MPL-2.0

//! Two replicas editing offline, then merging — printed as a narrative.
//!
//!     cargo run -p koru-core --example two_peers
//!
//! This is the headless version of the two-window walkthrough in
//! `docs/contributing/operations/RUNBOOK.md`. It exercises the same export → transport → import path;
//! the only difference is that "transport" here is a `Vec<u8>` handed between
//! two values in one process instead of files or a socket.

use koru_core::node::{Attr, AttrValue, Content, Node, NodeMeta};
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_core::{Kind, Mark, NodeId, Ref, WorkspaceId};

/// Render a line from its runs, so what is printed comes from the same place
/// the editor's layout does — marks and pills included, no parsing anywhere.
fn line_of(ws: &WorkspaceDoc, id: NodeId) -> String {
    let node = ws.read(id).unwrap();
    let Some(_) = node.content.as_text() else {
        return "<blob>".into();
    };
    let mut out = String::new();
    for run in ws.runs(id).unwrap() {
        if let Some(attr) = run.pill {
            let label = node
                .content
                .attrs()
                .iter()
                .find(|a| a.id == attr)
                .map(|a| match &a.value {
                    AttrValue::Date(d) => d.to_string(),
                    AttrValue::FloatingDateTime(f) => f.to_string(),
                    AttrValue::ZonedDateTime(z) => {
                        format!("{} {}", z.instant_ms(), z.zone())
                    }
                    AttrValue::Number(n) => n.to_string(),
                    AttrValue::Bool(b) => b.to_string(),
                    AttrValue::Document(reference) => reference.document.to_storage(),
                    AttrValue::Text(t) => t.clone(),
                    AttrValue::Url(u) => u.clone(),
                    AttrValue::Ref(r) => ws.resolve(*r).label().to_string(),
                })
                .unwrap_or_default();
            out.push_str(&format!("[{label}]"));
            continue;
        }
        let deco = match (run.bold, run.italic, run.underline) {
            (true, _, _) => "*",
            (_, true, _) => "/",
            (_, _, true) => "_",
            _ => "",
        };
        out.push_str(&format!("{deco}{}{deco}", run.text));
    }
    out
}

fn show(label: &str, ws: &WorkspaceDoc) {
    println!("\n  {label}");
    for (id, depth) in ws.walk().unwrap() {
        let node = ws.read(id).unwrap();
        let tick = if node.meta.done { "x" } else { " " };
        let level = node
            .meta
            .level_name
            .as_deref()
            .map(|l| format!("  ({l})"))
            .unwrap_or_default();
        println!(
            "    {}[{tick}] {}{level}",
            "  ".repeat(depth),
            line_of(ws, id)
        );
    }
}

/// Everything each side knows, swapped both ways.
fn sync(a: &mut WorkspaceDoc, b: &mut WorkspaceDoc) {
    let from_a = a.export_all_updates().unwrap();
    let from_b = b.export_all_updates().unwrap();
    println!(
        "\n  ── sync: A→B {} bytes, B→A {} bytes ──",
        from_a.len(),
        from_b.len()
    );
    a.import(&from_b).unwrap();
    b.import(&from_a).unwrap();
}

fn main() {
    // Both peers start from the same snapshot, as they would after one of them
    // opened a workspace the other had already created.
    let mut a = WorkspaceDoc::create(WorkspaceId::new(), "acme").unwrap();
    // Contacts live in the library, so they are referenceable without being
    // rows in the outline.
    let kim = a
        .insert(
            Parent::Root,
            Node::new(Kind::Contact, Content::text("Kim")).unwrap(),
        )
        .unwrap();
    let reward = a
        .insert(Parent::Root, Node::task("payout flow").unwrap())
        .unwrap();
    let invite = a
        .insert(Parent::Root, Node::task("Invite email").unwrap())
        .unwrap();
    let mut b = WorkspaceDoc::from_snapshot(&a.export_snapshot().unwrap()).unwrap();

    println!("\n=== both peers start from the same snapshot ===");
    show("A", &a);
    show("B", &b);

    println!("\n=== now both go offline and edit independently ===");

    // A restructures: wraps the two tasks into a group, and bolds a word on the
    // line B is about to edit.
    let phase = a
        .insert(
            Parent::Root,
            Node::task("Phase 1").unwrap().with_meta(NodeMeta {
                level_name: Some("Phase".into()),
                ..Default::default()
            }),
        )
        .unwrap();
    a.move_node(reward, Parent::Node(phase), 0).unwrap();
    a.move_node(invite, Parent::Node(phase), 1).unwrap();
    a.insert_text(reward, 0, "reward ").unwrap();
    a.set_mark(reward, 0.."reward".len(), Mark::Bold, true)
        .unwrap();
    println!("\n  A wraps both tasks into a group, calls the level a \"Phase\",");
    println!("  and types a bold word at the head of one of them");

    // B edits the *same* line — at the other end — attaches an assignee, and
    // ticks the other task off.
    let end = b.text_len(reward).unwrap();
    b.insert_text(reward, end, " is blocked ").unwrap();
    let at = b.text_len(reward).unwrap();
    b.insert_pill(
        reward,
        at,
        &Attr::new("assignee", AttrValue::Ref(Ref::new(kim))),
    )
    .unwrap();
    b.set_done(invite, true).unwrap();
    println!("  B appends to the tail of that same line, assigns it, ticks the other off");

    show("A (offline)", &a);
    show("B (offline)", &b);

    sync(&mut a, &mut b);

    println!("\n=== after merge ===");
    show("A", &a);
    show("B", &b);

    let line = line_of(&a, reward);
    assert_eq!(line, line_of(&b, reward), "replicas disagree");
    assert!(line.contains("reward"), "A's head edit survived: {line}");
    assert!(
        line.contains("is blocked"),
        "B's tail edit survived: {line}"
    );
    assert!(
        a.runs(reward).unwrap().iter().any(|r| r.bold),
        "A's mark survived B's edit to the same line"
    );
    assert_eq!(
        a.read(reward).unwrap().assignees().count(),
        1,
        "and B's assignee came along"
    );
    assert!(a.ancestors(reward).unwrap().contains(&phase));

    println!("\n  A's restructure and B's edits both survived:");
    println!("    - the merged line:           {line:?}");
    println!(
        "    - the tick is intact:        {}",
        a.read(invite).unwrap().meta.done
    );
    println!(
        "    - both sit inside the group: depth {}",
        a.depth(reward).unwrap()
    );
    println!(
        "    - the assignee resolves to:  {:?}",
        a.read(reward)
            .unwrap()
            .assignees()
            .next()
            .map(|r| a.resolve(r).label().to_string())
    );
    println!("\n  No last-write-wins. Nothing was lost.\n");
}
