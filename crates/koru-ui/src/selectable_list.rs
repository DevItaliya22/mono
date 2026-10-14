// SPDX-License-Identifier: MPL-2.0

//! Geometry and selection for a keyboard/mouse driven list.

use koru_draw::geometry::Rect;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListScrollbar {
    pub track_top: f32,
    pub track_height: f32,
    pub thumb_top: f32,
    pub thumb_height: f32,
    pub max_scroll: f32,
}

impl ListScrollbar {
    pub fn thumb_rect(self, x: f32, width: f32) -> Rect {
        Rect::new(x, self.thumb_top, width, self.thumb_height)
    }

    pub fn scroll_for_pointer(self, pointer_y: f32, grab_offset: f32) -> f32 {
        let travel = (self.track_height - self.thumb_height).max(0.0);
        if travel == 0.0 {
            return 0.0;
        }
        let thumb_top = (pointer_y - grab_offset).clamp(self.track_top, self.track_top + travel);
        ((thumb_top - self.track_top) / travel) * self.max_scroll
    }
}

#[derive(Clone, Debug)]
pub struct SelectableList {
    pub selected: usize,
    pub scroll: f32,
    pub row_height: f32,
}

impl SelectableList {
    pub fn new(row_height: f32) -> Self {
        Self {
            selected: 0,
            scroll: 0.0,
            row_height,
        }
    }

    pub fn step(&mut self, delta: isize, len: usize, viewport_height: f32) -> bool {
        if len == 0 {
            self.selected = 0;
            self.scroll = 0.0;
            return false;
        }
        let next = (self.selected as isize + delta).clamp(0, len as isize - 1) as usize;
        let changed = next != self.selected;
        self.selected = next;
        self.ensure_visible(len, viewport_height);
        changed
    }

    pub fn row_at(&self, top: f32, y: f32, len: usize) -> Option<usize> {
        if y < top {
            return None;
        }
        let row = ((y - top + self.scroll) / self.row_height).floor() as usize;
        (row < len).then_some(row)
    }

    pub fn row_y(&self, top: f32, row: usize) -> f32 {
        top + row as f32 * self.row_height - self.scroll
    }

    pub fn ensure_visible(&mut self, len: usize, viewport_height: f32) {
        let top = self.selected as f32 * self.row_height;
        let bottom = top + self.row_height;
        if top < self.scroll {
            self.scroll = top;
        } else if bottom > self.scroll + viewport_height {
            self.scroll = (bottom - viewport_height).max(0.0);
        }
        self.clamp_scroll(len, viewport_height);
    }

    pub fn max_scroll(&self, len: usize, viewport_height: f32) -> f32 {
        self.max_scroll_for_content(len as f32 * self.row_height, viewport_height)
    }

    /// The scroll range for mixed-height content that still uses this list's
    /// selection and pixel offset. Landing search is three result rows followed
    /// by a heading and compact recent-query rows, so a row count cannot express
    /// its real extent.
    pub fn max_scroll_for_content(&self, content_height: f32, viewport_height: f32) -> f32 {
        (content_height - viewport_height).max(0.0)
    }

    pub fn clamp_scroll(&mut self, len: usize, viewport_height: f32) {
        self.clamp_scroll_for_content(len as f32 * self.row_height, viewport_height);
    }

    pub fn clamp_scroll_for_content(&mut self, content_height: f32, viewport_height: f32) {
        self.scroll = self.scroll.clamp(
            0.0,
            self.max_scroll_for_content(content_height, viewport_height),
        );
    }

    pub fn scroll_by(&mut self, amount: f32, len: usize, viewport_height: f32) {
        self.scroll_by_content(amount, len as f32 * self.row_height, viewport_height);
    }

    pub fn scroll_by_content(&mut self, amount: f32, content_height: f32, viewport_height: f32) {
        self.scroll += amount;
        self.clamp_scroll_for_content(content_height, viewport_height);
    }

    pub fn scrollbar(
        &self,
        len: usize,
        viewport_height: f32,
        track_top: f32,
        min_thumb: f32,
    ) -> Option<ListScrollbar> {
        self.scrollbar_for_content(
            len as f32 * self.row_height,
            viewport_height,
            track_top,
            min_thumb,
        )
    }

    pub fn scrollbar_for_content(
        &self,
        content_height: f32,
        viewport_height: f32,
        track_top: f32,
        min_thumb: f32,
    ) -> Option<ListScrollbar> {
        let max_scroll = self.max_scroll_for_content(content_height, viewport_height);
        if max_scroll <= 0.0 || viewport_height <= 0.0 {
            return None;
        }
        let thumb_height = (viewport_height * viewport_height / content_height)
            .clamp(min_thumb.min(viewport_height), viewport_height);
        let travel = viewport_height - thumb_height;
        let thumb_top = track_top + travel * (self.scroll / max_scroll).clamp(0.0, 1.0);
        Some(ListScrollbar {
            track_top,
            track_height: viewport_height,
            thumb_top,
            thumb_height,
            max_scroll,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_stays_visible_and_hit_test_shares_scroll_math() {
        let mut list = SelectableList::new(20.0);
        list.step(8, 10, 60.0);
        assert_eq!(list.selected, 8);
        let y = list.row_y(40.0, 8) + 2.0;
        assert_eq!(list.row_at(40.0, y, 10), Some(8));
    }

    #[test]
    fn wheel_scroll_is_clamped_without_dragging_selection_back_into_view() {
        let mut list = SelectableList::new(20.0);
        list.scroll_by(80.0, 10, 60.0);
        assert_eq!(list.scroll, 80.0);
        list.scroll_by(1_000.0, 10, 60.0);
        assert_eq!(list.scroll, 140.0);
    }

    #[test]
    fn scrollbar_thumb_maps_between_scroll_and_track() {
        let mut list = SelectableList::new(20.0);
        list.scroll = 70.0;
        let bar = list.scrollbar(10, 60.0, 30.0, 20.0).unwrap();
        assert_eq!(bar.thumb_height, 20.0);
        assert_eq!(bar.thumb_top, 50.0);
        assert_eq!(bar.scroll_for_pointer(70.0, 20.0), 70.0);
    }

    #[test]
    fn mixed_height_content_uses_its_real_pixel_extent() {
        let mut list = SelectableList::new(70.0);
        list.scroll_by_content(1_000.0, 414.0, 300.0);
        assert_eq!(list.scroll, 114.0);

        let bar = list
            .scrollbar_for_content(414.0, 300.0, 80.0, 24.0)
            .unwrap();
        assert_eq!(bar.max_scroll, 114.0);
        assert!(bar.thumb_height > 200.0);
    }
}
