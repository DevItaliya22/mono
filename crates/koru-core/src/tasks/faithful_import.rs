// SPDX-License-Identifier: MPL-2.0

//! **The named trusted door**, and the only one ([ADR-286]).
//!
//! Every function here restores stored state verbatim: no command runs, no
//! classification runs, no activity intent is produced, and no marker is
//! fabricated from a value that would create one interactively. An import is a
//! restore of something that already happened, not a thing a person just did —
//! so a completed task with an unchecked child comes back completed, and
//! nothing reopens it on the way in.
//!
//! The five importer crates (`koru-import-markdown`, `-notion`, `-onenote`,
//! `-legacy-workspace`, `-files`) use these two functions and nothing else —
//! plus one more caller, decided 2026-08-31: **the clipboard's carried-fields
//! path**. Pasting a copied task restores its marker, lifecycle and due
//! *verbatim* through this door, because the commands would shatter the
//! paste's single undo step, `complete_task` would cascade downward over the
//! copied subtree state SPEC §6 says survives, and re-deriving the marker
//! would invent a creation reason. **The paste gesture itself is still a user
//! command**: its structural effect classifies and emits through
//! `reclassify` at the paste commit — only the carried fields come through
//! here, exactly as they were copied.
//!
//! This is a real hole by design: anything that can reach this module can write
//! task state without a command. The guard is that the module has one name, a
//! known set of callers, and a sweep.
//!
//! [ADR-286]: ../../../docs/decisions/ADR-276-300.md#adr-286-taskboundary--six-canonical-commands-own-task-state-and-every-host-is-a-thin-wrapper-around-them

use crate::attributes::{KEY_DUE, KEY_TASK, KEY_TASK_STATE, Lifecycle};
use crate::ids::NodeId;
use crate::node::{Attr, AttrValue};
use crate::schema::{Result, WorkspaceDoc};

use super::{TaskFields, lifecycle_def, marker_def};

/// Restore `NodeMeta::done` exactly as the archive recorded it.
///
/// Not [`WorkspaceDoc::set_checkbox`]: that is a *gesture*, guarded and
/// grouped as its own undo step, and an import is neither.
pub fn restore_done(ws: &mut WorkspaceDoc, node: NodeId, done: bool) -> Result<()> {
    ws.set_done(node, done)
}

/// Restore the task-only fields exactly as the archive recorded them.
///
/// A `None` field is left absent — an archive with a due and no marker restores
/// a **due-derived** task, which is what it was. The lifecycle is written only
/// when it is `completed`, because absence is `open` and writing one would be
/// inventing a transition the archive never recorded.
pub fn restore_task(ws: &mut WorkspaceDoc, node: NodeId, fields: &TaskFields) -> Result<()> {
    let existing = |ws: &WorkspaceDoc, role: &str| -> Result<Vec<Attr>> {
        Ok(ws
            .read(node)?
            .content
            .attrs()
            .iter()
            .filter(|a| a.role == role)
            .cloned()
            .collect())
    };
    if let Some(marker) = fields.marker {
        let def = marker_def();
        let records = existing(ws, KEY_TASK)?;
        ws.set_one_attr_raw(
            node,
            &def,
            records,
            AttrValue::Text(marker.as_str().to_owned()),
        )?;
    }
    if let Some(due) = &fields.due {
        let def = crate::attributes::system_definition(KEY_DUE)
            .expect("koru.due is a registered system attribute");
        let records = existing(ws, KEY_DUE)?;
        ws.set_one_attr_raw(node, &def, records, AttrValue::from(due.clone()))?;
    }
    if fields.lifecycle == Lifecycle::Completed {
        let def = lifecycle_def();
        let records = existing(ws, KEY_TASK_STATE)?;
        ws.set_one_attr_raw(
            node,
            &def,
            records,
            AttrValue::Text(Lifecycle::Completed.as_str().to_owned()),
        )?;
    }
    Ok(())
}
