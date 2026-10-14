// SPDX-License-Identifier: MPL-2.0

use koru_session_protocol::{CellWidth, ColumnIndex, RowIndex, TerminalRow};

use crate::{CellPoint, CellSelection};

pub(crate) fn selected_text<'a>(
    selection: CellSelection,
    columns: u16,
    mut row_at: impl FnMut(RowIndex) -> Option<&'a TerminalRow>,
) -> Option<String> {
    let (start, end) = ordered(selection);
    let mut text = String::new();
    for row_index in start.row.0..=end.row.0 {
        let row = row_at(RowIndex(row_index))?;
        let first = if row_index == start.row.0 {
            start.column.0
        } else {
            0
        };
        let last = if row_index == end.row.0 {
            end.column.0
        } else {
            columns.saturating_sub(1)
        };
        let mut physical = String::new();
        for cell in &row.cells {
            if cell.width == CellWidth::Zero {
                continue;
            }
            let cell_last = cell
                .column
                .0
                .saturating_add(u16::from(cell.width.columns()).saturating_sub(1));
            if cell.column.0 <= last && cell_last >= first {
                physical.push_str(&cell.text);
            }
        }
        if last == columns.saturating_sub(1) {
            while physical.ends_with(' ') {
                physical.pop();
            }
        }
        text.push_str(&physical);
        if row_index != end.row.0 && !row.wrapped_to_next {
            text.push('\n');
        }
    }
    Some(text)
}

pub(crate) fn semantic_selection(row: &TerminalRow, point: CellPoint) -> CellSelection {
    let column = normalize_wide(row, point.column.0);
    let is_word = cell_text(row, column)
        .and_then(|text| text.chars().next())
        .is_some_and(word_character);
    let mut first = column;
    while first > 0
        && cell_text(row, first - 1)
            .and_then(|text| text.chars().next())
            .is_some_and(|character| word_character(character) == is_word)
    {
        first -= 1;
    }
    let mut last = column;
    while last + 1 < u16::MAX
        && cell_text(row, last + 1)
            .and_then(|text| text.chars().next())
            .is_some_and(|character| word_character(character) == is_word)
    {
        last += 1;
    }
    CellSelection {
        anchor: CellPoint {
            row: point.row,
            column: ColumnIndex(first),
        },
        focus: CellPoint {
            row: point.row,
            column: ColumnIndex(last),
        },
    }
}

pub(crate) fn line_selection(row: RowIndex, columns: u16) -> CellSelection {
    CellSelection {
        anchor: CellPoint {
            row,
            column: ColumnIndex(0),
        },
        focus: CellPoint {
            row,
            column: ColumnIndex(columns.saturating_sub(1)),
        },
    }
}

pub(crate) fn normalize_point(row: &TerminalRow, point: CellPoint) -> CellPoint {
    CellPoint {
        column: ColumnIndex(normalize_wide(row, point.column.0)),
        ..point
    }
}

fn ordered(selection: CellSelection) -> (CellPoint, CellPoint) {
    if (selection.anchor.row, selection.anchor.column)
        <= (selection.focus.row, selection.focus.column)
    {
        (selection.anchor, selection.focus)
    } else {
        (selection.focus, selection.anchor)
    }
}

fn normalize_wide(row: &TerminalRow, column: u16) -> u16 {
    let continuation = row
        .cells
        .iter()
        .find(|cell| cell.column.0 == column)
        .is_some_and(|cell| cell.width == CellWidth::Zero);
    if continuation {
        column.saturating_sub(1)
    } else {
        column
    }
}

fn cell_text(row: &TerminalRow, column: u16) -> Option<&str> {
    row.cells
        .iter()
        .find(|cell| cell.column.0 == column && cell.width != CellWidth::Zero)
        .map(|cell| cell.text.as_str())
}

fn word_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '-' | '.' | '/' | '\\' | '~')
}
