// SPDX-License-Identifier: MPL-2.0

//! **The routed attribute write, and what a host owes for it.**
//!
//! [ADR-286] keeps priority out of the six canonical commands on purpose: a
//! priority is an ordinary attribute that happens to owe a `PriorityChanged`
//! intent when the node is a task. A due is the opposite — an ordinary-looking
//! attribute that *is* a command, because a deadline alone makes a task. The
//! routing table that decides which is [`route_attribute`]; this module is the
//! write that follows it, and the sink a host drains afterwards.
//!
//! **Down here because two hosts need it.** It was written in `koru-editor`,
//! where it was first needed, and stayed there while the editor was the only
//! surface that set a priority. The Tasks page is the second: it writes
//! through `koru-workspace-host`, which deliberately does not depend on the
//! editor — *"this crate must stay ignorant of what a row is, and the manifest
//! is what enforces that"* — so the choice was to duplicate the routing and
//! the event construction in the host, or to put the mechanism where both can
//! reach it. Nothing here knows what a row is: a free function over a
//! `WorkspaceDoc` and a slice of nodes, and a `Vec` of what it owed.
//!
//! The editor re-exports every name under its old paths, so no existing call
//! site moved. That is deliberate: the one caller today is what keeps the
//! move honest.
//!
//! [ADR-286]: ../../../../docs/decisions/ADR-276-300.md#adr-286-taskboundary--six-canonical-commands-own-task-state-and-every-host-is-a-thin-wrapper-around-them
//! [`route_attribute`]: crate::tasks::route_attribute

use crate::attributes::Priority;
use crate::ids::{EntityRef, NodeId};
use crate::node::AttrValue;
use crate::schema::{Result, StoreError, WorkspaceDoc};
use crate::tasks::{
    ActivityIntent, AttrChange, AttributeRoute, CausationId, TaskOutcome, refs, route_attribute,
};

#[cfg(test)]
mod tests;

/// What one task gesture produced and the host still owes: the intents to
/// attempt, and the causation id every envelope built from them carries as its
/// `correlation_id`.
///
/// Two fields rather than the whole [`TaskOutcome`] because the outcome's other
/// fields are the *caller's* answer — what changed, what cascaded — and are
/// consumed here and now; this is the part that outlives the gesture.
#[derive(Clone, PartialEq, Debug)]
pub struct TaskActivity {
    /// Minted by the command, never by a host or by this module — which is
    /// what makes desktop and MCP group one cascade identically.
    pub causation: CausationId,
    /// In outcome order. A host attempts them in that order and never reorders,
    /// filters or invents one.
    pub intents: Vec<ActivityIntent>,
}

impl TaskActivity {
    /// What a command left for the host, or `None` when it asked for nothing —
    /// a guarded no-op produces no intents and owes no event.
    pub fn of(outcome: &TaskOutcome) -> Option<Self> {
        if outcome.activity.is_empty() {
            return None;
        }
        Some(Self {
            causation: outcome.causation,
            intents: outcome.activity.clone(),
        })
    }

    /// One intent that belongs to no command.
    ///
    /// The **priority door** is the only case ([ADR-286]'s routing table):
    /// setting priority on a node that is already a task takes the ordinary
    /// attribute door and then owes a `task.priority.changed`. No command runs,
    /// so there is no causation id to inherit and this one is its own chain
    /// root.
    ///
    /// [ADR-286]: ../../../../docs/decisions/ADR-276-300.md#adr-286-taskboundary--six-canonical-commands-own-task-state-and-every-host-is-a-thin-wrapper-around-them
    pub fn alone(intent: ActivityIntent) -> Self {
        Self {
            causation: CausationId::new(),
            intents: vec![intent],
        }
    }
}

/// Where a view keeps what its host has not drained yet.
pub type ActivitySink = Vec<TaskActivity>;

/// How much a sink holds before it starts letting the oldest go.
///
/// A host that never drains is a host with no activity feed, and that must cost
/// nothing worse than the feed: an undrained sink may not grow for the length
/// of a session. Activity is best effort and is never retried or fabricated
/// later (SPEC §9), so the oldest attempt is the right one to lose.
pub const SINK_LIMIT: usize = 512;

/// Add to a sink, keeping it bounded. **The one way in** — every entry,
/// including the priority door's, comes through here, so the bound holds
/// whichever arm filled it.
pub fn remember(sink: &mut ActivitySink, owed: TaskActivity) {
    if sink.len() >= SINK_LIMIT {
        sink.remove(0);
    }
    sink.push(owed);
}

/// **Run one canonical command and record what the host owes it.**
///
/// The outcome is returned to the caller immediately — the app's optimistic
/// model, and the reason a task ticks the moment you click it. Persistence is
/// not this function's business and neither is emission: the host marks the
/// session dirty on the version change it already watches for, and drains
/// `sink` for the activity attempt.
///
/// `Option` because [`crate::tasks::reclassify`] answers `None` when nothing
/// above the changed node was completed; the six commands always answer
/// `Some`.
pub fn run(
    sink: &mut ActivitySink,
    ws: &mut WorkspaceDoc,
    command: impl FnOnce(&mut WorkspaceDoc) -> Result<Option<TaskOutcome>>,
) -> Result<Option<TaskOutcome>> {
    let outcome = command(ws)?;
    if let Some(outcome) = &outcome
        && let Some(owed) = TaskActivity::of(outcome)
    {
        remember(sink, owed);
    }
    Ok(outcome)
}

/// Set one attribute over a set of nodes, **routing each of them**
/// ([ADR-286]'s table, asked through [`route_attribute`] so the editor, the
/// Tasks page, MCP and any future surface route identically).
///
/// A due becomes [`set_due`](crate::tasks::set_due), which normalizes the
/// marker in and answers an outcome; a command-only key is refused at the door
/// and simply does not land; priority on a node that is already a task takes
/// the ordinary door and then owes a `task.priority.changed`; everything else
/// is unchanged. Nodes that end on the ordinary door are applied together, in
/// one call, so a selection is still one write.
///
/// A node the router refuses — a due on a kind that cannot carry task state —
/// is skipped rather than failing the whole commit: half an apply over a mixed
/// selection beats a refused one, which is the rule `set_attribute_on_all`
/// already follows for a value a definition will not take.
///
/// **But nothing landing at all is a failure, and it says so.** The
/// skip-and-continue rule above used to be written `let _ = …` on every arm, so
/// a commit where *no* node changed still answered `Ok` — and the editor had
/// already taken the typed `/due 2026-09-04` back out by then. The user's
/// typing vanished and no chip arrived. Partial success stays `Ok`; total
/// failure is reported, with the first refusal as the reason.
///
/// **No undo boundary of its own.** `set_attribute_on_all` joins whatever
/// group is open, so a caller that wants the write to be its own `Ctrl+Z`
/// brackets it with [`compose`](crate::tasks::compose) — the editor does, and
/// so must any host.
///
/// [ADR-286]: ../../../../docs/decisions/ADR-276-300.md#adr-286-taskboundary--six-canonical-commands-own-task-state-and-every-host-is-a-thin-wrapper-around-them
pub fn set_attribute(
    ws: &mut WorkspaceDoc,
    nodes: &[NodeId],
    key: &str,
    value: AttrValue,
    owed: &mut ActivitySink,
) -> Result<()> {
    let mut ordinary: Vec<NodeId> = Vec::new();
    let mut landed = 0usize;
    let mut refused: Option<StoreError> = None;
    for &node in nodes {
        match route_attribute(ws, node, key) {
            Ok(AttributeRoute::Refused) => {}
            Ok(AttributeRoute::Due) => {
                // The value has already passed the picker; a due that is not a
                // temporal at all is not a due, and the command would refuse it
                // anyway.
                let Some(due) = value.temporal() else {
                    continue;
                };
                match run(owed, ws, |ws| {
                    crate::tasks::set_due(ws, node, due).map(Some)
                }) {
                    Ok(_) => landed += 1,
                    Err(why) => refused = refused.or(Some(why)),
                }
            }
            Ok(AttributeRoute::PriorityOnTask) => {
                let before = ws.read(node).ok().and_then(|n| n.priority());
                let after = match &value {
                    AttrValue::Text(token) => Priority::parse(token),
                    _ => None,
                };
                ordinary.push(node);
                if before != after
                    && let Some(task) = task_ref(ws, node)
                {
                    remember(
                        owed,
                        TaskActivity::alone(ActivityIntent::PriorityChanged {
                            task,
                            change: match before {
                                Some(_) => AttrChange::Changed,
                                None => AttrChange::Set,
                            },
                            priority: after,
                        }),
                    );
                }
            }
            Ok(AttributeRoute::Ordinary) => ordinary.push(node),
            Err(why) => refused = refused.or(Some(why)),
        }
    }
    if ordinary.is_empty() {
        return match refused {
            Some(why) if landed == 0 => Err(why),
            _ => Ok(()),
        };
    }
    ws.set_attribute_on_all(&ordinary, key, value).map(|_| ())
}

/// [`set_attribute`]'s other half: clear one key over a set of nodes, routed
/// the same way. A due becomes [`clear_due`](crate::tasks::clear_due), which
/// **marks first** so taking a deadline away never takes the task with it.
pub fn clear_attribute(
    ws: &mut WorkspaceDoc,
    nodes: &[NodeId],
    key: &str,
    owed: &mut ActivitySink,
) -> Result<()> {
    let mut ordinary: Vec<NodeId> = Vec::new();
    let mut landed = 0usize;
    let mut refused: Option<StoreError> = None;
    for &node in nodes {
        match route_attribute(ws, node, key) {
            Ok(AttributeRoute::Refused) => {}
            Ok(AttributeRoute::Due) => {
                match run(owed, ws, |ws| crate::tasks::clear_due(ws, node).map(Some)) {
                    Ok(_) => landed += 1,
                    Err(why) => refused = refused.or(Some(why)),
                }
            }
            Ok(AttributeRoute::PriorityOnTask) => {
                let had = ws.read(node).ok().and_then(|n| n.priority());
                ordinary.push(node);
                if had.is_some()
                    && let Some(task) = task_ref(ws, node)
                {
                    remember(
                        owed,
                        TaskActivity::alone(ActivityIntent::PriorityChanged {
                            task,
                            change: AttrChange::Cleared,
                            priority: None,
                        }),
                    );
                }
            }
            Ok(AttributeRoute::Ordinary) => ordinary.push(node),
            Err(why) => refused = refused.or(Some(why)),
        }
    }
    if ordinary.is_empty() {
        return match refused {
            Some(why) if landed == 0 => Err(why),
            _ => Ok(()),
        };
    }
    ws.clear_attribute_on_all(&ordinary, key).map(|_| ())
}

/// This node addressed **as a task** — the subject an activity intent is filed
/// under. `None` when the document carries no workspace identity, which is the
/// same read a command's own preflight makes first.
///
/// The domain's minter, not a second spelling of it: both call sites here are
/// on the `PriorityOnTask` route, which has already decided the node is a task,
/// so no task test is owed here.
fn task_ref(ws: &WorkspaceDoc, node: NodeId) -> Option<EntityRef> {
    refs::task_ref(ws, node)
}
