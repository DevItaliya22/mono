// SPDX-License-Identifier: MPL-2.0

//! The chain planner for canvas workflows: from a start node, which agent and
//! output nodes run, in what order, and what to do next.
//!
//! **UI-free and pure.** Everything here is a function of a
//! [`CanvasSnapshot`] and of the states the runner reports; nothing starts an
//! agent, writes a page, reads a clock or draws. That is the whole reason it is
//! a crate of its own rather than a module of the canvas surface: the delegate
//! worker is a library over ports that the gateway reaches through one trait
//! (ADR-347-delegateworker), and a chain that runs there has to be planned
//! there — from the same snapshot, by the same code — without the worker
//! linking a window or the window linking the worker. Its only dependency is
//! `koru-core`, and a test holds that fence.
//!
//! **The shape.** A workflow node is a marked card on the canvas
//! (`koru_core::canvas`): a *start*, an *agent* or an *output*. A chain is a
//! start node and every agent and output node it reaches through arrows whose
//! two ends are both workflow nodes. An arrow from a card, an image or an agent
//! node into an agent node is also an **input** — what the agent is given to
//! read — which is what [`inputs_of`] answers. Arrows never run anything on
//! their own; pressing a start node does.
//!
//! **One pass per run.** [`plan`] is asked again every time a step changes
//! state, and each step is started at most once per run: a step leaves
//! `NotStarted` exactly once. A step is ready when every step upstream of it
//! has finished with a result; the start node counts as finished. A step that
//! failed, was stopped, or finished with nothing to hand on halts the chain
//! *there* — nothing downstream of it will run — while branches that do not
//! depend on it carry on.

use std::collections::{HashMap, HashSet};

use koru_core::NodeId;
use koru_core::canvas::{CanvasSnapshot, ObjectKind, dominant_direction, reading_order};

/// What a step of a chain is.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum StepKind {
    /// An agent node: runs its agent on its instruction and its inputs.
    Agent,
    /// An output node: writes what reaches it to its page.
    Output,
}

/// A start node and everything it runs.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Chain {
    pub start: NodeId,
    /// Every agent and output node the start reaches, in an order in which
    /// each comes after everything upstream of it. Ties read in the canvas's
    /// reading order, so the order is stable across devices.
    pub steps: Vec<(NodeId, StepKind)>,
    /// The chain's arrows, `(from, to)`, once each, in step order. Both ends
    /// are the start or a step.
    pub edges: Vec<(NodeId, NodeId)>,
}

/// Why a start node has no chain to run.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ChainError {
    /// The node is not a start node on this canvas.
    NotAStart,
    /// The part of the canvas the start reaches loops back on itself; this
    /// node is on the loop.
    Cycle(NodeId),
    /// The start reaches no agent or output node.
    Empty,
}

impl std::fmt::Display for ChainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAStart => write!(f, "not a start node"),
            Self::Cycle(node) => write!(f, "the chain loops back through {node:?}"),
            Self::Empty => write!(f, "the start node reaches no agent or output node"),
        }
    }
}

impl std::error::Error for ChainError {}

/// Where one step of a run is, as the runner reports it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum StepState {
    NotStarted,
    Running,
    /// Done. `has_result` is whether it produced anything to hand on — an
    /// agent that answered nothing did not.
    Finished {
        has_result: bool,
    },
    Failed,
    /// A person stopped it.
    Stopped,
}

/// Why a chain halted at a step.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum HaltReason {
    Failed,
    Stopped,
    /// It finished without a result, so there is nothing to hand downstream.
    NoResult,
}

/// One thing the runner should do now.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Next {
    /// Start this agent node; everything upstream of it has a result.
    StartAgent(NodeId),
    /// Write this output node's page; everything upstream of it has a result.
    WriteOutput(NodeId),
    /// Nothing can start yet; a step is running.
    Wait,
    /// Every step finished with a result.
    Done,
    /// The chain halted at this step; nothing downstream of it will run.
    Halted { at: NodeId, reason: HaltReason },
}

/// The chain a start node runs.
///
/// Follows arrows whose two ends are both workflow nodes, forwards from
/// `start`. An arrow *into* a start node is not part of any chain — a start
/// has no inputs. A loop anywhere in the reachable part is refused, naming a
/// node on it, before anything could run.
pub fn chain_from(snapshot: &CanvasSnapshot, start: NodeId) -> Result<Chain, ChainError> {
    if workflow_kind(snapshot, start) != Some(ObjectKind::Start) {
        return Err(ChainError::NotAStart);
    }
    let rank = canvas_rank(snapshot);
    let rank_of = |id: &NodeId| rank.get(id).copied().unwrap_or(usize::MAX);

    // The workflow arrows, once each, never into a start.
    let mut out: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    let mut seen_edges = HashSet::new();
    for arrow in &snapshot.arrows {
        let (Some(_), Some(to_kind)) = (
            workflow_kind(snapshot, arrow.from),
            workflow_kind(snapshot, arrow.to),
        ) else {
            continue;
        };
        if to_kind != ObjectKind::Start && seen_edges.insert((arrow.from, arrow.to)) {
            out.entry(arrow.from).or_default().push(arrow.to);
        }
    }
    for targets in out.values_mut() {
        targets.sort_by_key(rank_of);
    }
    let targets_of = |id: NodeId| out.get(&id).map(Vec::as_slice).unwrap_or_default();

    let mut reachable = vec![start];
    let mut seen = HashSet::from([start]);
    let mut at = 0;
    while at < reachable.len() {
        for &next in targets_of(reachable[at]) {
            if seen.insert(next) {
                reachable.push(next);
            }
        }
        at += 1;
    }
    if reachable.len() == 1 {
        return Err(ChainError::Empty);
    }
    if let Some(on_loop) = find_cycle(start, &targets_of) {
        return Err(ChainError::Cycle(on_loop));
    }

    // Kahn's order over what the start reaches, the earliest-reading ready
    // node first. The start is the only node nothing points into.
    let mut waiting_on: HashMap<NodeId, usize> = reachable.iter().map(|&id| (id, 0)).collect();
    for &id in &reachable {
        for next in targets_of(id) {
            *waiting_on.entry(*next).or_default() += 1;
        }
    }
    let mut ready = vec![start];
    let mut order = Vec::with_capacity(reachable.len());
    while !ready.is_empty() {
        let pick = (0..ready.len())
            .min_by_key(|&i| (rank_of(&ready[i]), ready[i]))
            .expect("not empty");
        let id = ready.swap_remove(pick);
        order.push(id);
        for &next in targets_of(id) {
            let count = waiting_on.get_mut(&next).expect("reachable");
            *count -= 1;
            if *count == 0 {
                ready.push(next);
            }
        }
    }

    let position: HashMap<NodeId, usize> =
        order.iter().enumerate().map(|(i, &id)| (id, i)).collect();
    let mut edges: Vec<(NodeId, NodeId)> = order
        .iter()
        .flat_map(|&from| targets_of(from).iter().map(move |&to| (from, to)))
        .collect();
    edges.sort_by_key(|(from, to)| (position[from], position[to]));
    let steps = order
        .into_iter()
        .skip(1)
        .map(|id| {
            let kind = match workflow_kind(snapshot, id) {
                Some(ObjectKind::Output) => StepKind::Output,
                _ => StepKind::Agent,
            };
            (id, kind)
        })
        .collect();
    Ok(Chain {
        start,
        steps,
        edges,
    })
}

/// What `node` is given to read: the sources of arrows into it that are
/// cards, images or agent nodes (an agent hands on its result), once each, in
/// the canvas's reading order — objects first, then arrivals.
pub fn inputs_of(snapshot: &CanvasSnapshot, node: NodeId) -> Vec<NodeId> {
    let rank = canvas_rank(snapshot);
    let mut sources: Vec<NodeId> = snapshot
        .arrows
        .iter()
        .filter(|arrow| arrow.to == node)
        .map(|arrow| arrow.from)
        .filter(|&from| {
            matches!(
                snapshot.get(from).map(|object| object.kind),
                Some(ObjectKind::Card | ObjectKind::Image | ObjectKind::Agent)
            )
        })
        .collect();
    sources.sort_by_key(|id| (rank.get(id).copied().unwrap_or(usize::MAX), *id));
    sources.dedup();
    sources
}

/// What to do now, given where each step is. A step missing from `states` has
/// not started; the start node's state is never asked.
///
/// The answer is every step that can start now (in step order), then every
/// step the chain halted at; then `Wait` if nothing can start but something
/// is running, or `Done` if every step finished with a result. An answer with
/// halts and no `Wait` is a run that is over.
pub fn plan(chain: &Chain, states: &HashMap<NodeId, StepState>) -> Vec<Next> {
    let state = |id: NodeId| states.get(&id).copied().unwrap_or(StepState::NotStarted);
    let mut upstream: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for &(from, to) in &chain.edges {
        upstream.entry(to).or_default().push(from);
    }

    // Steps that can never run in this run: halted, or downstream of a halt.
    // Step order puts every upstream step first, so one pass settles it.
    let mut blocked: HashSet<NodeId> = HashSet::new();
    let mut actions = Vec::new();
    let mut halts = Vec::new();
    let mut running = false;
    for &(step, kind) in &chain.steps {
        let above = upstream.get(&step).map(Vec::as_slice).unwrap_or_default();
        let upstream_blocked = above.iter().any(|id| blocked.contains(id));
        let upstream_ready = above
            .iter()
            .all(|&id| id == chain.start || state(id) == StepState::Finished { has_result: true });
        match state(step) {
            StepState::Running => running = true,
            StepState::Finished { has_result: true } => {}
            StepState::Finished { has_result: false } => {
                blocked.insert(step);
                halts.push(Next::Halted {
                    at: step,
                    reason: HaltReason::NoResult,
                });
            }
            StepState::Failed => {
                blocked.insert(step);
                halts.push(Next::Halted {
                    at: step,
                    reason: HaltReason::Failed,
                });
            }
            StepState::Stopped => {
                blocked.insert(step);
                halts.push(Next::Halted {
                    at: step,
                    reason: HaltReason::Stopped,
                });
            }
            StepState::NotStarted if upstream_blocked => {
                blocked.insert(step);
            }
            StepState::NotStarted if upstream_ready => actions.push(match kind {
                StepKind::Agent => Next::StartAgent(step),
                StepKind::Output => Next::WriteOutput(step),
            }),
            StepState::NotStarted => {}
        }
    }

    let mut next = actions;
    let nothing_to_start = next.is_empty();
    next.extend(halts);
    if nothing_to_start && running {
        next.push(Next::Wait);
    }
    if next.is_empty() {
        // Nothing to start, nothing halted and nothing running: with an
        // acyclic chain that means every step finished with a result.
        next.push(Next::Done);
    }
    next
}

/// The kind of a workflow node on this canvas, or `None` for anything else.
fn workflow_kind(snapshot: &CanvasSnapshot, id: NodeId) -> Option<ObjectKind> {
    snapshot.get(id).map(|object| object.kind).filter(|kind| {
        matches!(
            kind,
            ObjectKind::Start | ObjectKind::Agent | ObjectKind::Output
        )
    })
}

/// Each object's place in the canvas's own reading order (in its dominant
/// direction, so every device agrees), then the arrivals in their order. The
/// tie-break for everything here that would otherwise depend on hash order.
fn canvas_rank(snapshot: &CanvasSnapshot) -> HashMap<NodeId, usize> {
    reading_order(&snapshot.objects, dominant_direction(snapshot))
        .into_iter()
        .chain(snapshot.arrivals.iter().map(|arrival| arrival.id))
        .enumerate()
        .map(|(rank, id)| (id, rank))
        .collect()
}

/// A node on a loop reachable from `start`, if there is one — the target of
/// the first edge found pointing back into the path being walked. Iterative,
/// so a long chain cannot exhaust the stack.
fn find_cycle<'a>(start: NodeId, targets_of: &impl Fn(NodeId) -> &'a [NodeId]) -> Option<NodeId> {
    let mut on_path: HashSet<NodeId> = HashSet::from([start]);
    let mut finished: HashSet<NodeId> = HashSet::new();
    let mut stack: Vec<(NodeId, usize)> = vec![(start, 0)];
    while let Some((node, next)) = stack.last_mut() {
        let node = *node;
        let targets = targets_of(node);
        match targets.get(*next) {
            Some(&target) => {
                *next += 1;
                if on_path.contains(&target) {
                    return Some(target);
                }
                if !finished.contains(&target) {
                    on_path.insert(target);
                    stack.push((target, 0));
                }
            }
            None => {
                on_path.remove(&node);
                finished.insert(node);
                stack.pop();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests;
