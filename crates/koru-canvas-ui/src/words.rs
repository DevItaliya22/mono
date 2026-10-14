// SPDX-License-Identifier: MPL-2.0

//! **Every word this surface says, through `koru-i18n`.**
//!
//! The fixed words are keys; the dynamic ones — what a running agent is
//! doing, how long it has been at it, why it failed, what a card says — are
//! the caller's, filled into a template rather than glued on, because a
//! sentence built from pieces is right only in the language it was built for.

use koru_i18n::{Key, Lang, fill, t};

use crate::model::{NodeStatus, ObjectKind, SceneObject};

/// The word for what an object is.
pub(crate) fn kind_word(lang: Lang, kind: ObjectKind) -> &'static str {
    t(
        lang,
        match kind {
            ObjectKind::Card => Key::CanvasCard,
            ObjectKind::Image => Key::CanvasImage,
            ObjectKind::Start => Key::CanvasStart,
            ObjectKind::Agent => Key::CanvasAgent,
            ObjectKind::Output => Key::CanvasOutput,
        },
    )
}

/// A node's status in words, or `None` for one with nothing to say: idle, or
/// no status at all.
pub(crate) fn status_text(lang: Lang, status: &NodeStatus) -> Option<String> {
    Some(match status {
        NodeStatus::Idle => return None,
        NodeStatus::NeedsResponse => t(lang, Key::DelegateRunNeedsYou).to_owned(),
        NodeStatus::Running { activity, elapsed } => {
            match (activity.trim().is_empty(), elapsed.trim().is_empty()) {
                (false, false) => fill(t(lang, Key::CanvasStatusRunning), &[activity, elapsed]),
                (false, true) => activity.clone(),
                (true, false) => elapsed.clone(),
                (true, true) => t(lang, Key::CanvasStatusRunningPlain).to_owned(),
            }
        }
        NodeStatus::Done => t(lang, Key::CanvasStatusDone).to_owned(),
        NodeStatus::Failed(why) if why.trim().is_empty() => {
            t(lang, Key::CanvasStatusFailedPlain).to_owned()
        }
        NodeStatus::Failed(why) => fill(t(lang, Key::CanvasStatusFailed), &[why]),
        NodeStatus::Stopped => t(lang, Key::CanvasStatusStopped).to_owned(),
        NodeStatus::NotStarted => t(lang, Key::CanvasStatusNotStarted).to_owned(),
    })
}

/// What a screen reader says for an object: its kind, its first line, and
/// its status when it has one.
pub(crate) fn object_label(lang: Lang, object: &SceneObject) -> String {
    let kind = kind_word(lang, object.kind);
    let text = object.text.trim();
    let named = if text.is_empty() {
        kind.to_owned()
    } else {
        fill(t(lang, Key::CanvasObjectLabel), &[kind, text])
    };
    match object
        .status
        .as_ref()
        .and_then(|status| status_text(lang, status))
    {
        Some(status) => fill(t(lang, Key::CanvasObjectStatus), &[&named, &status]),
        None => named,
    }
}

/// The zoom readout: `125%`, in the reader's digits and with the reader's
/// spacing around the sign.
pub(crate) fn zoom_percent(lang: Lang, zoom: f64) -> String {
    let percent = (zoom * 100.0).round().max(0.0) as u64;
    fill(
        t(lang, Key::CanvasZoomPercent),
        &[&koru_i18n::number::decimal(lang, percent)],
    )
}
