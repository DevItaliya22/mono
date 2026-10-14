// SPDX-License-Identifier: MPL-2.0

//! The planner, over real canvases built with `koru_core::canvas` in memory —
//! no disk, no store — so what is planned is what a surface would have drawn.

use std::collections::HashMap;

use koru_core::canvas::{self, CanvasSnapshot, ObjectKind};
use koru_core::node::Node;
use koru_core::schema::{Parent, WorkspaceDoc};
use koru_core::{BlobHash, CanvasPoint, NodeId, WorkspaceId};

use super::*;

struct Board {
    ws: WorkspaceDoc,
    root: NodeId,
}

impl Board {
    fn new() -> Self {
        let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "flow").unwrap();
        let root = ws
            .insert(Parent::Root, Node::document("Flow").unwrap())
            .unwrap();
        Self { ws, root }
    }

    fn node(&mut self, kind: ObjectKind, x: i32, y: i32) -> NodeId {
        canvas::add_object(
            &mut self.ws,
            self.root,
            kind,
            CanvasPoint::new(x, y),
            "a node",
        )
        .unwrap()
    }

    fn start(&mut self, x: i32, y: i32) -> NodeId {
        self.node(ObjectKind::Start, x, y)
    }

    fn agent(&mut self, x: i32, y: i32) -> NodeId {
        self.node(ObjectKind::Agent, x, y)
    }

    fn output(&mut self, x: i32, y: i32) -> NodeId {
        self.node(ObjectKind::Output, x, y)
    }

    fn arrow(&mut self, from: NodeId, to: NodeId) {
        canvas::add_arrow(&mut self.ws, self.root, from, to).unwrap();
    }

    fn snapshot(&self) -> CanvasSnapshot {
        canvas::read(&self.ws, self.root).unwrap()
    }

    fn chain(&self, start: NodeId) -> Chain {
        chain_from(&self.snapshot(), start).unwrap()
    }
}

fn states(pairs: &[(NodeId, StepState)]) -> HashMap<NodeId, StepState> {
    pairs.iter().copied().collect()
}

const DONE: StepState = StepState::Finished { has_result: true };

// ------------------------------------------------------------------ chains

/// Start → agent → agent → output: one step at a time, each once.
#[test]
fn a_straight_chain_runs_one_step_at_a_time() {
    let mut b = Board::new();
    let s = b.start(0, 0);
    let a1 = b.agent(300, 0);
    let a2 = b.agent(600, 0);
    let out = b.output(900, 0);
    b.arrow(s, a1);
    b.arrow(a1, a2);
    b.arrow(a2, out);

    let chain = b.chain(s);
    assert_eq!(
        chain,
        Chain {
            start: s,
            steps: vec![
                (a1, StepKind::Agent),
                (a2, StepKind::Agent),
                (out, StepKind::Output)
            ],
            edges: vec![(s, a1), (a1, a2), (a2, out)],
        }
    );

    assert_eq!(plan(&chain, &HashMap::new()), vec![Next::StartAgent(a1)]);
    let mut now = states(&[(a1, StepState::Running)]);
    assert_eq!(plan(&chain, &now), vec![Next::Wait]);
    now.insert(a1, DONE);
    assert_eq!(
        plan(&chain, &now),
        vec![Next::StartAgent(a2)],
        "a1 is not started twice"
    );
    now.insert(a2, DONE);
    assert_eq!(plan(&chain, &now), vec![Next::WriteOutput(out)]);
    now.insert(out, StepState::Running);
    assert_eq!(plan(&chain, &now), vec![Next::Wait]);
    now.insert(out, DONE);
    assert_eq!(plan(&chain, &now), vec![Next::Done]);
}

/// One start, two agents: both start at once, in reading order — the one on
/// the left first, wherever it sits in the stacking order.
#[test]
fn a_fan_out_starts_every_branch_in_reading_order() {
    let mut b = Board::new();
    let s = b.start(0, 0);
    let right = b.agent(600, 300);
    let left = b.agent(0, 301);
    b.arrow(s, right);
    b.arrow(s, left);

    let chain = b.chain(s);
    assert_eq!(
        chain.steps,
        vec![(left, StepKind::Agent), (right, StepKind::Agent)]
    );
    assert_eq!(
        plan(&chain, &HashMap::new()),
        vec![Next::StartAgent(left), Next::StartAgent(right)]
    );
}

/// Two agents into a third: it waits for both, and only both.
#[test]
fn a_fan_in_waits_for_every_upstream_result() {
    let mut b = Board::new();
    let s = b.start(0, 0);
    let a1 = b.agent(300, 0);
    let a2 = b.agent(300, 300);
    let a3 = b.agent(600, 150);
    b.arrow(s, a1);
    b.arrow(s, a2);
    b.arrow(a1, a3);
    b.arrow(a2, a3);

    let chain = b.chain(s);
    assert_eq!(chain.steps.last(), Some(&(a3, StepKind::Agent)));
    let now = states(&[(a1, DONE), (a2, StepState::Running)]);
    assert_eq!(plan(&chain, &now), vec![Next::Wait]);
    let now = states(&[(a1, DONE), (a2, StepState::NotStarted)]);
    assert_eq!(plan(&chain, &now), vec![Next::StartAgent(a2)]);
    let now = states(&[(a1, DONE), (a2, DONE)]);
    assert_eq!(plan(&chain, &now), vec![Next::StartAgent(a3)]);
}

/// An output fed by two agents writes once both have a result — and halts,
/// rather than writing half, when one finished with nothing.
#[test]
fn an_output_with_two_upstream_agents() {
    let mut b = Board::new();
    let s = b.start(0, 0);
    let a1 = b.agent(300, 0);
    let a2 = b.agent(300, 300);
    let out = b.output(600, 150);
    b.arrow(s, a1);
    b.arrow(s, a2);
    b.arrow(a1, out);
    b.arrow(a2, out);

    let chain = b.chain(s);
    assert_eq!(chain.steps.last(), Some(&(out, StepKind::Output)));
    assert_eq!(
        plan(&chain, &states(&[(a1, DONE), (a2, StepState::Running)])),
        vec![Next::Wait]
    );
    assert_eq!(
        plan(&chain, &states(&[(a1, DONE), (a2, DONE)])),
        vec![Next::WriteOutput(out)]
    );
    assert_eq!(
        plan(
            &chain,
            &states(&[(a1, DONE), (a2, StepState::Finished { has_result: false })])
        ),
        vec![Next::Halted {
            at: a2,
            reason: HaltReason::NoResult
        }]
    );
    assert_eq!(
        plan(&chain, &states(&[(a1, DONE), (a2, DONE), (out, DONE)])),
        vec![Next::Done]
    );
}

/// A loop is refused before anything runs, naming a node that is on it —
/// not the start, and not a node merely upstream of the loop.
#[test]
fn a_cycle_is_refused_naming_a_node_on_it() {
    let mut b = Board::new();
    let s = b.start(0, 0);
    let a1 = b.agent(300, 0);
    let a2 = b.agent(600, 0);
    let a3 = b.agent(900, 0);
    b.arrow(s, a1);
    b.arrow(a1, a2);
    b.arrow(a2, a3);
    b.arrow(a3, a2);

    match chain_from(&b.snapshot(), s) {
        Err(ChainError::Cycle(node)) => assert!(node == a2 || node == a3, "{node:?}"),
        other => panic!("expected a cycle, got {other:?}"),
    }
}

/// An arrow into a start node is not part of a chain — a start has no inputs
/// — so it does not make a loop.
#[test]
fn an_arrow_back_into_the_start_is_not_a_loop() {
    let mut b = Board::new();
    let s = b.start(0, 0);
    let a1 = b.agent(300, 0);
    b.arrow(s, a1);
    b.arrow(a1, s);
    let chain = b.chain(s);
    assert_eq!(chain.steps, vec![(a1, StepKind::Agent)]);
    assert_eq!(chain.edges, vec![(s, a1)]);
}

/// Failed, stopped and result-less each halt the chain *there*: nothing
/// downstream starts, a branch that does not depend on it carries on, and
/// the run ends when nothing is left running.
#[test]
fn a_failed_stopped_or_result_less_step_halts_its_branch() {
    let mut b = Board::new();
    let s = b.start(0, 0);
    let a1 = b.agent(300, 0);
    let a2 = b.agent(600, 0);
    let side = b.agent(300, 300);
    b.arrow(s, a1);
    b.arrow(a1, a2);
    b.arrow(s, side);
    let chain = b.chain(s);

    for (state, reason) in [
        (StepState::Failed, HaltReason::Failed),
        (StepState::Stopped, HaltReason::Stopped),
        (
            StepState::Finished { has_result: false },
            HaltReason::NoResult,
        ),
    ] {
        let halted = Next::Halted { at: a1, reason };
        assert_eq!(
            plan(&chain, &states(&[(a1, state)])),
            vec![Next::StartAgent(side), halted],
            "{reason:?}: the side branch still starts"
        );
        assert_eq!(
            plan(&chain, &states(&[(a1, state), (side, StepState::Running)])),
            vec![halted, Next::Wait],
            "{reason:?}: wait for what is still running"
        );
        let over = plan(&chain, &states(&[(a1, state), (side, DONE)]));
        assert_eq!(over, vec![halted], "{reason:?}: over, and a2 never ran");
    }
}

/// An output node is a step like any other: an arrow on from it waits for
/// its page to be written.
#[test]
fn a_step_after_an_output_waits_for_the_page() {
    let mut b = Board::new();
    let s = b.start(0, 0);
    let a1 = b.agent(300, 0);
    let out = b.output(600, 0);
    let a2 = b.agent(900, 0);
    b.arrow(s, a1);
    b.arrow(a1, out);
    b.arrow(out, a2);
    let chain = b.chain(s);
    assert_eq!(
        plan(&chain, &states(&[(a1, DONE), (out, StepState::Running)])),
        vec![Next::Wait]
    );
    assert_eq!(
        plan(&chain, &states(&[(a1, DONE), (out, DONE)])),
        vec![Next::StartAgent(a2)]
    );
}

// ------------------------------------------------------------------ what is not a chain

#[test]
fn only_a_start_node_has_a_chain() {
    let mut b = Board::new();
    let card = b.node(ObjectKind::Card, 0, 0);
    let agent = b.agent(300, 0);
    let snapshot = b.snapshot();
    assert_eq!(chain_from(&snapshot, card), Err(ChainError::NotAStart));
    assert_eq!(chain_from(&snapshot, agent), Err(ChainError::NotAStart));
    assert_eq!(
        chain_from(&snapshot, NodeId::new()),
        Err(ChainError::NotAStart)
    );
}

/// A start with nothing after it, and a node nothing points into: neither is
/// anything to run.
#[test]
fn a_node_with_no_inputs_is_not_part_of_a_chain() {
    let mut b = Board::new();
    let s = b.start(0, 0);
    let lonely = b.agent(300, 0);
    assert_eq!(chain_from(&b.snapshot(), s), Err(ChainError::Empty));
    assert!(inputs_of(&b.snapshot(), lonely).is_empty());

    let a1 = b.agent(300, 300);
    b.arrow(s, a1);
    let chain = b.chain(s);
    assert_eq!(
        chain.steps,
        vec![(a1, StepKind::Agent)],
        "the lonely agent is not reached"
    );
}

/// A card between two agents is an input, not a link in the chain: the
/// agent after it is not reached through it.
#[test]
fn a_card_does_not_carry_a_chain() {
    let mut b = Board::new();
    let s = b.start(0, 0);
    let a1 = b.agent(300, 0);
    let card = b.node(ObjectKind::Card, 600, 0);
    let a2 = b.agent(900, 0);
    b.arrow(s, a1);
    b.arrow(a1, card);
    b.arrow(card, a2);
    assert_eq!(b.chain(s).steps, vec![(a1, StepKind::Agent)]);
    assert_eq!(inputs_of(&b.snapshot(), a2), vec![card]);
}

// ------------------------------------------------------------------ inputs

/// An agent reads the cards, images and agents pointing at it, in reading
/// order; a start or an output pointing at it hands it nothing.
#[test]
fn inputs_are_cards_images_and_agents_in_reading_order() {
    let mut b = Board::new();
    let s = b.start(0, 0);
    let target = b.agent(600, 600);
    let lower = b.node(ObjectKind::Card, 0, 300);
    let upper_right = b.node(ObjectKind::Card, 400, 10);
    let upper_left = canvas::add_image(
        &mut b.ws,
        b.root,
        CanvasPoint::new(0, 0),
        BlobHash("b3:1".into()),
        "image/png",
        None,
        None,
        200,
    )
    .unwrap();
    let feeding_agent = b.agent(300, 600);
    let out = b.output(900, 0);
    for from in [lower, upper_right, s, upper_left, out, feeding_agent] {
        b.arrow(from, target);
    }
    assert_eq!(
        inputs_of(&b.snapshot(), target),
        vec![upper_left, upper_right, lower, feeding_agent]
    );
}

/// On a right-to-left canvas the rightmost of a row is read first.
#[test]
fn inputs_follow_a_right_to_left_canvas() {
    let mut b = Board::new();
    let target = b.agent(300, 600);
    let right = canvas::add_object(
        &mut b.ws,
        b.root,
        ObjectKind::Card,
        CanvasPoint::new(600, 0),
        "ملاحظات الاجتماع",
    )
    .unwrap();
    let left = canvas::add_object(
        &mut b.ws,
        b.root,
        ObjectKind::Card,
        CanvasPoint::new(0, 0),
        "خطة العمل الأسبوعية",
    )
    .unwrap();
    canvas::set_text(&mut b.ws, target, "لخص").unwrap();
    b.arrow(left, target);
    b.arrow(right, target);
    assert!(dominant_direction(&b.snapshot()));
    assert_eq!(inputs_of(&b.snapshot(), target), vec![right, left]);
}

// ------------------------------------------------------------------ the fence

/// **The fence.** This crate is planned into the delegate worker, so what it
/// links is what that worker links: the canvas model and nothing that draws,
/// stores or runs. Read at compile time, so the test touches no disk.
#[test]
fn it_depends_on_the_model_and_nothing_else() {
    let manifest = include_str!("../Cargo.toml");
    let section = manifest
        .split("[dependencies]")
        .nth(1)
        .expect("a [dependencies] table")
        .split("\n[")
        .next()
        .unwrap();
    let names: Vec<&str> = section
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once('=').map(|(name, _)| name.trim()))
        .collect();
    assert_eq!(names, vec!["koru-core"]);
    assert!(
        !manifest.contains("[dev-dependencies]"),
        "tests need nothing more"
    );
}
