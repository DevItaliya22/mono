// SPDX-License-Identifier: MPL-2.0

//! The slash menu's real search field and visible commands.
//!
//! The query owns typing throughout arrow navigation. Result rows are invocable
//! menu items, not selection items: marking one selected would remove its UIA
//! Invoke pattern and make activating the current result a no-op. The command
//! labels remain the owner's existing drawn labels; the query name is localized.
//! Other editor picker kinds remain outside this projection.

use accesskit::{Action, Role};
use koru_core::schema::WorkspaceDoc;
use koru_draw::geometry::Rect;
use koru_editor::components::{
    Overlay,
    command_menu::{self, CommandMenu},
};
use koru_editor::{Editing, Editor};
use koru_i18n::{Key, t};
use koru_workspace_host::accessibility::{ControlPart, Occurrence, Revision, SemanticNode};

use super::{Shell, instance_of};
use crate::{
    leaf::Leaf,
    mainview::{MainView, tab::TabView},
};

const KIND: &str = "command";
const QUERY: ControlPart = ControlPart::new("query");

fn editor(view: &MainView) -> Option<&Editor> {
    let TabView::Leaf(Leaf::Document(document)) = &view.tabs().active_tab().view else {
        return None;
    };
    Some(&document.editor)
}

fn editor_mut(view: &mut MainView) -> Option<&mut Editor> {
    let index = view.tabs().active();
    let TabView::Leaf(Leaf::Document(document)) = &mut view.tabs_mut().get_mut(index)?.view else {
        return None;
    };
    Some(&mut document.editor)
}

fn current(view: &MainView) -> Option<(&CommandMenu, Rect)> {
    let editor = editor(view)?;
    let Overlay::Command(menu) = &editor.view.overlay else {
        return None;
    };
    // Drawing and pointer routing translate this editor-local panel by the focused pane.
    Some((
        menu,
        translated(view.focused_view_rect(), editor.overlay_panel_rect()?),
    ))
}

fn translated(panel: Rect, local: Rect) -> Rect {
    Rect::new(panel.x + local.x, panel.y + local.y, local.w, local.h)
}

fn project(cx: &Shell, menu: &CommandMenu, panel: Rect) -> SemanticNode {
    let mut children = Vec::new();
    if let Some(query) = &menu.query {
        children.push(
            SemanticNode::new(cx.instance(KIND, menu.instance, QUERY), Role::TextInput)
                .with_label(t(cx.lang, Key::A11ySearch))
                .with_value(query.value())
                .with_bounds(translated(panel, command_menu::search_rect(panel.w)))
                .with_action(Action::Focus),
        );
    }
    children.extend(menu.visible_controls(panel.w).map(|(command, rect)| {
        SemanticNode::new(
            cx.instance(KIND, menu.instance, ControlPart::new(command.name())),
            Role::MenuItem,
        )
        .with_label(command.label())
        .with_bounds(translated(panel, rect))
        .with_action(Action::Click)
        .disabled(command_menu::disabled(command, &menu.allows).is_some())
    }));
    SemanticNode::new(
        cx.instance(KIND, menu.instance, ControlPart::ROOT),
        Role::Menu,
    )
    .with_label(t(cx.lang, Key::A11yMenu))
    .with_bounds(panel)
    .with_children(children)
}

pub(super) fn semantics(view: &MainView, cx: &Shell) -> Option<SemanticNode> {
    let (menu, panel) = current(view)?;
    Some(project(cx, menu, panel))
}

/// Called after the shell's higher-priority keyboard owners have been checked.
pub(super) fn focus(view: &MainView, cx: &Shell) -> Option<Occurrence> {
    let (menu, _) = current(view)?;
    (editor(view)?.view.active && menu.has_search())
        .then(|| cx.instance(KIND, menu.instance, QUERY))
}

pub(super) fn revise(view: &MainView, rev: &mut Revision) {
    let Some((menu, panel)) = current(view) else {
        rev.byte(0);
        return;
    };
    rev.byte(1)
        .u64(menu.instance)
        .u64(menu.sel as u64)
        .u64(menu.first_visible() as u64);
    rev.f32(panel.x).f32(panel.y).f32(panel.w).f32(panel.h);
    rev.bool(menu.has_search());
    if let Some(query) = &menu.query {
        rev.str(&query.value());
    }
    for command in &menu.hits {
        rev.str(command.name())
            .bool(command_menu::disabled(*command, &menu.allows).is_some());
    }
}

pub(super) fn action(
    view: &mut MainView,
    ws: &mut WorkspaceDoc,
    target: Occurrence,
    action: Action,
) -> bool {
    let Some(instance) = instance_of(target, KIND) else {
        return false;
    };
    let Some((menu, _)) = current(view) else {
        return false;
    };
    if instance != menu.instance {
        return false;
    }
    if target.part == QUERY {
        if action != Action::Focus || !menu.has_search() {
            return false;
        }
        view.ai_focused = false;
        view.explore.clear_focus();
        if let Some(editor) = editor_mut(view) {
            editor.view.active = true;
        }
        return true;
    }
    if action != Action::Click {
        return false;
    }
    let Some(outcome) = menu.invoke_visible(target.part.name()) else {
        return false;
    };
    let trigger = menu.trigger.clone();
    let Some(editor) = editor_mut(view) else {
        return false;
    };
    editor.apply_overlay_outcome(ws, Some(trigger), outcome);
    true
}

#[cfg(test)]
mod tests;
