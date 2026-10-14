// SPDX-License-Identifier: MPL-2.0

//! **Deleting a run, from the window's side.**
//!
//! Two gestures ask for it — the run row's right-click menu in the Agents
//! navigator, and dragging a run onto the pinned Trash row — and both come
//! through [`MainView::queue_run_trash`], because the confirmation and the
//! receipt live there and no gesture may reach one without the other. That
//! is the same "there is one door" `mainview/trash.rs` is built on, and
//! this is the run-shaped door beside it.
//!
//! ## Two halves, and why the window has one of them
//!
//! A run *is* a Run document in the workspace's manifest, so putting one in
//! the trash is that document's move into the workspace's bin — reversible,
//! synced, and exactly what happens to every other document in Koru
//! ([ADR-69]). That move is a manifest edit, and the manifest belongs to
//! the window. Everything else is device-local and belongs to the delegate
//! worker: refusing a run that is still working, remembering what the
//! person said about its results, and — when the trash is emptied —
//! removing the ledger's rows, the run's authority record and the folder it
//! worked in.
//!
//! So every verb is: **ask the worker, and finish what it answers.** The
//! worker's half happens on the run index's own thread, which is what keeps
//! a named pipe and an HTTP round trip out of a frame (standing requirement
//! 3); [`MainView::drain_run_deletions`] is where its answers land.
//!
//! **The order is not arbitrary on the empty.** The worker goes first and
//! the document only follows on success, because removing a folder is the
//! one step something outside Koru can refuse — and a run whose folder is
//! gone but whose record is not is a run nobody can finish deleting.
//!
//! [ADR-69]: ../../../../docs/decisions/ADR-051-075.md

use std::time::Instant;

use koru_delegate_core::{ArtifactDisposition, FolderOutcome, RunId, RunName};
use koru_i18n::strings::{Key, t};
use koru_i18n::{fill, number};
use koru_runs_ui::{DeletionAnswer, DeletionVerb};
use koru_ui::{Confirm, Toast};

use super::{MainView, ToastPayload, trash::ConfirmSubject};

/// **The same document, spelled for the shell.** The delegation vocabulary
/// and the window's own each have a `DocumentId`; they are the same uuid
/// and two types, so the crossing happens here and in one line rather than
/// at each of the three call sites below.
fn document_of(id: koru_delegate_core::DocumentId) -> koru_core::DocumentId {
    koru_core::DocumentId::from_uuid(id.as_uuid())
}

impl MainView {
    /// **Which of these runs may still be writing**, in the order given.
    ///
    /// The listing this window already holds is the answer, and the menu
    /// asks it twice — once to dim its one row and once for the sentence
    /// beside it — so it is one function rather than two readings of the
    /// same field that could drift apart. The door below partitions the
    /// rows instead, because it needs what each one produced as well.
    pub(super) fn runs_still_working(&self, runs: &[RunId]) -> Vec<RunId> {
        let listing = self.explore.agent_runs();
        runs.iter()
            .copied()
            .filter(|run| {
                listing
                    .iter()
                    .any(|row| row.run == *run && row.motion.unfinished())
            })
            .collect()
    }

    /// **The one door.** Every gesture arrives here — the menu, the drag
    /// onto the bin, one run or a selection of them; the question is asked
    /// here, once for the set, and the runs are only sent to the worker
    /// once it is answered.
    ///
    /// A run that is still working is refused **here as well as there**.
    /// The worker's refusal is the authority — it is the only party that
    /// knows what is running — but this window already has the listing, and
    /// a question about deleting something should not be asked and then
    /// taken back a round trip later.
    ///
    /// **A refusal inside a set does not cancel the set, and is not
    /// silent.** The runs that can go are asked about, and the same dialog
    /// says how many are staying where they are — an operation that
    /// reported success on three of four and never mentioned the fourth
    /// teaches a person that it moved all four.
    pub(super) fn queue_run_trash(&mut self, runs: Vec<RunId>) -> bool {
        let lang = crate::format::lang();
        // Runs the listing no longer holds are dropped without a word: the
        // row each was named from has gone.
        let rows: Vec<koru_runs_ui::RunRow> = runs
            .iter()
            .filter_map(|run| {
                self.explore
                    .agent_runs()
                    .iter()
                    .find(|row| row.run == *run)
                    .cloned()
            })
            .collect();
        if rows.is_empty() {
            return false;
        }
        let (working, going): (Vec<_>, Vec<_>) =
            rows.iter().partition(|row| row.motion.unfinished());
        let Some(first) = going.first() else {
            // Nothing to ask about: every one of them is still writing.
            let message = match working.len() {
                1 => fill(
                    t(lang, Key::RunStillWorking),
                    &[&RunName::of(working[0].run).to_string()],
                ),
                many => fill(
                    t(lang, Key::RunsStillWorking),
                    &[&number::decimal(lang, many as u64)],
                ),
            };
            self.toast = Some((
                Toast::new(message, None, Instant::now()),
                ToastPayload::None,
            ));
            return true;
        };
        // **The run's name for one, a count for several.** A list of names
        // in a title is a title nobody reads to the end of.
        let title = if going.len() == 1 {
            fill(
                t(lang, Key::RunTrashTitle),
                &[&RunName::of(first.run).to_string()],
            )
        } else {
            fill(
                t(lang, Key::RunsTrashTitle),
                &[&number::decimal(lang, going.len() as u64)],
            )
        };
        let results: u32 = going.iter().map(|row| row.results).sum();
        // **The question has three answers when the runs left something
        // behind**, because two things are being decided and they are not
        // the same decision: the runs go into the bin either way, and what
        // happens to their results when the bin is emptied is the person's
        // to choose now, while nothing has happened yet. **Asked once for
        // the set**, never once per run.
        let one = going.len() == 1;
        let body = match (results, one) {
            (0, true) => t(lang, Key::RunTrashBody).to_owned(),
            (0, false) => t(lang, Key::RunsTrashBody).to_owned(),
            (1, true) => t(lang, Key::RunTrashResultsBodyOne).to_owned(),
            (1, false) => t(lang, Key::RunsTrashResultsBodyOne).to_owned(),
            (many, true) => fill(
                t(lang, Key::RunTrashResultsBodyMany),
                &[&number::decimal(lang, u64::from(many))],
            ),
            (many, false) => fill(
                t(lang, Key::RunsTrashResultsBodyMany),
                &[&number::decimal(lang, u64::from(many))],
            ),
        };
        let body = match working.len() {
            0 => body,
            1 => format!("{body} {}", t(lang, Key::RunsTrashWorkingSkippedOne)),
            many => format!(
                "{body} {}",
                fill(
                    t(lang, Key::RunsTrashWorkingSkippedMany),
                    &[&number::decimal(lang, many as u64)],
                )
            ),
        };
        let confirm = if results == 0 {
            Confirm::new(title, body, t(lang, Key::RunTrashConfirm))
                // Reversible, and explicitly invoked: ADR-265's second row.
                .defaulting_to_confirm()
        } else {
            Confirm::new(title, body, t(lang, Key::RunTrashKeepResults))
                .with_alternative(t(lang, Key::RunTrashDeleteResults))
        };
        let going: Vec<RunId> = going.iter().map(|row| row.run).collect();
        self.confirm = Some((confirm, ConfirmSubject::TrashRuns(going)));
        true
    }

    /// The answer, whichever of the three it was. One ask per run on the
    /// index's thread — so a run the worker refuses stops that run and not
    /// the rest, exactly as [`Self::purge_runs`] does; the window hears back
    /// through [`Self::drain_run_deletions`].
    pub(super) fn send_run_trash(
        &mut self,
        runs: Vec<RunId>,
        artifacts: ArtifactDisposition,
    ) -> bool {
        let acted = !runs.is_empty();
        for run in runs {
            self.run_index.trash(run, artifacts);
        }
        acted
    }

    /// Take runs back out of the bin: the worker forgets what was decided
    /// about their results, and the documents come back out. One ask each,
    /// for [`Self::send_run_trash`]'s reason.
    pub(super) fn restore_runs(&mut self, runs: Vec<RunId>) -> bool {
        let acted = !runs.is_empty();
        for run in runs {
            self.run_index.restore_run(run);
        }
        acted
    }

    /// **Empty the trash of these runs.** One purge each, so a folder the
    /// filesystem refuses stops that run and not the rest.
    pub(super) fn purge_runs(&mut self, runs: Vec<RunId>) -> bool {
        let acted = !runs.is_empty();
        for run in runs {
            self.run_index.purge(run);
        }
        acted
    }

    /// The runs the workspace's bin is holding, as the index last read
    /// them. What the Trash tab draws and what emptying it acts on.
    pub(super) fn trashed_runs(&self) -> Vec<RunId> {
        self.run_index.trashed().iter().map(|row| row.run).collect()
    }

    /// **Which of these Trash rows are runs.**
    ///
    /// The bin holds two kinds of thing in one table, and the two are
    /// restored and deleted along different paths — a run's half of each is
    /// the delegate worker's. A run's row is keyed by the run's own uuid,
    /// the same identity the Agents navigator gives it, so the split is a
    /// lookup in the listing rather than a guess about an id's shape.
    ///
    /// The same shape as `split_documents` beside it, and for the same
    /// reason: sending a row down the wrong path is a gesture that fails in
    /// silence.
    pub(super) fn split_runs(
        &self,
        nodes: Vec<koru_core::NodeId>,
    ) -> (Vec<RunId>, Vec<koru_core::NodeId>) {
        let trashed = self.run_index.trashed();
        let mut runs = Vec::new();
        let mut rest = Vec::new();
        for node in nodes {
            match trashed
                .iter()
                .find(|row| row.run.as_uuid() == node.as_uuid())
            {
                Some(row) => runs.push(row.run),
                None => rest.push(node),
            }
        }
        (runs, rest)
    }

    /// **What the worker did, finished here.** Each answer moves the Run
    /// document and says what happened; nothing here decides anything the
    /// worker has not already settled.
    pub(super) fn drain_run_deletions(&mut self) -> bool {
        let answers = self.run_index.take_deletions();
        if answers.is_empty() {
            return false;
        }
        let lang = crate::format::lang();
        // **One receipt for the batch.** A set of four trashed runs answers
        // four times, and four toasts in a row is three receipts nobody
        // reads and one Undo that takes back a quarter of the gesture — the
        // reason `trashed_batch_message` exists on the document side. The
        // trash and restore halves are gathered here and spoken for once
        // below; the empty stays per run, where the folder's fate is a fact
        // about that run (docs/deferred/README.md).
        let mut trashed: Vec<RunId> = Vec::new();
        let mut restored: Vec<RunId> = Vec::new();
        for answer in answers {
            match answer {
                DeletionAnswer::Trashed { run, document } => {
                    self.ask_pane(koru_workspace_host::PaneRequest::TrashDocuments(vec![
                        document_of(document),
                    ]));
                    self.close_tabs_showing_run(run);
                    trashed.push(run);
                }
                DeletionAnswer::Restored { run, document } => {
                    self.ask_pane(koru_workspace_host::PaneRequest::RestoreDocuments(vec![
                        document_of(document),
                    ]));
                    restored.push(run);
                }
                DeletionAnswer::Purged {
                    run,
                    document,
                    folder,
                    ..
                } => {
                    // The device-local half has already happened; the
                    // document is the last of it.
                    self.ask_pane(koru_workspace_host::PaneRequest::PurgeDocuments(vec![
                        document_of(document),
                    ]));
                    let name = RunName::of(run).to_string();
                    // **The trash says what it did about the folder.** An
                    // operation that quietly does less than it offered
                    // teaches a person that it did more.
                    let key = match folder {
                        FolderOutcome::Removed => Key::RunDeletedFolderRemoved,
                        FolderOutcome::Kept => Key::RunDeletedFolderKept,
                        FolderOutcome::Absent => Key::RunDeletedFolderAbsent,
                    };
                    self.toast = Some((
                        Toast::new(fill(t(lang, key), &[&name]), None, Instant::now()),
                        ToastPayload::None,
                    ));
                }
                DeletionAnswer::Refused { run, verb, code } => {
                    let name = RunName::of(run).to_string();
                    let message = match (verb, code.as_str()) {
                        (_, "wrong_status") => fill(t(lang, Key::RunStillWorking), &[&name]),
                        (DeletionVerb::Purge, "run_folder_not_removed") => {
                            fill(t(lang, Key::RunDeleteFolderRefused), &[&name])
                        }
                        // Anything else is the gateway or the worker not
                        // answering. The code is stable so a log can carry
                        // it; what a person reads is that nothing changed.
                        _ => fill(t(lang, Key::RunDeleteFailed), &[&name]),
                    };
                    self.toast = Some((
                        Toast::new(message, None, Instant::now()),
                        ToastPayload::None,
                    ));
                }
            }
        }
        // After the per-run answers, so a batch's receipt is what stands.
        if !restored.is_empty() {
            let message = match restored.len() {
                1 => fill(
                    t(lang, Key::RunRestored),
                    &[&RunName::of(restored[0]).to_string()],
                ),
                many => fill(
                    t(lang, Key::RunsRestored),
                    &[&number::decimal(lang, many as u64)],
                ),
            };
            self.toast = Some((
                Toast::new(message, None, Instant::now()),
                ToastPayload::None,
            ));
        }
        if !trashed.is_empty() {
            let message = match trashed.len() {
                1 => fill(
                    t(lang, Key::RunTrashed),
                    &[&RunName::of(trashed[0]).to_string()],
                ),
                many => fill(
                    t(lang, Key::RunsTrashed),
                    &[&number::decimal(lang, many as u64)],
                ),
            };
            self.toast = Some((
                Toast::new(message, Some(t(lang, Key::TrashUndo)), Instant::now()),
                // **One Undo for the whole set**: taking back the last of
                // four is not an undo of what was asked for.
                ToastPayload::RestoreRuns(trashed),
            ));
        }
        // The listing on screen is now a lie in both halves.
        self.run_index.refresh();
        self.explore_stale = true;
        true
    }

    /// Close the tabs this window has open on a run that is going into the
    /// bin — its own tab, and nothing else.
    ///
    /// **Deliberately local**, exactly as `close_tabs_showing` is for a
    /// document: a peer's window keeps its tab. And deliberately not the
    /// run's *results*: a result is not the run any more (ADR-368), it
    /// outlives the run's tab, and the person may well be reading one.
    fn close_tabs_showing_run(&mut self, run: RunId) {
        let doomed: Vec<usize> = (0..self.tabs().len())
            .rev()
            .filter(|&i| {
                matches!(
                    self.tabs().get(i).map(|tab| &tab.view),
                    Some(super::tab::TabView::Run(view)) if view.run() == Some(run)
                )
            })
            .collect();
        for i in doomed {
            self.close(i);
        }
    }
}

#[cfg(test)]
mod tests;
