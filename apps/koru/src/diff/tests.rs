// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_history::TextSpan;
use koru_history::unified::{BlockView, DiffRow, DiffStats};

fn block(text: &str) -> BlockView {
    BlockView {
        kind: "paragraph".into(),
        depth: 1,
        text: text.into(),
        done: None,
        heading: None,
    }
}

fn comparison(rows: Vec<DiffRow>) -> Comparison {
    Comparison {
        diff: UnifiedDiff {
            rows,
            stats: DiffStats::default(),
        },
        from_recorded_at_ms: 0,
        to_recorded_at_ms: 60_000,
        from_reason: Key::HistoryReasonManual,
        to_reason: Key::HistoryReasonIdle,
        from_label: None,
        to_label: None,
    }
}

#[test]
fn a_comparison_lays_out_header_and_rows() {
    let mut view = DiffView::new(koru_i18n::Lang::default());
    view.set_viewport(800.0, 600.0);
    view.set_comparison(comparison(vec![
        DiffRow::Context(block("kept")),
        DiffRow::Added(block("new line")),
        DiffRow::Removed(block("old line")),
        DiffRow::Modified {
            block: block("The red fox."),
            spans: vec![
                TextSpan {
                    tag: DiffTag::Equal,
                    text: "The ".into(),
                },
                TextSpan {
                    tag: DiffTag::Delete,
                    text: "brown".into(),
                },
                TextSpan {
                    tag: DiffTag::Insert,
                    text: "red".into(),
                },
                TextSpan {
                    tag: DiffTag::Equal,
                    text: " fox.".into(),
                },
            ],
            kind_changed: false,
            metadata_changed: false,
            marks_changed: false,
        },
        DiffRow::Skipped { blocks: 7 },
    ]));
    view.rebuild();
    // Three header rows plus five diff rows.
    assert_eq!(view.rows.len(), 8);
    assert!(view.content_h > 0.0);
    // The added and removed rows carry a band; the modified row carries
    // word highlights instead.
    assert!(view.rows[4].band.is_some());
    assert!(view.rows[5].band.is_some());
    assert!(view.rows[6].band.is_none());
    assert_eq!(view.rows[6].highlights.len(), 2);
}

#[test]
fn scroll_clamps_to_the_content() {
    let mut view = DiffView::new(koru_i18n::Lang::default());
    view.set_viewport(800.0, 100.0);
    view.set_comparison(comparison(
        (0..40)
            .map(|i| DiffRow::Context(block(&format!("row {i}"))))
            .collect(),
    ));
    view.rebuild();
    view.scroll_by(MouseScrollDelta::PixelDelta(
        winit::dpi::PhysicalPosition::new(0.0, -1.0e6),
    ));
    assert_eq!(view.scroll, view.max_scroll());
    view.scroll_by(MouseScrollDelta::PixelDelta(
        winit::dpi::PhysicalPosition::new(0.0, 1.0e6),
    ));
    assert_eq!(view.scroll, 0.0);
}

#[test]
fn a_properties_only_change_still_says_something() {
    let mut view = DiffView::new(koru_i18n::Lang::default());
    view.set_viewport(800.0, 600.0);
    view.set_comparison(comparison(vec![DiffRow::Modified {
        block: BlockView {
            done: Some(true),
            ..block("ship it")
        },
        spans: vec![TextSpan {
            tag: DiffTag::Equal,
            text: "ship it".into(),
        }],
        kind_changed: false,
        metadata_changed: true,
        marks_changed: false,
    }]));
    view.rebuild();
    // The row exists and carried no word highlights — the label is in the
    // text itself, which layout measured (wider than the bare words alone).
    assert_eq!(view.rows.len(), 4);
    assert!(view.rows[3].highlights.is_empty());
}
