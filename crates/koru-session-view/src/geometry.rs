// SPDX-License-Identifier: MPL-2.0

use koru_session_protocol::{ColumnIndex, CursorShape, RowIndex, RowRange};
use vello::kurbo::{Point, Rect};

use crate::{TerminalCursorHeight, TerminalCursorWidth};

/// **The gap between a session's content and the left and right edges of the
/// pane it is drawn in**, in logical pixels.
///
/// The raw grid had none, so column 0 touched the pane divider and a prompt
/// glyph sat flush against the hairline. Two pixels keep the glyphs off the
/// hairline while the content fills the pane (it was eight, which read as an
/// empty band on both sides) — but it is *not* a draw offset. The columns a
/// session is created and resized with are measured from the width that is
/// left (`SessionSurface::grid_size_for`), because a PTY told it has room the
/// grid does not draw in writes into pixels nobody sees, and pointer
/// hit-testing is measured from the same edge or a click lands one cell off.
///
/// **One number for all three of a session's presentations** — the raw grid,
/// the managed transcript and the composer band. They are the same pane showing
/// different things, and a reader switching between them must not see the text
/// move sideways. Vertical padding stays each presentation's own: a grid has
/// none, a transcript and a composer have four.
pub const CONTENT_INSET_X: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellPoint {
    pub row: RowIndex,
    pub column: ColumnIndex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellSelection {
    pub anchor: CellPoint,
    pub focus: CellPoint,
}

impl CellSelection {
    fn ordered(self) -> (CellPoint, CellPoint) {
        if (self.anchor.row, self.anchor.column) <= (self.focus.row, self.focus.column) {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }
}

/// The single fixed-cell coordinate system used by painting and hit testing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridGeometry {
    pub bounds: Rect,
    pub rows: RowRange,
    pub columns: u16,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl GridGeometry {
    pub fn new(
        bounds: Rect,
        rows: RowRange,
        columns: u16,
        cell_width: f32,
        cell_height: f32,
    ) -> Self {
        Self {
            bounds,
            rows,
            columns,
            cell_width,
            cell_height,
        }
    }

    pub fn cell_rect(self, point: CellPoint) -> Option<Rect> {
        let local_row = point.row.0.checked_sub(self.rows.first.0)?;
        if local_row >= u64::from(self.rows.count) || point.column.0 >= self.columns {
            return None;
        }
        let x = self.bounds.x0 + f64::from(point.column.0) * f64::from(self.cell_width);
        let y = self.bounds.y0 + local_row as f64 * f64::from(self.cell_height);
        Some(Rect::new(
            x,
            y,
            x + f64::from(self.cell_width),
            y + f64::from(self.cell_height),
        ))
    }

    pub fn hit_test(self, point: Point) -> Option<CellPoint> {
        if !self.bounds.contains(point) || self.cell_width <= 0.0 || self.cell_height <= 0.0 {
            return None;
        }
        let column = ((point.x - self.bounds.x0) / f64::from(self.cell_width)).floor() as u16;
        let local_row = ((point.y - self.bounds.y0) / f64::from(self.cell_height)).floor() as u64;
        if column >= self.columns || local_row >= u64::from(self.rows.count) {
            return None;
        }
        Some(CellPoint {
            row: RowIndex(self.rows.first.0 + local_row),
            column: ColumnIndex(column),
        })
    }

    pub fn cursor_rect(self, point: CellPoint, shape: CursorShape) -> Option<Rect> {
        let cell = self.cell_rect(point)?;
        Self::cursor_rect_in_cell(
            cell,
            shape,
            TerminalCursorWidth::Application,
            TerminalCursorHeight::Application,
        )
    }

    pub fn cursor_rect_with(
        self,
        point: CellPoint,
        shape: CursorShape,
        width: TerminalCursorWidth,
        height: TerminalCursorHeight,
    ) -> Option<Rect> {
        let cell = self.cell_rect(point)?;
        Self::cursor_rect_in_cell(cell, shape, width, height)
    }

    fn cursor_rect_in_cell(
        cell: Rect,
        shape: CursorShape,
        width: TerminalCursorWidth,
        height: TerminalCursorHeight,
    ) -> Option<Rect> {
        let authored = match shape {
            CursorShape::Block => Some(cell),
            CursorShape::Beam => Some(Rect::new(cell.x0, cell.y0, cell.x0 + 2.0, cell.y1)),
            CursorShape::Underline => Some(Rect::new(cell.x0, cell.y1 - 2.0, cell.x1, cell.y1)),
            CursorShape::Hidden => None,
        }?;
        let width = width
            .pixels()
            .map_or(authored.width(), |value| value.min(cell.width()).max(0.5));
        let height = height
            .scale()
            .map_or(authored.height(), |scale| cell.height() * scale);
        let y = cell.y0 + (cell.height() - height) * 0.5;
        Some(Rect::new(cell.x0, y, cell.x0 + width, y + height))
    }

    /// Produces at most one rectangle per visible row. A selection spanning
    /// retained history is clipped before this bounded loop begins.
    pub fn selection_rects(self, selection: CellSelection) -> Vec<Rect> {
        let (start, end) = selection.ordered();
        let visible_start = start.row.0.max(self.rows.first.0);
        let visible_end = end
            .row
            .0
            .min(self.rows.first.0 + u64::from(self.rows.count.saturating_sub(1)));
        if visible_start > visible_end {
            return Vec::new();
        }
        let mut rects = Vec::with_capacity((visible_end - visible_start + 1) as usize);
        for row in visible_start..=visible_end {
            let first = if row == start.row.0 {
                start.column.0
            } else {
                0
            };
            let last_exclusive = if row == end.row.0 {
                end.column.0.saturating_add(1)
            } else {
                self.columns
            }
            .min(self.columns);
            if first >= last_exclusive {
                continue;
            }
            let left = self.cell_rect(CellPoint {
                row: RowIndex(row),
                column: ColumnIndex(first),
            });
            let right = self.cell_rect(CellPoint {
                row: RowIndex(row),
                column: ColumnIndex(last_exclusive - 1),
            });
            if let (Some(left), Some(right)) = (left, right) {
                rects.push(Rect::new(left.x0, left.y0, right.x1, right.y1));
            }
        }
        rects
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(first: u64) -> GridGeometry {
        GridGeometry::new(
            Rect::new(10.0, 20.0, 810.0, 820.0),
            RowRange {
                first: RowIndex(first),
                count: 40,
            },
            100,
            8.0,
            20.0,
        )
    }

    #[test]
    fn every_visible_cell_round_trips_through_hit_testing() {
        let geometry = geometry(10_000);
        for row in 10_000..10_040 {
            for column in 0..100 {
                let cell = CellPoint {
                    row: RowIndex(row),
                    column: ColumnIndex(column),
                };
                let rect = geometry.cell_rect(cell).unwrap();
                assert_eq!(geometry.hit_test(rect.center()), Some(cell));
            }
        }
    }

    #[test]
    fn huge_selection_visits_only_visible_rows() {
        let rects = geometry(50_000).selection_rects(CellSelection {
            anchor: CellPoint {
                row: RowIndex(0),
                column: ColumnIndex(2),
            },
            focus: CellPoint {
                row: RowIndex(100_000),
                column: ColumnIndex(7),
            },
        });
        assert_eq!(rects.len(), 40);
    }

    #[test]
    fn cursor_shapes_share_cell_geometry() {
        let geometry = geometry(0);
        let point = CellPoint {
            row: RowIndex(1),
            column: ColumnIndex(2),
        };
        let cell = geometry.cell_rect(point).unwrap();
        assert_eq!(geometry.cursor_rect(point, CursorShape::Block), Some(cell));
        assert_eq!(geometry.cursor_rect(point, CursorShape::Hidden), None);
        assert_eq!(
            geometry
                .cursor_rect(point, CursorShape::Beam)
                .unwrap()
                .width(),
            2.0
        );
        assert_eq!(
            geometry
                .cursor_rect(point, CursorShape::Underline)
                .unwrap()
                .height(),
            2.0
        );
    }

    #[test]
    fn cursor_overrides_preserve_hidden_and_fit_inside_the_cell() {
        let geometry = geometry(0);
        let point = CellPoint {
            row: RowIndex(0),
            column: ColumnIndex(0),
        };
        let cursor = geometry
            .cursor_rect_with(
                point,
                CursorShape::Block,
                TerminalCursorWidth::Thin,
                TerminalCursorHeight::Short,
            )
            .expect("visible cursor");
        assert!(cursor.width() < 8.0);
        assert!(cursor.height() < 20.0);
        assert!(cursor.y0 > geometry.bounds.y0);
        assert!(
            geometry
                .cursor_rect_with(
                    point,
                    CursorShape::Hidden,
                    TerminalCursorWidth::X8,
                    TerminalCursorHeight::Tall,
                )
                .is_none()
        );
    }
}
