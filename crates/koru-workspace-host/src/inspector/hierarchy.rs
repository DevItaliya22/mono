// SPDX-License-Identifier: MPL-2.0

//! Complete captured hierarchy; never inferred widget layers.
use super::*;

const TREE_INDENT: f32 = 14.0;
const DISCLOSURE_W: f32 = 16.0;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct HierarchyRow {
    pub index: usize,
    pub depth: usize,
    pub has_children: bool,
    pub collapsed: bool,
}

impl Inspector {
    /// Returns whether the node was opened, so the host can reveal its window.
    pub(super) fn click_hierarchy_row(&mut self, index: usize, now: std::time::Instant) -> bool {
        let double = self
            .hierarchy_click
            .is_some_and(|(previous, revision, then, pointer)| {
                previous == index
                    && revision == self.snapshot.revision
                    && now.saturating_duration_since(then) < crate::app::DOUBLE_CLICK
                    && (pointer.0 - self.pointer.0).abs() <= 4.0
                    && (pointer.1 - self.pointer.1).abs() <= 4.0
            });
        self.pause_refresh();
        self.select(Some(index));
        self.focus_index = 4;
        if double {
            self.hierarchy_click = None;
            self.activate(7);
        } else {
            self.hierarchy_click = Some((index, self.snapshot.revision, now, self.pointer));
        }
        double
    }
    /// Both app picking and tree navigation keep the same selected row visible.
    /// Preserve the scroll position when that row is already in view.
    pub(super) fn reveal_selection(&mut self) {
        let Some(selected) = self.selected else {
            return;
        };
        // A pick in the app must remain discoverable even if an ancestor was
        // collapsed during an earlier review of this frozen capture.
        let mut parent = self.snapshot.records.get(selected).and_then(|r| r.parent);
        while let Some(index) = parent {
            self.collapsed_hierarchy.remove(&index);
            parent = self.snapshot.records.get(index).and_then(|r| r.parent);
        }
        let rows = self.hierarchy_rows();
        let Some(selected_row) = rows.iter().position(|row| row.index == selected) else {
            return;
        };
        let visible = (self.control_rect(4).h / ROW).floor().max(1.0) as usize;
        if selected_row < self.hierarchy_scroll {
            self.hierarchy_scroll = selected_row;
        } else if selected_row >= self.hierarchy_scroll + visible {
            self.hierarchy_scroll = selected_row + 1 - visible;
        }
        let limit = rows.len().saturating_sub(visible);
        self.hierarchy_scroll = self.hierarchy_scroll.min(limit);
    }

    pub(super) fn focus_order(&self) -> Vec<usize> {
        let mut order = vec![PICK, COPY, RAYS, PIN, CLOSE, 8, 7];
        if self.show_hierarchy {
            order.push(REFRESH);
        }
        order.push(if self.show_hierarchy { 4 } else { 5 });
        if !self.detached {
            order.push(6);
        }
        order
    }

    pub(super) fn hierarchy_rows(&self) -> Vec<HierarchyRow> {
        // Collector order is preorder: each parent precedes its children.
        let count = self.snapshot.records.len();
        let mut depths = vec![0; count];
        let mut hidden = vec![false; count];
        let mut has_children = vec![false; count];
        for record in &self.snapshot.records {
            if let Some(parent) = record.parent.filter(|parent| *parent < count) {
                has_children[parent] = true;
            }
        }
        let mut rows = Vec::with_capacity(count);
        for (index, record) in self.snapshot.records.iter().enumerate() {
            if let Some(parent) = record.parent.filter(|parent| *parent < index) {
                depths[index] = depths[parent] + 1;
                hidden[index] = hidden[parent] || self.collapsed_hierarchy.contains(&parent);
            }
            if !hidden[index] {
                rows.push(HierarchyRow {
                    index,
                    depth: depths[index],
                    has_children: has_children[index],
                    collapsed: self.collapsed_hierarchy.contains(&index),
                });
            }
        }
        rows
    }

    pub(super) fn toggle_hierarchy_row(&mut self, index: usize) {
        if !self.collapsed_hierarchy.remove(&index) {
            self.collapsed_hierarchy.insert(index);
        }
        self.clamp_scroll();
        self.dirty_semantics = true;
    }

    pub(super) fn hierarchy_disclosure_contains(&self, depth: usize, x: f32) -> bool {
        let left = self.bounds.x + PAD + depth.min(24) as f32 * TREE_INDENT;
        (left..left + DISCLOSURE_W).contains(&x)
    }

    pub(super) fn hierarchy_description(&self) -> String {
        self.hierarchy_rows()
            .into_iter()
            .map(|row| {
                let record = &self.snapshot.records[row.index];
                format!(
                    "level {}: {} ({}){}{}",
                    row.depth,
                    display_name(record),
                    record.kind,
                    if row.has_children {
                        if row.collapsed {
                            " — collapsed"
                        } else {
                            " — expanded"
                        }
                    } else {
                        ""
                    },
                    if self.selected == Some(row.index) {
                        " — selected"
                    } else {
                        ""
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(super) fn draw_hierarchy(&mut self, canvas: &mut Scene, root: Affine) {
        let b = self.bounds;
        let rect = self.control_rect(REFRESH);
        let lang = crate::accessibility::ui_lang();
        let label = t(
            lang,
            if self.refresh_paused {
                CopyKey::InspectorResumeRefresh
            } else {
                CopyKey::InspectorPauseRefresh
            },
        );
        koru_ui::button::ActionButton::new(label, koru_ui::button::ActionStyle::Ghost, None)
            .draw_focused(
                canvas,
                root,
                self.engine.get_or_insert_with(TextEngine::new),
                rect,
                rect.contains(self.pointer.0, self.pointer.1),
                self.focus && self.focus_index == REFRESH,
            );
        let rows = self.hierarchy_rows();
        for (position, &row) in rows.iter().enumerate().skip(self.hierarchy_scroll) {
            let y = b.y + TREE_TOP + (position - self.hierarchy_scroll) as f32 * ROW;
            if y + ROW > b.y + b.h - PAD {
                break;
            }
            let record = &self.snapshot.records[row.index];
            let label = format!("{} · {}", display_name(record), record.kind);
            if self.selected == Some(row.index) {
                scene::fill(
                    canvas,
                    root,
                    theme::p().hover_tint,
                    &scene::rect(b.x + PAD, y, b.w - 2.0 * PAD, ROW),
                );
            }
            let disclosure_x = b.x + PAD + row.depth.min(24) as f32 * TREE_INDENT;
            if row.has_children {
                let glyph = if row.collapsed {
                    koru_draw::icons::CARET_RIGHT
                } else {
                    koru_draw::icons::CARET_DOWN
                };
                let icon = self
                    .engine
                    .get_or_insert_with(TextEngine::new)
                    .icon_layout(glyph, 12.0);
                scene::draw_layout_first_line(
                    canvas,
                    root,
                    &icon,
                    disclosure_x,
                    y + (ROW - icon.height()) * 0.5,
                    Some(theme::p().text_dim),
                );
            }
            self.text(
                canvas,
                root,
                &label,
                disclosure_x + DISCLOSURE_W,
                y,
                (b.x + b.w - PAD - theme::SCROLLBAR_GUTTER_W) - (disclosure_x + DISCLOSURE_W),
            );
        }
    }
}
