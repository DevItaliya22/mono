// SPDX-License-Identifier: MPL-2.0

//! The editor's seam onto the six canonical task commands.
//!
//! **The editor issues commands and consumes in-memory results; its host owns
//! I/O** ([ADR-286]). A gesture here — a checkbox click, a `/due`, a chip's
//! `×`, a paste — runs a `koru_core::tasks` command against the document it
//! was handed, takes the [`TaskOutcome`] back synchronously, and leaves what
//! the *host* owes behind in a sink the host drains. Nothing in this crate
//! emits an event, opens a file or dials a socket, which is what makes the
//! no-I/O-on-the-event-thread assertion provable rather than reviewed.
//!
//! [`run`] is the editor-visible path, and it is a closure taker for the same
//! reason the host wrapper is: *the editor runs a command* is then structural
//! rather than a convention. It is also what the golden cases replay through —
//! `crates/koru-editor/src/tasks/tests.rs` hands `Command::apply_to` straight in,
//! so this seam cannot quietly run something else.
//!
//! [ADR-286]: ../../../docs/decisions/ADR-276-300.md#adr-286-taskboundary--six-canonical-commands-own-task-state-and-every-host-is-a-thin-wrapper-around-them

use std::sync::atomic::{AtomicU64, Ordering};

use koru_core::schema::WorkspaceDoc;

#[cfg(test)]
mod tests;

// ------------------------------------------------- what refused, as a number

/// Attribute commits that landed on **nothing at all**.
///
/// **Coalesced, content-free, and not a log line** — the rule the host's
/// `tasks::Counters` already follows, for the reason stated at the top of this
/// module: nothing in this crate emits an event, opens a file or dials a
/// socket, so a number is the whole of what a view may honestly keep. There is
/// deliberately no environment read here; the debug row belongs to whoever owns
/// `KORU_TASKS_DEBUG`.
///
/// It exists because the alternative was silence. A `/due` whose command
/// refused used to take the typed trigger text out and report nothing, so the
/// only evidence a user had was their own typing disappearing.
static REFUSED_COMMITS: AtomicU64 = AtomicU64::new(0);

/// Record one attribute commit that changed nothing.
pub fn note_refused_commit() {
    REFUSED_COMMITS.fetch_add(1, Ordering::Relaxed);
}

/// How many attribute commits have refused since the process started.
pub fn refused_commits() -> u64 {
    REFUSED_COMMITS.load(Ordering::Relaxed)
}

/// The seam's vocabulary, **re-exported from the domain** — the types and the
/// two runners moved down to `koru_core::tasks::routed` when the Tasks page
/// became the second host to write a routed attribute, and the names stayed
/// here so no call site moved. See that module for why.
pub use koru_core::tasks::routed::{ActivitySink, SINK_LIMIT, TaskActivity, remember, run};

/// **Today, in this machine's zone** — the day a completion is recorded on.
///
/// `koru_core::tasks::complete_task` takes the day rather than reading one:
/// *today* is a zone question and the domain carries no timezone database,
/// the rule [`BucketWindows`] already states from the other side. This is the
/// editor's answer to it, in one place so three call sites cannot drift.
///
/// [`BucketWindows`]: koru_store_wire::tasks::BucketWindows
pub fn today() -> koru_core::time::DateOnly {
    koru_core::time::DateOnly::new(chrono::Local::now().date_naive())
}

// ------------------------------------------------- the content-burst note
//
// `task.content.changed` is the one event with no [`ActivityIntent`]: it
// belongs to no command, because the domain does not know what an editing burst
// is. Its coalescing owner is the *host* wrapper
// (`koru_workspace_host::tasks::ContentBursts`), and all this crate owes it is
// the note ADR-273 describes — "the burst set is a map the editor's own edit
// path inserts into; emission reads it".

/// How many tasks may be noted before the oldest is let go.
///
/// The same bound and the same reasoning as [`SINK_LIMIT`]: a host that never
/// drains is a host with no activity feed, and that must not also be a host
/// with a growing `Vec`. A key here is a *task*, not a keystroke.
pub const EDITED_LIMIT: usize = 512;

/// **The `task:` reference for one node, or `None` when it is not a task.**
///
/// One function rather than the spelling written twice, because two callers ask
/// the same question from opposite ends of a burst: the edit path asks *did I
/// just change a task's text*, and the boundary asks *which task is the caret
/// sitting on*. A second copy of the minting is how one of them would end up
/// naming a subject the other could not match, and a burst that never closes is
/// the failure that produces. The minting itself is one layer further down, in
/// [`koru_core::tasks::refs::task_ref`], which is what makes *every* surface's
/// spelling one spelling; what this adds is the task test.
///
/// A document with no workspace identity answers `None`: that is the same read
/// every command's own preflight makes first, and a reference missing its first
/// segment is not one.
pub fn task_ref(ws: &WorkspaceDoc, node: koru_core::NodeId) -> Option<koru_core::EntityRef> {
    if !matches!(koru_core::tasks::is_task_node(ws, node), Ok(true)) {
        return None;
    }
    koru_core::tasks::refs::task_ref(ws, node)
}

/// **The tasks whose own text this view has edited since the host last looked.**
///
/// Deliberately not a bag of [`ActivityIntent`]s. The intent list is what a
/// *command* answered; this is a set of subjects that names no event, carries
/// no reason and decides no boundary — the host decides all three, which is
/// what keeps the coalescing rule in exactly one place.
///
/// **Never the text**, and structurally so: an [`koru_core::EntityRef`] is
/// three ids and there is nowhere here to put a string.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct TaskTextEdits {
    /// Insertion-ordered and deduplicated. A `Vec` rather than a `HashSet`
    /// because it is almost always empty, at most a handful long, and the order
    /// it is handed to the host in should be the order the user typed in.
    tasks: Vec<koru_core::EntityRef>,
}

impl TaskTextEdits {
    /// **One edit landed on a node's own text.** Records nothing unless that
    /// node is a task.
    ///
    /// The task test is one meta read of a node the caller has just written to,
    /// not a walk — and it is asked *here*, per edit, rather than at the
    /// boundary, because by the time a burst is flushed the node may have been
    /// deleted and a reference to it would name nothing.
    ///
    /// A document with no workspace identity records nothing: that is the same
    /// read every command's own preflight makes first, and a reference missing
    /// its first segment is not one.
    pub fn note(&mut self, ws: &WorkspaceDoc, node: koru_core::NodeId) {
        let Some(task) = task_ref(ws, node) else {
            return;
        };
        if self.tasks.contains(&task) {
            return;
        }
        if self.tasks.len() >= EDITED_LIMIT {
            self.tasks.remove(0);
        }
        self.tasks.push(task);
    }

    /// What the host has not seen yet, taken.
    pub fn take(&mut self) -> Vec<koru_core::EntityRef> {
        std::mem::take(&mut self.tasks)
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }
}
