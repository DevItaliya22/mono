// SPDX-License-Identifier: MPL-2.0

//! **The surface at work** — the acceptance point:
//! open a canvas, create some nodes, link them, and press Run on the start
//! node. Driven through the container exactly as the shell drives it, pointer
//! and keyboard, with the chain runner's stand-in recording what it was asked.

use super::*;

use crate::mainview::canvas_chain::{self, CanvasNodeState, ChainCall};
use koru_canvas_ui::NodeStatus;
use koru_core::canvas::{self as model, CanvasSnapshot, ObjectKind};
use koru_core::node::CanvasPoint;
use koru_draw::geometry::Rect;
use koru_session_protocol::{CapabilitySet, ExecutableAvailability, ProviderReadiness, capability};

use crate::mainview::canvas_chain::{agent_key_text, parse_agent_key};

/// The canvas tab's view rect, in the window's space.
fn pane(v: &MainView) -> Rect {
    v.panes
        .view_rect_for(v.content(), v.panes.focused_path())
        .expect("a pane")
}

/// Where a canvas point is in the window, under the canvas's camera now.
fn window_point(v: &MainView, x: f64, y: f64) -> (f32, f32) {
    let rect = pane(v);
    let surface = active_canvas(v).surface();
    let (vx, vy) = surface.camera().to_view(surface.viewport(), (x, y));
    (rect.x + vx as f32, rect.y + vy as f32)
}

/// The canvas point under a window point.
fn canvas_point(v: &MainView, (x, y): (f32, f32)) -> CanvasPoint {
    let rect = pane(v);
    let surface = active_canvas(v).surface();
    let (cx, cy) = surface.camera().to_canvas(
        surface.viewport(),
        (f64::from(x - rect.x), f64::from(y - rect.y)),
    );
    CanvasPoint::new(cx.round() as i32, cy.round() as i32)
}

fn read(ws: &WorkspaceDoc, root: NodeId) -> CanvasSnapshot {
    model::read(ws, root).expect("the canvas reads")
}

fn click(v: &mut MainView, ws: &mut WorkspaceDoc, (x, y): (f32, f32)) {
    View::mouse_move(v, x, y);
    View::mouse_down(v, ws, x, y, false);
    View::mouse_up(v, ws);
}

/// What the host does for a double-click: two presses and releases, the
/// double-click after the second press.
fn double_click(v: &mut MainView, ws: &mut WorkspaceDoc, (x, y): (f32, f32)) {
    View::mouse_move(v, x, y);
    View::mouse_down(v, ws, x, y, false);
    View::mouse_up(v, ws);
    View::mouse_down(v, ws, x, y, false);
    View::mouse_double_click(v, ws, x, y);
    View::mouse_up(v, ws);
}

fn press(v: &mut MainView, ws: &mut WorkspaceDoc, key: NamedKey, mods: ModifiersState) -> bool {
    v.key(ws, &Key::Named(key), None, mods)
}

fn chord(v: &mut MainView, ws: &mut WorkspaceDoc, letter: &str, mods: ModifiersState) -> bool {
    v.key(ws, &Key::Character(letter.into()), Some(letter), mods)
}

fn save(v: &mut MainView, ws: &mut WorkspaceDoc) {
    assert!(press(v, ws, NamedKey::Enter, koru_keys::PRIMARY));
}

fn undo(v: &mut MainView, ws: &mut WorkspaceDoc) {
    assert!(chord(v, ws, "z", koru_keys::PRIMARY));
}

fn escape(v: &mut MainView, ws: &mut WorkspaceDoc) {
    assert!(press(v, ws, NamedKey::Escape, ModifiersState::empty()));
}

/// A menu opened by the pointer has nothing lit: the first ArrowDown lights
/// row 0.
fn choose_row(v: &mut MainView, ws: &mut WorkspaceDoc, row: usize) {
    for _ in 0..=row {
        press(v, ws, NamedKey::ArrowDown, ModifiersState::empty());
    }
    press(v, ws, NamedKey::Enter, ModifiersState::empty());
}

/// Right-click an empty canvas point and choose a row of the canvas's menu:
/// Add card, Add start node, Add agent node, Add output node, Insert image….
fn add_from_menu(v: &mut MainView, ws: &mut WorkspaceDoc, at: (f64, f64), row: usize) {
    let (x, y) = window_point(v, at.0, at.1);
    View::mouse_move(v, x, y);
    assert!(View::context_menu(v, ws, x, y), "the canvas's menu opened");
    choose_row(v, ws, row);
}

/// A point inside an object, clear of its buttons and handles: just inside
/// its top-left corner, where a node's header glyph sits.
fn inside(v: &MainView, snapshot: &CanvasSnapshot, node: NodeId) -> (f32, f32) {
    let at = snapshot.get(node).and_then(|o| o.at).expect("placed");
    window_point(v, f64::from(at.x) + 5.0, f64::from(at.y) + 5.0)
}

/// Click inside an object.
fn click_on(v: &mut MainView, ws: &mut WorkspaceDoc, snapshot: &CanvasSnapshot, node: NodeId) {
    let point = inside(v, snapshot, node);
    click(v, ws, point);
}

/// The selected object's own menu (Shift+F10), then one of its rows — a
/// menu opened from the keyboard starts on row 0.
fn object_menu_row(v: &mut MainView, ws: &mut WorkspaceDoc, row: usize) {
    assert!(press(v, ws, NamedKey::F10, ModifiersState::SHIFT));
    for _ in 0..row {
        press(v, ws, NamedKey::ArrowDown, ModifiersState::empty());
    }
    press(v, ws, NamedKey::Enter, ModifiersState::empty());
}

fn maintain(v: &mut MainView, ws: &mut WorkspaceDoc) {
    View::maintain(v, ws, std::time::Instant::now());
}

/// A canvas in front whose document records undo, as every document the
/// workspace opens does (`Workspace::open_document`).
fn undoable() -> OnACanvas {
    let mut on = on_a_canvas();
    on.canvas_ws.enable_undo();
    let _ = canvas_chain::take_calls();
    on
}

/// A card put on the canvas by another writer, and the tab caught up.
fn a_card(
    v: &mut MainView,
    ws: &mut WorkspaceDoc,
    root: NodeId,
    at: (i32, i32),
    text: &str,
) -> NodeId {
    let card = model::add_object(
        ws,
        root,
        ObjectKind::Card,
        CanvasPoint::new(at.0, at.1),
        text,
    )
    .expect("a card");
    View::adopt(v, ws);
    card
}

// ------------------------------------------------------------------ a card

/// **A double-click on empty space, then Save, is a card with that text at
/// that point** — one undo step, and Ctrl+Z takes it away.
#[test]
fn a_double_click_and_a_saved_dialog_make_one_card_at_that_point() {
    let OnACanvas {
        mut v,
        mut canvas_ws,
        root,
        ..
    } = undoable();
    let (x, y) = over_the_view(&v);
    let (x, y) = (x - 120.0, y - 60.0);
    let at = canvas_point(&v, (x, y));
    double_click(&mut v, &mut canvas_ws, (x, y));

    assert!(active_canvas(&v).is_editing_text(), "the dialog is up");
    assert!(
        v.tabs().active_tab().view.is_prompting(),
        "and owns the keys"
    );
    assert!(
        read(&canvas_ws, root).objects.is_empty(),
        "nothing written yet"
    );
    assert!(!canvas_ws.can_undo(), "not even an undo step");

    type_text(&mut v, &mut canvas_ws, "Plan the launch");
    // Enter is a new line in the dialog, not a save.
    enter(&mut v, &mut canvas_ws);
    type_text(&mut v, &mut canvas_ws, "with the team");
    save(&mut v, &mut canvas_ws);

    assert!(!active_canvas(&v).is_editing_text());
    let canvas = read(&canvas_ws, root);
    assert_eq!(canvas.objects.len(), 1);
    let card = &canvas.objects[0];
    assert_eq!(card.kind, ObjectKind::Card);
    assert_eq!(card.text, "Plan the launch");
    assert_eq!(card.lines, vec!["with the team".to_owned()]);
    assert_eq!(card.at, Some(at), "where the double-click was");
    assert_eq!(
        active_canvas(&v).surface().selection(),
        &[crate::canvas_view::object_key(card.id)],
        "and selected"
    );

    undo(&mut v, &mut canvas_ws);
    assert!(read(&canvas_ws, root).objects.is_empty(), "one Ctrl+Z");
    assert!(!canvas_ws.can_undo(), "it was one step");
    assert!(
        active_canvas(&v).surface().semantics().is_empty(),
        "and the scene follows"
    );
    // Ctrl+Y brings it back, and so does Ctrl+Shift+Z.
    assert!(chord(&mut v, &mut canvas_ws, "y", koru_keys::PRIMARY));
    assert_eq!(read(&canvas_ws, root).objects.len(), 1);
    undo(&mut v, &mut canvas_ws);
    assert!(chord(&mut v, &mut canvas_ws, "z", koru_keys::PRIMARY_SHIFT));
    assert_eq!(read(&canvas_ws, root).objects.len(), 1);
}

/// **A cancelled dialog makes nothing** — Esc, or Save with nothing typed. A
/// card nobody typed into is never made, and a stray click does not close it.
#[test]
fn a_cancelled_or_empty_dialog_makes_nothing() {
    let OnACanvas {
        mut v,
        mut canvas_ws,
        root,
        ..
    } = undoable();
    let point = over_the_view(&v);

    double_click(&mut v, &mut canvas_ws, point);
    type_text(&mut v, &mut canvas_ws, "Never mind");
    escape(&mut v, &mut canvas_ws);
    assert!(!active_canvas(&v).is_editing_text());

    double_click(&mut v, &mut canvas_ws, point);
    save(&mut v, &mut canvas_ws);
    assert!(!active_canvas(&v).is_editing_text());

    // Enter with nothing selected asks for a card in the middle of the view —
    // the same dialog — and a press outside it does not close it.
    assert!(press(
        &mut v,
        &mut canvas_ws,
        NamedKey::Enter,
        ModifiersState::empty()
    ));
    assert!(active_canvas(&v).is_editing_text());
    let rect = pane(&v);
    click(&mut v, &mut canvas_ws, (rect.x + 4.0, rect.y + 4.0));
    assert!(
        active_canvas(&v).is_editing_text(),
        "a stray click keeps it"
    );
    escape(&mut v, &mut canvas_ws);

    assert!(read(&canvas_ws, root).objects.is_empty());
    assert!(!canvas_ws.can_undo(), "and left no step behind");
}

/// **Dragging a card moves it** — one `at` write, one undo step, and nothing
/// written until the button comes up.
#[test]
fn dragging_a_card_moves_it_in_one_write() {
    let OnACanvas {
        mut v,
        mut canvas_ws,
        root,
        ..
    } = on_a_canvas();
    let card = a_card(&mut v, &mut canvas_ws, root, (0, 0), "Move me");
    assert_eq!(
        active_canvas(&v).surface().semantics().len(),
        1,
        "caught up"
    );
    canvas_ws.enable_undo();

    let from = window_point(&v, 10.0, 20.0);
    let before = canvas_ws.version();
    View::mouse_move(&mut v, from.0, from.1);
    View::mouse_down(&mut v, &mut canvas_ws, from.0, from.1, false);
    for step in 1..=5 {
        let t = step as f32 / 5.0;
        View::mouse_move(&mut v, from.0 + 50.0 * t, from.1 + 30.0 * t);
    }
    assert!(v.tabs().active_tab().view.is_dragging());
    assert_eq!(canvas_ws.version(), before, "nothing written mid-drag");
    View::mouse_up(&mut v, &mut canvas_ws);

    let moved = read(&canvas_ws, root);
    assert_eq!(moved.get(card).unwrap().at, Some(CanvasPoint::new(50, 30)));
    assert_eq!(moved.objects[0].text, "Move me", "and nothing else changed");
    undo(&mut v, &mut canvas_ws);
    assert_eq!(
        read(&canvas_ws, root).get(card).unwrap().at,
        Some(CanvasPoint::new(0, 0))
    );
    assert!(!canvas_ws.can_undo(), "one step");
}

/// **Double-click a card, change its text, Save** — the dialog opens over
/// what it says, and the edit is one step.
#[test]
fn editing_a_cards_text_is_one_step() {
    let OnACanvas {
        mut v,
        mut canvas_ws,
        root,
        ..
    } = on_a_canvas();
    let card = a_card(&mut v, &mut canvas_ws, root, (0, 0), "Draft");
    canvas_ws.enable_undo();
    let snapshot = read(&canvas_ws, root);
    let point = inside(&v, &snapshot, card);
    double_click(&mut v, &mut canvas_ws, point);
    assert!(active_canvas(&v).is_editing_text());
    type_text(&mut v, &mut canvas_ws, " two");
    save(&mut v, &mut canvas_ws);
    assert_eq!(read(&canvas_ws, root).get(card).unwrap().text, "Draft two");
    undo(&mut v, &mut canvas_ws);
    assert_eq!(read(&canvas_ws, root).get(card).unwrap().text, "Draft");
}

// ----------------------------------------------------------------- a chain

/// Everything the acceptance point needs: a start node, an agent node and an
/// output node, made from the canvas's menu, and arrows start → agent →
/// output drawn with the object menu's **Connect to…**.
fn a_chain(v: &mut MainView, ws: &mut WorkspaceDoc, root: NodeId) -> (NodeId, NodeId, NodeId) {
    add_from_menu(v, ws, (-400.0, -40.0), 1);
    assert!(
        !active_canvas(v).is_editing_text(),
        "a start node asks nothing"
    );
    add_from_menu(v, ws, (-60.0, -40.0), 2);
    assert!(
        active_canvas(v).is_editing_text(),
        "an agent asks for its instruction"
    );
    type_text(v, ws, "Summarise the cards");
    save(v, ws);
    add_from_menu(v, ws, (280.0, -40.0), 3);
    assert!(
        active_canvas(v).is_editing_text(),
        "an output asks for its page title"
    );
    type_text(v, ws, "Weekly report");
    save(v, ws);

    let canvas = read(ws, root);
    let of = |kind| {
        canvas
            .objects
            .iter()
            .find(|o| o.kind == kind)
            .map(|o| o.id)
            .expect("made")
    };
    let (start, agent, output) = (
        of(ObjectKind::Start),
        of(ObjectKind::Agent),
        of(ObjectKind::Output),
    );
    assert_eq!(canvas.get(agent).unwrap().text, "Summarise the cards");
    assert_eq!(canvas.get(output).unwrap().text, "Weekly report");

    // start → agent: select the start node, Connect to…, click the agent.
    click_on(v, ws, &canvas, start);
    object_menu_row(v, ws, 2);
    click_on(v, ws, &canvas, agent);
    // agent → output: an agent's menu has Choose agent… before Connect to….
    click_on(v, ws, &canvas, agent);
    object_menu_row(v, ws, 3);
    click_on(v, ws, &canvas, output);
    (start, agent, output)
}

/// **Start → agent → output is two arrows**; a second arrow between the same
/// two is refused; deleting the agent hides both arrows and Ctrl+Z brings all
/// three back.
#[test]
fn a_chain_is_two_arrows_and_deleting_its_middle_undoes_whole() {
    let OnACanvas {
        mut v,
        mut canvas_ws,
        root,
        ..
    } = undoable();
    let (start, agent, output) = a_chain(&mut v, &mut canvas_ws, root);
    let canvas = read(&canvas_ws, root);
    assert_eq!(canvas.objects.len(), 3);
    let ends: Vec<(NodeId, NodeId)> = canvas.arrows.iter().map(|a| (a.from, a.to)).collect();
    assert_eq!(ends, vec![(start, agent), (agent, output)]);

    // The same arrow again is the model's to refuse; nothing is added.
    click_on(&mut v, &mut canvas_ws, &canvas, start);
    object_menu_row(&mut v, &mut canvas_ws, 2);
    click_on(&mut v, &mut canvas_ws, &canvas, agent);
    assert_eq!(
        read(&canvas_ws, root).arrows.len(),
        2,
        "a duplicate is refused"
    );

    // Delete the agent: select it and press Delete.
    click_on(&mut v, &mut canvas_ws, &canvas, agent);
    assert!(press(
        &mut v,
        &mut canvas_ws,
        NamedKey::Delete,
        ModifiersState::empty()
    ));
    let after = read(&canvas_ws, root);
    assert_eq!(after.objects.len(), 2);
    assert!(after.arrows.is_empty(), "both arrows dangle and are hidden");

    undo(&mut v, &mut canvas_ws);
    let back = read(&canvas_ws, root);
    assert_eq!(back.objects.len(), 3, "the agent is back");
    assert_eq!(back.arrows.len(), 2, "and both arrows with it");
    assert_eq!(active_canvas(&v).surface().semantics().len(), 3);
}

/// **Run on the start node runs the chain** — this canvas, this root, this
/// node — pressed with the mouse on the node's own Run button. Stop, from its
/// menu while it runs, stops the chain; an agent node runs on its own.
#[test]
fn run_and_stop_on_a_start_node_reach_the_chain_runner() {
    let OnACanvas {
        mut v,
        mut canvas_ws,
        canvas,
        root,
        ..
    } = undoable();
    let (start, agent, _) = a_chain(&mut v, &mut canvas_ws, root);
    let _ = canvas_chain::take_calls();

    // The Run button: a start node with no text has its header, then Run.
    let at = read(&canvas_ws, root).get(start).unwrap().at.unwrap();
    let h = f64::from(koru_ui::ActionButton::H);
    let button = window_point(
        &v,
        f64::from(at.x) + 20.0,
        f64::from(at.y) + 12.0 + h + 4.0 + h * 0.5,
    );
    click(&mut v, &mut canvas_ws, button);
    assert_eq!(
        canvas_chain::take_calls(),
        vec![ChainCall::RunChain {
            canvas,
            root,
            start
        }]
    );

    // While it runs, its menu offers Stop where Run was.
    canvas_chain::set_states(
        canvas,
        [(
            start,
            CanvasNodeState::Running {
                activity: None,
                elapsed: Some("0:03".into()),
            },
        )]
        .into_iter()
        .collect(),
    );
    maintain(&mut v, &mut canvas_ws);
    let snapshot = read(&canvas_ws, root);
    click_on(&mut v, &mut canvas_ws, &snapshot, start);
    object_menu_row(&mut v, &mut canvas_ws, 1);
    assert_eq!(
        canvas_chain::take_calls(),
        vec![ChainCall::StopChain { canvas, start }]
    );

    // An agent node, on its own.
    click_on(&mut v, &mut canvas_ws, &snapshot, agent);
    object_menu_row(&mut v, &mut canvas_ws, 1);
    assert_eq!(
        canvas_chain::take_calls(),
        vec![ChainCall::RunAgentNode {
            canvas,
            root,
            node: agent
        }]
    );
    canvas_chain::set_states(canvas, std::collections::HashMap::new());
}

// ------------------------------------------------------------------ agents

fn delegatable(kind: &str) -> ProviderReadiness {
    ProviderReadiness {
        kind: kind.parse().unwrap(),
        availability: ExecutableAvailability::Available { version: None },
        capabilities: CapabilitySet::new(vec![capability::MANAGED_DELEGATION.parse().unwrap()]),
        failure: None,
    }
}

/// **Choose agent…** lists the agents ready here and puts the one chosen on
/// the node; a new agent node starts with the default one.
#[test]
fn choose_agent_sets_the_nodes_agent_from_the_ready_ones() {
    let OnACanvas {
        mut v,
        mut canvas_ws,
        root,
        ..
    } = undoable();
    v.provider_readiness = Some(
        koru_session_profiles::MANAGED_PROVIDER_REGISTRATIONS
            .iter()
            .map(|entry| delegatable(entry.kind))
            .collect(),
    );
    maintain(&mut v, &mut canvas_ws);
    let ready = v.canvas_runs.ready_agents();
    assert!(ready.len() >= 2, "this build registers at least two agents");
    let tokens: Vec<String> = ready.iter().map(|(key, _)| agent_key_text(key)).collect();
    assert_eq!(
        active_canvas(&v)
            .decor()
            .agents
            .iter()
            .map(|(token, _)| token.clone())
            .collect::<Vec<_>>(),
        tokens,
        "the canvas was told who is ready"
    );

    add_from_menu(&mut v, &mut canvas_ws, (0.0, 0.0), 2);
    escape(&mut v, &mut canvas_ws);
    let canvas = read(&canvas_ws, root);
    let agent = canvas.objects[0].id;
    assert_eq!(
        canvas.objects[0].agent,
        v.canvas_runs
            .default_agent()
            .map(|key| agent_key_text(&key)),
        "a new agent node starts with the default agent"
    );

    // Its menu: Edit, Run, Choose agent…
    click_on(&mut v, &mut canvas_ws, &canvas, agent);
    object_menu_row(&mut v, &mut canvas_ws, 2);
    assert!(
        active_canvas(&v).is_choosing_agent(),
        "the agent menu is up"
    );
    choose_row(&mut v, &mut canvas_ws, 1);
    assert!(!active_canvas(&v).is_choosing_agent());
    let chosen = read(&canvas_ws, root).get(agent).unwrap().agent.clone();
    assert_eq!(chosen.as_deref(), Some(tokens[1].as_str()));
    // The node says the agent's name, not its stored key.
    let label = &active_canvas(&v).surface().semantics()[0].label;
    assert!(!label.is_empty());
    assert_eq!(
        parse_agent_key(&tokens[1]),
        Some(ready[1].0.clone()),
        "the stored key reads back as the profile it names"
    );
}

/// With no agent ready, the menu says so in one row and asks the host.
#[test]
fn with_no_agent_ready_the_menu_says_so() {
    let OnACanvas {
        mut v,
        mut canvas_ws,
        root,
        ..
    } = undoable();
    add_from_menu(&mut v, &mut canvas_ws, (0.0, 0.0), 2);
    escape(&mut v, &mut canvas_ws);
    let canvas = read(&canvas_ws, root);
    let agent = canvas.objects[0].id;
    assert_eq!(canvas.objects[0].agent, None, "no default without an agent");
    let _ = v.canvas_runs.take_wants_readiness();

    click_on(&mut v, &mut canvas_ws, &canvas, agent);
    object_menu_row(&mut v, &mut canvas_ws, 2);
    assert!(active_canvas(&v).is_choosing_agent());
    assert!(v.canvas_runs.take_wants_readiness(), "the host was asked");
    // The one row cannot be chosen: Enter keeps the menu and writes nothing.
    choose_row(&mut v, &mut canvas_ws, 0);
    assert!(active_canvas(&v).is_choosing_agent());
    assert_eq!(read(&canvas_ws, root).get(agent).unwrap().agent, None);
    escape(&mut v, &mut canvas_ws);
    assert!(!active_canvas(&v).is_choosing_agent());
}

/// **Statuses come from the runner**: what `canvas_node_states` says is what
/// each node shows, and what a screen reader hears.
#[test]
fn the_scene_shows_the_runners_statuses() {
    let OnACanvas {
        mut v,
        mut canvas_ws,
        canvas,
        root,
        ..
    } = undoable();
    let (start, agent, output) = a_chain(&mut v, &mut canvas_ws, root);
    canvas_chain::set_states(
        canvas,
        [
            (start, CanvasNodeState::Done),
            (
                agent,
                CanvasNodeState::Running {
                    activity: Some("Reading the cards".into()),
                    elapsed: Some("0:12".into()),
                },
            ),
            (output, CanvasNodeState::NotReady("No page yet".into())),
        ]
        .into_iter()
        .collect(),
    );
    maintain(&mut v, &mut canvas_ws);
    let decor = active_canvas(&v).decor();
    assert_eq!(decor.statuses.get(&start), Some(&NodeStatus::Done));
    assert_eq!(
        decor.statuses.get(&agent),
        Some(&NodeStatus::Running {
            activity: "Reading the cards".into(),
            elapsed: "0:12".into()
        })
    );
    assert_eq!(
        decor.statuses.get(&output),
        Some(&NodeStatus::Failed("No page yet".into()))
    );
    let labels: Vec<String> = active_canvas(&v)
        .surface()
        .semantics()
        .into_iter()
        .map(|o| o.label)
        .collect();
    assert!(
        labels.iter().any(|l| l.contains("Reading the cards")),
        "{labels:?}"
    );
    // A pass that changes nothing rebuilds nothing.
    let revision = active_canvas(&v).surface().revision();
    maintain(&mut v, &mut canvas_ws);
    assert_eq!(active_canvas(&v).surface().revision(), revision);
    canvas_chain::set_states(canvas, std::collections::HashMap::new());
}

// ----------------------------------------------------------------- landing

/// **A search hit on a card lands on it**: the canvas centres on the card and
/// selects it, whether its tab was open or is built for the landing.
#[test]
fn a_search_hit_on_a_card_lands_on_it() {
    let OnACanvas {
        mut v,
        mut canvas_ws,
        canvas,
        root,
        ..
    } = on_a_canvas();
    let card = a_card(&mut v, &mut canvas_ws, root, (3000, 2000), "Far away");
    assert!(View::open_search_target(
        &mut v,
        &canvas_ws,
        Some(row(canvas)),
        Some(card),
        None,
        Some("Far")
    ));
    let surface = active_canvas(&v).surface();
    assert_eq!(surface.selection(), &[crate::canvas_view::object_key(card)]);
    let camera = surface.camera();
    assert!(
        camera.center_x > 3000.0 && camera.center_y > 2000.0,
        "{camera:?}"
    );

    // …and a canvas tab built for the landing, in a fresh window.
    let (mut fresh, _) = container();
    View::set_entries(
        &mut fresh,
        vec![listed(canvas, "Board", KIND_CANVAS)],
        Some(canvas),
    );
    assert!(View::open_search_target(
        &mut fresh,
        &canvas_ws,
        Some(row(canvas)),
        Some(card),
        None,
        None
    ));
    assert_eq!(
        active_canvas(&fresh).surface().selection(),
        &[crate::canvas_view::object_key(card)]
    );
}

// ----------------------------------------------------------- accessibility

/// **A screen reader hears the objects**, in reading order, each named by its
/// kind and first line, inside the canvas tab's panel — and an action on one
/// is a click on it.
#[test]
fn the_accessibility_tree_lists_the_objects() {
    use accesskit::Role;
    use koru_workspace_host::accessibility::{Scope, SemanticNode, ViewSlot, WindowSlot};

    fn flatten(node: &SemanticNode, out: &mut Vec<SemanticNode>) {
        out.push(node.clone());
        for child in &node.children {
            flatten(child, out);
        }
    }

    let OnACanvas {
        mut v,
        mut canvas_ws,
        root,
        ..
    } = on_a_canvas();
    let second = a_card(&mut v, &mut canvas_ws, root, (400, 0), "Second");
    a_card(&mut v, &mut canvas_ws, root, (0, 0), "First");
    v.seat_accessibility(WindowSlot::from_raw(1));

    let mut nodes = Vec::new();
    for node in v.semantics(ViewSlot::from_raw(1), Scope::Whole) {
        flatten(&node, &mut nodes);
    }
    let options: Vec<&SemanticNode> = nodes
        .iter()
        .filter(|n| n.role == Role::ListBoxOption)
        .collect();
    let labels: Vec<&str> = options.iter().filter_map(|n| n.label.as_deref()).collect();
    assert_eq!(labels, vec!["Card: First", "Card: Second"], "reading order");
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::ListBox && n.children.len() == 2)
    );

    // An action on an object is a click on it: it selects, and the focus
    // follows.
    let target = options[1].occurrence;
    assert!(View::accessibility_action(
        &mut v,
        &mut canvas_ws,
        target,
        accesskit::Action::Focus,
        None
    ));
    assert_eq!(
        active_canvas(&v).surface().selection(),
        &[crate::canvas_view::object_key(second)]
    );
    assert_eq!(v.accessibility_focus(ViewSlot::from_raw(1)), Some(target));
}

// -------------------------------------------------------------------- keys

/// **A held arrow key is one move, written when it comes up** — the release
/// reaches the canvas through `View::key_up`.
#[test]
fn an_arrow_nudge_is_written_on_the_release() {
    let OnACanvas {
        mut v,
        mut canvas_ws,
        root,
        ..
    } = on_a_canvas();
    let card = a_card(&mut v, &mut canvas_ws, root, (0, 0), "Nudge");
    let snapshot = read(&canvas_ws, root);
    click_on(&mut v, &mut canvas_ws, &snapshot, card);
    let before = canvas_ws.version();
    for _ in 0..3 {
        press(
            &mut v,
            &mut canvas_ws,
            NamedKey::ArrowRight,
            ModifiersState::empty(),
        );
    }
    assert_eq!(canvas_ws.version(), before, "held: nothing written");
    assert!(View::key_up(
        &mut v,
        &mut canvas_ws,
        &Key::Named(NamedKey::ArrowRight)
    ));
    assert_eq!(
        read(&canvas_ws, root).get(card).unwrap().at,
        Some(CanvasPoint::new(3, 0))
    );
}

/// **Losing the window mid-drag writes nothing.**
#[test]
fn losing_focus_mid_drag_writes_nothing() {
    let OnACanvas {
        mut v,
        mut canvas_ws,
        root,
        ..
    } = on_a_canvas();
    let card = a_card(&mut v, &mut canvas_ws, root, (0, 0), "Stay");
    let from = window_point(&v, 10.0, 20.0);
    let before = canvas_ws.version();
    View::mouse_move(&mut v, from.0, from.1);
    View::mouse_down(&mut v, &mut canvas_ws, from.0, from.1, false);
    View::mouse_move(&mut v, from.0 + 40.0, from.1 + 40.0);
    View::set_window_focused(&mut v, false);
    View::mouse_up(&mut v, &mut canvas_ws);
    assert_eq!(canvas_ws.version(), before);
    assert_eq!(
        read(&canvas_ws, root).get(card).unwrap().at,
        Some(CanvasPoint::new(0, 0))
    );
}

// ---------------------------------------------------------------- pages

/// **An output node's link opens its page like any document** — a press on
/// the link, through the one request every door uses.
#[test]
fn an_output_nodes_link_opens_its_page() {
    let OnACanvas {
        mut v,
        mut canvas_ws,
        root,
        page,
        ..
    } = on_a_canvas();
    let output = model::add_object(
        &mut canvas_ws,
        root,
        ObjectKind::Output,
        CanvasPoint::new(0, 0),
        "",
    )
    .unwrap();
    model::set_output_page(&mut canvas_ws, output, page).unwrap();
    View::adopt(&mut v, &canvas_ws);
    maintain(&mut v, &mut canvas_ws);
    let _ = requests(&mut v);
    assert_eq!(
        active_canvas(&v)
            .decor()
            .pages
            .get(&page)
            .map(String::as_str),
        Some("Brief"),
        "the page's title, from the listing"
    );

    // An output node with no text: its header, then its link.
    let h = f64::from(row_h());
    let link = window_point(&v, 20.0, 12.0 + h + 4.0 + h * 0.5);
    click(&mut v, &mut canvas_ws, link);
    assert!(
        requests(&mut v).iter().any(|r| matches!(
            r,
            PaneRequest::Navigate {
                target: NavigationTarget::Document { document, .. },
                ..
            } if *document == page
        )),
        "the page was asked for"
    );
}

/// A node's header row and its buttons are one compact action tall.
fn row_h() -> f32 {
    koru_ui::ActionButton::H
}

/// **Place on canvas** copies an agent's result out as a card beside it.
#[test]
fn place_result_copies_an_agents_result_beside_it() {
    let OnACanvas {
        mut v,
        mut canvas_ws,
        root,
        ..
    } = on_a_canvas();
    let agent = model::add_object(
        &mut canvas_ws,
        root,
        ObjectKind::Agent,
        CanvasPoint::new(0, 0),
        "",
    )
    .unwrap();
    model::set_result(
        &mut canvas_ws,
        agent,
        &["Three findings".to_owned(), "and a fourth".to_owned()],
    )
    .unwrap();
    View::adopt(&mut v, &canvas_ws);
    canvas_ws.enable_undo();
    // Reached the way the node's own button reaches it: a request.
    if let TabView::Canvas(canvas) = v.active_view() {
        canvas.push_request(koru_canvas_ui::CanvasRequest::PlaceResult {
            key: crate::canvas_view::object_key(agent),
        });
        assert!(canvas.perform(&mut canvas_ws));
    }
    let canvas = read(&canvas_ws, root);
    let card = canvas
        .objects
        .iter()
        .find(|o| o.kind == ObjectKind::Card)
        .expect("a card");
    assert_eq!(card.text, "Three findings");
    assert_eq!(card.lines, vec!["and a fourth".to_owned()]);
    assert_eq!(
        card.at,
        Some(CanvasPoint::new(model::DEFAULT_WIDTH + 24, 0)),
        "beside the node"
    );
    undo(&mut v, &mut canvas_ws);
    assert_eq!(read(&canvas_ws, root).objects.len(), 1, "one step");
}

/// **Images dropped on the canvas land where the pointer is**, side by side,
/// at most 480 wide, in one step.
#[test]
fn dropped_images_land_under_the_pointer_in_one_step() {
    let OnACanvas {
        mut v,
        mut canvas_ws,
        root,
        ..
    } = on_a_canvas();
    canvas_ws.enable_undo();
    let point = over_the_view(&v);
    View::mouse_move(&mut v, point.0, point.1);
    let at = canvas_point(&v, point);
    View::paste_images(
        &mut v,
        &mut canvas_ws,
        None,
        &[
            koru_workspace_host::PastedImage {
                hash: "fnv1a:01".into(),
                mime: "image/png".into(),
                width: Some(1600),
                height: Some(900),
                name: None,
            },
            koru_workspace_host::PastedImage {
                hash: "fnv1a:02".into(),
                mime: "image/png".into(),
                width: Some(200),
                height: Some(100),
                name: None,
            },
        ],
    );
    let canvas = read(&canvas_ws, root);
    assert_eq!(canvas.objects.len(), 2);
    assert!(canvas.objects.iter().all(|o| o.kind == ObjectKind::Image));
    assert_eq!(canvas.objects[0].at, Some(at));
    assert_eq!(canvas.objects[0].extent.map(|e| e.w), Some(480));
    assert_eq!(
        canvas.objects[1].at,
        Some(CanvasPoint::new(at.x + 480 + 24, at.y))
    );
    assert_eq!(canvas.objects[1].extent.map(|e| e.w), Some(200));
    undo(&mut v, &mut canvas_ws);
    assert!(read(&canvas_ws, root).objects.is_empty(), "one step");
}

/// **Insert image…** asks the host for its picker with the canvas's root, and
/// the file it comes back with lands where the menu was opened.
#[test]
fn insert_image_asks_the_picker_and_places_what_it_returns() {
    let OnACanvas {
        mut v,
        mut canvas_ws,
        root,
        ..
    } = on_a_canvas();
    add_from_menu(&mut v, &mut canvas_ws, (100.0, 50.0), 4);
    assert_eq!(View::take_wants_image(&mut v), Some(root));
    assert_eq!(View::take_wants_image(&mut v), None, "asked once");
    View::insert_image(
        &mut v,
        &mut canvas_ws,
        root,
        "fnv1a:03",
        "image/png",
        Some(300),
        Some(300),
    );
    let canvas = read(&canvas_ws, root);
    assert_eq!(canvas.objects.len(), 1);
    assert_eq!(canvas.objects[0].kind, ObjectKind::Image);
    assert_eq!(canvas.objects[0].at, Some(CanvasPoint::new(100, 50)));
}
