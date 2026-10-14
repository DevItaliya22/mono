// SPDX-License-Identifier: MPL-2.0

//! The one document a brand-new profile opens on.
//!
//! **This module is deliberately sealed.** It has one public function, it is
//! called from exactly one place, and no string in it appears anywhere else in
//! the program. Nothing imports it for a heading level or a phrase; if the copy
//! below is wrong, this is the only file to open, and if the seed is ever
//! dropped, this is the only file to delete.
//!
//! `lib.rs` declares `pub mod seed;` and deliberately does **not** re-export
//! [`seed_new_profile`], so every call site reads
//! `koru_workspace::seed::seed_new_profile(..)` and is visibly the seed.
//!
//! ## What it writes, and what it does not
//!
//! One workspace entry named `Example`, and one `Get started` document in that
//! workspace's **library**. Nothing in the outline — `koru_editor::prepare`
//! still puts the single empty line there, so a fresh workspace still opens on
//! one empty typable row and the test that pins that passes untouched.
//!
//! No images. Bytes would mean writing a blob to disk, which is exactly the
//! coupling "keep the seed isolated" rules out — this function takes two
//! documents and touches nothing else.
//!
//! ## Why it is so careful about running twice
//!
//! [ADR-21](../../../docs/decisions/README.md) records a seed that failed to tell the
//! session it had happened: a fresh workspace was never published, so two peers
//! opening it each seeded their own copy and the merge kept both. The guards
//! here are the lesson from that — two of them, each sufficient on its own, plus
//! a caller that writes to disk before the first frame.

use koru_core::node::{Heading, Node};
use koru_core::profile::ProfileDoc;
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_core::{Kind, StoreError};

/// What the first workspace is called.
const WORKSPACE: &str = "Example";

/// What the first document is called.
const DOCUMENT: &str = "Get started";

/// The body, as (heading level, text). Ordinary content: the person who opens
/// it can rename it, edit it or delete it, and nothing in the program will
/// notice or put it back.
const BODY: &[(Option<Heading>, &str)] = &[
    (
        None,
        "Everything in Koru is a node — a task, a document, a person, a picture — and every view is a way of looking at the same tree.",
    ),
    (Some(Heading::H2), "Start typing"),
    (
        None,
        "Press Enter for a new line, Tab to indent it under the one above, and Shift+Tab to bring it back out.",
    ),
    (
        None,
        "Type / for a menu of things to insert, and @ to mention somebody. Both make a pill, which is a reference rather than a piece of text — rename what it points at and every pill follows.",
    ),
    (Some(Heading::H2), "Find your way around"),
    (
        None,
        "Ctrl+E opens this pane. Ctrl+J opens the one on the right. Ctrl+T makes a document, Ctrl+W closes a tab, and Ctrl+1 to Ctrl+9 jump to one.",
    ),
    (
        None,
        "Ctrl+B, Ctrl+I and Ctrl+U do what you expect. Light and dark follow Windows, or pick Light/Dark on the welcome page.",
    ),
    (Some(Heading::H2), "Your work is a folder"),
    (
        None,
        "Koru writes everything into the folder you chose, and keeps timestamped backups beside it. Nothing is anywhere else, and nothing is sent anywhere.",
    ),
    (
        None,
        "This document is yours now — change it, or delete it once you have what you need from it.",
    ),
];

/// Fill a brand-new profile with the one workspace a person opens on.
///
/// Called once, ever, by the code that has just **created** a profile document —
/// never on one loaded from disk. `workspace` must be the freshly created
/// [`WorkspaceDoc`]; its id is what gets registered.
///
/// `created` is the registration timestamp, passed in for the same reason
/// [`ProfileDoc::ensure_workspace`] takes one: this crate does not read a clock,
/// and the test can then assert an exact order.
///
/// Returns `false` and writes **nothing** when the profile already lists a
/// workspace. That is a structural guard, independent of the caller's
/// `Opened::Fresh` check, and either alone is sufficient — which is the point.
/// The caller must still flush both documents to disk before the first frame,
/// or a second instance opening the same folder would find it fresh too.
pub fn seed_new_profile(
    profile: &mut ProfileDoc,
    workspace: &mut WorkspaceDoc,
    created: i64,
) -> Result<bool, StoreError> {
    if !profile.workspaces().is_empty() {
        return Ok(false);
    }
    let Some(id) = workspace.id() else {
        // A workspace with no id in its meta is not one this function made, and
        // registering it would put an unopenable row in the switcher.
        return Ok(false);
    };

    profile.ensure_workspace(id, WORKSPACE, created)?;
    workspace.rename(WORKSPACE)?;

    let doc = workspace.insert(Parent::Root, Node::document(DOCUMENT)?)?;
    for (heading, text) in BODY {
        let para = workspace.insert(Parent::Node(doc), Node::paragraph(*text)?)?;
        if heading.is_some() {
            workspace.set_heading(para, *heading)?;
        }
    }
    // This used to assert the walk came back **empty** — the seed wrote into the
    // library and `prepare` opened on the outline, so anything reachable here
    // meant the seed had written to the wrong tree. There is one tree now, so
    // the check inverts: the document and its body are exactly what should be
    // reachable, and an empty walk would mean the seed wrote nothing.
    debug_assert_eq!(
        workspace.walk()?.len(),
        1 + BODY.len(),
        "the seed writes one document and its body, and nothing else"
    );
    Ok(true)
}

/// Whether a node is the seeded document, for the caller that wants to open on
/// it. Matched on kind and text rather than a remembered id, because the id is
/// not worth a meta key that would then be storage forever — and because a
/// person who renames it has said they do not want this.
pub fn is_get_started(ws: &WorkspaceDoc, id: koru_core::NodeId) -> bool {
    ws.read(id)
        .map(|n| n.kind == Kind::Document && n.content.as_text() == Some(DOCUMENT))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests;
