// SPDX-License-Identifier: MPL-2.0

//! **The one geometry function.** Drawing, hit-testing, the hover, the
//! keyboard, the scroll clamp and the accessibility projection all come from
//! [`ChatsSurface::layout`], so a row cannot be clickable somewhere other than
//! where it is drawn, nor announced somewhere other than where it is.
//!
//! **One clipped rectangle.** [`rows`](ChatsSurface::rows) returns only what
//! falls inside the band between `top` and the area's floor — which is both
//! the draw's cull and the hit test's bounds — and the draw clips to the same
//! band. A row scrolled half under the heading is half drawn and half
//! clickable, never drawn in one place and pressable in another.

use koru_draw::geometry::Rect;
use koru_draw::theme;

use crate::{ChatsSurface, Focus, Load};

/// A chat row: the title on one line, who answered and when on the next.
pub(crate) const CHAT_H: f32 = 40.0;
/// A sentence under a row — *Deleting…*, or why a delete was refused.
const NOTE_H: f32 = 17.0;
/// Air between *New chat* and the first chat.
const NEW_GAP: f32 = 4.0;
/// The leading lane: *New chat*'s plus, a private chat's mark, and nothing
/// for the rest — reserved on every row, so every label starts on one line.
const LEAD_W: f32 = 16.0;
/// Air under the last row, so the list does not end flush against the edge.
pub(crate) const FOOTER_PAD: f32 = 8.0;

/// Which state the list is in, when it is in one rather than showing rows.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Notice {
    Loading,
    /// Never drawn as an empty list.
    Unavailable,
    Empty,
}

/// One line of the list. Indices rather than ids because a row is a
/// *position*; what survives a refresh — the keyboard, the open chat — is
/// held by id on the surface.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Row {
    /// *New chat*, at the head of the list.
    New,
    Notice(Notice),
    Chat(usize),
    /// The line under a chat whose delete is being sent or was refused.
    Note(usize),
}

impl Row {
    /// Whether the keyboard and the pointer can choose this row.
    pub fn pressable(&self) -> bool {
        matches!(self, Self::New | Self::Chat(_))
    }

    fn height(&self) -> f32 {
        match self {
            Self::New => theme::NAV_ROW_H,
            Self::Chat(_) => CHAT_H,
            Self::Note(_) => NOTE_H,
            Self::Notice(_) => NOTE_H * 2.0,
        }
    }

    fn gap_above(&self) -> f32 {
        match self {
            Self::Chat(0) | Self::Notice(_) => NEW_GAP,
            _ => 0.0,
        }
    }
}

/// Every rect inside a row. See [`ChatsSurface::row_parts`].
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct RowParts {
    /// The leading lane. Reserved always.
    pub lead: Rect,
    /// The title — *New chat*'s label, a chat's title, a note's sentence.
    pub title: Rect,
    /// Who answered and when. Zero-height on every row but a chat.
    pub meta: Rect,
}

impl ChatsSurface {
    /// Every row, in order, with its rect — **before** the band is applied,
    /// because the scroll extent and a screen reader both need the rows that
    /// are off screen. `&self`: an accessibility projection reads this.
    pub fn layout(&self, area: Rect, top: f32) -> Vec<(Row, Rect)> {
        let plan = self.plan();
        let mut out = Vec::with_capacity(plan.len());
        let mut y = top - self.scroll;
        for row in plan {
            y += row.gap_above();
            let h = row.height();
            out.push((row, Rect::new(area.x, y, area.w.max(0.0), h)));
            y += h;
        }
        out
    }

    /// [`layout`](Self::layout), counted.
    pub(crate) fn laid_out(&mut self, area: Rect, top: f32) -> Vec<(Row, Rect)> {
        let rows = self.layout(area, top);
        self.work.rows = rows.len();
        rows
    }

    /// The rows, with nothing said about where they are.
    fn plan(&self) -> Vec<Row> {
        let mut plan = vec![Row::New];
        match self.load {
            Load::Cold | Load::Loading if self.chats.is_empty() => {
                plan.push(Row::Notice(Notice::Loading));
                return plan;
            }
            Load::Unavailable => plan.push(Row::Notice(Notice::Unavailable)),
            _ => {}
        }
        if self.chats.is_empty() {
            if self.load == Load::Ready {
                plan.push(Row::Notice(Notice::Empty));
            }
            return plan;
        }
        for (index, chat) in self.chats.iter().enumerate() {
            plan.push(Row::Chat(index));
            if self.deleting.contains_key(&chat.id) {
                plan.push(Row::Note(index));
            }
        }
        plan
    }

    /// The band rows are drawn and pressed inside: from `top` to the area's
    /// floor, less the footer's air.
    pub fn band(&self, area: Rect, top: f32) -> Rect {
        let bottom = area.y + area.h - FOOTER_PAD;
        Rect::new(area.x, top, area.w.max(0.0), (bottom - top).max(0.0))
    }

    /// The rows inside the band — the cull, and the hit test's bounds.
    pub fn rows(&mut self, area: Rect, top: f32) -> Vec<(Row, Rect)> {
        if area.w <= 0.0 {
            self.work.rows = 0;
            return Vec::new();
        }
        let band = self.band(area, top);
        self.laid_out(area, top)
            .into_iter()
            .filter(|(_, r)| r.y + r.h > band.y && r.y < band.y + band.h)
            .collect()
    }

    /// Every rect inside a row, from the row's own rect alone.
    pub fn row_parts(&self, row: Row, r: Rect) -> RowParts {
        let inner_x = r.x + theme::NAV_PAD;
        let inner_w = (r.w - theme::NAV_PAD * 2.0).max(0.0);
        let lead = Rect::new(inner_x, r.y, LEAD_W.min(inner_w), theme::NAV_ROW_H);
        let text_x = inner_x + LEAD_W + 6.0;
        let text_w = (inner_x + inner_w - text_x).max(0.0);
        match row {
            Row::Chat(_) => {
                let title_h = theme::NAV_ROW_H - 2.0;
                let title = Rect::new(text_x, r.y + 2.0, text_w, title_h);
                let meta = Rect::new(
                    text_x,
                    title.y + title.h,
                    text_w,
                    (r.h - title_h - 4.0).max(0.0),
                );
                RowParts {
                    lead: Rect::new(lead.x, title.y, lead.w, title.h),
                    title,
                    meta,
                }
            }
            _ => RowParts {
                lead: Rect::new(lead.x, r.y, lead.w, r.h),
                title: Rect::new(text_x, r.y, text_w, r.h),
                meta: Rect::new(text_x, r.y + r.h, text_w, 0.0),
            },
        }
    }

    /// The row the keyboard is on, if it is on one that exists.
    pub(crate) fn focus_row(&self) -> Option<Row> {
        match self.focus? {
            Focus::New => Some(Row::New),
            Focus::Chat(id) => self
                .chats
                .iter()
                .position(|chat| chat.id == id)
                .map(Row::Chat),
        }
    }

    /// How tall the list is at this width, unscrolled.
    fn content_h(&self, area: Rect, top: f32) -> f32 {
        self.layout(area, top)
            .last()
            .map(|(_, r)| r.y + r.h + self.scroll - top)
            .unwrap_or(0.0)
    }

    pub(crate) fn clamp_scroll(&mut self, area: Rect, top: f32) {
        let band = self.band(area, top).h;
        let max = (self.content_h(area, top) - band).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }

    pub fn scroll_by(&mut self, area: Rect, top: f32, dy: f32) {
        self.scroll -= dy;
        self.clamp_scroll(area, top);
    }

    /// Bring one row into the band, moving as little as it takes — nothing
    /// when it is already there. A focus ring below the fold is a focus a
    /// person cannot see.
    pub(crate) fn scroll_into_view(&mut self, row: Row, area: Rect, top: f32) {
        let Some((_, r)) = self
            .layout(area, top)
            .into_iter()
            .find(|(candidate, _)| *candidate == row)
        else {
            return;
        };
        let band = self.band(area, top);
        if r.y < band.y {
            self.scroll -= band.y - r.y;
        } else if r.y + r.h > band.y + band.h {
            self.scroll += r.y + r.h - (band.y + band.h);
        }
        self.clamp_scroll(area, top);
    }

    /// Which row is under a point, if any — inside the band only.
    pub fn row_at(&mut self, area: Rect, top: f32, x: f32, y: f32) -> Option<(Row, Rect)> {
        if !self.band(area, top).contains(x, y) {
            return None;
        }
        self.rows(area, top)
            .into_iter()
            .find(|(_, r)| y >= r.y && y < r.y + r.h)
    }

    /// Where *New chat* sits, unculled — for the accessibility projection.
    pub fn new_chat_rect(&self, area: Rect, top: f32) -> Option<Rect> {
        self.layout(area, top)
            .into_iter()
            .find_map(|(row, rect)| (row == Row::New).then_some(rect))
    }

    /// Every chat and where its row sits, unculled and in order — what a
    /// screen reader is told, including the rows scrolled out of the band.
    pub fn chat_rects(&self, area: Rect, top: f32) -> Vec<(crate::ChatId, Rect)> {
        self.layout(area, top)
            .into_iter()
            .filter_map(|(row, rect)| match row {
                Row::Chat(index) => self.chats.get(index).map(|chat| (chat.id, rect)),
                _ => None,
            })
            .collect()
    }
}
