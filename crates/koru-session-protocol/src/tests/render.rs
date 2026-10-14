// SPDX-License-Identifier: MPL-2.0

use crate::*;

fn row(index: u64, text: &str) -> TerminalRow {
    TerminalRow {
        index: RowIndex(index),
        cells: vec![TerminalCell {
            column: ColumnIndex(0),
            text: text.into(),
            width: CellWidth::One,
            style: CellStyle::default(),
        }],
        wrapped_to_next: false,
    }
}

fn snapshot() -> RenderSnapshot {
    RenderSnapshot {
        session_id: super::session_id(),
        terminal_generation: super::terminal_generation(1),
        viewport_generation: super::viewport_generation(2),
        sequence: super::seq(3),
        viewport: RowRange {
            first: RowIndex(100),
            count: 2,
        },
        size: GridSize::new(120, 40).unwrap(),
        rows: vec![row(100, "a"), row(101, "b")],
        cursor: CursorState {
            row: RowIndex(101),
            column: ColumnIndex(1),
            shape: CursorShape::Block,
            blinking: true,
        },
        modes: TerminalModes::default(),
        attention: AttentionState::default(),
    }
}

#[test]
fn snapshot_and_pages_are_bounded_by_their_declared_rows() {
    let mut snapshot = snapshot();
    assert_eq!(snapshot.validate(), Ok(()));
    snapshot.rows.push(row(102, "off viewport"));
    assert_eq!(snapshot.validate(), Err(ErrorCode::InvalidSize));

    let page = ScrollbackPage {
        session_id: super::session_id(),
        terminal_generation: super::terminal_generation(1),
        viewport_generation: super::viewport_generation(2),
        sequence: super::seq(3),
        range: RowRange {
            first: RowIndex(80),
            count: 2,
        },
        rows: vec![row(80, "a"), row(81, "b")],
        has_earlier: true,
        has_later: true,
    };
    assert_eq!(page.validate(), Ok(()));
}

#[test]
fn delta_requires_exact_generation_sequence_and_visible_damage() {
    let snapshot = snapshot();
    let mut delta = RenderDelta {
        session_id: snapshot.session_id,
        terminal_generation: snapshot.terminal_generation,
        viewport_generation: snapshot.viewport_generation,
        sequence: super::seq(4),
        rows: vec![row(101, "changed")],
        cursor: None,
        modes: None,
        attention: None,
    };
    assert_eq!(delta.follows(&snapshot), Ok(()));
    delta.sequence = super::seq(5);
    assert_eq!(delta.follows(&snapshot), Err(ErrorCode::StreamGap));
    delta.sequence = super::seq(4);
    delta.viewport_generation = super::viewport_generation(3);
    assert_eq!(delta.follows(&snapshot), Err(ErrorCode::StaleGeneration));
    delta.viewport_generation = snapshot.viewport_generation;
    delta.rows = vec![row(99, "off viewport")];
    assert_eq!(delta.follows(&snapshot), Err(ErrorCode::InvalidRequest));
}

#[test]
fn empty_damage_can_carry_cursor_or_mode_state() {
    let snapshot = snapshot();
    let delta = RenderDelta {
        session_id: snapshot.session_id,
        terminal_generation: snapshot.terminal_generation,
        viewport_generation: snapshot.viewport_generation,
        sequence: super::seq(4),
        rows: Vec::new(),
        cursor: Some(snapshot.cursor),
        modes: Some(TerminalModes {
            bracketed_paste: true,
            ..TerminalModes::default()
        }),
        attention: None,
    };
    assert_eq!(delta.follows(&snapshot), Ok(()));
}

#[test]
fn cell_width_is_explicit_for_wide_and_continuation_cells() {
    assert_eq!(CellWidth::Zero.columns(), 0);
    assert_eq!(CellWidth::One.columns(), 1);
    assert_eq!(CellWidth::Two.columns(), 2);
}
