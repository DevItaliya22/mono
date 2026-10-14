// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use koru_session_protocol::{
    AttentionState, CursorState, ErrorCode, GridSize, RenderDelta, RenderSnapshot, RenderUpdate,
    RowIndex, RowRange, SessionId, StreamSequence, TerminalGeneration, TerminalModes, TerminalRow,
    ViewportGeneration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    Snapshot { generation_changed: bool },
    Delta { damaged_rows: Vec<RowIndex> },
    RecoveryRequested,
}

#[derive(Clone, Debug)]
pub struct TerminalModel {
    pub(crate) session_id: SessionId,
    pub(crate) terminal_generation: TerminalGeneration,
    pub(crate) viewport_generation: ViewportGeneration,
    pub(crate) sequence: StreamSequence,
    pub(crate) viewport: RowRange,
    pub(crate) size: GridSize,
    pub(crate) rows: BTreeMap<RowIndex, TerminalRow>,
    pub(crate) cursor: CursorState,
    pub(crate) modes: TerminalModes,
    pub(crate) attention: AttentionState,
}

impl TerminalModel {
    pub fn from_snapshot(snapshot: RenderSnapshot) -> Result<Self, ErrorCode> {
        validate_snapshot(&snapshot)?;
        Ok(Self {
            session_id: snapshot.session_id,
            terminal_generation: snapshot.terminal_generation,
            viewport_generation: snapshot.viewport_generation,
            sequence: snapshot.sequence,
            viewport: snapshot.viewport,
            size: snapshot.size,
            rows: snapshot
                .rows
                .into_iter()
                .map(|row| (row.index, row))
                .collect(),
            cursor: snapshot.cursor,
            modes: snapshot.modes,
            attention: snapshot.attention,
        })
    }

    pub fn viewport(&self) -> RowRange {
        self.viewport
    }
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }
    pub fn terminal_generation(&self) -> TerminalGeneration {
        self.terminal_generation
    }
    pub fn viewport_generation(&self) -> ViewportGeneration {
        self.viewport_generation
    }
    pub fn sequence(&self) -> StreamSequence {
        self.sequence
    }
    pub fn size(&self) -> GridSize {
        self.size
    }
    pub fn cursor(&self) -> CursorState {
        self.cursor
    }
    pub fn modes(&self) -> TerminalModes {
        self.modes
    }

    pub(crate) fn apply_delta(&mut self, delta: RenderDelta) -> Result<Vec<RowIndex>, ErrorCode> {
        if delta.session_id != self.session_id
            || delta.terminal_generation != self.terminal_generation
            || delta.viewport_generation != self.viewport_generation
        {
            return Err(ErrorCode::StaleGeneration);
        }
        if self.sequence.next() != Some(delta.sequence) {
            return Err(ErrorCode::StreamGap);
        }
        let end = self.viewport.first.0 + u64::from(self.viewport.count);
        for row in &delta.rows {
            validate_row(row, self.size.columns)?;
            if row.index.0 < self.viewport.first.0 || row.index.0 >= end {
                return Err(ErrorCode::InvalidRequest);
            }
        }
        let damaged = delta.rows.iter().map(|row| row.index).collect();
        for row in delta.rows {
            self.rows.insert(row.index, row);
        }
        if let Some(cursor) = delta.cursor {
            self.cursor = cursor;
        }
        if let Some(modes) = delta.modes {
            self.modes = modes;
        }
        if let Some(attention) = delta.attention {
            self.attention = attention;
        }
        self.sequence = delta.sequence;
        Ok(damaged)
    }

    pub fn apply(&mut self, update: RenderUpdate) -> Result<ApplyOutcome, ErrorCode> {
        match update {
            RenderUpdate::Snapshot { snapshot } => {
                let replacement = Self::from_snapshot(snapshot)?;
                let generation_changed = replacement.session_id != self.session_id
                    || replacement.terminal_generation != self.terminal_generation;
                *self = replacement;
                Ok(ApplyOutcome::Snapshot { generation_changed })
            }
            RenderUpdate::Delta { delta } => self
                .apply_delta(delta)
                .map(|damaged_rows| ApplyOutcome::Delta { damaged_rows }),
            RenderUpdate::Gap { .. } => Ok(ApplyOutcome::RecoveryRequested),
        }
    }
}

fn validate_snapshot(snapshot: &RenderSnapshot) -> Result<(), ErrorCode> {
    snapshot.validate()?;
    let end = snapshot.viewport.first.0 + u64::from(snapshot.viewport.count);
    let mut previous = None;
    for row in &snapshot.rows {
        validate_row(row, snapshot.size.columns)?;
        if row.index.0 < snapshot.viewport.first.0 || row.index.0 >= end {
            return Err(ErrorCode::InvalidRequest);
        }
        if previous.is_some_and(|index| row.index <= index) {
            return Err(ErrorCode::InvalidRequest);
        }
        previous = Some(row.index);
    }
    Ok(())
}

fn validate_row(row: &TerminalRow, columns: u16) -> Result<(), ErrorCode> {
    let mut previous = None;
    for cell in &row.cells {
        if cell.column.0 >= columns || previous.is_some_and(|column| cell.column.0 <= column) {
            return Err(ErrorCode::InvalidRequest);
        }
        let end = u32::from(cell.column.0) + u32::from(cell.width.columns());
        if end > u32::from(columns) {
            return Err(ErrorCode::InvalidRequest);
        }
        previous = Some(cell.column.0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use koru_session_protocol::{CellStyle, CellWidth, ColumnIndex, CursorShape, TerminalCell};

    fn snapshot() -> RenderSnapshot {
        RenderSnapshot {
            session_id: SessionId::mint(),
            terminal_generation: TerminalGeneration::new(1).unwrap(),
            viewport_generation: ViewportGeneration::new(1).unwrap(),
            sequence: StreamSequence::new(1).unwrap(),
            viewport: RowRange {
                first: RowIndex(10),
                count: 2,
            },
            size: GridSize::new(4, 2).unwrap(),
            rows: vec![TerminalRow {
                index: RowIndex(10),
                cells: vec![TerminalCell {
                    column: ColumnIndex(0),
                    text: "a".into(),
                    width: CellWidth::One,
                    style: CellStyle::default(),
                }],
                wrapped_to_next: false,
            }],
            cursor: CursorState {
                row: RowIndex(10),
                column: ColumnIndex(0),
                shape: CursorShape::Block,
                blinking: false,
            },
            modes: TerminalModes::default(),
            attention: AttentionState::default(),
        }
    }

    #[test]
    fn validates_against_current_sequence_not_original_snapshot() {
        let initial = snapshot();
        let mut model = TerminalModel::from_snapshot(initial.clone()).unwrap();
        for sequence in 2..=3 {
            model
                .apply_delta(RenderDelta {
                    session_id: initial.session_id,
                    terminal_generation: initial.terminal_generation,
                    viewport_generation: initial.viewport_generation,
                    sequence: StreamSequence::new(sequence).unwrap(),
                    rows: Vec::new(),
                    cursor: None,
                    modes: None,
                    attention: None,
                })
                .unwrap();
        }
        assert_eq!(model.sequence.get(), 3);
    }

    #[test]
    fn rejects_unsorted_or_out_of_grid_cells() {
        let mut invalid = snapshot();
        invalid.rows[0].cells.push(TerminalCell {
            column: ColumnIndex(0),
            text: "b".into(),
            width: CellWidth::One,
            style: CellStyle::default(),
        });
        assert_eq!(
            TerminalModel::from_snapshot(invalid).unwrap_err(),
            ErrorCode::InvalidRequest
        );
    }
}
