// SPDX-License-Identifier: MPL-2.0

//! The shipping Host uses ManagerView, so exercise that boundary rather than
//! calling MainView's otherwise-working projection directly. All state is local.
use super::*;
use accesskit::Action;
use koru_core::DocumentId;
use koru_workspace::durability::{DocumentDurability, Durability, LinkHealth};
use koru_workspace_host::accessibility::{
    CaptionGeometry, Scope, SemanticNode, ViewSlot, WindowSlot,
};

const WINDOW: WindowSlot = WindowSlot::from_raw(17);
const VIEW: ViewSlot = ViewSlot::from_raw(3);

fn container() -> (ManagerView, WorkspaceDoc, NodeId, DocumentId) {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "test").unwrap();
    let root = koru_workspace::document(&mut ws, "Fixture").unwrap();
    let paragraph = ws
        .insert(
            Parent::Node(root),
            Node::paragraph("Editable fixture").unwrap(),
        )
        .unwrap();
    let document = DocumentId::new();
    let mut view = ManagerView::Container(MainView::new());
    View::set_viewport(&mut view, 1200.0, 700.0);
    assert!(View::show_active_document(&mut view, &ws, Some(document)));
    View::seat_accessibility(&mut view, WINDOW, VIEW);
    (view, ws, paragraph, document)
}

fn flatten(nodes: Vec<SemanticNode>) -> Vec<SemanticNode> {
    fn add(node: SemanticNode, result: &mut Vec<SemanticNode>) {
        let children = node.children.clone();
        result.push(node);
        for child in children {
            add(child, result);
        }
    }
    let mut result = Vec::new();
    for node in nodes {
        add(node, &mut result);
    }
    result
}

fn nodes(view: &ManagerView) -> Vec<SemanticNode> {
    flatten(View::semantics(view, VIEW, Scope::Whole))
}

#[test]
fn shipping_manager_seats_and_publishes_the_container_subtree_and_caption_bounds() {
    let (view, _, paragraph, _) = container();
    let tree = nodes(&view);
    assert!(
        !tree.is_empty(),
        "Host::V must not fall back to the empty View projection"
    );
    assert!(
        tree.iter()
            .all(|node| node.occurrence.locator().starts_with("koru:w17/v3/"))
    );
    assert!(
        tree.iter()
            .any(|node| node.occurrence.locator().ends_with("/s:body#root"))
    );
    let paragraph = format!("/n:{}#text", paragraph.as_uuid().simple());
    assert!(
        tree.iter()
            .any(|node| node.occurrence.locator().ends_with(&paragraph))
    );
    let caption = View::window_command_bounds(&view);
    assert!(caption.minimize.is_some() && caption.maximize.is_some() && caption.close.is_some());
}

#[test]
fn shipping_manager_routes_focus_actions_revisions_and_retirement() {
    let (mut view, mut ws, paragraph, _) = container();
    let suffix = format!("/n:{}#text", paragraph.as_uuid().simple());
    let target = nodes(&view)
        .into_iter()
        .find(|node| node.occurrence.locator().ends_with(&suffix))
        .expect("published paragraph")
        .occurrence;
    let before = View::semantics_revision(&view);
    assert!(View::accessibility_action(
        &mut view,
        &mut ws,
        target,
        Action::Focus,
        None
    ));
    assert_eq!(View::accessibility_focus(&view, VIEW), Some(target));
    assert_ne!(View::semantics_revision(&view), before);
    let _ = View::semantics(&view, VIEW, Scope::Delta);
    let _ = View::accessibility_retired(&view);
    ws.delete(paragraph).unwrap();
    View::adopt(&mut view, &ws);
    let _ = View::semantics(&view, VIEW, Scope::Delta);
    assert!(View::accessibility_retired(&view).contains(&target));
    assert!(
        View::accessibility_retired(&view).is_empty(),
        "retirement remains a drain"
    );
    assert!(!View::accessibility_action(
        &mut view,
        &mut ws,
        target,
        Action::Focus,
        None
    ));
}

fn durability(document: DocumentId, durable: bool) -> Durability {
    Durability {
        documents: vec![(
            document,
            DocumentDurability {
                durable,
                in_flight: usize::from(!durable),
                buffered: false,
                buffer_failure: None,
                stopped: None,
                link: LinkHealth::Live,
                read_only: None,
            },
        )],
        document_list: None,
    }
}

#[test]
fn shipping_manager_durability_reaches_the_published_save_status() {
    let (mut view, _, _, document) = container();
    View::set_durability(&mut view, &durability(document, false));
    let pending = nodes(&view)
        .into_iter()
        .find(|node| node.occurrence.locator().ends_with("/s:save#document"))
        .expect("pending status")
        .label;
    let before = View::semantics_revision(&view);
    View::set_durability(&mut view, &durability(document, true));
    let saved = nodes(&view)
        .into_iter()
        .find(|node| node.occurrence.locator().ends_with("/s:save#document"))
        .expect("saved status")
        .label;
    assert_ne!(pending, saved);
    assert_eq!(
        saved.as_deref(),
        Some(koru_i18n::t(
            koru_i18n::Lang::En,
            koru_i18n::Key::A11ySaveSaved
        ))
    );
    assert_ne!(View::semantics_revision(&view), before);
}

#[test]
fn document_popout_keeps_its_explicitly_unsupported_accessibility_behavior() {
    let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "test").unwrap();
    let root = koru_workspace::document(&mut ws, "Popout").unwrap();
    let mut view = ManagerView::Document(DocumentPane::open(&ws, root, PopoutClose::default()));
    View::seat_accessibility(&mut view, WINDOW, VIEW);
    View::set_durability(&mut view, &durability(DocumentId::new(), true));
    assert!(View::semantics(&view, VIEW, Scope::Whole).is_empty());
    assert!(View::accessibility_retired(&view).is_empty());
    assert_eq!(View::accessibility_focus(&view, VIEW), None);
    assert_eq!(
        View::window_command_bounds(&view),
        CaptionGeometry::default()
    );
    assert_eq!(View::semantics_revision(&view), 0);
    let target = koru_workspace_host::accessibility::Occurrence::model(
        WINDOW,
        VIEW,
        root.as_uuid(),
        koru_workspace_host::accessibility::ControlPart::new("text"),
    );
    assert!(!View::accessibility_action(
        &mut view,
        &mut ws,
        target,
        Action::Focus,
        None
    ));
}
