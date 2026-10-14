// SPDX-License-Identifier: MPL-2.0

use std::{cell::RefCell, rc::Rc, sync::Arc};

use koru_draw::text::TextEngine;
use koru_session_observability::{Counter, SessionStats};
use koru_session_protocol::{
    AttentionState, CellStyle, CellWidth, ColumnIndex, CursorShape, CursorState, GridSize,
    RenderDelta, RenderSnapshot, RenderUpdate, RowIndex, RowRange, SessionId, StreamGap,
    StreamSequence, TerminalCell, TerminalGeneration, TerminalModes, TerminalRow,
    ViewportGeneration,
};
use koru_session_view::{
    CellPoint, CellSelection, SessionSurfaceIntent, SessionSurfaceInteraction, TerminalIntent,
    TerminalView,
};
use vello::{
    Scene,
    kurbo::{Affine, Rect},
};

#[derive(Clone, Default)]
struct Recorder(Rc<RefCell<Vec<TerminalIntent>>>);

impl SessionSurfaceInteraction for Recorder {
    fn dispatch(&mut self, intent: SessionSurfaceIntent) {
        let SessionSurfaceIntent::Terminal(intent) = intent else {
            panic!("terminal view emitted a different surface intent");
        };
        self.0.borrow_mut().push(intent);
    }
}

fn row(index: u64, marker: &str) -> TerminalRow {
    TerminalRow {
        index: RowIndex(index),
        cells: (0..120)
            .map(|column| TerminalCell {
                column: ColumnIndex(column),
                text: if column == 0 {
                    marker.into()
                } else {
                    "x".into()
                },
                width: CellWidth::One,
                style: if column < 30 {
                    CellStyle::default()
                } else if column < 60 {
                    CellStyle {
                        bold: true,
                        ..CellStyle::default()
                    }
                } else if column < 90 {
                    CellStyle {
                        italic: true,
                        ..CellStyle::default()
                    }
                } else {
                    CellStyle {
                        underline: true,
                        ..CellStyle::default()
                    }
                },
            })
            .collect(),
        wrapped_to_next: false,
    }
}

fn snapshot(first_visible: u64) -> RenderSnapshot {
    let overscan_first = first_visible.saturating_sub(2);
    RenderSnapshot {
        session_id: SessionId::mint(),
        terminal_generation: TerminalGeneration::new(1).unwrap(),
        viewport_generation: ViewportGeneration::new(1).unwrap(),
        sequence: StreamSequence::new(1).unwrap(),
        viewport: RowRange {
            first: RowIndex(overscan_first),
            count: 44,
        },
        size: GridSize::new(120, 40).unwrap(),
        rows: (overscan_first..overscan_first + 44)
            .map(|index| row(index, "x"))
            .collect(),
        cursor: CursorState {
            row: RowIndex(first_visible),
            column: ColumnIndex(0),
            shape: CursorShape::Beam,
            blinking: false,
        },
        modes: TerminalModes::default(),
        attention: AttentionState::default(),
    }
}

fn bounds() -> Rect {
    Rect::new(0.0, 0.0, 1_200.0, 800.0)
}

fn draw(
    view: &mut TerminalView<Recorder>,
    engine: &mut TextEngine,
) -> koru_session_view::RenderWork {
    let mut scene = Scene::new();
    view.draw(engine, &mut scene, Affine::IDENTITY, bounds(), 1.0)
}

#[test]
fn culls_overscan_before_shaping_and_reuses_unchanged_rows() {
    let stats = Arc::new(SessionStats::new(true));
    let mut view = TerminalView::with_stats(Recorder::default(), stats.clone());
    let snapshot = snapshot(10_000);
    view.set_visible_rows(RowRange {
        first: RowIndex(10_000),
        count: 40,
    });
    view.apply(RenderUpdate::Snapshot { snapshot }).unwrap();
    let mut engine = TextEngine::new();

    let first = draw(&mut view, &mut engine);
    assert_eq!(first.rows_considered, 40);
    assert_eq!(first.rows_shaped, 40);
    assert_eq!(first.style_runs, 160);
    assert_eq!(first.rows_appended_to_scene, 40);

    let second = draw(&mut view, &mut engine);
    assert_eq!(second.rows_shaped, 0);
    assert_eq!(second.cache_hits, 40);
    assert_eq!(second.rows_appended_to_scene, 40);
    assert_eq!(second.cache_invalidations, 0);

    let totals = stats.snapshot().unwrap();
    assert_eq!(totals.counter(Counter::RowsShaped), 40);
    assert_eq!(totals.counter(Counter::CachedRowHits), 40);
    assert_eq!(totals.counter(Counter::RowsAppendedToScene), 80);
    assert_eq!(totals.counter(Counter::RenderRuns), 160);
}

#[test]
fn visible_damage_shapes_one_row_and_offscreen_damage_shapes_none() {
    let mut snapshot = snapshot(100);
    let ids = (
        snapshot.session_id,
        snapshot.terminal_generation,
        snapshot.viewport_generation,
    );
    let mut view = TerminalView::new(Recorder::default());
    view.set_visible_rows(RowRange {
        first: RowIndex(100),
        count: 40,
    });
    view.apply(RenderUpdate::Snapshot {
        snapshot: snapshot.clone(),
    })
    .unwrap();
    let mut engine = TextEngine::new();
    draw(&mut view, &mut engine);

    view.apply(RenderUpdate::Delta {
        delta: RenderDelta {
            session_id: ids.0,
            terminal_generation: ids.1,
            viewport_generation: ids.2,
            sequence: StreamSequence::new(2).unwrap(),
            rows: vec![row(110, "changed")],
            cursor: None,
            modes: None,
            attention: None,
        },
    })
    .unwrap();
    let visible = draw(&mut view, &mut engine);
    assert_eq!((visible.rows_shaped, visible.cache_hits), (1, 39));

    snapshot.sequence = StreamSequence::new(2).unwrap();
    view.apply(RenderUpdate::Delta {
        delta: RenderDelta {
            session_id: ids.0,
            terminal_generation: ids.1,
            viewport_generation: ids.2,
            sequence: StreamSequence::new(3).unwrap(),
            rows: vec![row(98, "overscan")],
            cursor: None,
            modes: None,
            attention: None,
        },
    })
    .unwrap();
    let offscreen = draw(&mut view, &mut engine);
    assert_eq!((offscreen.rows_shaped, offscreen.cache_hits), (0, 40));
}

#[test]
fn hidden_updates_do_no_layout_or_scene_work() {
    let stats = Arc::new(SessionStats::new(true));
    let mut view = TerminalView::with_stats(Recorder::default(), stats.clone());
    view.set_visible_rows(RowRange {
        first: RowIndex(0),
        count: 40,
    });
    view.apply(RenderUpdate::Snapshot {
        snapshot: snapshot(0),
    })
    .unwrap();
    view.set_visible(false);
    let mut engine = TextEngine::new();
    let hidden = draw(&mut view, &mut engine);
    assert_eq!(hidden, Default::default());
    let totals = stats.snapshot().unwrap();
    assert_eq!(totals.counter(Counter::RowsShaped), 0);
    assert_eq!(totals.counter(Counter::RowsAppendedToScene), 0);

    view.set_visible(true);
    assert_eq!(draw(&mut view, &mut engine).rows_shaped, 40);
}

#[test]
fn work_is_identical_at_zero_ten_thousand_and_one_hundred_thousand() {
    let mut engine = TextEngine::new();
    let mut measured = Vec::new();
    for first in [0, 10_000, 100_000] {
        let mut view = TerminalView::new(Recorder::default());
        view.set_visible_rows(RowRange {
            first: RowIndex(first),
            count: 40,
        });
        view.apply(RenderUpdate::Snapshot {
            snapshot: snapshot(first),
        })
        .unwrap();
        measured.push(draw(&mut view, &mut engine));
    }
    assert_eq!(measured[0], measured[1]);
    assert_eq!(measured[1], measured[2]);
}

#[test]
fn gap_dispatches_typed_recovery_without_identity_or_transport() {
    let recorder = Recorder::default();
    let observed = recorder.0.clone();
    let snapshot = snapshot(0);
    let mut view = TerminalView::new(recorder);
    view.apply(RenderUpdate::Snapshot {
        snapshot: snapshot.clone(),
    })
    .unwrap();
    let outcome = view
        .apply(RenderUpdate::Gap {
            gap: StreamGap {
                session_id: snapshot.session_id,
                terminal_generation: snapshot.terminal_generation,
                viewport_generation: snapshot.viewport_generation,
                expected: StreamSequence::new(2).unwrap(),
                actual: StreamSequence::new(4).unwrap(),
                resnapshot_required: true,
            },
        })
        .unwrap();
    assert!(matches!(
        outcome,
        koru_session_view::ApplyOutcome::RecoveryRequested
    ));
    assert_eq!(*observed.borrow(), vec![TerminalIntent::RecoverSnapshot]);
}

#[test]
fn selection_and_cursor_are_bounded_semantic_layers() {
    let mut view = TerminalView::new(Recorder::default());
    view.set_visible_rows(RowRange {
        first: RowIndex(50_000),
        count: 40,
    });
    view.set_selection(Some(CellSelection {
        anchor: CellPoint {
            row: RowIndex(0),
            column: ColumnIndex(3),
        },
        focus: CellPoint {
            row: RowIndex(100_000),
            column: ColumnIndex(7),
        },
    }));
    view.apply(RenderUpdate::Snapshot {
        snapshot: snapshot(50_000),
    })
    .unwrap();
    let mut engine = TextEngine::new();
    draw(&mut view, &mut engine);
    let selection_rows = view
        .paint_records()
        .iter()
        .filter(|record| matches!(record, koru_session_view::PaintRecord::Selection { .. }))
        .count();
    assert_eq!(selection_rows, 40);
    assert_eq!(
        view.paint_records()
            .iter()
            .filter(|record| matches!(record, koru_session_view::PaintRecord::Cursor { .. }))
            .count(),
        1
    );
}

#[test]
fn one_host_text_engine_serves_more_than_one_view() {
    let mut engine = TextEngine::new();
    let mut first = TerminalView::new(Recorder::default());
    let mut second = TerminalView::new(Recorder::default());
    for (view, offset) in [(&mut first, 0), (&mut second, 10_000)] {
        view.set_visible_rows(RowRange {
            first: RowIndex(offset),
            count: 40,
        });
        view.apply(RenderUpdate::Snapshot {
            snapshot: snapshot(offset),
        })
        .unwrap();
        assert_eq!(draw(view, &mut engine).rows_shaped, 40);
    }
}

#[test]
fn alternate_screen_rows_never_bleed_from_primary_cache() {
    let initial = snapshot(0);
    let ids = (
        initial.session_id,
        initial.terminal_generation,
        initial.viewport_generation,
    );
    let mut view = TerminalView::new(Recorder::default());
    view.set_visible_rows(RowRange {
        first: RowIndex(0),
        count: 40,
    });
    view.apply(RenderUpdate::Snapshot { snapshot: initial })
        .unwrap();
    let mut engine = TextEngine::new();
    draw(&mut view, &mut engine);

    view.apply(RenderUpdate::Delta {
        delta: RenderDelta {
            session_id: ids.0,
            terminal_generation: ids.1,
            viewport_generation: ids.2,
            sequence: StreamSequence::new(2).unwrap(),
            rows: vec![row(0, "alternate")],
            cursor: None,
            modes: Some(TerminalModes {
                alternate_screen: true,
                ..TerminalModes::default()
            }),
            attention: None,
        },
    })
    .unwrap();
    assert_eq!(draw(&mut view, &mut engine).rows_shaped, 1);
    assert!(view.paint_records().iter().any(|record| matches!(
        record,
        koru_session_view::PaintRecord::Text { text, .. } if text.starts_with("alternate")
    )));

    view.apply(RenderUpdate::Delta {
        delta: RenderDelta {
            session_id: ids.0,
            terminal_generation: ids.1,
            viewport_generation: ids.2,
            sequence: StreamSequence::new(3).unwrap(),
            rows: vec![row(0, "primary")],
            cursor: None,
            modes: Some(TerminalModes::default()),
            attention: None,
        },
    })
    .unwrap();
    assert_eq!(draw(&mut view, &mut engine).rows_shaped, 1);
    assert!(view.paint_records().iter().any(|record| matches!(
        record,
        koru_session_view::PaintRecord::Text { text, .. } if text.starts_with("primary")
    )));
}

#[test]
fn switching_sessions_discards_the_previous_row_space_and_follows_the_new_tail() {
    let mut view = TerminalView::new(Recorder::default());
    view.set_visible_rows(RowRange {
        first: RowIndex(98),
        count: 40,
    });
    view.apply(RenderUpdate::Snapshot {
        snapshot: snapshot(100),
    })
    .unwrap();
    assert_eq!(view.geometry().map(|geometry| geometry.rows.first), None);

    view.apply(RenderUpdate::Snapshot {
        snapshot: snapshot(10_000),
    })
    .unwrap();
    let mut engine = TextEngine::new();
    draw(&mut view, &mut engine);
    assert_eq!(
        view.geometry().unwrap().rows.first,
        RowIndex(10_002),
        "the old session viewport must not survive a session switch"
    );
}

#[test]
fn host_reset_removes_rows_selection_and_surface_identity() {
    let mut view = TerminalView::new(Recorder::default());
    view.apply(RenderUpdate::Snapshot {
        snapshot: snapshot(100),
    })
    .unwrap();
    view.set_selection(Some(CellSelection {
        anchor: CellPoint {
            row: RowIndex(100),
            column: ColumnIndex(0),
        },
        focus: CellPoint {
            row: RowIndex(100),
            column: ColumnIndex(2),
        },
    }));
    view.reset();
    assert!(view.model().is_none());
    assert!(view.selected_text().is_none());
    assert!(view.geometry().is_none());
}
