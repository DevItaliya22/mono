// SPDX-License-Identifier: MPL-2.0

//! The MCP host's task wrapper — [ADR-286]'s named path for this crate.
//!
//! A wrapper adds **scheduling and attribution and nothing else**. It never
//! validates, never decides a lifecycle, never reads a projection row, and
//! never invents an event the outcome did not name. That is why it is shaped
//! as a closure taker rather than a method per command: *a host runs a command*
//! is then structural rather than a convention somebody has to keep.
//!
//! What this host owes a task change, and how it differs from the desktop's
//! ([`docs/architecture/DATA_FLOW.md`], §*A task gesture, and what each host
//! owes it*):
//!
//! - **Synchronous with the edit**: the command, on the bridge's own worker.
//!   `apply_ops` runs twice as it already did — once on a `fork_for_read` to
//!   validate, so a refusal never reaches the real document — and a task
//!   command's preflight therefore runs on the fork first.
//! - **Off-thread: nothing.** MCP has no UI thread to protect, so there is no
//!   scheduling here at all; the wrapper's whole contribution is attribution
//!   and the activity attempt.
//! - **Success is shown after durability.** The host's existing
//!   `Store::persist` blocks up to ten seconds and answers `durability_timeout`
//!   otherwise, and a task op leaves that **exactly** as it is: a caller that
//!   got `ok` knows the bytes are on the disk (ADR-79).
//! - **A persistence failure surfaces as the existing `apply_ops` error.** The
//!   CRDT edit is not rolled back — there is no rollback — and the next
//!   successful submit carries it. Identical to every other MCP edit.
//! - **Activity is attempted after the durability wait**, best effort, one
//!   attempt, failures logged coalesced. A lost diagnostic never costs somebody
//!   their edit (SPEC §9).
//! - **Attribution is [`ACTOR`]** — `Actor::agent("mcp")` in the events
//!   vocabulary. It is the only identity a task event carries here, and it is
//!   **not a permission**: SPEC §10 defers fine-grained task authorization
//!   entirely, and nothing in this module branches on who is asking.
//!
//! ## There is no events emission seam in this process yet
//!
//! `koru-mcp-server` links no events crate, and it may not acquire one as part
//! of this plan. So the mapping from a domain [`ActivityIntent`] to the
//! registry row's *name, subject, correlation and attrs* lives here as
//! [`ActivityAttempt`] — the payload in a wire-neutral form, key for key and
//! token for token with `koru_events::catalog`'s twelve `Task*` payloads — and
//! it leaves through the [`ActivitySink`] callback [ADR-286] describes. The
//! shipped sink is [`LogSink`], which records one coalesced line per call.
//!
//! Whoever wires the real journal replaces that sink with one that builds
//! `koru_events::catalog::Task*` and calls `Draft::in_chain(EventId(
//! attempt.correlation))` — the correlation is already the command's own
//! [`CausationId`], so a cascade groups identically here and on the desktop.
//! Nothing else in this module moves.
//!
//! [ADR-286]: ../../../docs/decisions/ADR-276-300.md#adr-286-taskboundary--six-canonical-commands-own-task-state-and-every-host-is-a-thin-wrapper-around-them
//! [`docs/architecture/DATA_FLOW.md`]: ../../../docs/architecture/DATA_FLOW.md

use uuid::Uuid;

use koru_core::attributes::{Priority, TaskReason};
use koru_core::schema::Result as CoreResult;
use koru_core::tasks::{ActivityIntent, AttrChange, ChildClass, TaskOutcome};
use koru_core::time::TemporalKind;
use koru_core::{EntityRef, Kind, WorkspaceDoc};

/// Who a task change made through this bridge is attributed to.
///
/// The registered lowercase token behind `Actor::agent("mcp")`:
/// `ActorKind::Agent`, no `ActorId`. The desktop emits `Actor::user()`, and
/// **that difference is the whole of the difference** — the same
/// `TaskOutcome`, the same `LifecycleReason`, the same `CausationId` and the
/// same intent list reach both.
pub const ACTOR: &str = "mcp";

/// One attribute of an activity attempt, in the three shapes the twelve task
/// rows use.
///
/// Deliberately the same closed set as `koru_events::AttrValue`'s
/// `Token`/`Id`/`Flag`: there is **no free-string variant**, which is what
/// makes "no title, no text, no date, no tag name, no color, no custom value"
/// a structural guarantee here rather than a review promise.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AttrToken {
    /// A closed vocabulary token.
    Token(&'static str),
    /// A `NodeId`'s UUID — privacy class `LocalReference`, stripped
    /// mechanically at any telemetry boundary.
    Id(Uuid),
    Flag(bool),
}

/// The triple behind a `task:<workspace>:<document>:<node>` subject.
///
/// UUIDs rather than the domain newtypes, because that is the shape the events
/// crate's `TaskSubject` takes: the conversion happens once, here, at the
/// wrapper.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TaskSubject {
    pub workspace: Uuid,
    pub document: Uuid,
    pub node: Uuid,
}

impl TaskSubject {
    fn of(task: &EntityRef) -> Self {
        Self {
            workspace: task.workspace_id().as_uuid(),
            document: task.document_id().as_uuid(),
            node: task.node_id().as_uuid(),
        }
    }
}

/// What this host would append to the activity journal for one
/// [`ActivityIntent`] — an **attempt**, never a guarantee (SPEC §9: the task
/// mutation commits first, and a failed append never rolls it back).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ActivityAttempt {
    /// The registry name — ADR-273's table, taken from the intent rather than
    /// decided here.
    pub event: &'static str,
    pub subject: TaskSubject,
    /// Always [`ACTOR`].
    pub actor: &'static str,
    /// The command's own causation id, carried as `correlation_id` on every
    /// envelope of one command, cascade included.
    ///
    /// `None` for `task.content.changed` and for nothing else: that event
    /// belongs to no command, so it has no causation id to inherit and **is its
    /// own chain root** (ADR-273). An `Option` rather than a zero UUID because
    /// a reader grouping by chain root has to be able to tell "no chain" from
    /// "a chain whose id happens to be nil".
    pub correlation: Option<Uuid>,
    /// The row's attrs, in the registry's declaration order.
    pub attrs: Vec<(&'static str, AttrToken)>,
}

/// Where activity attempts go once the edit is durable.
///
/// A callback rather than a dependency: this crate links no events crate, and
/// [ADR-286] does not let it acquire one to say something was ticked. An
/// implementation that builds `koru_events::catalog::Task*` payloads belongs to
/// whichever process owns the journal.
///
/// [ADR-286]: ../../../docs/decisions/ADR-276-300.md#adr-286-taskboundary--six-canonical-commands-own-task-state-and-every-host-is-a-thin-wrapper-around-them
pub trait ActivitySink {
    fn append(&mut self, attempt: &ActivityAttempt);

    /// Called once after the batch, whether or not anything was appended.
    /// **Coalesced by construction**: a sink reports per call, never per
    /// attempt, because a batch that touched forty tasks must not be forty log
    /// lines.
    fn finish(&mut self, _appended: usize) {}
}

/// The sink this process has today: one `tracing` line per `edit_document`
/// call that produced any task activity, naming counts and tokens and **no
/// content**.
#[derive(Default)]
pub struct LogSink {
    events: Vec<&'static str>,
}

impl ActivitySink for LogSink {
    fn append(&mut self, attempt: &ActivityAttempt) {
        self.events.push(attempt.event);
    }

    fn finish(&mut self, appended: usize) {
        if appended == 0 {
            return;
        }
        // One line for the batch. Event names and a count — a subject id is a
        // reference and a token is closed, but neither belongs in an ordinary
        // log line that nobody asked for.
        tracing::debug!(
            actor = ACTOR,
            attempts = appended,
            events = ?self.events,
            "task activity is mapped and has nowhere to go yet",
        );
        self.events.clear();
    }
}

/// The wrapper: one per `edit_document` call.
///
/// It accumulates what the ops in that call produced, because **MCP's burst is
/// the call** — a batch API's natural unit of "somebody was editing" is the
/// batch, and inventing a timer inside a request would be a worse answer than
/// none.
#[derive(Default, Debug)]
pub struct Wrapper {
    outcomes: Vec<TaskOutcome>,
    attempts: Vec<ActivityAttempt>,
    /// The tasks whose own text this call changed, deduplicated — see
    /// [`note_content_changed`](Self::note_content_changed).
    content: Vec<TaskSubject>,
}

impl Wrapper {
    pub fn new() -> Self {
        Self::default()
    }

    /// **A text operation changed a task's own text.**
    ///
    /// [ADR-286] gives MCP a different burst from the desktop's and gives the
    /// reason: *the batch is the burst*. A batch API's natural unit of
    /// "somebody was editing" is the call, and inventing a timer inside a
    /// request would be a worse answer than none — so there is no clock here,
    /// no five-second quiet period and no caret.
    ///
    /// **Coalesced by the data structure**: the record is a set of *tasks*, so
    /// forty `replace_text` ops on one task are one event, and two tasks in one
    /// call are one event each — the same "one per task per burst" the desktop
    /// keeps, with the call standing in for the burst.
    ///
    /// A node that is not a task records nothing, and a document with no
    /// workspace identity records nothing: a reference missing its first
    /// segment is not one, and the same read is what every command's own
    /// preflight makes first.
    ///
    /// [ADR-286]: ../../../docs/decisions/ADR-276-300.md#adr-286-taskboundary--six-canonical-commands-own-task-state-and-every-host-is-a-thin-wrapper-around-them
    pub fn note_content_changed(
        &mut self,
        ws: &WorkspaceDoc,
        node: koru_core::NodeId,
    ) -> CoreResult<()> {
        if !koru_core::tasks::is_task_node(ws, node)? {
            return Ok(());
        }
        let Some(reference) = koru_core::tasks::refs::task_ref(ws, node) else {
            return Ok(());
        };
        let subject = TaskSubject::of(&reference);
        if !self.content.contains(&subject) {
            self.content.push(subject);
        }
        Ok(())
    }

    /// Run one canonical command. [ADR-286]'s `run`.
    ///
    /// The closure is the whole of what the domain is asked to do; everything
    /// this function adds — the attribution and the recorded attempt — is
    /// invisible to the outcome by design, because none of it may change a
    /// domain answer.
    ///
    /// [ADR-286]: ../../../docs/decisions/ADR-276-300.md#adr-286-taskboundary--six-canonical-commands-own-task-state-and-every-host-is-a-thin-wrapper-around-them
    pub fn run(
        &mut self,
        ws: &mut WorkspaceDoc,
        command: impl FnOnce(&mut WorkspaceDoc) -> CoreResult<TaskOutcome>,
    ) -> CoreResult<TaskOutcome> {
        let outcome = self.run_optional(ws, |ws| command(ws).map(Some))?;
        // Unreachable by construction: the closure above wrapped a `TaskOutcome`
        // in `Some` and `run_optional` returns what the command answered.
        outcome.ok_or(koru_core::StoreError::Malformed(
            "a task command answered nothing",
        ))
    }

    /// The same, for the classification hook, which answers `None` when
    /// nothing above the changed node was completed.
    ///
    /// It exists because `tasks::reclassify` is in the golden table and in the
    /// vocabulary a host must be able to run; ADR-273 writes `run`'s signature
    /// for the six commands, which always answer `Some`.
    pub fn run_optional(
        &mut self,
        ws: &mut WorkspaceDoc,
        command: impl FnOnce(&mut WorkspaceDoc) -> CoreResult<Option<TaskOutcome>>,
    ) -> CoreResult<Option<TaskOutcome>> {
        let outcome = command(ws)?;
        if let Some(outcome) = outcome.as_ref() {
            self.attempts.extend(attempts_of(outcome));
            self.outcomes.push(outcome.clone());
        }
        Ok(outcome)
    }

    /// **Record one intent that belongs to no command.**
    ///
    /// The **priority door** is the only case ([ADR-286]'s routing table, and
    /// `koru_editor::tasks::TaskActivity::alone` is the desktop's identical
    /// half): setting or clearing `koru.priority` on a node that is *already* a
    /// task takes the ordinary attribute door — no command runs, no lifecycle
    /// moves, no marker is written — and then owes a `task.priority.changed`.
    /// Priority on a plain node owes nothing at all, which is why the caller
    /// asks `route_attribute` first rather than this function guessing.
    ///
    /// With no command there is no causation id to inherit, so one is minted
    /// here and the intent is its own chain root.
    ///
    /// [ADR-286]: ../../../docs/decisions/ADR-276-300.md#adr-286-taskboundary--six-canonical-commands-own-task-state-and-every-host-is-a-thin-wrapper-around-them
    pub fn note_intent(&mut self, intent: ActivityIntent) {
        let causation = koru_core::tasks::CausationId::new();
        self.attempts.push(ActivityAttempt {
            event: intent.event(),
            subject: TaskSubject::of(intent.subject()),
            actor: ACTOR,
            correlation: Some(causation.0),
            attrs: attrs_of(&intent),
        });
    }

    /// Every outcome this call produced, in order.
    pub fn outcomes(&self) -> &[TaskOutcome] {
        &self.outcomes
    }

    /// Every activity attempt this call produced, in delivery order: the
    /// commands' in the order they ran, then the call's coalesced
    /// `task.content.changed` tail.
    ///
    /// The content attempts are built here rather than accumulated because
    /// there is nothing to accumulate — the row has no attrs and no
    /// correlation, so a subject is the whole of it.
    pub fn attempts(&self) -> Vec<ActivityAttempt> {
        let mut all = self.attempts.clone();
        all.extend(self.content.iter().map(|&subject| ActivityAttempt {
            event: "task.content.changed",
            subject,
            actor: ACTOR,
            // Its own chain root: no command produced it, so there is no
            // causation id to carry.
            correlation: None,
            // Empty by construction — the text is what it must never carry.
            attrs: Vec::new(),
        }));
        all
    }

    pub fn is_empty(&self) -> bool {
        self.attempts.is_empty() && self.content.is_empty()
    }

    /// Hand the batch's attempts to a sink. **Called after the durability
    /// wait**, so an appended activity always describes bytes that are on the
    /// disk.
    pub fn deliver(&self, sink: &mut dyn ActivitySink) {
        let attempts = self.attempts();
        for attempt in &attempts {
            sink.append(attempt);
        }
        sink.finish(attempts.len());
    }
}

/// Map one outcome's intents onto the registry rows.
///
/// An exhaustive `match` over [`ActivityIntent`], which is what makes a
/// thirteenth intent a compile error here rather than an event that silently
/// stops being emitted.
fn attempts_of(outcome: &TaskOutcome) -> Vec<ActivityAttempt> {
    outcome
        .activity
        .iter()
        .map(|intent| ActivityAttempt {
            event: intent.event(),
            subject: TaskSubject::of(intent.subject()),
            actor: ACTOR,
            correlation: Some(outcome.causation.0),
            attrs: attrs_of(intent),
        })
        .collect()
}

/// The row's attrs, key for key and token for token with ADR-273's table.
fn attrs_of(intent: &ActivityIntent) -> Vec<(&'static str, AttrToken)> {
    use AttrToken::{Flag, Id, Token};
    match intent {
        ActivityIntent::Created {
            reason, node_kind, ..
        } => vec![
            ("reason", Token(created_reason(*reason))),
            ("node_kind", Token(node_kind_token(*node_kind))),
        ],
        ActivityIntent::Removed { .. } => Vec::new(),
        ActivityIntent::Completed { reason, .. } | ActivityIntent::Reopened { reason, .. } => {
            vec![("reason", Token(reason.as_str()))]
        }
        ActivityIntent::DueChanged {
            change,
            temporal_kind,
            ..
        } => vec![
            ("change", Token(change_token(*change))),
            ("temporal_kind", Token(temporal_token(*temporal_kind))),
        ],
        ActivityIntent::PriorityChanged {
            change, priority, ..
        } => {
            let mut attrs = vec![("change", Token(change_token(*change)))];
            // Absent means removed, never `low` — the row declares `priority`
            // optional for exactly this.
            if let Some(priority) = priority {
                attrs.push(("priority", Token(priority_token(*priority))));
            }
            attrs
        }
        ActivityIntent::ChildAdded {
            child,
            class,
            reopened,
            ..
        }
        | ActivityIntent::ChildMoved {
            child,
            class,
            reopened,
            ..
        } => vec![
            ("child", Id(child.as_uuid())),
            ("child_class", Token(class_token(*class))),
            ("reopened", Flag(*reopened)),
        ],
        ActivityIntent::ChildRemoved { child, .. } => vec![("child", Id(child.as_uuid()))],
        ActivityIntent::ChildCompleted {
            child,
            child_is_task,
            ..
        } => vec![
            ("child", Id(child.as_uuid())),
            ("child_is_task", Flag(*child_is_task)),
        ],
        ActivityIntent::ChildReopened {
            child,
            child_is_task,
            reopened_parent,
            ..
        } => vec![
            ("child", Id(child.as_uuid())),
            ("child_is_task", Flag(*child_is_task)),
            ("reopened_parent", Flag(*reopened_parent)),
        ],
    }
}

/// `task.created`'s `reason` — the marker value SPEC §3 stores, which is the
/// same token.
fn created_reason(reason: TaskReason) -> &'static str {
    reason.as_str()
}

/// `task.created`'s `node_kind`, over SPEC §4's closed capable set. Every
/// command resolves its target through `require_capable` before it produces a
/// `Created` intent, so only those five ever arrive; `Kind::as_str` is already
/// the vocabulary's spelling for each.
fn node_kind_token(kind: Kind) -> &'static str {
    kind.as_str()
}

fn change_token(change: AttrChange) -> &'static str {
    change.as_str()
}

/// The *shape* a deadline now has, never its value. `None` once it has been
/// cleared — and never `datetime_floating`, because `set_due` refuses a
/// floating wall clock.
fn temporal_token(kind: Option<TemporalKind>) -> &'static str {
    match kind {
        Some(kind) => kind.token(),
        None => "none",
    }
}

fn priority_token(priority: Priority) -> &'static str {
    priority.as_str()
}

fn class_token(class: ChildClass) -> &'static str {
    class.as_str()
}

#[cfg(test)]
mod tests;
