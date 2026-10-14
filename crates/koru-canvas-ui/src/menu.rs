// SPDX-License-Identifier: MPL-2.0

//! **The right-click menus**: what their rows are and what each asks for.
//!
//! The geometry, the drawing and the keyboard stepping are the shared
//! [`koru_ui::Menu`] — the same widget the container's context menus use, so
//! a canvas menu looks and steps like every other one in Koru. What lives here
//! is only which rows exist over which target and what choosing one does.

use std::borrow::Cow;

use koru_i18n::{Key, Lang, t};
use koru_ui::MenuItem;

use crate::model::{ObjectKind, SceneObject};

/// What a row does.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Action {
    Add(ObjectKind),
    InsertImage,
    Edit,
    Run,
    Stop,
    Respond,
    ChooseAgent,
    ConnectTo,
    BringToFront,
    SendToBack,
    Delete,
    DeleteArrow,
}

/// One row: its action and its words, fixed when the menu opens.
#[derive(Clone, Debug)]
pub(crate) struct Row {
    pub action: Action,
    label: &'static str,
}

impl MenuItem for Row {
    fn label(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.label)
    }

    fn destructive(&self) -> bool {
        matches!(self.action, Action::Delete | Action::DeleteArrow)
    }
}

fn row(lang: Lang, action: Action) -> Row {
    let key = match action {
        Action::Add(ObjectKind::Card) | Action::Add(ObjectKind::Image) => Key::CanvasAddCard,
        Action::Add(ObjectKind::Start) => Key::CanvasAddStartNode,
        Action::Add(ObjectKind::Agent) => Key::CanvasAddAgentNode,
        Action::Add(ObjectKind::Output) => Key::CanvasAddOutputNode,
        Action::InsertImage => Key::CanvasInsertImage,
        Action::Edit => Key::CanvasEdit,
        Action::Run => Key::CanvasRun,
        Action::Stop => Key::CanvasStop,
        Action::Respond => Key::CanvasRespond,
        Action::ChooseAgent => Key::CanvasChooseAgent,
        Action::ConnectTo => Key::CanvasConnectTo,
        Action::BringToFront => Key::CanvasBringToFront,
        Action::SendToBack => Key::CanvasSendToBack,
        Action::Delete => Key::CanvasDelete,
        Action::DeleteArrow => Key::CanvasDeleteArrow,
    };
    Row {
        action,
        label: t(lang, key),
    }
}

/// The menu over empty canvas. Without `workflows` it adds cards and
/// images only.
pub(crate) fn canvas_rows(lang: Lang, workflows: bool) -> Vec<Row> {
    [
        Action::Add(ObjectKind::Card),
        Action::Add(ObjectKind::Start),
        Action::Add(ObjectKind::Agent),
        Action::Add(ObjectKind::Output),
        Action::InsertImage,
    ]
    .into_iter()
    .filter(|action| workflows || !matches!(action, Action::Add(kind) if kind.is_workflow()))
    .map(|action| row(lang, action))
    .collect()
}

/// The menu over one object.
///
/// Without `workflows` a workflow node offers no Run, Stop, Respond or agent
/// choice; it can still be edited, connected, ordered and deleted.
pub(crate) fn object_rows(lang: Lang, object: &SceneObject, workflows: bool) -> Vec<Row> {
    let mut actions = vec![Action::Edit];
    if workflows && object.kind.runs() {
        let running = object.status.as_ref().is_some_and(|s| s.is_running());
        actions.push(if running { Action::Stop } else { Action::Run });
    }
    if workflows && object.kind == ObjectKind::Agent {
        if object.status == Some(crate::model::NodeStatus::NeedsResponse) {
            actions.push(Action::Respond);
        }
        actions.push(Action::ChooseAgent);
    }
    actions.extend([
        Action::ConnectTo,
        Action::BringToFront,
        Action::SendToBack,
        Action::Delete,
    ]);
    actions
        .into_iter()
        .map(|action| row(lang, action))
        .collect()
}

/// The menu over an arrow.
pub(crate) fn arrow_rows(lang: Lang) -> Vec<Row> {
    vec![row(lang, Action::DeleteArrow)]
}
