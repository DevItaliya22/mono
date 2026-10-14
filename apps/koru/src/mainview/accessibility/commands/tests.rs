// SPDX-License-Identifier: MPL-2.0

//! In-memory shipping keyboard and semantic routes; no Store or desktop.
use super::*;
use koru_core::{NodeId, node::Node, schema::Parent};
use koru_i18n::Lang;
use koru_workspace::Workspace;
use koru_workspace_host::{
    View,
    accessibility::{Scope, ViewSlot, WindowSlot},
};
use winit::keyboard::{Key as InputKey, ModifiersState, NamedKey};

fn fixture() -> (MainView, Workspace, NodeId) {
    let (mut view, mut workspace) = crate::mainview::harness::container();
    let document = workspace
        .ws
        .insert(Parent::Root, Node::document("Commands").unwrap())
        .unwrap();
    let subject = workspace
        .ws
        .insert(
            Parent::Node(document),
            Node::paragraph("M3 checklist item").unwrap(),
        )
        .unwrap();
    view.seat_accessibility(WindowSlot::from_raw(1));
    view.open_node(&workspace.ws, document);
    super::super::document::put_caret_in(&mut view, subject);
    key(&mut view, &mut workspace, NamedKey::Home);
    (view, workspace, subject)
}

fn key(view: &mut MainView, workspace: &mut Workspace, key: NamedKey) {
    View::key(
        view,
        &mut workspace.ws,
        &InputKey::Named(key),
        None,
        ModifiersState::empty(),
    );
}

fn type_text(view: &mut MainView, workspace: &mut Workspace, text: &str) {
    View::key(
        view,
        &mut workspace.ws,
        &InputKey::Character(text.into()),
        Some(text),
        ModifiersState::empty(),
    );
}

fn shell(view: &MainView) -> Shell {
    view.shell(ViewSlot::from_raw(1))
}

fn text(view: &MainView, subject: NodeId) -> String {
    editor(view)
        .unwrap()
        .body
        .rows
        .iter()
        .find(|row| row.id == subject)
        .unwrap()
        .text
        .to_string()
}

#[test]
fn real_slash_query_keeps_text_out_of_the_document_and_enter_converts_the_same_node() {
    let (mut view, mut workspace, subject) = fixture();
    let paragraph_focus = view.accessibility_focus(ViewSlot::from_raw(1));
    type_text(&mut view, &mut workspace, "/");
    let cx = shell(&view);
    let query_focus = focus(&view, &cx).expect("slash owns the real query field");
    assert_eq!(view.accessibility_focus(cx.view), Some(query_focus));
    assert_eq!(query_focus.part, QUERY);
    assert_eq!(text(&view, subject), "/M3 checklist item");
    let before = view.semantics_revision();
    type_text(&mut view, &mut workspace, "task");
    assert_ne!(
        before,
        view.semantics_revision(),
        "query edits must republish chrome"
    );
    let menu = semantics(&view, &cx).unwrap();
    let query = menu
        .children
        .iter()
        .find(|node| node.occurrence.part == QUERY)
        .unwrap();
    assert_eq!(query.role, Role::TextInput);
    assert_eq!(query.value.as_deref(), Some("task"));
    assert_eq!(query.actions, vec![Action::Focus]);
    assert_eq!(view.accessibility_focus(cx.view), Some(query_focus));
    assert_eq!(text(&view, subject), "/M3 checklist item");
    key(&mut view, &mut workspace, NamedKey::Enter);
    assert!(current(&view).is_none());
    assert!(koru_core::tasks::is_task_node(&workspace.ws, subject).unwrap());
    assert_eq!(text(&view, subject), "M3 checklist item");
    assert_eq!(view.accessibility_focus(cx.view), paragraph_focus);
}

#[test]
fn query_and_results_use_the_owners_rectangles_and_localized_query_name() {
    let (mut view, mut workspace, _) = fixture();
    type_text(&mut view, &mut workspace, "/");
    if !view.sidebar.is_open() {
        assert!(view.toggle_sidebar());
    }
    let origin = view.focused_view_rect();
    assert!(
        origin.x > 0.0 && origin.y > 0.0,
        "exercise sidebar and header translation"
    );
    let local_panel = editor(&view).unwrap().overlay_panel_rect().unwrap();
    let (menu, panel) = current(&view).unwrap();
    assert_eq!(panel.x, origin.x + local_panel.x);
    assert_eq!(panel.y, origin.y + local_panel.y);
    let query_rect = translated(panel, command_menu::search_rect(panel.w));
    let pointer = view.to_leaf(
        query_rect.x + query_rect.w * 0.5,
        query_rect.y + query_rect.h * 0.5,
    );
    let local_query = translated(local_panel, command_menu::search_rect(local_panel.w));
    assert!((pointer.0 - (local_query.x + local_query.w * 0.5)).abs() < 0.01);
    assert!((pointer.1 - (local_query.y + local_query.h * 0.5)).abs() < 0.01);
    let mut cx = shell(&view);
    for lang in Lang::ALL {
        cx.lang = lang;
        let published = project(&cx, menu, panel);
        let query = &published.children[0];
        assert_eq!(query.label.as_deref(), Some(t(lang, Key::A11ySearch)));
        assert_eq!(
            query.bounds,
            Some(translated(panel, command_menu::search_rect(panel.w)))
        );
        for ((command, rect), node) in menu
            .visible_controls(panel.w)
            .zip(published.children.iter().skip(1))
        {
            assert_eq!(node.occurrence.part.name(), command.name());
            assert_eq!(node.bounds, Some(translated(panel, rect)));
            assert_eq!(node.role, Role::MenuItem);
            assert_eq!(node.actions, vec![Action::Click]);
            assert_eq!(node.selected, None, "current result must remain invocable");
        }
    }
}

#[test]
fn closed_openings_retire_and_cannot_receive_successor_actions() {
    let (mut view, mut workspace, _) = fixture();
    type_text(&mut view, &mut workspace, "/");
    let cx = shell(&view);
    let old_query = focus(&view, &cx).unwrap();
    view.semantics(cx.view, Scope::Whole);
    key(&mut view, &mut workspace, NamedKey::Escape);
    view.semantics(cx.view, Scope::Delta);
    assert!(view.accessibility_retired().contains(&old_query));
    assert!(!view.accessibility_action(&mut workspace.ws, old_query, Action::Focus, None));
    // Escape preserves the typed slash; remove it through the normal keyboard path.
    key(&mut view, &mut workspace, NamedKey::Home);
    key(&mut view, &mut workspace, NamedKey::Delete);
    type_text(&mut view, &mut workspace, "/");
    let next = focus(&view, &cx).unwrap();
    assert_ne!(old_query, next);
    assert!(!view.accessibility_action(&mut workspace.ws, old_query, Action::Focus, None));
    assert!(view.accessibility_action(&mut workspace.ws, next, Action::Focus, None));
}

#[test]
fn semantic_activation_uses_current_result_identity_and_the_existing_command() {
    let (mut view, mut workspace, subject) = fixture();
    type_text(&mut view, &mut workspace, "/");
    let cx = shell(&view);
    let instance = current(&view).unwrap().0.instance;
    let old = cx.instance(KIND, instance, ControlPart::new("attachment"));
    type_text(&mut view, &mut workspace, "task");
    assert!(!view.accessibility_action(&mut workspace.ws, old, Action::Click, None));
    let task = cx.instance(KIND, instance, ControlPart::new("task"));
    assert!(view.accessibility_action(&mut workspace.ws, task, Action::Click, None));
    assert!(koru_core::tasks::is_task_node(&workspace.ws, subject).unwrap());
    assert_eq!(text(&view, subject), "M3 checklist item");
    assert!(!view.accessibility_action(&mut workspace.ws, task, Action::Click, None));
}
