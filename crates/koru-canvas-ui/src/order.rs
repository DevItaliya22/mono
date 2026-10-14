// SPDX-License-Identifier: MPL-2.0

//! **Reading order** — the order Tab walks and a screen reader reads.
//!
//! Over stored positions only, because the other readers of a canvas — export,
//! AI context, MCP, the phone — have no text layout and so no heights, and a
//! canvas has to read the same everywhere (CANVAS §5, *Reading order*):
//!
//! 1. objects form **rows**: one joins the current row when its `y` is within
//!    [`ROW_BAND`] of the row's *first* object;
//! 2. rows read top to bottom, and within a row objects read left to right.
//!
//! Arrivals have no position; the caller puts them last, in the order they
//! arrived. The canvas is never mirrored, so neither is this.

#[cfg(test)]
mod tests;

/// How far below a row's first object another may start and still be in that
/// row, in canvas pixels.
pub const ROW_BAND: i32 = 40;

/// The indices of `positions` in reading order.
pub fn reading_order(positions: &[(i32, i32)]) -> Vec<usize> {
    let mut by_top: Vec<usize> = (0..positions.len()).collect();
    by_top.sort_by_key(|&i| (positions[i].1, positions[i].0, i));
    let mut out = Vec::with_capacity(positions.len());
    let mut row: Vec<usize> = Vec::new();
    let mut row_top = 0i64;
    for i in by_top {
        let y = i64::from(positions[i].1);
        if !row.is_empty() && y - row_top > i64::from(ROW_BAND) {
            flush(&mut row, &mut out, positions);
        }
        if row.is_empty() {
            row_top = y;
        }
        row.push(i);
    }
    flush(&mut row, &mut out, positions);
    out
}

fn flush(row: &mut Vec<usize>, out: &mut Vec<usize>, positions: &[(i32, i32)]) {
    row.sort_by_key(|&i| (positions[i].0, positions[i].1, i));
    out.append(row);
}
