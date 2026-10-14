// SPDX-License-Identifier: MPL-2.0

use std::sync::Mutex;

use koru_draw::{
    text::TextEngine,
    theme::{self, Density},
};
use koru_session_protocol::{
    AttentionState, CellStyle, CellWidth, ColumnIndex, CursorShape, CursorState, GridSize,
    RenderSnapshot, RenderUpdate, RowIndex, RowRange, SessionId, StreamSequence, TerminalCell,
    TerminalColor, TerminalGeneration, TerminalModes, TerminalRow, ViewportGeneration,
};
use koru_session_view::{CellPoint, CellSelection, TerminalView};
use serde_json::{Value, json};
use vello::{
    Scene,
    kurbo::{Affine, Rect},
};

static THEME_LOCK: Mutex<()> = Mutex::new(());

fn styled_cell(column: u16, text: &str, width: CellWidth, style: CellStyle) -> TerminalCell {
    TerminalCell {
        column: ColumnIndex(column),
        text: text.into(),
        width,
        style,
    }
}

fn snapshot() -> RenderSnapshot {
    let default = CellStyle::default();
    let row = TerminalRow {
        index: RowIndex(0),
        cells: vec![
            styled_cell(0, "plain", CellWidth::One, default),
            styled_cell(
                1,
                "-> != ===",
                CellWidth::One,
                CellStyle {
                    bold: true,
                    ..default
                },
            ),
            styled_cell(
                2,
                "e\u{301}",
                CellWidth::One,
                CellStyle {
                    italic: true,
                    ..default
                },
            ),
            styled_cell(
                3,
                "界",
                CellWidth::Two,
                CellStyle {
                    underline: true,
                    ..default
                },
            ),
            styled_cell(4, "", CellWidth::Zero, default),
            styled_cell(
                5,
                "─",
                CellWidth::One,
                CellStyle {
                    strikeout: true,
                    ..default
                },
            ),
            styled_cell(
                6,
                "한",
                CellWidth::One,
                CellStyle {
                    foreground: TerminalColor::Indexed { index: 196 },
                    dim: true,
                    inverse: true,
                    ..default
                },
            ),
            styled_cell(
                7,
                "\u{e0b0}",
                CellWidth::One,
                CellStyle {
                    foreground: TerminalColor::Rgb {
                        red: 12,
                        green: 34,
                        blue: 56,
                    },
                    ..default
                },
            ),
        ],
        wrapped_to_next: false,
    };
    RenderSnapshot {
        session_id: SessionId::mint(),
        terminal_generation: TerminalGeneration::new(1).unwrap(),
        viewport_generation: ViewportGeneration::new(1).unwrap(),
        sequence: StreamSequence::new(1).unwrap(),
        viewport: RowRange {
            first: RowIndex(0),
            count: 1,
        },
        size: GridSize::new(12, 1).unwrap(),
        rows: vec![row],
        cursor: CursorState {
            row: RowIndex(0),
            column: ColumnIndex(9),
            shape: CursorShape::Underline,
            blinking: false,
        },
        modes: TerminalModes {
            alternate_screen: true,
            ..TerminalModes::default()
        },
        attention: AttentionState::default(),
    }
}

fn render(scale: f32) -> (koru_session_view::RenderWork, Vec<Value>) {
    let mut view = TerminalView::new(());
    view.set_visible_rows(RowRange {
        first: RowIndex(0),
        count: 1,
    });
    view.set_selection(Some(CellSelection {
        anchor: CellPoint {
            row: RowIndex(0),
            column: ColumnIndex(2),
        },
        focus: CellPoint {
            row: RowIndex(0),
            column: ColumnIndex(4),
        },
    }));
    view.apply(RenderUpdate::Snapshot {
        snapshot: snapshot(),
    })
    .unwrap();
    let mut engine = TextEngine::new();
    let mut scene = Scene::new();
    let work = view.draw(
        &mut engine,
        &mut scene,
        Affine::IDENTITY,
        Rect::new(0.0, 0.0, 800.0, 40.0),
        scale,
    );
    let records = serde_json::to_value(view.paint_records())
        .unwrap()
        .as_array()
        .unwrap()
        .clone();
    (work, records)
}

fn compact(records: Vec<Value>) -> Vec<String> {
    fn color(value: &Value) -> String {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|part| part.as_u64().unwrap().to_string())
            .collect::<Vec<_>>()
            .join(",")
    }
    records
        .into_iter()
        .map(|record| {
            let layer = record["layer"].as_str().unwrap();
            let row = record["row"].as_u64().unwrap();
            match layer {
                "background" => format!(
                    "bg:{row}:{}:{}:{}",
                    record["first_column"],
                    record["columns"],
                    color(&record["color"])
                ),
                "text" => format!(
                    "text:{row}:{}:{}:{}:{}:{}",
                    record["first_column"],
                    record["columns"],
                    record["face"].as_str().unwrap(),
                    color(&record["foreground"]),
                    record["text"].as_str().unwrap()
                ),
                "selection" => format!(
                    "selection:{row}:{}:{}",
                    record["first_column"], record["columns"]
                ),
                "cursor" => format!(
                    "cursor:{row}:{}:{}",
                    record["column"],
                    record["shape"].as_str().unwrap()
                ),
                _ => unreachable!(),
            }
        })
        .collect()
}

#[test]
fn semantic_render_page_matches_reviewed_fixture() {
    let _guard = THEME_LOCK.lock().unwrap();
    let original_dark = theme::is_dark();
    let original_density = theme::density();
    let original_zoom = theme::zoom();
    theme::set_zoom(1.0);
    let mut matrix = Vec::new();
    let mut pages = Vec::new();
    for (theme_name, dark) in [("light", false), ("dark", true)] {
        theme::set_dark(dark);
        for (density_name, density) in [
            ("compact", Density::Compact),
            ("normal", Density::Normal),
            ("comfortable", Density::Comfortable),
        ] {
            theme::set_density(density);
            for scale in [1.0_f32, 1.25, 2.0] {
                let (work, records) = render(scale);
                matrix.push(json!(format!(
                    "{theme_name}/{density_name}/{scale}/{}/{}/{}/{}",
                    theme::role::terminal().metrics.line_height,
                    work.rows_shaped,
                    work.style_runs,
                    work.rows_appended_to_scene,
                )));
                if density == Density::Normal && scale == 1.0 {
                    pages.push(json!({ "theme": theme_name, "records": compact(records) }));
                }
            }
        }
    }
    theme::set_dark(original_dark);
    theme::set_density(original_density);
    theme::set_zoom(original_zoom);

    let actual = json!({ "matrix": matrix, "pages": pages });
    let expected: Value =
        serde_json::from_str(include_str!("fixtures/grid.expected.json")).unwrap();
    if actual != expected {
        panic!(
            "semantic grid fixture changed:\n{}",
            serde_json::to_string_pretty(&actual).unwrap()
        );
    }
}

/// A cell with no background of its own has to be the terminal's background
/// *exactly*, and a theme change has to reach every cached row in the same
/// frame.
///
/// This is what makes "paint the ground once" safe. A default-background cell
/// paints nothing and shows the fill under it, so the colour it *resolves* to
/// and the colour that fill uses are no longer merely consistent — they have to
/// be one value, or every run of ordinary text is a box of the wrong shade. A
/// full-screen TUI is where that shows, because it paints its own canvas with
/// explicit backgrounds and then writes most of its text with only a foreground.
#[test]
fn default_background_cells_are_the_pane_background_in_both_palettes() {
    let _guard = THEME_LOCK.lock().unwrap();
    let original_dark = theme::is_dark();
    let original_zoom = theme::zoom();
    theme::set_zoom(1.0);

    let mut view = TerminalView::new(());
    view.set_visible_rows(RowRange {
        first: RowIndex(0),
        count: 1,
    });
    view.apply(RenderUpdate::Snapshot {
        snapshot: contrast_snapshot(),
    })
    .unwrap();
    let mut engine = TextEngine::new();
    let mut draw = |view: &mut TerminalView<()>| {
        let mut scene = Scene::new();
        view.draw(
            &mut engine,
            &mut scene,
            Affine::IDENTITY,
            Rect::new(0.0, 0.0, 800.0, 40.0),
            1.0,
        );
        backgrounds(view)
    };

    for dark in [false, true] {
        theme::set_dark(dark);
        let painted = draw(&mut view);
        let background = rgba8(koru_session_view::terminal_background());
        assert_eq!(
            painted[0], background,
            "a default-background cell must be the pane fill"
        );
        assert_eq!(
            painted[1],
            [10, 20, 30, 255],
            "an explicit background holds"
        );
        assert_eq!(
            painted[2],
            rgba8(theme::terminal_palette().foreground),
            "reverse video makes the default *foreground* the background"
        );
        assert_ne!(painted[2], background, "…which is not the pane fill");
    }

    // The rows above were shaped under the dark palette. Flipping back has to
    // reshape all of them, not leave the frame mixing two palettes.
    theme::set_dark(false);
    let painted = draw(&mut view);
    assert_eq!(painted[0], rgba8(koru_session_view::terminal_background()));

    theme::set_dark(original_dark);
    theme::set_zoom(original_zoom);
}

fn contrast_snapshot() -> RenderSnapshot {
    let mut snapshot = snapshot();
    snapshot.rows = vec![TerminalRow {
        index: RowIndex(0),
        cells: vec![
            styled_cell(0, "a", CellWidth::One, CellStyle::default()),
            styled_cell(
                1,
                "b",
                CellWidth::One,
                CellStyle {
                    background: TerminalColor::Rgb {
                        red: 10,
                        green: 20,
                        blue: 30,
                    },
                    ..CellStyle::default()
                },
            ),
            styled_cell(
                2,
                "c",
                CellWidth::One,
                CellStyle {
                    inverse: true,
                    ..CellStyle::default()
                },
            ),
        ],
        wrapped_to_next: false,
    }];
    snapshot
}

fn backgrounds(view: &TerminalView<()>) -> Vec<[u64; 4]> {
    serde_json::to_value(view.paint_records())
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["layer"] == "background")
        .map(|record| {
            let parts = record["color"].as_array().unwrap();
            std::array::from_fn(|index| parts[index].as_u64().unwrap())
        })
        .collect()
}

fn rgba8(color: peniko::Color) -> [u64; 4] {
    color
        .components
        .map(|component| (component.clamp(0.0, 1.0) * 255.0).round() as u64)
}

#[test]
fn theme_density_and_scale_each_invalidate_once() {
    let _guard = THEME_LOCK.lock().unwrap();
    let original_dark = theme::is_dark();
    let original_density = theme::density();
    let original_zoom = theme::zoom();
    theme::set_zoom(1.0);
    let mut view = TerminalView::new(());
    view.set_visible_rows(RowRange {
        first: RowIndex(0),
        count: 1,
    });
    view.apply(RenderUpdate::Snapshot {
        snapshot: snapshot(),
    })
    .unwrap();
    let mut engine = TextEngine::new();
    let mut draw = |view: &mut TerminalView<()>, scale| {
        let mut scene = Scene::new();
        view.draw(
            &mut engine,
            &mut scene,
            Affine::IDENTITY,
            Rect::new(0.0, 0.0, 800.0, 40.0),
            scale,
        )
    };
    let initial = draw(&mut view, 1.0);
    assert_eq!((initial.cache_invalidations, initial.rows_shaped), (1, 1));
    let stable = draw(&mut view, 1.0);
    assert_eq!((stable.cache_invalidations, stable.rows_shaped), (0, 0));

    // Display zoom is a transform in both hosts, never a font size: the cell
    // is measured at 100% whatever the `theme::zoom()` global says, so moving
    // it invalidates nothing. (It used to re-measure the cell, which is why
    // Koru Terminal pinned the global at 1.0 and koru.exe's grid ran at zoom².)
    theme::set_zoom(1.1);
    let zoomed = draw(&mut view, 1.0);
    assert_eq!((zoomed.cache_invalidations, zoomed.rows_shaped), (0, 0));
    theme::set_zoom(1.0);

    theme::set_dark(!original_dark);
    let changed = draw(&mut view, 1.0);
    assert_eq!((changed.cache_invalidations, changed.rows_shaped), (1, 1));
    assert_eq!(draw(&mut view, 1.0).cache_invalidations, 0);

    theme::set_density(if original_density == Density::Compact {
        Density::Normal
    } else {
        Density::Compact
    });
    let changed = draw(&mut view, 1.0);
    assert_eq!((changed.cache_invalidations, changed.rows_shaped), (1, 1));
    assert_eq!(draw(&mut view, 1.0).cache_invalidations, 0);

    let changed = draw(&mut view, 1.25);
    assert_eq!((changed.cache_invalidations, changed.rows_shaped), (1, 1));
    let stable = draw(&mut view, 1.25);
    assert_eq!((stable.cache_invalidations, stable.rows_shaped), (0, 0));

    theme::set_dark(original_dark);
    theme::set_density(original_density);
    theme::set_zoom(original_zoom);
}

/// **The cell is measured at 100%, whatever the `theme::zoom()` global says.**
///
/// Both hosts spell display zoom as a transform — Koru Terminal's ADR-271 root
/// scale, koru.exe's whole-window scale — with the layout viewport divided by
/// it, so a cell that read the global as a font size would be zoomed twice
/// (Q-E8 P10: koru.exe's session grid at zoom² while the window was at zoom).
/// This pins the measurement itself, on fresh captures, because the epoch test
/// above can only see *invalidation*: a re-added multiplier with an unchanged
/// epoch would keep serving stale metrics and pass it.
#[test]
fn the_cell_ignores_the_zoom_global() {
    let _guard = THEME_LOCK.lock().unwrap();
    let original_zoom = theme::zoom();
    let mut engine = TextEngine::new();
    theme::set_zoom(1.0);
    let base = koru_session_view::TerminalStyle::capture(&mut engine, 1.0);
    theme::set_zoom(2.0);
    let doubled = koru_session_view::TerminalStyle::capture(&mut engine, 1.0);
    theme::set_zoom(original_zoom);
    assert_eq!(
        (base.cell_width, base.cell_height),
        (doubled.cell_width, doubled.cell_height),
        "the zoom is a transform, not a font size"
    );
    assert_eq!(
        (base.metrics.size, base.metrics.line_height),
        (doubled.metrics.size, doubled.metrics.line_height),
    );
}
