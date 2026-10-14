// SPDX-License-Identifier: MPL-2.0

use std::{env, fs, path::PathBuf};

use koru_draw::text::TextEngine;
use koru_session_protocol::{
    AttentionState, CellStyle, CellWidth, ColumnIndex, CursorShape, CursorState, GridSize,
    RenderSnapshot, RenderUpdate, RowIndex, RowRange, SessionId, StreamSequence, TerminalCell,
    TerminalGeneration, TerminalModes, TerminalRow, ViewportGeneration,
};
use koru_session_view::TerminalView;
use vello::{
    Scene,
    kurbo::{Affine, Rect},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let mut root: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        if arg == "--root" {
            root = args.next().map(PathBuf::from);
        }
    }
    let root = root.ok_or("usage: render-page --root <isolated-directory>")?;
    fs::create_dir_all(&root)?;

    let row = TerminalRow {
        index: RowIndex(0),
        cells: ["plain", "-> != ===", "界", "e\u{301}", "\u{e0b0}"]
            .into_iter()
            .enumerate()
            .map(|(column, text)| TerminalCell {
                column: ColumnIndex(column as u16),
                text: text.into(),
                width: CellWidth::One,
                style: CellStyle::default(),
            })
            .collect(),
        wrapped_to_next: false,
    };
    let snapshot = RenderSnapshot {
        session_id: SessionId::mint(),
        terminal_generation: TerminalGeneration::new(1).expect("one is positive"),
        viewport_generation: ViewportGeneration::new(1).expect("one is positive"),
        sequence: StreamSequence::new(1).expect("one is positive"),
        viewport: RowRange {
            first: RowIndex(0),
            count: 1,
        },
        size: GridSize::new(80, 24).expect("fixture grid is non-empty"),
        rows: vec![row],
        cursor: CursorState {
            row: RowIndex(0),
            column: ColumnIndex(0),
            shape: CursorShape::Block,
            blinking: false,
        },
        modes: TerminalModes::default(),
        attention: AttentionState::default(),
    };
    let mut view = TerminalView::new(());
    view.set_visible_rows(RowRange {
        first: RowIndex(0),
        count: 1,
    });
    view.apply(RenderUpdate::Snapshot { snapshot })
        .map_err(|error| error.to_string())?;
    let mut engine = TextEngine::new();
    let mut scene = Scene::new();
    view.draw(
        &mut engine,
        &mut scene,
        Affine::IDENTITY,
        Rect::new(0.0, 0.0, 800.0, 400.0),
        1.0,
    );
    let json = serde_json::to_vec_pretty(view.paint_records())?;
    fs::write(root.join("terminal-render-page.json"), json)?;
    Ok(())
}
