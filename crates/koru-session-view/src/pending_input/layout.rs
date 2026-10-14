// SPDX-License-Identifier: MPL-2.0

use koru_draw::geometry::Rect;

use koru_session_protocol::AttachmentId;

/// The band's height. It was 146 with a 12-pixel floor; the floor shrank to
/// four (the pane-filling pass) and the band gave those eight pixels back to
/// the grid above it, so the rows between the heading, the chips and the
/// editor are unchanged.
pub const COMPOSER_HEIGHT: f32 = 138.0;
/// The band's side margins are the pane's, so the heading, the attachment chips
/// and the send button line up with the grid or the transcript above them —
/// [`CONTENT_INSET_X`](crate::CONTENT_INSET_X). This was 12 against a grid at 0,
/// which read as three different left edges in one pane.
const PAD_X: f32 = crate::CONTENT_INSET_X;
/// The band's own floor, which nothing above it shares. Four keeps the editor
/// off the pane's bottom edge without the empty band twelve drew under it.
const PAD_Y: f32 = 4.0;
const HEADING_H: f32 = 22.0;
const CHIP_H: f32 = 46.0;
const EDITOR_H: f32 = 40.0;
const BUTTON_W: f32 = 40.0;
const GAP: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingInputHit {
    Draft,
    Attach,
    Send,
    Retry,
    Remove(AttachmentId),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttachmentLayout {
    pub id: AttachmentId,
    pub chip: Rect,
    pub preview: Rect,
    pub remove: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PendingInputLayout {
    pub area: Rect,
    pub heading: Rect,
    pub delivery: Rect,
    pub attachments: Vec<AttachmentLayout>,
    pub overflow: Option<Rect>,
    pub error: Rect,
    pub draft: Rect,
    pub attach: Rect,
    pub send: Rect,
}

impl PendingInputLayout {
    pub fn solve(area: Rect, ids: impl IntoIterator<Item = AttachmentId>) -> Self {
        let area = Rect::new(area.x, area.y, area.w.max(0.0), area.h.max(0.0));
        let heading = Rect::new(area.x + PAD_X, area.y + 6.0, area.w * 0.45, HEADING_H);
        let delivery = Rect::new(
            area.x + area.w * 0.45,
            area.y + 6.0,
            (area.w * 0.55 - PAD_X).max(0.0),
            HEADING_H,
        );
        let chip_y = heading.y + HEADING_H;
        let chip_right = area.x + area.w - PAD_X;
        let mut x = area.x + PAD_X;
        let mut attachments = Vec::new();
        let ids: Vec<_> = ids.into_iter().collect();
        for id in ids.iter().copied() {
            let remaining = chip_right - x;
            if remaining < 74.0 {
                break;
            }
            let width = remaining.min(104.0);
            let chip = Rect::new(x, chip_y, width, CHIP_H);
            attachments.push(AttachmentLayout {
                id,
                chip,
                preview: Rect::new(chip.x + 4.0, chip.y + 4.0, 38.0, 38.0),
                remove: Rect::new(chip.x + chip.w - 22.0, chip.y + 2.0, 20.0, 20.0),
            });
            x += width + GAP;
        }
        let overflow = (attachments.len() < ids.len())
            .then(|| Rect::new(x, chip_y, (chip_right - x).clamp(0.0, 48.0), CHIP_H));
        let editor_y = area.y + area.h - PAD_Y - EDITOR_H;
        let attach = Rect::new(area.x + PAD_X, editor_y, BUTTON_W, EDITOR_H);
        let send = Rect::new(
            area.x + area.w - PAD_X - BUTTON_W,
            editor_y,
            BUTTON_W,
            EDITOR_H,
        );
        let draft = Rect::new(
            attach.x + attach.w + GAP,
            editor_y,
            (send.x - GAP - attach.x - attach.w - GAP).max(0.0),
            EDITOR_H,
        );
        let error = Rect::new(
            draft.x,
            chip_y + CHIP_H,
            draft.w,
            (editor_y - chip_y - CHIP_H).max(0.0),
        );
        Self {
            area,
            heading,
            delivery,
            attachments,
            overflow,
            error,
            draft,
            attach,
            send,
        }
    }

    pub fn hit(&self, x: f32, y: f32, retrying: bool) -> Option<PendingInputHit> {
        self.attach
            .contains(x, y)
            .then_some(PendingInputHit::Attach)
            .or_else(|| {
                self.send.contains(x, y).then_some(if retrying {
                    PendingInputHit::Retry
                } else {
                    PendingInputHit::Send
                })
            })
            .or_else(|| {
                self.attachments
                    .iter()
                    .find(|item| item.remove.contains(x, y))
                    .map(|item| PendingInputHit::Remove(item.id))
            })
            .or_else(|| self.draft.contains(x, y).then_some(PendingInputHit::Draft))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_geometry_is_bounded_by_the_composer() {
        let ids = (0..20).map(|_| AttachmentId::mint()).collect::<Vec<_>>();
        let layout = PendingInputLayout::solve(Rect::new(0.0, 0.0, 540.0, COMPOSER_HEIGHT), ids);
        assert!(layout.overflow.is_some());
        assert!(layout.attachments.len() < 20);
        assert!(
            layout
                .attachments
                .iter()
                .all(|item| item.chip.x + item.chip.w <= layout.area.x + layout.area.w)
        );
    }
}
