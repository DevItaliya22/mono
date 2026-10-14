// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_core::WorkspaceId;
use koru_core::node::CanvasPoint;
use winit::dpi::PhysicalPosition;

/// A canvas document as `Workspace::create_canvas` leaves it: a titled root
/// and nothing under it.
fn a_canvas(title: &str) -> (WorkspaceDoc, NodeId) {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "t").expect("create");
    let root = koru_workspace::seed_document(&mut ws, title, koru_core::explore::KIND_CANVAS)
        .expect("seed");
    (ws, root)
}

fn type_text(view: &mut CanvasView, ws: &mut WorkspaceDoc, text: &str) {
    for c in text.chars() {
        let s = c.to_string();
        View::key(
            view,
            ws,
            &Key::Character(s.as_str().into()),
            Some(&s),
            ModifiersState::empty(),
        );
    }
}

fn press(view: &mut CanvasView, ws: &mut WorkspaceDoc, key: NamedKey) -> bool {
    View::key(view, ws, &Key::Named(key), None, ModifiersState::empty())
}

/// The seam's first promise: which document, and which node a tab over it is
/// rooted at.
#[test]
fn a_canvas_view_answers_its_document_and_its_root() {
    let (_, root) = a_canvas("Board");
    let document = DocumentId::new();
    let view = CanvasView::new(document, root);
    assert_eq!(view.document(), document);
    assert_eq!(view.root(), root);
    assert_eq!(View::document_root(&view), Some(root));
    assert!(!view.is_dragging());
    assert!(!view.is_renaming());
}

/// **The name prompt writes the root's text**, which is the canvas's name —
/// the same node a page's rename writes, so the manifest mirrors it and every
/// cache of the title follows. It opens seeded and selected, so typing
/// replaces.
#[test]
fn the_name_prompt_renames_the_canvas_by_its_root() {
    let (mut ws, root) = a_canvas("Board");
    let mut view = CanvasView::new(DocumentId::new(), root);
    view.set_viewport(900.0, 600.0);
    assert!(view.start_rename(&ws));
    assert!(view.is_renaming());
    type_text(&mut view, &mut ws, "Plans");
    assert!(press(&mut view, &mut ws, NamedKey::Enter));
    assert!(!view.is_renaming(), "Enter commits and closes");
    assert_eq!(ws.read(root).unwrap().content.as_text(), Some("Plans"));
}

/// Escape and a press outside put the prompt away and change nothing.
#[test]
fn escape_or_a_press_outside_leaves_the_name_alone() {
    let (mut ws, root) = a_canvas("Board");
    let mut view = CanvasView::new(DocumentId::new(), root);
    view.set_viewport(900.0, 600.0);

    assert!(view.start_rename(&ws));
    type_text(&mut view, &mut ws, "Nope");
    assert!(press(&mut view, &mut ws, NamedKey::Escape));
    assert!(!view.is_renaming());
    assert_eq!(ws.read(root).unwrap().content.as_text(), Some("Board"));

    assert!(view.start_rename(&ws));
    type_text(&mut view, &mut ws, "Nope");
    assert!(View::mouse_down(&mut view, &mut ws, 2.0, 2.0, false));
    assert!(!view.is_renaming(), "a press on the scrim closes it");
    assert_eq!(ws.read(root).unwrap().content.as_text(), Some("Board"));
}

/// A press in the field selects text, and while it does the view is dragging —
/// what keeps the container forwarding pointer moves past the pane's edge.
#[test]
fn a_press_in_the_field_is_a_drag_until_release() {
    let (mut ws, root) = a_canvas("Board");
    let mut view = CanvasView::new(DocumentId::new(), root);
    view.set_viewport(900.0, 600.0);
    assert!(view.start_rename(&ws));
    let field = view.field();
    let (x, y) = field.centre();
    assert!(View::mouse_down(&mut view, &mut ws, x, y, false));
    assert!(view.is_dragging());
    View::mouse_move(&mut view, x + 10.0, y);
    assert!(View::mouse_up(&mut view, &mut ws));
    assert!(!view.is_dragging());
    assert!(view.is_renaming(), "a press inside keeps the prompt");
}

/// The prompt names nothing it cannot find: a view handed a document that does
/// not hold its root opens no prompt, rather than one that would write into
/// the wrong document.
#[test]
fn no_prompt_over_a_document_that_is_not_the_canvas() {
    let (_, root) = a_canvas("Board");
    let (stranger, _) = a_canvas("Other");
    let mut view = CanvasView::new(DocumentId::new(), root);
    assert!(!view.start_rename(&stranger));
    assert!(!view.is_renaming());
}

/// **Zoom is claimed**, wheel and chord alike, with the raw delta and the
/// pointer as handed — the placeholder's proof that the seam reaches it.
#[test]
fn the_canvas_claims_zoom_and_records_what_it_was_handed() {
    let (_, root) = a_canvas("Board");
    let mut view = CanvasView::new(DocumentId::new(), root);
    let pinch = MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 3.5));
    assert!(View::zoom_wheel(&mut view, pinch, 120.0, 80.0));
    assert!(View::zoom_key(&mut view, ZoomIntent::In));
    let seen = view.gestures();
    assert_eq!(
        seen.zoom_wheel,
        Some(WheelSeen {
            delta: (0.0, 3.5),
            at: (120.0, 80.0)
        })
    );
    assert_eq!(seen.zoom_key, Some(ZoomIntent::In));
    assert_eq!(seen.zooms_claimed, 2);
}

/// The plain wheel arrives with the pane-local pointer it came at, and the
/// modifiers are kept for a drag that has none of its own.
#[test]
fn the_wheel_and_the_modifiers_reach_the_canvas() {
    let (_, root) = a_canvas("Board");
    let mut view = CanvasView::new(DocumentId::new(), root);
    view.wheel(MouseScrollDelta::LineDelta(0.0, -2.0), 40.0, 30.0);
    assert_eq!(
        view.gestures().wheel,
        Some(WheelSeen {
            delta: (0.0, -2.0),
            at: (40.0, 30.0)
        })
    );
    View::set_mods(&mut view, ModifiersState::SHIFT);
    assert_eq!(view.mods(), ModifiersState::SHIFT);
}

/// It paints — the surface, and an open prompt — and leaves the scene
/// balanced, at a sane size and at none.
#[test]
fn drawing_the_canvas_leaves_the_scene_balanced() {
    let (ws, root) = a_canvas("Board");
    let workspace = koru_workspace::Workspace::new(ws);
    let images = ImageCache::new();
    let mut view = CanvasView::new(DocumentId::new(), root);
    for (w, h) in [(900.0, 600.0), (0.0, 0.0)] {
        view.set_viewport(w, h);
        let mut scene = Scene::new();
        View::draw(
            &mut view,
            &mut scene,
            &workspace,
            &images,
            Affine::IDENTITY,
            true,
            (0.0, 0.0),
        );
    }
    assert!(view.start_rename(&workspace.ws));
    view.set_viewport(900.0, 600.0);
    let mut scene = Scene::new();
    View::draw(
        &mut view,
        &mut scene,
        &workspace,
        &images,
        Affine::IDENTITY,
        true,
        (0.0, 0.0),
    );
}

// ------------------------------------------------------------ the surface

use koru_canvas_ui::{CanvasRequest, NodeStatus};
use koru_core::canvas::{self as model, ObjectKind};

fn draw_once(view: &mut CanvasView, workspace: &koru_workspace::Workspace) {
    let mut scene = Scene::new();
    View::draw(
        view,
        &mut scene,
        workspace,
        &ImageCache::new(),
        Affine::IDENTITY,
        true,
        (0.0, 0.0),
    );
}

#[test]
fn responding_is_a_read_only_request_and_has_an_accessible_button() {
    use accesskit::{Action, Role};
    use koru_workspace_host::accessibility::{Occurrence, ViewSlot, WindowSlot};

    let (mut ws, root) = a_canvas("Board");
    let agent = model::add_object(
        &mut ws,
        root,
        ObjectKind::Agent,
        CanvasPoint::new(0, 0),
        "Ask me",
    )
    .unwrap();
    let mut workspace = koru_workspace::Workspace::new(ws);
    let mut view = CanvasView::over(DocumentId::new(), root, &workspace.ws);
    view.set_viewport(900.0, 600.0);
    view.set_decor(Decor {
        statuses: [(agent, NodeStatus::NeedsResponse)].into_iter().collect(),
        ..Default::default()
    });
    draw_once(&mut view, &workspace);
    let before = workspace.ws.version();
    view.push_request(CanvasRequest::Respond {
        key: object_key(agent),
    });
    assert!(view.perform(&mut workspace.ws));
    assert_eq!(view.take_asks(), vec![CanvasAsk::Respond(agent)]);
    assert_eq!(
        workspace.ws.version(),
        before,
        "opening the question changes no content"
    );

    let cx = accessibility::Context {
        window: WindowSlot::from_raw(1),
        view: ViewSlot::from_raw(1),
        area: Rect::new(10.0, 20.0, 900.0, 600.0),
        lang: koru_i18n::Lang::En,
        name: "Board".into(),
    };
    let tree = view.semantics(&cx);
    let object = &tree.children[0].children[0];
    let button = &object.children[0];
    assert_eq!(button.role, Role::Button);
    assert_eq!(button.label.as_deref(), Some("Respond…"));
    let target = Occurrence::model(
        cx.window,
        cx.view,
        agent.as_uuid(),
        accessibility::parts::RESPOND,
    );
    assert!(view.accessibility_action(target, Action::Click));
    assert_eq!(view.take_asks(), vec![CanvasAsk::Respond(agent)]);
    view.set_decor(Decor::default());
    assert!(
        !view.accessibility_action(target, Action::Click),
        "stale buttons cannot respond"
    );
}

/// **The view reads its canvas when it is built over it**, and again only
/// when the document has moved.
#[test]
fn the_view_reads_its_canvas_and_rereads_only_when_it_moved() {
    let (mut ws, root) = a_canvas("Board");
    model::add_object(
        &mut ws,
        root,
        ObjectKind::Card,
        CanvasPoint::new(0, 0),
        "One",
    )
    .unwrap();
    let mut view = CanvasView::over(DocumentId::new(), root, &ws);
    assert_eq!(view.snapshot().unwrap().objects.len(), 1);
    assert!(!view.catch_up(&ws), "nothing moved: nothing read");
    model::add_object(
        &mut ws,
        root,
        ObjectKind::Card,
        CanvasPoint::new(300, 0),
        "Two",
    )
    .unwrap();
    assert!(view.catch_up(&ws), "a new card: read again");
    assert_eq!(view.surface().semantics().len(), 2);
    // A document that does not hold the canvas is never read.
    let (stranger, _) = a_canvas("Other");
    assert!(!view.catch_up(&stranger));
}

/// **A request is performed on the document, and the scene answers it** —
/// a move written once, a refused arrow snapped back.
#[test]
fn a_request_is_written_and_the_scene_is_handed_back() {
    let (mut ws, root) = a_canvas("Board");
    let a =
        model::add_object(&mut ws, root, ObjectKind::Card, CanvasPoint::new(0, 0), "A").unwrap();
    let mut view = CanvasView::over(DocumentId::new(), root, &ws);
    view.set_viewport(900.0, 600.0);
    let key = object_key(a);
    view.push_request(CanvasRequest::Move { key, x: 40, y: 10 });
    view.push_request(CanvasRequest::Connect { from: key, to: key });
    let revision = view.surface().revision();
    assert!(view.perform(&mut ws));
    assert_eq!(
        model::read(&ws, root).unwrap().objects[0].at,
        Some(CanvasPoint::new(40, 10))
    );
    assert!(
        model::read(&ws, root).unwrap().arrows.is_empty(),
        "a self-arrow is refused"
    );
    assert!(
        view.surface().revision() > revision,
        "and the scene was handed back"
    );
    assert!(!view.perform(&mut ws), "nothing left to do");
}

/// **The decor dresses the scene**: an agent by its name (or by its stored
/// key when this machine does not offer it), a node's status, a page's title.
#[test]
fn the_decor_names_agents_statuses_and_pages() {
    let (mut ws, root) = a_canvas("Board");
    let agent = model::add_object(
        &mut ws,
        root,
        ObjectKind::Agent,
        CanvasPoint::new(0, 0),
        "Do it",
    )
    .unwrap();
    model::set_agent(&mut ws, agent, "codex", None).unwrap();
    let other = model::add_object(
        &mut ws,
        root,
        ObjectKind::Agent,
        CanvasPoint::new(300, 0),
        "",
    )
    .unwrap();
    model::set_agent(&mut ws, other, "custom:elsewhere", None).unwrap();
    let mut view = CanvasView::over(DocumentId::new(), root, &ws);
    let decor = Decor {
        agents: vec![("codex".into(), "Codex".into())],
        agent_names: [("codex".to_owned(), "Codex".to_owned())]
            .into_iter()
            .collect(),
        default_agent: Some("codex".into()),
        statuses: [(agent, NodeStatus::Done)].into_iter().collect(),
        pages: Default::default(),
    };
    assert!(view.set_decor(decor.clone()));
    assert!(!view.set_decor(decor), "the same decor rebuilds nothing");
    let scene = decor::scene_of(view.snapshot().unwrap(), view.decor(), koru_i18n::Lang::En);
    let named = |id| {
        scene
            .objects
            .iter()
            .find(|o| o.key == object_key(id))
            .unwrap()
            .clone()
    };
    assert_eq!(named(agent).agent.as_deref(), Some("Codex"));
    assert_eq!(named(agent).status, Some(NodeStatus::Done));
    assert_eq!(named(other).agent.as_deref(), Some("custom:elsewhere"));
    assert_eq!(named(other).status, None, "no state: no status line");
}

/// **It draws with everything open** — objects, the agent menu, the text
/// dialog — and a running node keeps the frames coming only while it is
/// drawn.
#[test]
fn drawing_with_the_dialog_and_the_agent_menu_open() {
    let (mut ws, root) = a_canvas("Board");
    let agent = model::add_object(
        &mut ws,
        root,
        ObjectKind::Agent,
        CanvasPoint::new(0, 0),
        "Summarise",
    )
    .unwrap();
    model::add_object(
        &mut ws,
        root,
        ObjectKind::Card,
        CanvasPoint::new(-300, 0),
        "Input",
    )
    .unwrap();
    let mut workspace = koru_workspace::Workspace::new(ws);
    let mut view = CanvasView::over(DocumentId::new(), root, &workspace.ws);
    view.set_viewport(900.0, 600.0);
    view.set_decor(Decor {
        agents: vec![("codex".into(), "Codex".into())],
        statuses: [(
            agent,
            NodeStatus::Running {
                activity: "Reading".into(),
                elapsed: "0:02".into(),
            },
        )]
        .into_iter()
        .collect(),
        ..Decor::default()
    });
    assert!(!View::animating(&view), "nothing turns before it is drawn");
    draw_once(&mut view, &workspace);
    assert!(View::animating(&view), "a running node's mark turns");

    view.push_request(CanvasRequest::ChooseAgent {
        key: object_key(agent),
    });
    assert!(view.perform(&mut workspace.ws));
    assert!(view.is_choosing_agent());
    draw_once(&mut view, &workspace);

    let key = Key::Named(NamedKey::Escape);
    View::key(
        &mut view,
        &mut workspace.ws,
        &key,
        None,
        ModifiersState::empty(),
    );
    assert!(!view.is_choosing_agent());
    view.push_request(CanvasRequest::EditText {
        key: object_key(agent),
    });
    assert!(view.perform(&mut workspace.ws));
    assert!(view.is_editing_text());
    draw_once(&mut view, &workspace);
    assert_eq!(View::cursor_icon(&view), koru_ui::cursor::IDLE);
}

/// **Ctrl+Z never unmakes the canvas.** A new canvas is seeded after the
/// store has switched undo on, so its first step is its own root; undoing
/// past the first card must stop there, with the canvas still under the tab.
#[test]
fn undo_never_unmakes_the_canvas_itself() {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "t").expect("create");
    ws.enable_undo();
    let root = koru_workspace::seed_document(&mut ws, "Board", koru_core::explore::KIND_CANVAS)
        .expect("seed");
    ws.checkpoint();
    let mut view = CanvasView::over(DocumentId::new(), root, &ws);
    view.set_viewport(900.0, 600.0);
    view.push_request(CanvasRequest::AddObject {
        kind: koru_canvas_ui::ObjectKind::Start,
        x: 0,
        y: 0,
    });
    assert!(view.perform(&mut ws));
    assert_eq!(model::read(&ws, root).unwrap().objects.len(), 1);

    let ctrl_z = |view: &mut CanvasView, ws: &mut WorkspaceDoc| {
        View::key(
            view,
            ws,
            &Key::Character("z".into()),
            Some("z"),
            koru_keys::PRIMARY,
        )
    };
    assert!(ctrl_z(&mut view, &mut ws));
    assert!(
        model::read(&ws, root).unwrap().objects.is_empty(),
        "the node went"
    );
    assert!(ctrl_z(&mut view, &mut ws));
    assert!(ws.read(root).is_ok(), "the canvas did not");
    assert_eq!(ws.read(root).unwrap().content.as_text(), Some("Board"));
}

/// **A canvas in a build that hides agents** (`ENABLE_CANVAS` on,
/// `ENABLE_AGENTS` off): the surface offers
/// no workflow half, and the requests that belong to it are refused at
/// `perform` even when something asks for them — no start, agent or output
/// node is made, nothing runs, nothing is answered. A card still is.
#[test]
fn with_agents_hidden_a_canvas_is_cards_and_images() {
    use koru_features::{Features, testing};
    testing::with(
        Features {
            canvas: true,
            ..Features::ALL_OFF
        },
        || {
            let (mut ws, root) = a_canvas("Board");
            let agent = model::add_object(
                &mut ws,
                root,
                ObjectKind::Agent,
                CanvasPoint::new(0, 0),
                "Ask me",
            )
            .unwrap();
            let mut view = CanvasView::over(DocumentId::new(), root, &ws);
            view.set_viewport(900.0, 600.0);
            assert!(!view.surface().workflows());
            let key = object_key(agent);
            for request in [
                CanvasRequest::Run { key },
                CanvasRequest::Stop { key },
                CanvasRequest::Respond { key },
                CanvasRequest::ChooseAgent { key },
                CanvasRequest::AddObject {
                    kind: koru_canvas_ui::ObjectKind::Agent,
                    x: 300,
                    y: 0,
                },
                CanvasRequest::AddObject {
                    kind: koru_canvas_ui::ObjectKind::Start,
                    x: 300,
                    y: 200,
                },
            ] {
                view.push_request(request);
            }
            let _ = view.perform(&mut ws);
            assert!(view.take_asks().is_empty(), "nothing runs or answers");
            assert_eq!(
                model::read(&ws, root).unwrap().objects.len(),
                1,
                "no workflow node was made"
            );
        },
    );
    // With agents on, the same view offers the workflow half.
    let (_, root) = a_canvas("Board");
    assert!(
        CanvasView::new(DocumentId::new(), root)
            .surface()
            .workflows()
    );
}
