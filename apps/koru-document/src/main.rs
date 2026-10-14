// SPDX-License-Identifier: MPL-2.0

//! One document in one window.
//!
//! Which document? The first in the workspace's library — there is no outline
//! here to pick from, so a workspace with none gets one made. Opening a
//! *particular* document is the manager's job (`cargo run -p koru`), which has
//! an outline to click a pill in.

use koru_core::schema::{Parent, WorkspaceDoc};
use koru_core::{Kind, NodeId};
use koru_document::BuiltInDocument;
use koru_workspace::Workspace;
use koru_workspace_host::Host;

struct Documents;

/// The first document in the library, if there is one.
fn first_document(ws: &WorkspaceDoc) -> Option<NodeId> {
    ws.children(Parent::Root)
        .ok()?
        .into_iter()
        .find(|&id| matches!(ws.read(id), Ok(n) if n.kind == Kind::Document))
}

impl Host for Documents {
    type V = BuiltInDocument;

    fn initial(&mut self, workspace: &mut Workspace) -> BuiltInDocument {
        let doc = match first_document(&workspace.ws) {
            Some(doc) => doc,
            // A workspace with no documents gets one, the same find-or-create a
            // `/document` pill performs. Without it there is nothing to show.
            None => match koru_workspace::document(&mut workspace.ws, "Untitled") {
                Some(doc) => {
                    workspace.touched();
                    doc
                }
                None => panic!("could not create a document to show"),
            },
        };
        workspace.enable_undo();
        BuiltInDocument::open(&workspace.ws, doc).owns_the_window(&workspace.ws)
    }

    fn duplicate(&mut self, ws: &WorkspaceDoc) -> BuiltInDocument {
        let doc = first_document(ws).expect("a document existed when the window opened");
        BuiltInDocument::open(ws, doc).owns_the_window(ws)
    }

    fn open_document(&mut self, ws: &WorkspaceDoc, doc: NodeId) -> Option<BuiltInDocument> {
        Some(BuiltInDocument::open(ws, doc).owns_the_window(ws))
    }
}

fn main() {
    koru_workspace_host::run(Documents, "Koru Document");
}
