// SPDX-License-Identifier: MPL-2.0

use koru_session_protocol::{
    CellStyle, CellWidth, ColumnIndex, CursorShape, GridSize, MouseEncoding, MouseTrackingMode,
    RowIndex, RowRange, TerminalColor, TerminalModes, TerminalRow,
};
use koru_terminal_emulator::{
    BlockedAction, EmulatorError, EmulatorEvent, MAX_EVENTS_PER_BATCH, MAX_OSC_BYTES,
    MAX_PAGE_ROWS, MAX_SCROLLBACK_ROWS, MAX_TITLE_CHARS, SelectionKind, SelectionPoint,
    TerminalEmulator, TerminalFace, UNICODE_WIDTH_VERSION,
};
use serde::Serialize;

fn emulator(columns: u16, rows: u16) -> TerminalEmulator {
    TerminalEmulator::new(GridSize::new(columns, rows).unwrap()).unwrap()
}

fn visible(term: &TerminalEmulator) -> Vec<TerminalRow> {
    term.visible_rows(term.visible_range()).unwrap().rows
}

fn decode_hex(source: &str) -> Vec<u8> {
    let source: String = source
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    assert_eq!(source.len() % 2, 0);
    (0..source.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&source[index..index + 2], 16).unwrap())
        .collect()
}

#[derive(Serialize)]
struct FixtureSnapshot {
    rows: Vec<CompactRow>,
    cursor_row: RowIndex,
    cursor_column: ColumnIndex,
}

#[derive(Serialize)]
struct CompactRow {
    index: RowIndex,
    cells: Vec<CompactCell>,
}

#[derive(Serialize)]
struct CompactCell {
    column: ColumnIndex,
    text: String,
    width: &'static str,
    foreground: String,
    background: String,
    flags: Vec<&'static str>,
}

fn significant_rows(rows: Vec<TerminalRow>) -> Vec<CompactRow> {
    rows.into_iter()
        .map(|row| CompactRow {
            index: row.index,
            cells: row
                .cells
                .into_iter()
                .filter(|cell| {
                    cell.text != " "
                        || cell.width != CellWidth::One
                        || cell.style != CellStyle::default()
                })
                .map(compact_cell)
                .collect(),
        })
        .filter(|row| !row.cells.is_empty())
        .collect()
}

fn compact_cell(cell: koru_session_protocol::TerminalCell) -> CompactCell {
    let mut flags = Vec::new();
    for (enabled, name) in [
        (cell.style.bold, "bold"),
        (cell.style.dim, "dim"),
        (cell.style.italic, "italic"),
        (cell.style.underline, "underline"),
        (cell.style.strikeout, "strikeout"),
        (cell.style.inverse, "inverse"),
    ] {
        if enabled {
            flags.push(name);
        }
    }
    CompactCell {
        column: cell.column,
        text: cell.text,
        width: match cell.width {
            CellWidth::Zero => "zero",
            CellWidth::One => "one",
            CellWidth::Two => "two",
        },
        foreground: compact_color(cell.style.foreground),
        background: compact_color(cell.style.background),
        flags,
    }
}

fn compact_color(color: TerminalColor) -> String {
    match color {
        TerminalColor::DefaultForeground => "default_foreground".to_owned(),
        TerminalColor::DefaultBackground => "default_background".to_owned(),
        TerminalColor::Indexed { index } => format!("indexed:{index}"),
        TerminalColor::Rgb { red, green, blue } => format!("rgb:{red},{green},{blue}"),
    }
}

#[test]
fn archived_bytes_match_the_expected_cell_snapshot() {
    let bytes = decode_hex(include_str!("fixtures/compatibility.ansi.hex"));
    let mut term = emulator(24, 6);
    let update = term.parse(&bytes);
    assert_eq!(
        update.events,
        vec![
            EmulatorEvent::TitleChanged(Some("Koru Shell".to_owned())),
            EmulatorEvent::Bell,
        ]
    );

    let cursor = term.cursor();
    let actual = serde_json::to_value(FixtureSnapshot {
        rows: significant_rows(visible(&term)),
        cursor_row: cursor.row,
        cursor_column: cursor.column,
    })
    .unwrap();
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/compatibility.expected.json")).unwrap();
    assert_eq!(
        actual,
        expected,
        "actual snapshot:\n{}",
        serde_json::to_string_pretty(&actual).unwrap()
    );
}

#[test]
fn whole_and_one_byte_chunks_converge_to_the_same_state() {
    let bytes = decode_hex(include_str!("fixtures/compatibility.ansi.hex"));
    let mut whole = emulator(24, 6);
    let whole_events = whole.parse(&bytes).events;
    let mut fragmented = emulator(24, 6);
    let fragmented_events: Vec<_> = bytes
        .iter()
        .flat_map(|byte| fragmented.parse(std::slice::from_ref(byte)).events)
        .collect();

    assert_eq!(visible(&whole), visible(&fragmented));
    assert_eq!(whole.cursor(), fragmented.cursor());
    assert_eq!(whole.modes(), fragmented.modes());
    assert_eq!(whole_events, fragmented_events);
}

#[test]
fn unchanged_cells_do_not_become_row_damage() {
    let mut term = emulator(10, 2);
    assert_eq!(term.parse(b"a").damaged_rows.len(), 1);
    let unchanged = term.parse(b"");
    assert!(unchanged.damaged_rows.is_empty());
    assert!(unchanged.visible_range.is_none());
    assert!(unchanged.cursor.is_none());
    assert!(unchanged.modes.is_none());

    // Changing only the template for future cells does not replace a row.
    assert!(term.parse(b"\x1b[1m").damaged_rows.is_empty());
}

#[test]
fn ansi_modes_cursor_alt_screen_and_resize_are_preserved() {
    let mut term = emulator(12, 3);
    term.parse(b"primary");
    let update = term.parse(b"\x1b[?1h\x1b[?2004h\x1b[?1000h\x1b[?1006h\x1b[?6h\x1b[?7l\x1b[5 q");
    assert_eq!(
        update.modes,
        Some(TerminalModes {
            alternate_screen: false,
            application_cursor: true,
            bracketed_paste: true,
            mouse_reporting: true,
            mouse_tracking: MouseTrackingMode::Click,
            mouse_encoding: MouseEncoding::Sgr,
            focus_reporting: false,
            origin_mode: true,
            auto_wrap: false,
        })
    );
    assert_eq!(term.cursor().shape, CursorShape::Beam);
    assert!(term.cursor().blinking);

    let alt = term.parse(b"\x1b[?1049hALT");
    assert!(alt.modes.unwrap().alternate_screen);
    assert!(visible(&term)[0].cells.iter().any(|cell| cell.text == "A"));
    term.parse(b"\x1b[?1049l");
    assert!(visible(&term)[0].cells.iter().any(|cell| cell.text == "p"));

    term.parse(b"\x1b[?25l");
    assert_eq!(term.cursor().shape, CursorShape::Hidden);
    let resized = term.resize(GridSize::new(18, 4).unwrap()).unwrap();
    assert_eq!(
        term.size(),
        GridSize {
            columns: 18,
            rows: 4
        }
    );
    assert!(!resized.damaged_rows.is_empty());
}

#[test]
fn unicode_width_contract_is_pinned() {
    assert_eq!(UNICODE_WIDTH_VERSION, "unicode-width 0.2.2");
    for (text, expected_column, expected_cell_text, expected_width) in [
        ("·", 1, "·", CellWidth::One),
        ("e\u{301}", 1, "e\u{301}", CellWidth::One),
        ("中", 2, "中", CellWidth::Two),
        ("✈\u{fe0f}", 1, "✈\u{fe0f}", CellWidth::One),
        ("👩\u{200d}💻", 4, "👩\u{200d}", CellWidth::Two),
    ] {
        let mut term = emulator(12, 2);
        term.parse(text.as_bytes());
        assert_eq!(
            term.cursor().column,
            ColumnIndex(expected_column),
            "{text:?}"
        );
        let first = &visible(&term)[0].cells[0];
        assert_eq!(first.text, expected_cell_text, "{text:?}");
        assert_eq!(first.width, expected_width, "{text:?}");
    }
}

#[test]
fn colors_styles_and_wide_spacers_use_protocol_types() {
    let mut term = emulator(12, 2);
    term.parse(b"\x1b[1;3;4;7;9;38;5;123;48;2;4;5;6mX\x1b[0m");
    let cell = &visible(&term)[0].cells[0];
    assert_eq!(cell.text, "X");
    assert!(cell.style.bold);
    assert!(cell.style.italic);
    assert!(cell.style.underline);
    assert!(cell.style.inverse);
    assert!(cell.style.strikeout);
    assert_eq!(
        TerminalFace::for_style(cell.style),
        TerminalFace::BoldItalic
    );
    assert_eq!(
        cell.style.foreground,
        koru_session_protocol::TerminalColor::Indexed { index: 123 }
    );
    assert_eq!(
        cell.style.background,
        koru_session_protocol::TerminalColor::Rgb {
            red: 4,
            green: 5,
            blue: 6
        }
    );

    term.parse("中".as_bytes());
    let cells = &visible(&term)[0].cells;
    assert_eq!(cells[1].width, CellWidth::Two);
    assert_eq!(cells[2].width, CellWidth::Zero);
    assert!(cells[2].text.is_empty());
}

#[test]
fn selection_uses_stable_retained_coordinates() {
    let mut term = emulator(10, 3);
    term.parse(b"hello world");
    let row = term.visible_range().first;
    let selected = term
        .select(
            SelectionKind::Simple,
            SelectionPoint {
                row,
                column: ColumnIndex(0),
            },
            SelectionPoint {
                row,
                column: ColumnIndex(4),
            },
        )
        .unwrap();
    assert_eq!(selected.as_deref(), Some("hello"));
    term.clear_selection();
}

#[test]
fn osc52_and_external_queries_cannot_take_action() {
    let mut term = emulator(10, 2);
    let osc52 = term.parse(b"\x1b]52;c;SGVsbG8=\x07");
    assert!(
        osc52.events.is_empty(),
        "OSC 52 is disabled in the emulator config"
    );

    let query = term.parse(b"\x1b]10;?\x07");
    assert_eq!(
        query.events,
        vec![EmulatorEvent::ActionBlocked(BlockedAction::ColorQuery)]
    );

    let mut large_title = b"\x1b]0;".to_vec();
    large_title.extend(std::iter::repeat_n(b'x', 1024 * 1024));
    large_title.push(0x07);
    let events: Vec<_> = large_title
        .chunks(333)
        .flat_map(|chunk| term.parse(chunk).events)
        .collect();
    let Some(EmulatorEvent::TitleChanged(Some(title))) = events.last() else {
        panic!("large title did not produce a bounded title event");
    };
    assert_eq!(title.chars().count(), MAX_TITLE_CHARS);
    assert!(MAX_OSC_BYTES < large_title.len());

    // The terminator is forwarded after truncation, so ordinary output resumes.
    term.parse(b"ok");
    assert!(visible(&term)[0].cells.iter().any(|cell| cell.text == "o"));

    let bells = vec![0x07; MAX_EVENTS_PER_BATCH * 4];
    let events = term.parse(&bells).events;
    assert_eq!(events.len(), MAX_EVENTS_PER_BATCH);
    assert_eq!(events.last(), Some(&EmulatorEvent::EventsTruncated));
}

#[test]
fn row_reads_are_bounded_and_independent_of_scrollback_depth() {
    let mut empty = emulator(8, 4);
    empty.parse(b"tail");
    let empty_range = RowRange {
        first: empty.visible_range().first,
        count: 2,
    };
    let empty_read = empty.visible_rows(empty_range).unwrap();

    let mut full = emulator(8, 4);
    let mut output = Vec::with_capacity((MAX_SCROLLBACK_ROWS + 4) * 3);
    for _ in 0..MAX_SCROLLBACK_ROWS + 4 {
        output.extend_from_slice(b"x\r\n");
    }
    let filled = full.parse(&output);
    assert_eq!(filled.visible_range, Some(full.visible_range()));
    assert_eq!(
        full.retained_range().len(),
        (MAX_SCROLLBACK_ROWS + 4) as u64
    );
    let full_range = RowRange {
        first: full.visible_range().first,
        count: 2,
    };
    let full_read = full.visible_rows(full_range).unwrap();
    assert_eq!(empty_read.visited_cells, 16);
    assert_eq!(full_read.visited_cells, empty_read.visited_cells);

    let rebased = full.parse(b"y\r\n");
    assert!(rebased.retention_rebased);

    assert_eq!(
        full.scrollback_page(RowRange {
            first: RowIndex(0),
            count: MAX_PAGE_ROWS + 1
        }),
        Err(EmulatorError::PageTooLarge {
            maximum: MAX_PAGE_ROWS
        })
    );
}

#[test]
fn invalid_sizes_and_ranges_fail_before_touching_the_grid() {
    assert!(matches!(
        TerminalEmulator::new(GridSize {
            columns: 1,
            rows: 1
        }),
        Err(EmulatorError::InvalidSize)
    ));
    let term = emulator(8, 2);
    assert_eq!(
        term.scrollback_page(RowRange {
            first: RowIndex(0),
            count: 0
        }),
        Err(EmulatorError::EmptyRange)
    );
}
