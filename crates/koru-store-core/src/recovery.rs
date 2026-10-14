// SPDX-License-Identifier: MPL-2.0

//! Getting a document back, including when part of it is unreadable.
//!
//! ```text
//! latest sound checkpoint          replay everything above it
//!   │                                 │
//!   ├── verifies      ──────────────► normal
//!   ├── newest damaged, older ok ───► recovered, from that older one
//!   └── none survive  ──────────────► recovered, from the journal alone
//! ```
//!
//! Three rules the whole module is arranged around:
//!
//! - **A recovery is a value, not a log line.** [`open_document`] returns
//!   [`Opened`], so the caller has to hold a [`Recovery`] to get at the
//!   document. Reporting it is the default; swallowing it is a deliberate act.
//! - **Nothing damaged is deleted.** A checkpoint that fails its checksum is
//!   skipped and left exactly where it is. Recovery must not be able to destroy
//!   the thing it was called to preserve, and a failed checkpoint is evidence.
//! - **A bad update stops the replay; it does not skip it.** Loro's history is
//!   causal, so update *n+1* may depend on *n*. Skipping a broken update and
//!   applying what follows would build a document from a history that never
//!   happened. The replay stops at the first failure, reports how many were
//!   left, and the document is whatever was reachable — which is a prefix of
//!   the truth rather than a blend of it.
//!
//! That last one is the difference between a document that is *behind* and a
//! document that is *wrong*, and only one of those can be fixed by syncing.

use crate::errors::Result;
use crate::ids::DocumentId;
use crate::lifecycle::{LiveDocument, checksum};
use crate::ports::{Opened, Recovery, WorkspaceRepo};

/// Load a document: newest sound checkpoint, then replay everything above it.
pub fn open_document(repo: &dyn WorkspaceRepo, id: DocumentId) -> Result<Opened<LiveDocument>> {
    // Everything below decodes Loro bytes, and loro-internal warns about an
    // inconsistent document ("Missing in parent's children") with no field
    // saying which one. The span is what puts the id on the dependency's own
    // lines — the fmt layers print the current span's fields as a prefix.
    let _span = tracing::info_span!("doc", document = %id).entered();
    let Some(state) = repo.document(id)? else {
        // Not a recovery — there is genuinely nothing filed under this id.
        return Ok(Opened::Fresh(LiveDocument::empty(id)));
    };

    let sound = repo.latest_sound_checkpoint(id)?;
    let recorded = state.latest_checkpoint_seq;

    let mut problems: Vec<String> = Vec::new();
    let mut quarantined: Vec<String> = Vec::new();

    // Was there a newer checkpoint that did not survive? The document's
    // recorded position says which one *should* have been used.
    if let Some(expected) = recorded
        && sound.as_ref().map(|c| c.seq) != Some(expected)
    {
        problems.push(match &sound {
            Some(c) => format!(
                "checkpoint {expected} did not verify; fell back to {}",
                c.seq
            ),
            None => format!("checkpoint {expected} did not verify and no earlier one survived"),
        });
        // Named, not moved. The rows stay in the database.
        for header in repo.checkpoints(id)? {
            if header.seq > sound.as_ref().map(|c| c.seq).unwrap_or(0)
                && let Some(c) = repo.checkpoint(id, header.seq)?
                && checksum(&c.snapshot) != c.checksum
            {
                quarantined.push(format!("checkpoint {}", header.seq));
            }
        }
    }

    let (mut live, from) = match &sound {
        Some(c) => (LiveDocument::from_snapshot(id, &c.snapshot)?, Some(c.seq)),
        None => {
            if recorded.is_none() && repo.update_count(id)? == 0 {
                // A document created but never written to. Empty is correct,
                // and it is not a recovery.
                return Ok(Opened::Normal(LiveDocument::empty(id)));
            }
            (LiveDocument::empty(id), None)
        }
    };

    // Replay from the checkpoint that actually loaded — not from the one the
    // document *said* it had, or the updates it covers would be applied twice.
    let pending = repo.updates_after(id, from)?;
    let total = pending.len();
    let mut applied = 0usize;
    for update in pending {
        if let Err(e) = live.import(&update.bytes) {
            problems.push(format!(
                "update {} would not apply ({e}); the replay stopped there",
                update.seq
            ));
            break;
        }
        applied += 1;
    }
    let skipped = total - applied;

    if problems.is_empty() {
        return Ok(Opened::Normal(live));
    }
    Ok(Opened::Recovered(
        live,
        Recovery {
            problem: problems.join("; "),
            from_checkpoint: from,
            quarantined,
            skipped_updates: skipped,
        },
    ))
}

#[cfg(test)]
mod tests;
