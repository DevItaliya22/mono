// SPDX-License-Identifier: MPL-2.0

//! **The crumb a delegation leaves behind** (delegation roadmap D7, D8).
//!
//! Delegating leaves a pill in the document the gesture came from, and it
//! stays after the run is over as durable provenance. This module is that one
//! write: what it needs to know, and how it reaches a document that may not be
//! the one on screen by the time the run id comes back.
//!
//! **Why it is here and not in the view.** A view is handed the document it is
//! showing; the document a delegation came from is the one the *gesture* named,
//! and a person who switched tabs while the request was out is an ordinary
//! case rather than a lost pill. So the surface raises a [`RunPill`] and the
//! shell serves it through [`koru_workspace::Workspace::edit_document`],
//! exactly as the Tasks surfaces' commands reach a task in a document nobody
//! is looking at.
//!
//! **What is written is the approved shape in `docs/architecture/DATAMODEL.md`:**
//! one attribute record under
//! the open role `koru.run`, carrying an `AttrValue::Document` naming the Run
//! document, with a short localized label. It rides the existing `"ref"`
//! storage token, so `koru_core::schema::SCHEMA_VERSION` is untouched and an
//! older build preserves the record and re-exports it byte for byte.

use koru_core::node::{Attr, AttrValue};
use koru_core::schema::WorkspaceDoc;
use koru_core::{DocumentId, DocumentReference, NodeId};
use koru_workspace::tasks::Landed;

/// One pill to write: which run, and where the gesture that started it was.
///
/// **No status, no device, no objective.** The record carries an id and a
/// label; everything a person reads about the run's state is projected from
/// the Run document by that id, so a pill can never disagree with the run it
/// names.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RunPill {
    /// The document the gesture happened in.
    pub document: DocumentId,
    /// The row it happened on — where the pill goes.
    pub node: NodeId,
    /// The Run document the pill points at.
    pub run: DocumentId,
    /// What an older build draws, and what this one draws until the run's own
    /// header has been read. Localized by the caller; short by design.
    pub label: String,
}

impl RunPill {
    /// The attribute this pill is, exactly as it is stored.
    pub fn attr(&self) -> Attr {
        Attr::labelled(
            koru_core::attributes::ROLE_RUN,
            AttrValue::Document(DocumentReference::new(self.run)),
            self.label.clone(),
        )
    }
}

/// Write the pill, wherever its document is, and publish.
///
/// The row's own text decides how: a paragraph, a heading or a task gets an
/// **inline pill at the end of what is written there**, separated by one space
/// when there is text to separate it from — the crumb follows the origin
/// rather than replacing anything somebody typed. A row with no text at all —
/// a file, an image — gets the same record as a **whole-node attribute**,
/// which is the shape P1.0 measured for a delegated file.
///
/// **Writing twice is not an error and does not write twice.** A row that
/// already carries this run's pill is left exactly as it is, so a retry, a
/// replayed answer or two windows racing produce one mark.
pub fn serve(workspace: &mut koru_workspace::Workspace, pill: &RunPill) -> Result<Landed, String> {
    let attr = pill.attr();
    let run = pill.run;
    let node = pill.node;
    workspace
        .edit_document(pill.document, move |doc| {
            if already_there(doc, node, run)? {
                return Ok(());
            }
            let read = doc.read(node)?;
            let Some(text) = read.content.as_text().map(str::to_owned) else {
                // No text to anchor in: the record goes on the node itself.
                doc.set_attr(node, &attr)?;
                return Ok(());
            };
            let mut at = text.len();
            if !text.is_empty() && !text.ends_with(char::is_whitespace) {
                doc.insert_text(node, at, " ")?;
                at += 1;
            }
            doc.insert_pill(node, at, &attr)?;
            Ok(())
        })
        .map(|(_, landed)| landed)
        .map_err(|error| error.to_string())
}

/// Whether this row already names this run.
fn already_there(
    doc: &WorkspaceDoc,
    node: NodeId,
    run: DocumentId,
) -> Result<bool, koru_core::StoreError> {
    Ok(doc.read(node)?.content.attrs().iter().any(|attr| {
        attr.role == koru_core::attributes::ROLE_RUN
            && attr
                .as_document_reference()
                .is_some_and(|reference| reference.document == run)
    }))
}

#[cfg(test)]
mod tests;
