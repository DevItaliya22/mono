// SPDX-License-Identifier: MPL-2.0

//! **The activity drain** — the container's half of the desktop task wrapper
//! ([ADR-286]).
//!
//! A task gesture happens *inside a view*: the checkbox is
//! `Body::set_done`, the `/due` arm is the editor's, and both hold the document
//! for the length of the call. Neither can name `koru-workspace-host`, so
//! neither emits. What they do instead is run the canonical command and leave
//! the [`TaskActivity`] it answered — the causation id and the typed intents,
//! in outcome order — on the body.
//!
//! This module is what takes them. **After** an event has been dispatched, the
//! container drains every editor it holds and hands each batch to
//! [`TaskHost::attempt`], which is where the attribution and the scheduling
//! live. That ordering is the whole point: the drain runs when the gesture is
//! finished and the document is no longer borrowed, so a wrapper cannot find
//! itself half way through an edit.
//!
//! **Why every editor and not just the active one.** A tab that is not on
//! screen is still a live `Body`, and a menu row or an explore-column action
//! can apply a command to a document another tab is rooted at. Draining only
//! the tab in front would strand those intents until that tab happened to be
//! clicked — an event silently late by an arbitrary amount, which is worse than
//! one that is never sent. The sweep costs a `Vec::is_empty` per editor.
//!
//! **Nothing is allocated when nothing happened.** [`take_task_activity`] is a
//! `mem::take` of a `Vec` that is almost always empty, and the [`TaskHost`] is
//! built only once a batch exists. It is a wrapper over a `Box<dyn FnMut>`, so
//! building one where we stand is one allocation on a path that is already
//! sending an event; keeping one on the container would mean threading it
//! through a `ManagerView` that is an enum and has no field to put it in.
//!
//! [`take_task_activity`]: koru_editor::Body::take_task_activity
//! [ADR-286]: ../../../docs/decisions/ADR-276-300.md#adr-286-taskboundary--six-canonical-commands-own-task-state-and-every-host-is-a-thin-wrapper-around-them

use koru_core::EntityRef;
use koru_editor::tasks::TaskActivity;
use koru_workspace_host::tasks::TaskHost;

#[cfg(test)]
mod tests;

/// Hand a drained batch to the desktop wrapper.
///
/// Empty is the ordinary case and costs nothing: no host is built, no counter
/// moves, no line is printed.
pub fn attempt(owed: Vec<TaskActivity>) {
    if owed.is_empty() {
        return;
    }
    let mut host = TaskHost::default();
    for batch in owed {
        host.attempt(batch.causation, &batch.intents);
    }
    debug_counters();
}

/// **Emit the coalesced `task.content.changed` for a flushed burst.**
///
/// The sibling of [`attempt`], and separate for the reason
/// [`TaskHost::content_changed`] is separate from `TaskHost::attempt`: this
/// event belongs to no command, so there is no causation id to inherit and no
/// [`ActivityIntent`] to map. What arrives here has already been decided — by
/// [`TaskCaret`], by a closing tab, or by the host's idle tick — and this
/// function decides nothing.
///
/// Empty is the ordinary case and costs nothing.
///
/// [`ActivityIntent`]: koru_core::tasks::ActivityIntent
pub fn content_changed(tasks: &[EntityRef]) {
    if tasks.is_empty() {
        return;
    }
    TaskHost::default().content_changed(tasks);
    debug_counters();
}

// ------------------------------------------------------- the caret boundary

/// **What this window's caret was last sitting in**, and the whole of the
/// caret half of ADR-273's burst boundary.
///
/// A window has exactly one caret, which is why this is one `Option` rather
/// than a map: the burst a window can keep open is the one under the row it is
/// typing in, and *every* other burst it opened has already ended. That is not
/// an approximation — [`dispatched`](Self::dispatched) closes every task it is
/// handed that the caret is not on, in the same pass that opens it — so at most
/// one of this window's bursts is ever open, and remembering which is all the
/// state the boundary needs.
///
/// It is deliberately **not** in the editor. The editor notes *that* a task's
/// text changed; deciding that a burst ended is the host's, and keeping the two
/// on opposite sides of the crate boundary is what keeps "one event per task
/// per burst" a property of one type rather than an agreement between four.
#[derive(Default)]
pub struct TaskCaret {
    on: Option<EntityRef>,
}

impl TaskCaret {
    /// **One dispatched event's worth of editing**, resolved into the bursts it
    /// ended. The caller emits them; nothing is emitted from here.
    ///
    /// `edits` is what the view's editors noted since the last event — local
    /// gestures only — and `caret` is the task the caret is in *now*. Three
    /// things follow, in this order:
    ///
    /// 1. every edit opens or refreshes its burst;
    /// 2. an edit the caret is **not** on ends immediately — a menu row or an
    ///    explore-column action can change a task in a tab nobody is looking
    ///    at, and there is no caret there to leave it later;
    /// 3. the task the caret has *left* since the previous event ends, which is
    ///    the ordinary case: you type, and then you click somewhere else.
    ///
    /// Nothing is allocated and no document is read when this window has no
    /// burst open and nothing was edited, which is every event in a session
    /// that is not editing a task.
    pub fn dispatched(
        &mut self,
        edits: Vec<EntityRef>,
        caret: impl FnOnce() -> Option<EntityRef>,
    ) -> Vec<EntityRef> {
        if edits.is_empty() && self.on.is_none() {
            return Vec::new();
        }
        let now = std::time::Instant::now();
        let caret = caret();
        let mut ended = Vec::new();
        for task in edits {
            koru_workspace_host::tasks::note_content_edit(task, now);
            if Some(task) != caret {
                ended.extend(koru_workspace_host::tasks::content_caret_left(&task));
            }
        }
        if let Some(left) = self.on.filter(|left| Some(*left) != caret) {
            ended.extend(koru_workspace_host::tasks::content_caret_left(&left));
        }
        self.on = caret;
        ended
    }

    /// The task this window's caret is holding a burst open on, if any.
    pub fn open(&self) -> Option<EntityRef> {
        self.on
    }

    /// **The row is gone** — a tab closed over it. There is no caret left to
    /// leave it and no tick will ever see it again, so its burst ends now.
    pub fn gone(&mut self) -> Vec<EntityRef> {
        let Some(task) = self.on.take() else {
            return Vec::new();
        };
        koru_workspace_host::tasks::content_closed(&[task])
    }
}

/// One line, gated on `KORU_TASKS_DEBUG`, naming what the wrapper has done
/// since the process started.
///
/// **Counts and closed tokens, never content** — the same rule the intents
/// themselves follow, and the reason the counters are coalesced rather than a
/// line per event ([LOGGING]).
///
/// [LOGGING]: ../../../docs/contributing/operations/LOGGING.md
fn debug_counters() {
    if !enabled() {
        return;
    }
    eprintln!(
        "[koru:tasks] {}",
        koru_workspace_host::tasks::counters().describe()
    );
}

/// One line under the same switch, for the two store-backed surfaces' waits:
/// *covered*, *retrying*, *gave up*, *unsupported*, *unpublished*, *failed* —
/// transitions and counts, never a document's content, a title or a version
/// vector. What a person reads back to say how long the read model lagged.
pub fn debug_line(line: std::fmt::Arguments<'_>) {
    // The host's writer, so the line reaches `koru-tasks-debug.log` beside
    // the command outcomes rather than only the console.
    koru_workspace_host::app::tasks_debug_log(line);
}

fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| koru_env::switch(koru_env::name::KORU_TASKS_DEBUG))
}
