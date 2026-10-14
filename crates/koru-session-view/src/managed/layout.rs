// SPDX-License-Identifier: MPL-2.0

use koru_session_protocol::ApprovalId;
use vello::kurbo::{Point, Rect};

use super::interaction::ManagedHit;

pub(crate) const ITEM_GAP: f32 = 8.0;
pub(crate) const ITEM_PAD_X: f32 = 12.0;
pub(crate) const ITEM_PAD_Y: f32 = 8.0;
/// The transcript's side margins are the pane's, not its own: a session that
/// switches between a raw grid and a managed transcript must not move its text
/// sideways. This was 12 while the grid had 0, which is the misalignment
/// [`CONTENT_INSET_X`](crate::CONTENT_INSET_X) now settles for both.
pub(crate) const TRANSCRIPT_PAD_X: f32 = crate::CONTENT_INSET_X;
/// Top and bottom, which nothing else shares. Four keeps the tail of the
/// transcript off the composer's top hairline while the content reaches the
/// pane's edges (it was twelve, which read as an empty band under the last
/// item).
pub(crate) const TRANSCRIPT_PAD_Y: f32 = 4.0;
pub(crate) const APPROVAL_WIDTH: f32 = 520.0;
pub(crate) const APPROVAL_EDGE: f32 = 16.0;
pub(crate) const BUTTON_H: f32 = 32.0;
pub(crate) const BUTTON_GAP: f32 = 8.0;
pub(crate) const BUTTON_PAD_X: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ManagedItemLayout {
    pub item_id: u64,
    pub rect: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ManagedApprovalLayout {
    pub panel: Rect,
    pub reject: Rect,
    pub approve: Rect,
    pub choices: Vec<(koru_session_protocol::ApprovalChoiceId, Rect)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ManagedLayout {
    pub items: Vec<ManagedItemLayout>,
    pub review_approvals: Vec<(ApprovalId, Rect)>,
    pub approval: Option<ManagedApprovalLayout>,
    pub cancel_turn: Option<Rect>,
}

impl ManagedLayout {
    pub fn hit(&self, x: f32, y: f32) -> Option<ManagedHit> {
        let point = Point::new(f64::from(x), f64::from(y));
        if let Some(approval) = &self.approval {
            for (id, rect) in &approval.choices {
                if rect.contains(point) {
                    return Some(ManagedHit::ApprovalChoice(*id));
                }
            }
            if approval.reject.contains(point) {
                return Some(ManagedHit::ApprovalReject);
            }
            if approval.approve.contains(point) {
                return Some(ManagedHit::ApprovalApprove);
            }
            // The confirmation owns the surface while open. A press outside
            // answers nothing and must not reach transcript controls.
            return None;
        }
        if let Some((approval_id, _)) = self
            .review_approvals
            .iter()
            .find(|(_, rect)| rect.contains(point))
        {
            return Some(ManagedHit::ReviewApproval(*approval_id));
        }
        self.cancel_turn
            .filter(|rect| rect.contains(point))
            .map(|_| ManagedHit::CancelTurn)
    }
}

pub(crate) fn approval_layout(
    viewport: Rect,
    panel_height: f32,
    reject_width: f32,
    approve_width: f32,
) -> ManagedApprovalLayout {
    let width = APPROVAL_WIDTH.min((viewport.width() as f32 - APPROVAL_EDGE * 2.0).max(0.0));
    let height = panel_height.min((viewport.height() as f32 - APPROVAL_EDGE * 2.0).max(0.0));
    let x = viewport.x0 as f32 + (viewport.width() as f32 - width) * 0.5;
    let y = viewport.y0 as f32 + (viewport.height() as f32 - height) * 0.5;
    let panel = Rect::new(
        f64::from(x),
        f64::from(y),
        f64::from(x + width),
        f64::from(y + height),
    );
    let approve_x = x + width - ITEM_PAD_X - approve_width;
    let reject_x = approve_x - BUTTON_GAP - reject_width;
    let button_y = y + height - ITEM_PAD_Y - BUTTON_H;
    ManagedApprovalLayout {
        panel,
        choices: Vec::new(),
        reject: Rect::new(
            f64::from(reject_x),
            f64::from(button_y),
            f64::from(reject_x + reject_width),
            f64::from(button_y + BUTTON_H),
        ),
        approve: Rect::new(
            f64::from(approve_x),
            f64::from(button_y),
            f64::from(approve_x + approve_width),
            f64::from(button_y + BUTTON_H),
        ),
    }
}
