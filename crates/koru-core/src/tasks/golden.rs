// SPDX-License-Identifier: MPL-2.0

//! The golden cases: what a canonical command does, as **data**.
//!
//! One table, three runners. [`CASES`] declares — for each case — the initial
//! document, the command, the CRDT state afterwards, the [`TaskOutcome`]'s
//! semantic fields and the [`ActivityIntent`] list, in a form that carries no
//! code of its own: every case is a `const` value built out of `&'static str`
//! labels and closed enums. `koru-core` runs them against the commands
//! directly (`golden/tests.rs` — the *reference run*, which is what makes the
//! table truthful); `koru-editor` and `koru-mcp-server` run the same table
//! through their own host wrapper and compare with [`verify`]. "Do desktop and
//! MCP agree" is then a table comparison rather than a code review
//! ([ADR-286]).
//!
//! **A case names nodes, never ids.** A [`NodeSpec`] carries a label; the
//! command names its target by label; every expectation is spelled in labels.
//! A `NodeId` is minted per run, so a table that mentioned one could not be a
//! constant — and an adapter that builds the document in its own process needs
//! to be able to say *which* node without sharing an id with anybody.
//!
//! **The initial document is restored, not commanded.** [`Fixture::build`]
//! writes marker, lifecycle and due through
//! [`faithful_import`](super::faithful_import), which is the named door for
//! exactly this: putting stored state on a node without a command running,
//! without classification, and without an activity intent. A fixture built by
//! calling commands would be asserting on the commands twice.
//!
//! **What a golden case is not.** It is not a substitute for the domain suite
//! in `tasks/tests.rs`: that one proves the *rules* (the classification table,
//! the per-kind matrix, the buckets, the preflight-injected failures). This
//! table proves that a **host** reaches the same answers the domain does, so
//! it stops at what an adapter can observe — the document, the outcome, and
//! the activity the host is asked to attempt.
//!
//! ## Consuming it from another crate
//!
//! ```toml
//! # crates/koru-editor/Cargo.toml, crates/koru-mcp-server/Cargo.toml
//! [dev-dependencies]
//! koru-core = { path = "../koru-core", features = ["golden"] }
//! ```
//!
//! The feature is off in every product build, so the table costs a shipped
//! binary nothing. It is `any(test, feature = "golden")` rather than
//! `cfg(test)` for the reason `koru-search-ui`'s `no-reader` exists: **a
//! dependency is never compiled with `cfg(test)`**, so a `cfg(test)` module
//! is invisible to exactly the crates that need this one.
//!
//! Then, per case — this doc-test is compiled and run as a **separate crate**
//! against the built `koru-core`, which is the same door
//! `koru-editor`'s and `koru-mcp-server`'s suites come through:
//!
//! ```
//! use koru_core::tasks::golden::{self, Fixture};
//!
//! for case in golden::CASES {
//!     let mut fx = Fixture::build(case);
//!     let target = fx.node(case.command.target());
//!     let got = case.command.apply_to(&mut fx.ws, target);
//!     golden::verify(case, &fx, &got).expect(case.id);
//! }
//! ```
//!
//! A **host** replaces the middle line with its own wrapper —
//! `host.run(&mut fx.ws, |ws| case.command.apply_to(ws, target))` — and
//! nothing else changes: [`Command::apply_to`] *is* the closure body a wrapper
//! takes ([ADR-286]'s `run`), so an adapter never re-implements the dispatch
//! and never gets to quietly run something else. Everything the wrapper adds —
//! scheduling, attribution, the durability wait — is invisible to [`verify`]
//! by design, because none of it may change a domain answer.
//!
//! [ADR-286]: ../../../docs/decisions/ADR-276-300.md#adr-286-taskboundary--six-canonical-commands-own-task-state-and-every-host-is-a-thin-wrapper-around-them

use crate::attributes::{KEY_PRIORITY, Lifecycle, Priority, TaskReason};
use crate::ids::{NodeId, WorkspaceId};
use crate::node::{Attr, AttrValue, BlobHash, Kind, ListStyle, Node};
use crate::schema::{Parent, Result, StoreError, WorkspaceDoc};
use crate::time::{
    DateOnly, FloatingDateTime, Temporal, TemporalKind, TimeZoneName, ZonedDateTime,
};

use super::{
    ActivityIntent, AttrChange, ChildClass, LifecycleReason, TaskEffect, TaskFields, TaskOutcome,
    TaskRefusal, faithful_import,
};

#[cfg(test)]
mod tests;

// --------------------------------------------------------------- the document

/// The shape a fixture node takes. Not `Kind` itself: a checklist item is a
/// paragraph under a checkbox-styled list block rather than a kind of its own,
/// and a case has to be able to say which one it means.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Shape {
    /// An ordinary text row — also what a checklist item is, when its parent
    /// is a [`CheckboxList`](Self::CheckboxList).
    Row,
    /// A checkbox-styled list block. It is never a task and never checkable;
    /// its items are.
    CheckboxList,
    Image,
    File,
    /// An excluded kind (SPEC §4). The refusal cases point at one.
    Code,
}

impl Shape {
    fn node(self) -> Node {
        match self {
            Self::Row => Node::paragraph("").expect("a paragraph is a valid node"),
            Self::CheckboxList => Node::list_block().expect("a list block is a valid node"),
            Self::Image => Node::image(
                BlobHash::new("fnv1a", "golden"),
                "image/png",
                Some(8),
                Some(8),
            )
            .expect("an image is a valid node"),
            Self::File => Node::file(
                "golden.pdf",
                BlobHash::new("fnv1a", "golden-file"),
                "application/pdf",
            )
            .expect("a file is a valid node"),
            Self::Code => Node::code("fn main() {}").expect("a code block is a valid node"),
        }
    }
}

/// A deadline, spelled so a case can be a constant. [`Temporal`] carries a
/// `String` zone name and so cannot be one.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Due {
    /// `YYYY-MM-DD`.
    Date(&'static str),
    /// `YYYY-MM-DDTHH:MM:SS` — a floating wall clock, which `koru.due`
    /// **refuses**: a shared deadline carries the zone it was meant in.
    Floating(&'static str),
    /// An instant plus the zone it was meant in.
    Zoned { instant_ms: i64, zone: &'static str },
}

impl Due {
    pub fn temporal(self) -> Temporal {
        match self {
            Self::Date(s) => Temporal::Date(DateOnly::parse(s).expect("a golden date parses")),
            Self::Floating(s) => {
                Temporal::Floating(FloatingDateTime::parse(s).expect("a golden wall clock parses"))
            }
            Self::Zoned { instant_ms, zone } => Temporal::Zoned(ZonedDateTime::new(
                instant_ms,
                TimeZoneName::parse(zone).expect("a golden zone name parses"),
            )),
        }
    }
}

/// One node of a case's initial document, restored verbatim.
#[derive(Copy, Clone, Debug)]
pub struct NodeSpec {
    /// How every expectation in the case names this node.
    pub label: &'static str,
    /// The label of its parent; `None` is the document root.
    pub parent: Option<&'static str>,
    pub shape: Shape,
    /// `NodeMeta::done` — content, restored as the archive had it.
    pub done: bool,
    /// The `koru.task` marker. Absent is a node that is not marked, which is
    /// still a task when it carries a [`due`](Self::due).
    pub marker: Option<TaskReason>,
    /// The canonical lifecycle. `Open` writes nothing, because absence *is*
    /// open.
    pub lifecycle: Lifecycle,
    pub due: Option<Due>,
    /// A general content attribute, here because Remove-from-Tasks has to be
    /// seen not clearing it.
    pub priority: Option<Priority>,
}

impl NodeSpec {
    /// A plain row under the document root, with nothing on it —
    /// [`shaped`](Self::shaped) turns it into any other shape.
    pub const fn new(label: &'static str) -> Self {
        Self {
            label,
            parent: None,
            shape: Shape::Row,
            done: false,
            marker: None,
            lifecycle: Lifecycle::Open,
            due: None,
            priority: None,
        }
    }

    pub const fn under(mut self, parent: &'static str) -> Self {
        self.parent = Some(parent);
        self
    }

    pub const fn shaped(mut self, shape: Shape) -> Self {
        self.shape = shape;
        self
    }

    pub const fn done(mut self) -> Self {
        self.done = true;
        self
    }

    pub const fn marked(mut self, reason: TaskReason) -> Self {
        self.marker = Some(reason);
        self
    }

    pub const fn completed(mut self) -> Self {
        self.lifecycle = Lifecycle::Completed;
        self
    }

    pub const fn due(mut self, due: Due) -> Self {
        self.due = Some(due);
        self
    }

    pub const fn priority(mut self, priority: Priority) -> Self {
        self.priority = Some(priority);
        self
    }
}

/// A built case: the document, the label table, and the fingerprint it started
/// with — which is what a refusal is checked against.
pub struct Fixture {
    pub ws: WorkspaceDoc,
    labels: Vec<(&'static str, NodeId)>,
    initial: String,
}

impl Fixture {
    /// Build `case`'s initial document. Panics rather than answering an error:
    /// a fixture that will not build is a broken table, not a test failure.
    pub fn build(case: &Case) -> Self {
        let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "golden")
            .expect("a golden workspace is creatable");
        ws.enable_undo();
        let mut fx = Self {
            ws,
            labels: Vec::new(),
            initial: String::new(),
        };
        for spec in case.nodes {
            let parent = match spec.parent {
                Some(label) => Parent::Node(fx.node(label)),
                None => Parent::Root,
            };
            let id = fx
                .ws
                .insert(parent, spec.shape.node())
                .expect("a golden node inserts");
            fx.labels.push((spec.label, id));
            if spec.shape == Shape::CheckboxList {
                fx.ws
                    .set_list_style(id, Some(ListStyle::Checkbox))
                    .expect("a golden list block takes a style");
            }
            if spec.done {
                faithful_import::restore_done(&mut fx.ws, id, true)
                    .expect("a golden checkbox restores");
            }
            if spec.marker.is_some() || spec.due.is_some() || spec.lifecycle != Lifecycle::Open {
                faithful_import::restore_task(
                    &mut fx.ws,
                    id,
                    &TaskFields {
                        marker: spec.marker,
                        due: spec.due.map(Due::temporal),
                        lifecycle: spec.lifecycle,
                        done: spec.done,
                        completed_at: None,
                        // Golden fixtures describe bucket placement from the
                        // due, the lifecycle and the priority. A pin is an
                        // extra membership on top of that placement, not a
                        // different placement, so it is not part of the corpus.
                        pinned_today: false,
                    },
                )
                .expect("a golden task state restores");
            }
            if let Some(priority) = spec.priority {
                fx.ws
                    .set_attribute(
                        id,
                        KEY_PRIORITY,
                        AttrValue::Text(priority.as_str().to_owned()),
                    )
                    .expect("a golden priority is one of the definition's options");
            }
        }
        fx.initial = fx.fingerprint();
        fx
    }

    /// The node a label names.
    pub fn node(&self, label: &str) -> NodeId {
        self.labels
            .iter()
            .find(|(l, _)| *l == label)
            .map(|(_, id)| *id)
            .unwrap_or_else(|| panic!("no golden node is labelled {label:?}"))
    }

    /// The label a node carries, `""` for one this case never named — which is
    /// a mismatch a caller sees rather than a panic in a comparison.
    pub fn label(&self, node: NodeId) -> &'static str {
        self.labels
            .iter()
            .find(|(_, id)| *id == node)
            .map(|(l, _)| *l)
            .unwrap_or("")
    }

    /// Everything a command could have written, as one string. Compared
    /// against [`initial`](Self::initial) to prove a refusal or a guarded
    /// no-op left the document exactly where it was.
    pub fn fingerprint(&self) -> String {
        let mut out = String::new();
        for (id, depth) in self.ws.walk_with_trash().expect("a golden document walks") {
            let node = self.ws.read(id).expect("a walked node reads");
            let mut attrs: Vec<String> = node
                .content
                .attrs()
                .iter()
                .map(|a| format!("{}={:?}", a.role, a.value))
                .collect();
            attrs.sort();
            out.push_str(&format!(
                "{depth} {} {:?} done={} [{}]\n",
                self.label(id),
                node.kind,
                node.meta.done,
                attrs.join(","),
            ));
        }
        out
    }

    /// The fingerprint the document had before the command ran.
    pub fn initial(&self) -> &str {
        &self.initial
    }
}

// ---------------------------------------------------------------- the command

/// The day every completion in the corpus is recorded on. **A constant, not a
/// clock**: a golden file compares bytes, and a replay that read the calendar
/// would differ from its own recording tomorrow.
fn golden_day() -> crate::time::DateOnly {
    crate::time::DateOnly::parse("2026-09-06").expect("a canonical day")
}

/// A canonical command plus its arguments, naming its target by label.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Command {
    EnsureTask {
        target: &'static str,
        reason: TaskReason,
    },
    SetDue {
        target: &'static str,
        due: Due,
    },
    ClearDue {
        target: &'static str,
    },
    CompleteTask {
        target: &'static str,
        reason: LifecycleReason,
    },
    ReopenTask {
        target: &'static str,
        reason: LifecycleReason,
    },
    RemoveFromTasks {
        target: &'static str,
    },
    /// Not one of the six: the classification hook a host calls **after** an
    /// insert has landed. It is in the table because it is where
    /// `task.child.added` and the auto-reopen come from, and a host that
    /// forgets to call it fails a golden case rather than a bug report.
    Reclassify {
        changed: &'static str,
    },
    /// The same decision told as a move.
    ReclassifyMoved {
        changed: &'static str,
    },
}

impl Command {
    /// The label of the node this command acts on.
    pub fn target(&self) -> &'static str {
        match self {
            Self::EnsureTask { target, .. }
            | Self::SetDue { target, .. }
            | Self::ClearDue { target }
            | Self::CompleteTask { target, .. }
            | Self::ReopenTask { target, .. }
            | Self::RemoveFromTasks { target } => target,
            Self::Reclassify { changed } | Self::ReclassifyMoved { changed } => changed,
        }
    }

    /// The function this case exercises, for a report.
    pub fn name(&self) -> &'static str {
        match self {
            Self::EnsureTask { .. } => "ensure_task",
            Self::SetDue { .. } => "set_due",
            Self::ClearDue { .. } => "clear_due",
            Self::CompleteTask { .. } => "complete_task",
            Self::ReopenTask { .. } => "reopen_task",
            Self::RemoveFromTasks { .. } => "remove_from_tasks",
            Self::Reclassify { .. } => "reclassify",
            Self::ReclassifyMoved { .. } => "reclassify_moved",
        }
    }

    /// **The closure body a host wrapper takes.** An adapter resolves the
    /// target label against its own fixture and passes this in, so a wrapper
    /// is exercised for scheduling and attribution and cannot substitute a
    /// different domain call.
    ///
    /// `Option` because [`reclassify`](super::reclassify) answers `None` when
    /// nothing above the changed node was completed; the six always answer
    /// `Some`.
    pub fn apply_to(&self, ws: &mut WorkspaceDoc, target: NodeId) -> Result<Option<TaskOutcome>> {
        match *self {
            Self::EnsureTask { reason, .. } => super::ensure_task(ws, target, reason).map(Some),
            Self::SetDue { due, .. } => super::set_due(ws, target, due.temporal()).map(Some),
            Self::ClearDue { .. } => super::clear_due(ws, target).map(Some),
            Self::CompleteTask { reason, .. } => {
                super::complete_task(ws, target, reason, golden_day()).map(Some)
            }
            Self::ReopenTask { reason, .. } => super::reopen_task(ws, target, reason).map(Some),
            Self::RemoveFromTasks { .. } => super::remove_from_tasks(ws, target).map(Some),
            Self::Reclassify { .. } => super::reclassify(ws, target),
            Self::ReclassifyMoved { .. } => super::reclassify_moved(ws, target),
        }
    }

    /// [`apply_to`](Self::apply_to) with the label resolved — the reference
    /// run's path, and the shortest thing an adapter can compare itself
    /// against.
    pub fn apply(&self, fx: &mut Fixture) -> Result<Option<TaskOutcome>> {
        let target = fx.node(self.target());
        self.apply_to(&mut fx.ws, target)
    }
}

// ----------------------------------------------------------- the expectations

/// One activity attempt, in labels and closed tokens. Compared as a whole
/// against the [`ActivityIntent`] the command produced.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Activity {
    /// The registry name — ADR-273's table.
    pub event: &'static str,
    /// The label of the task it is filed under.
    pub subject: &'static str,
    pub detail: Detail,
}

/// An intent's payload, label-for-id. **One variant per [`ActivityIntent`]
/// variant**, and [`describe`] maps between them with an exhaustive match — so
/// a thirteenth intent is a compile error here rather than a case that quietly
/// stops checking anything.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Detail {
    Created {
        reason: TaskReason,
        node_kind: Kind,
    },
    Removed,
    Completed {
        reason: LifecycleReason,
    },
    Reopened {
        reason: LifecycleReason,
    },
    DueChanged {
        change: AttrChange,
        temporal_kind: Option<TemporalKind>,
    },
    /// No command produces this one: priority is written at the generic
    /// attribute door, and the intent belongs to the door's caller (ADR-273's
    /// table says *the priority door*). It is here so the mapping stays
    /// one-to-one.
    PriorityChanged {
        change: AttrChange,
        priority: Option<Priority>,
    },
    ChildAdded {
        child: &'static str,
        class: ChildClass,
        reopened: bool,
    },
    ChildMoved {
        child: &'static str,
        class: ChildClass,
        reopened: bool,
    },
    /// Also not a command's: the structural delete/move-away hook produces it.
    ChildRemoved {
        child: &'static str,
    },
    ChildCompleted {
        child: &'static str,
        child_is_task: bool,
    },
    ChildReopened {
        child: &'static str,
        child_is_task: bool,
        reopened_parent: bool,
    },
}

/// One intent, in the table's vocabulary.
pub fn describe(intent: &ActivityIntent, fx: &Fixture) -> Activity {
    let detail = match *intent {
        ActivityIntent::Created {
            reason, node_kind, ..
        } => Detail::Created { reason, node_kind },
        ActivityIntent::Removed { .. } => Detail::Removed,
        ActivityIntent::Completed { reason, .. } => Detail::Completed { reason },
        ActivityIntent::Reopened { reason, .. } => Detail::Reopened { reason },
        ActivityIntent::DueChanged {
            change,
            temporal_kind,
            ..
        } => Detail::DueChanged {
            change,
            temporal_kind,
        },
        ActivityIntent::PriorityChanged {
            change, priority, ..
        } => Detail::PriorityChanged { change, priority },
        ActivityIntent::ChildAdded {
            child,
            class,
            reopened,
            ..
        } => Detail::ChildAdded {
            child: fx.label(child),
            class,
            reopened,
        },
        ActivityIntent::ChildMoved {
            child,
            class,
            reopened,
            ..
        } => Detail::ChildMoved {
            child: fx.label(child),
            class,
            reopened,
        },
        ActivityIntent::ChildRemoved { child, .. } => Detail::ChildRemoved {
            child: fx.label(child),
        },
        ActivityIntent::ChildCompleted {
            child,
            child_is_task,
            ..
        } => Detail::ChildCompleted {
            child: fx.label(child),
            child_is_task,
        },
        ActivityIntent::ChildReopened {
            child,
            child_is_task,
            reopened_parent,
            ..
        } => Detail::ChildReopened {
            child: fx.label(child),
            child_is_task,
            reopened_parent,
        },
    };
    Activity {
        event: intent.event(),
        subject: fx.label(intent.subject().node_id()),
        detail,
    }
}

/// What one node looks like after the command. **Every node a case declares
/// gets one**, checked by [`verify`], so a case cannot leave half the document
/// unexamined.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct After {
    pub label: &'static str,
    pub marker: Option<TaskReason>,
    pub lifecycle: Lifecycle,
    pub due: Option<Due>,
    pub done: bool,
    pub priority: Option<Priority>,
}

impl After {
    /// A node with nothing task-shaped on it.
    pub const fn plain(label: &'static str) -> Self {
        Self {
            label,
            marker: None,
            lifecycle: Lifecycle::Open,
            due: None,
            done: false,
            priority: None,
        }
    }

    pub const fn done(mut self) -> Self {
        self.done = true;
        self
    }

    pub const fn marked(mut self, reason: TaskReason) -> Self {
        self.marker = Some(reason);
        self
    }

    pub const fn completed(mut self) -> Self {
        self.lifecycle = Lifecycle::Completed;
        self
    }

    pub const fn due(mut self, due: Due) -> Self {
        self.due = Some(due);
        self
    }

    pub const fn priority(mut self, priority: Priority) -> Self {
        self.priority = Some(priority);
        self
    }
}

/// The [`TaskOutcome`] a case expects, plus the document it expects to be
/// looking at afterwards.
#[derive(Copy, Clone, Debug)]
pub struct Applied {
    pub effect: TaskEffect,
    /// The label of `outcome.task`.
    pub subject: &'static str,
    pub created_with_reason: Option<TaskReason>,
    pub reason: Option<LifecycleReason>,
    /// Labels, in outcome order.
    pub cascaded: &'static [&'static str],
    /// Labels, nearest first.
    pub reopened_ancestors: &'static [&'static str],
    /// In outcome order. `&[]` is an assertion, not an omission.
    pub activity: &'static [Activity],
    /// Every declared node, in any order.
    pub after: &'static [After],
}

impl Applied {
    /// The common shape: an effect, a subject, and nothing else set.
    pub const fn new(effect: TaskEffect, subject: &'static str) -> Self {
        Self {
            effect,
            subject,
            created_with_reason: None,
            reason: None,
            cascaded: &[],
            reopened_ancestors: &[],
            activity: &[],
            after: &[],
        }
    }

    pub const fn created_with(mut self, reason: TaskReason) -> Self {
        self.created_with_reason = Some(reason);
        self
    }

    pub const fn because(mut self, reason: LifecycleReason) -> Self {
        self.reason = Some(reason);
        self
    }

    pub const fn cascaded(mut self, labels: &'static [&'static str]) -> Self {
        self.cascaded = labels;
        self
    }

    pub const fn reopened(mut self, labels: &'static [&'static str]) -> Self {
        self.reopened_ancestors = labels;
        self
    }

    pub const fn activity(mut self, activity: &'static [Activity]) -> Self {
        self.activity = activity;
        self
    }

    pub const fn after(mut self, after: &'static [After]) -> Self {
        self.after = after;
        self
    }
}

/// How a command refused. Two shapes, because two different guards answer:
/// the kind/task predicate and the attribute registry.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Refusal {
    Task(TaskRefusal),
    /// `AttributeRejected` on this key — the value never passed validation.
    Attribute(&'static str),
}

/// What a case expects back.
#[derive(Copy, Clone, Debug)]
pub enum Expect {
    /// An error, and a document that has not moved.
    Refused(Refusal),
    /// `Ok(None)` — only [`Command::Reclassify`] answers it — and a document
    /// that has not moved.
    Nothing,
    Applied(Applied),
}

/// One golden case.
#[derive(Copy, Clone, Debug)]
pub struct Case {
    /// Stable, dotted, and quoted verbatim in a failure — an adapter names it
    /// in its own test output so a red run says which row disagreed.
    pub id: &'static str,
    /// One line of prose: what this case is holding down.
    pub what: &'static str,
    pub nodes: &'static [NodeSpec],
    pub command: Command,
    pub expect: Expect,
}

/// The case with this id.
pub fn case(id: &str) -> Option<&'static Case> {
    CASES.iter().find(|c| c.id == id)
}

// ------------------------------------------------------------- the comparison

/// Compare what a host got against what the case says. `Ok(())` or every
/// complaint at once — one per line, because a wrapper that drifts usually
/// drifts in more than one field and reporting the first is a slow bisect.
pub fn verify(
    case: &Case,
    fx: &Fixture,
    got: &Result<Option<TaskOutcome>>,
) -> std::result::Result<(), String> {
    let mut bad: Vec<String> = Vec::new();
    match (&case.expect, got) {
        (Expect::Refused(want), Err(err)) => {
            let matched = match (want, err) {
                (Refusal::Task(why), StoreError::TaskRejected { why: got, .. }) => why == got,
                (Refusal::Attribute(key), StoreError::AttributeRejected { key: got, .. }) => {
                    key == got
                }
                _ => false,
            };
            if !matched {
                bad.push(format!("expected {want:?}, got {err:?}"));
            }
            unchanged(fx, &mut bad);
        }
        (Expect::Refused(want), Ok(outcome)) => {
            bad.push(format!("expected {want:?}, got Ok({outcome:?})"));
        }
        (Expect::Nothing, Ok(None)) => unchanged(fx, &mut bad),
        (Expect::Nothing, other) => bad.push(format!("expected Ok(None), got {other:?}")),
        (Expect::Applied(_), Err(err)) => bad.push(format!("expected an outcome, got {err:?}")),
        (Expect::Applied(_), Ok(None)) => {
            bad.push("expected an outcome, got Ok(None)".to_owned());
        }
        (Expect::Applied(want), Ok(Some(outcome))) => applied(case, want, fx, outcome, &mut bad),
    }
    if bad.is_empty() {
        Ok(())
    } else {
        Err(format!("{}:\n  {}", case.id, bad.join("\n  ")))
    }
}

fn unchanged(fx: &Fixture, bad: &mut Vec<String>) {
    if fx.fingerprint() != fx.initial {
        bad.push(format!(
            "the document moved:\n--- before\n{}--- after\n{}",
            fx.initial,
            fx.fingerprint()
        ));
    }
}

fn applied(case: &Case, want: &Applied, fx: &Fixture, got: &TaskOutcome, bad: &mut Vec<String>) {
    if got.effect != want.effect {
        bad.push(format!(
            "effect: want {:?}, got {:?}",
            want.effect, got.effect
        ));
    }
    if got.task.kind() != "task" {
        bad.push(format!("the outcome's subject is a {}", got.task.kind()));
    }
    let subject = fx.label(got.task.node_id());
    if subject != want.subject {
        bad.push(format!("subject: want {:?}, got {subject:?}", want.subject));
    }
    if got.created_with_reason != want.created_with_reason {
        bad.push(format!(
            "created_with_reason: want {:?}, got {:?}",
            want.created_with_reason, got.created_with_reason
        ));
    }
    if got.reason != want.reason {
        bad.push(format!(
            "reason: want {:?}, got {:?}",
            want.reason, got.reason
        ));
    }
    let cascaded: Vec<&str> = got.cascaded.iter().map(|r| fx.label(r.node_id())).collect();
    if cascaded != want.cascaded {
        bad.push(format!(
            "cascaded: want {:?}, got {cascaded:?}",
            want.cascaded
        ));
    }
    let reopened: Vec<&str> = got
        .reopened_ancestors
        .iter()
        .map(|r| fx.label(r.node_id()))
        .collect();
    if reopened != want.reopened_ancestors {
        bad.push(format!(
            "reopened_ancestors: want {:?}, got {reopened:?}",
            want.reopened_ancestors
        ));
    }
    // Every event of one command shares the one causation id the command
    // minted — the property a wrapper turns into `correlation_id`.
    let activity: Vec<Activity> = got.activity.iter().map(|i| describe(i, fx)).collect();
    if activity != want.activity {
        bad.push(format!(
            "activity:\n    want {:#?}\n    got  {activity:#?}",
            want.activity
        ));
    }
    if want.effect == TaskEffect::Unchanged {
        unchanged(fx, bad);
    }
    // The document. Every declared node, so nothing is left unexamined.
    let declared: Vec<&str> = case.nodes.iter().map(|n| n.label).collect();
    let covered: Vec<&str> = want.after.iter().map(|a| a.label).collect();
    for label in &declared {
        if !covered.contains(label) {
            bad.push(format!("`after` never says what happened to {label:?}"));
        }
    }
    for expected in want.after {
        let id = fx.node(expected.label);
        let node = match fx.ws.read(id) {
            Ok(node) => node,
            Err(err) => {
                bad.push(format!("{}: {err:?}", expected.label));
                continue;
            }
        };
        let fields = TaskFields::of(&node);
        let priority = node
            .attr(KEY_PRIORITY)
            .and_then(Attr::as_text)
            .and_then(Priority::parse);
        if fields.due != expected.due.map(Due::temporal) {
            bad.push(format!(
                "{}: due want {:?}, got {:?}",
                expected.label, expected.due, fields.due
            ));
        }
        if fields.marker != expected.marker
            || fields.lifecycle != expected.lifecycle
            || fields.done != expected.done
            || priority != expected.priority
        {
            bad.push(format!(
                "{}: want marker={:?} lifecycle={:?} done={} priority={:?}, \
                 got marker={:?} lifecycle={:?} done={} priority={priority:?}",
                expected.label,
                expected.marker,
                expected.lifecycle,
                expected.done,
                expected.priority,
                fields.marker,
                fields.lifecycle,
                fields.done,
            ));
        }
    }
}

// ------------------------------------------------------------------ the table

const D1: Due = Due::Date("2026-09-10");
const D2: Due = Due::Date("2026-09-20");

/// The golden cases. Twenty-four rows; every one of the six commands, both
/// refusal tokens, both no-op shapes, and the classification hook.
pub const CASES: &[Case] = &[
    // ------------------------------------------------------------- ensure_task
    Case {
        id: "ensure.explicit",
        what: "/task on a plain row marks it and says the person asked",
        nodes: &[NodeSpec::new("row")],
        command: Command::EnsureTask {
            target: "row",
            reason: TaskReason::Explicit,
        },
        expect: Expect::Applied(
            Applied::new(TaskEffect::Created, "row")
                .created_with(TaskReason::Explicit)
                .activity(&[Activity {
                    event: "task.created",
                    subject: "row",
                    detail: Detail::Created {
                        reason: TaskReason::Explicit,
                        node_kind: Kind::Paragraph,
                    },
                }])
                .after(&[After::plain("row").marked(TaskReason::Explicit)]),
        ),
    },
    Case {
        id: "ensure.normalizes_a_due_derived_task",
        what: "a due-derived task keeps `due` as its reason, whatever the caller said",
        nodes: &[NodeSpec::new("row").due(D1)],
        command: Command::EnsureTask {
            target: "row",
            reason: TaskReason::Explicit,
        },
        expect: Expect::Applied(
            Applied::new(TaskEffect::Created, "row")
                .created_with(TaskReason::Due)
                .activity(&[Activity {
                    event: "task.created",
                    subject: "row",
                    detail: Detail::Created {
                        reason: TaskReason::Due,
                        node_kind: Kind::Paragraph,
                    },
                }])
                .after(&[After::plain("row").marked(TaskReason::Due).due(D1)]),
        ),
    },
    Case {
        id: "ensure.idempotent",
        what: "a repeated /task is a guarded no-op, so there is no phantom Ctrl+Z",
        nodes: &[NodeSpec::new("row").marked(TaskReason::Explicit)],
        command: Command::EnsureTask {
            target: "row",
            reason: TaskReason::Explicit,
        },
        expect: Expect::Applied(
            Applied::new(TaskEffect::Unchanged, "row")
                .after(&[After::plain("row").marked(TaskReason::Explicit)]),
        ),
    },
    Case {
        id: "ensure.adopts_a_checked_box",
        what: "promoting an already-checked childless item starts the task completed",
        nodes: &[
            NodeSpec::new("list").shaped(Shape::CheckboxList),
            NodeSpec::new("item").under("list").done(),
        ],
        command: Command::EnsureTask {
            target: "item",
            reason: TaskReason::Explicit,
        },
        expect: Expect::Applied(
            Applied::new(TaskEffect::Created, "item")
                .created_with(TaskReason::Explicit)
                .because(LifecycleReason::CheckboxCompleted)
                .activity(&[Activity {
                    event: "task.created",
                    subject: "item",
                    detail: Detail::Created {
                        reason: TaskReason::Explicit,
                        node_kind: Kind::Paragraph,
                    },
                }])
                .after(&[
                    After::plain("list"),
                    After::plain("item")
                        .marked(TaskReason::Explicit)
                        .completed()
                        .done(),
                ]),
        ),
    },
    Case {
        id: "ensure.reopens_the_completed_task_it_is_under",
        what: "a descendant becoming an open task reopens every completed task above it",
        nodes: &[
            NodeSpec::new("parent")
                .marked(TaskReason::Explicit)
                .completed()
                .done(),
            NodeSpec::new("note").under("parent"),
        ],
        command: Command::EnsureTask {
            target: "note",
            reason: TaskReason::Explicit,
        },
        expect: Expect::Applied(
            Applied::new(TaskEffect::Created, "note")
                .created_with(TaskReason::Explicit)
                .reopened(&["parent"])
                .activity(&[
                    Activity {
                        event: "task.created",
                        subject: "note",
                        detail: Detail::Created {
                            reason: TaskReason::Explicit,
                            node_kind: Kind::Paragraph,
                        },
                    },
                    Activity {
                        event: "task.reopened",
                        subject: "parent",
                        detail: Detail::Reopened {
                            reason: LifecycleReason::UnfinishedChildAdded,
                        },
                    },
                    Activity {
                        event: "task.child.added",
                        subject: "parent",
                        detail: Detail::ChildAdded {
                            child: "note",
                            class: ChildClass::UnfinishedStructural,
                            reopened: true,
                        },
                    },
                ])
                .after(&[
                    After::plain("parent").marked(TaskReason::Explicit),
                    After::plain("note").marked(TaskReason::Explicit),
                ]),
        ),
    },
    Case {
        id: "ensure.an_adopted_box_reopens_nothing",
        what: "a promotion that starts completed is finished work and leaves the chain alone",
        nodes: &[
            NodeSpec::new("parent")
                .marked(TaskReason::Explicit)
                .completed()
                .done(),
            NodeSpec::new("list")
                .under("parent")
                .shaped(Shape::CheckboxList),
            NodeSpec::new("item").under("list").done(),
        ],
        command: Command::EnsureTask {
            target: "item",
            reason: TaskReason::Explicit,
        },
        expect: Expect::Applied(
            Applied::new(TaskEffect::Created, "item")
                .created_with(TaskReason::Explicit)
                .because(LifecycleReason::CheckboxCompleted)
                .activity(&[Activity {
                    event: "task.created",
                    subject: "item",
                    detail: Detail::Created {
                        reason: TaskReason::Explicit,
                        node_kind: Kind::Paragraph,
                    },
                }])
                .after(&[
                    After::plain("parent")
                        .marked(TaskReason::Explicit)
                        .completed()
                        .done(),
                    After::plain("list"),
                    After::plain("item")
                        .marked(TaskReason::Explicit)
                        .completed()
                        .done(),
                ]),
        ),
    },
    Case {
        id: "ensure.excluded_kind_is_refused",
        what: "a code block cannot be made a task, and the refusal writes nothing",
        nodes: &[NodeSpec::new("code").shaped(Shape::Code)],
        command: Command::EnsureTask {
            target: "code",
            reason: TaskReason::Explicit,
        },
        expect: Expect::Refused(Refusal::Task(TaskRefusal::KindNotCapable)),
    },
    // ----------------------------------------------------------------- set_due
    Case {
        id: "set_due.creates_the_task",
        what: "a deadline on a plain row creates the task and dates it, in one command",
        nodes: &[NodeSpec::new("row")],
        command: Command::SetDue {
            target: "row",
            due: D1,
        },
        expect: Expect::Applied(
            Applied::new(TaskEffect::DueChanged, "row")
                .created_with(TaskReason::Due)
                .activity(&[
                    Activity {
                        event: "task.created",
                        subject: "row",
                        detail: Detail::Created {
                            reason: TaskReason::Due,
                            node_kind: Kind::Paragraph,
                        },
                    },
                    Activity {
                        event: "task.due.changed",
                        subject: "row",
                        detail: Detail::DueChanged {
                            change: AttrChange::Set,
                            temporal_kind: Some(TemporalKind::Date),
                        },
                    },
                ])
                .after(&[After::plain("row").marked(TaskReason::Due).due(D1)]),
        ),
    },
    Case {
        id: "set_due.reopens_the_completed_task_it_is_under",
        what: "a deadline alone makes a task, so it reopens the chain a /task would",
        nodes: &[
            NodeSpec::new("parent")
                .marked(TaskReason::Explicit)
                .completed()
                .done(),
            NodeSpec::new("note").under("parent"),
        ],
        command: Command::SetDue {
            target: "note",
            due: D1,
        },
        expect: Expect::Applied(
            Applied::new(TaskEffect::DueChanged, "note")
                .created_with(TaskReason::Due)
                .reopened(&["parent"])
                .activity(&[
                    Activity {
                        event: "task.created",
                        subject: "note",
                        detail: Detail::Created {
                            reason: TaskReason::Due,
                            node_kind: Kind::Paragraph,
                        },
                    },
                    Activity {
                        event: "task.due.changed",
                        subject: "note",
                        detail: Detail::DueChanged {
                            change: AttrChange::Set,
                            temporal_kind: Some(TemporalKind::Date),
                        },
                    },
                    Activity {
                        event: "task.reopened",
                        subject: "parent",
                        detail: Detail::Reopened {
                            reason: LifecycleReason::UnfinishedChildAdded,
                        },
                    },
                    Activity {
                        event: "task.child.added",
                        subject: "parent",
                        detail: Detail::ChildAdded {
                            child: "note",
                            class: ChildClass::UnfinishedStructural,
                            reopened: true,
                        },
                    },
                ])
                .after(&[
                    After::plain("parent").marked(TaskReason::Explicit),
                    After::plain("note").marked(TaskReason::Due).due(D1),
                ]),
        ),
    },
    Case {
        id: "set_due.moves_an_existing_deadline",
        what: "a second deadline changes rather than sets, and creates nothing",
        nodes: &[NodeSpec::new("row").marked(TaskReason::Explicit).due(D1)],
        command: Command::SetDue {
            target: "row",
            due: D2,
        },
        expect: Expect::Applied(
            Applied::new(TaskEffect::DueChanged, "row")
                .activity(&[Activity {
                    event: "task.due.changed",
                    subject: "row",
                    detail: Detail::DueChanged {
                        change: AttrChange::Changed,
                        temporal_kind: Some(TemporalKind::Date),
                    },
                }])
                .after(&[After::plain("row").marked(TaskReason::Explicit).due(D2)]),
        ),
    },
    Case {
        id: "set_due.zoned_is_carried_as_its_shape",
        what: "a zoned deadline reports `datetime_zoned` and never its value",
        nodes: &[NodeSpec::new("row").marked(TaskReason::Explicit)],
        command: Command::SetDue {
            target: "row",
            due: Due::Zoned {
                instant_ms: 1_789_000_000_000,
                zone: "Europe/Zurich",
            },
        },
        expect: Expect::Applied(
            Applied::new(TaskEffect::DueChanged, "row")
                .activity(&[Activity {
                    event: "task.due.changed",
                    subject: "row",
                    detail: Detail::DueChanged {
                        change: AttrChange::Set,
                        temporal_kind: Some(TemporalKind::Zoned),
                    },
                }])
                .after(&[After::plain("row")
                    .marked(TaskReason::Explicit)
                    .due(Due::Zoned {
                        instant_ms: 1_789_000_000_000,
                        zone: "Europe/Zurich",
                    })]),
        ),
    },
    Case {
        id: "set_due.floating_is_rejected",
        what: "a shared deadline carries its zone, so a floating wall clock never lands",
        nodes: &[NodeSpec::new("row")],
        command: Command::SetDue {
            target: "row",
            due: Due::Floating("2026-09-10T08:00:00"),
        },
        expect: Expect::Refused(Refusal::Attribute("koru.due")),
    },
    Case {
        id: "set_due.excluded_kind_is_refused",
        what: "SPEC §4's exclusion is enforced at the command, not only hidden in the UI",
        nodes: &[NodeSpec::new("code").shaped(Shape::Code)],
        command: Command::SetDue {
            target: "code",
            due: D1,
        },
        expect: Expect::Refused(Refusal::Task(TaskRefusal::KindNotCapable)),
    },
    // --------------------------------------------------------------- clear_due
    Case {
        id: "clear_due.keeps_the_task",
        what: "clearing marks first, so a due-derived task does not evaporate with its due",
        nodes: &[NodeSpec::new("row").due(D1)],
        command: Command::ClearDue { target: "row" },
        expect: Expect::Applied(
            Applied::new(TaskEffect::DueCleared, "row")
                .activity(&[Activity {
                    event: "task.due.changed",
                    subject: "row",
                    detail: Detail::DueChanged {
                        change: AttrChange::Cleared,
                        temporal_kind: None,
                    },
                }])
                .after(&[After::plain("row").marked(TaskReason::Due)]),
        ),
    },
    Case {
        id: "clear_due.no_due_is_a_no_op",
        what: "a guarded no-op on a task that never had a deadline",
        nodes: &[NodeSpec::new("row").marked(TaskReason::Explicit)],
        command: Command::ClearDue { target: "row" },
        expect: Expect::Applied(
            Applied::new(TaskEffect::Unchanged, "row")
                .after(&[After::plain("row").marked(TaskReason::Explicit)]),
        ),
    },
    // ----------------------------------------------------------- complete_task
    Case {
        id: "complete.childless",
        what: "a childless task completes and its box follows, in one step",
        nodes: &[NodeSpec::new("row").marked(TaskReason::Explicit)],
        command: Command::CompleteTask {
            target: "row",
            reason: LifecycleReason::ExplicitComplete,
        },
        expect: Expect::Applied(
            Applied::new(TaskEffect::Completed, "row")
                .because(LifecycleReason::ExplicitComplete)
                .activity(&[Activity {
                    event: "task.completed",
                    subject: "row",
                    detail: Detail::Completed {
                        reason: LifecycleReason::ExplicitComplete,
                    },
                }])
                .after(&[After::plain("row")
                    .marked(TaskReason::Explicit)
                    .completed()
                    .done()]),
        ),
    },
    Case {
        id: "complete.cascades_over_checkables_and_nested_tasks",
        what: "every checkable is checked, every nested task completes, a passive note is left alone",
        nodes: &[
            NodeSpec::new("parent").marked(TaskReason::Explicit),
            NodeSpec::new("child_task")
                .under("parent")
                .marked(TaskReason::Explicit),
            NodeSpec::new("list")
                .under("parent")
                .shaped(Shape::CheckboxList),
            NodeSpec::new("unchecked").under("list"),
            NodeSpec::new("checked").under("list").done(),
            NodeSpec::new("note").under("parent"),
        ],
        command: Command::CompleteTask {
            target: "parent",
            reason: LifecycleReason::ExplicitComplete,
        },
        expect: Expect::Applied(
            Applied::new(TaskEffect::Completed, "parent")
                .because(LifecycleReason::ExplicitComplete)
                .cascaded(&["child_task"])
                .activity(&[
                    Activity {
                        event: "task.completed",
                        subject: "parent",
                        detail: Detail::Completed {
                            reason: LifecycleReason::ExplicitComplete,
                        },
                    },
                    Activity {
                        event: "task.completed",
                        subject: "child_task",
                        detail: Detail::Completed {
                            reason: LifecycleReason::CascadeFromAncestor,
                        },
                    },
                ])
                .after(&[
                    After::plain("parent")
                        .marked(TaskReason::Explicit)
                        .completed()
                        .done(),
                    After::plain("child_task")
                        .marked(TaskReason::Explicit)
                        .completed()
                        .done(),
                    After::plain("list"),
                    After::plain("unchecked").done(),
                    After::plain("checked").done(),
                    After::plain("note"),
                ]),
        ),
    },
    Case {
        id: "complete.tells_the_task_above_it",
        what: "completing a nested task files a child.completed under its nearest task ancestor",
        nodes: &[
            NodeSpec::new("parent").marked(TaskReason::Explicit),
            NodeSpec::new("child")
                .under("parent")
                .marked(TaskReason::Explicit),
        ],
        command: Command::CompleteTask {
            target: "child",
            reason: LifecycleReason::CheckboxCompleted,
        },
        expect: Expect::Applied(
            Applied::new(TaskEffect::Completed, "child")
                .because(LifecycleReason::CheckboxCompleted)
                .activity(&[
                    Activity {
                        event: "task.completed",
                        subject: "child",
                        detail: Detail::Completed {
                            reason: LifecycleReason::CheckboxCompleted,
                        },
                    },
                    Activity {
                        event: "task.child.completed",
                        subject: "parent",
                        detail: Detail::ChildCompleted {
                            child: "child",
                            child_is_task: true,
                        },
                    },
                ])
                .after(&[
                    After::plain("parent").marked(TaskReason::Explicit),
                    After::plain("child")
                        .marked(TaskReason::Explicit)
                        .completed()
                        .done(),
                ]),
        ),
    },
    Case {
        id: "complete.not_a_task_is_refused",
        what: "completing something that is not a task refuses rather than creating one",
        nodes: &[NodeSpec::new("row")],
        command: Command::CompleteTask {
            target: "row",
            reason: LifecycleReason::ExplicitComplete,
        },
        expect: Expect::Refused(Refusal::Task(TaskRefusal::NotATask)),
    },
    // ------------------------------------------------------------- reopen_task
    Case {
        id: "reopen.never_cascades_downward",
        what: "reopening a task leaves everything below it completed",
        nodes: &[
            NodeSpec::new("parent")
                .marked(TaskReason::Explicit)
                .completed()
                .done(),
            NodeSpec::new("child")
                .under("parent")
                .marked(TaskReason::Explicit)
                .completed()
                .done(),
        ],
        command: Command::ReopenTask {
            target: "parent",
            reason: LifecycleReason::ExplicitReopen,
        },
        expect: Expect::Applied(
            Applied::new(TaskEffect::Reopened, "parent")
                .because(LifecycleReason::ExplicitReopen)
                .activity(&[Activity {
                    event: "task.reopened",
                    subject: "parent",
                    detail: Detail::Reopened {
                        reason: LifecycleReason::ExplicitReopen,
                    },
                }])
                .after(&[
                    After::plain("parent").marked(TaskReason::Explicit),
                    After::plain("child")
                        .marked(TaskReason::Explicit)
                        .completed()
                        .done(),
                ]),
        ),
    },
    Case {
        id: "reopen.reopens_every_completed_task_above_it",
        what: "the reopen chain runs upward, nearest first, under one causation id",
        nodes: &[
            NodeSpec::new("grandparent")
                .marked(TaskReason::Explicit)
                .completed()
                .done(),
            NodeSpec::new("parent")
                .under("grandparent")
                .marked(TaskReason::Explicit)
                .completed()
                .done(),
            NodeSpec::new("child")
                .under("parent")
                .marked(TaskReason::Explicit)
                .completed()
                .done(),
        ],
        command: Command::ReopenTask {
            target: "child",
            reason: LifecycleReason::ExplicitReopen,
        },
        expect: Expect::Applied(
            Applied::new(TaskEffect::Reopened, "child")
                .because(LifecycleReason::ExplicitReopen)
                .reopened(&["parent", "grandparent"])
                .activity(&[
                    Activity {
                        event: "task.reopened",
                        subject: "child",
                        detail: Detail::Reopened {
                            reason: LifecycleReason::ExplicitReopen,
                        },
                    },
                    Activity {
                        event: "task.reopened",
                        subject: "parent",
                        detail: Detail::Reopened {
                            reason: LifecycleReason::UnfinishedChildAdded,
                        },
                    },
                    Activity {
                        event: "task.reopened",
                        subject: "grandparent",
                        detail: Detail::Reopened {
                            reason: LifecycleReason::UnfinishedChildAdded,
                        },
                    },
                    Activity {
                        event: "task.child.reopened",
                        subject: "parent",
                        detail: Detail::ChildReopened {
                            child: "child",
                            child_is_task: true,
                            reopened_parent: true,
                        },
                    },
                ])
                .after(&[
                    After::plain("grandparent").marked(TaskReason::Explicit),
                    After::plain("parent").marked(TaskReason::Explicit),
                    After::plain("child").marked(TaskReason::Explicit),
                ]),
        ),
    },
    // ------------------------------------------------------- remove_from_tasks
    Case {
        id: "remove.keeps_the_box_and_the_priority",
        what: "removal clears marker, lifecycle and due — never check state, never priority",
        nodes: &[NodeSpec::new("row")
            .marked(TaskReason::Explicit)
            .completed()
            .due(D1)
            .done()
            .priority(Priority::High)],
        command: Command::RemoveFromTasks { target: "row" },
        expect: Expect::Applied(
            Applied::new(TaskEffect::Removed, "row")
                .activity(&[Activity {
                    event: "task.removed",
                    subject: "row",
                    detail: Detail::Removed,
                }])
                .after(&[After::plain("row").done().priority(Priority::High)]),
        ),
    },
    Case {
        id: "remove.not_a_task_is_a_no_op",
        what: "removal on a plain row is an answer, not an error — and not a refusal either",
        nodes: &[NodeSpec::new("row")],
        command: Command::RemoveFromTasks { target: "row" },
        expect: Expect::Applied(
            Applied::new(TaskEffect::Unchanged, "row").after(&[After::plain("row")]),
        ),
    },
    // ------------------------------------------------------------- reclassify
    Case {
        id: "reclassify.unfinished_child_reopens_the_task",
        what: "an unchecked item inserted under a completed task reopens it",
        nodes: &[
            NodeSpec::new("parent")
                .marked(TaskReason::Explicit)
                .completed()
                .done(),
            NodeSpec::new("list")
                .under("parent")
                .shaped(Shape::CheckboxList),
            NodeSpec::new("item").under("list"),
        ],
        command: Command::Reclassify { changed: "item" },
        expect: Expect::Applied(
            Applied::new(TaskEffect::Reopened, "parent")
                .because(LifecycleReason::UnfinishedChildAdded)
                .activity(&[
                    Activity {
                        event: "task.reopened",
                        subject: "parent",
                        detail: Detail::Reopened {
                            reason: LifecycleReason::UnfinishedChildAdded,
                        },
                    },
                    Activity {
                        event: "task.child.added",
                        subject: "parent",
                        detail: Detail::ChildAdded {
                            child: "item",
                            class: ChildClass::UnfinishedStructural,
                            reopened: true,
                        },
                    },
                ])
                .after(&[
                    After::plain("parent").marked(TaskReason::Explicit),
                    After::plain("list"),
                    After::plain("item"),
                ]),
        ),
    },
    Case {
        id: "reclassify.a_move_is_told_as_a_move",
        what: "the same decision, filed under task.child.moved",
        nodes: &[
            NodeSpec::new("parent")
                .marked(TaskReason::Explicit)
                .completed()
                .done(),
            NodeSpec::new("list")
                .under("parent")
                .shaped(Shape::CheckboxList),
            NodeSpec::new("item").under("list"),
        ],
        command: Command::ReclassifyMoved { changed: "item" },
        expect: Expect::Applied(
            Applied::new(TaskEffect::Reopened, "parent")
                .because(LifecycleReason::UnfinishedChildAdded)
                .activity(&[
                    Activity {
                        event: "task.reopened",
                        subject: "parent",
                        detail: Detail::Reopened {
                            reason: LifecycleReason::UnfinishedChildAdded,
                        },
                    },
                    Activity {
                        event: "task.child.moved",
                        subject: "parent",
                        detail: Detail::ChildMoved {
                            child: "item",
                            class: ChildClass::UnfinishedStructural,
                            reopened: true,
                        },
                    },
                ])
                .after(&[
                    After::plain("parent").marked(TaskReason::Explicit),
                    After::plain("list"),
                    After::plain("item"),
                ]),
        ),
    },
    Case {
        id: "reclassify.passive_content_reopens_nothing",
        what: "SPEC §2's other half: a note under a completed task changes nothing at all",
        nodes: &[
            NodeSpec::new("parent")
                .marked(TaskReason::Explicit)
                .completed()
                .done(),
            NodeSpec::new("note").under("parent"),
        ],
        command: Command::Reclassify { changed: "note" },
        expect: Expect::Nothing,
    },
    Case {
        id: "reclassify.an_image_task_is_structural",
        what: "an image that is itself an open task is unfinished structural work",
        nodes: &[
            NodeSpec::new("parent")
                .marked(TaskReason::Explicit)
                .completed()
                .done(),
            NodeSpec::new("picture")
                .under("parent")
                .shaped(Shape::Image)
                .marked(TaskReason::Explicit),
        ],
        command: Command::Reclassify { changed: "picture" },
        expect: Expect::Applied(
            Applied::new(TaskEffect::Reopened, "parent")
                .because(LifecycleReason::UnfinishedChildAdded)
                .activity(&[
                    Activity {
                        event: "task.reopened",
                        subject: "parent",
                        detail: Detail::Reopened {
                            reason: LifecycleReason::UnfinishedChildAdded,
                        },
                    },
                    Activity {
                        event: "task.child.added",
                        subject: "parent",
                        detail: Detail::ChildAdded {
                            child: "picture",
                            class: ChildClass::UnfinishedStructural,
                            reopened: true,
                        },
                    },
                ])
                .after(&[
                    After::plain("parent").marked(TaskReason::Explicit),
                    After::plain("picture").marked(TaskReason::Explicit),
                ]),
        ),
    },
];
