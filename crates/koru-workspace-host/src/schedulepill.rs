// SPDX-License-Identifier: MPL-2.0

//! The durable inline reference left by a recurring delegation.
//!
//! A schedule is not a Run document, so its pill stores the ScheduleId under
//! the open `koru.schedule` role. The stored name is the offline/older-build
//! fallback; the live window projects rename and lifecycle changes from the
//! schedule catalog.

use koru_core::node::{Attr, AttrValue};
use koru_core::schema::WorkspaceDoc;
use koru_core::{DocumentId, NodeId};
use koru_delegate_core::ScheduleId;
use koru_workspace::tasks::Landed;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SchedulePill {
    pub document: DocumentId,
    pub node: NodeId,
    pub schedule: ScheduleId,
    pub label: String,
}

impl SchedulePill {
    pub fn attr(&self) -> Attr {
        Attr::labelled(
            koru_core::attributes::ROLE_SCHEDULE,
            AttrValue::Text(self.schedule.to_storage()),
            self.label.clone(),
        )
    }
}

/// Write one idempotent schedule pill through the workspace's normal edit and
/// publish path. Text rows receive an inline crumb; non-text rows retain the
/// same reference as a whole-node attribute.
pub fn serve(
    workspace: &mut koru_workspace::Workspace,
    pill: &SchedulePill,
) -> Result<Landed, String> {
    let attr = pill.attr();
    let schedule = pill.schedule;
    let node = pill.node;
    workspace
        .edit_document(pill.document, move |doc| {
            if already_there(doc, node, schedule)? {
                return Ok(());
            }
            let read = doc.read(node)?;
            let Some(text) = read.content.as_text().map(str::to_owned) else {
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

fn already_there(
    doc: &WorkspaceDoc,
    node: NodeId,
    schedule: ScheduleId,
) -> Result<bool, koru_core::StoreError> {
    Ok(doc.read(node)?.content.attrs().iter().any(|attr| {
        attr.role == koru_core::attributes::ROLE_SCHEDULE
            && matches!(&attr.value, AttrValue::Text(id) if id == &schedule.to_storage())
    }))
}

#[cfg(test)]
mod tests;
