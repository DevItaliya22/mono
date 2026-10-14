// SPDX-License-Identifier: MPL-2.0

//! Validated plain spans for any caller that wants to paint matched text.

use koru_draw::text::TextEngine;
use koru_draw::{scene, theme};
use vello::Scene;
use vello::kurbo::Affine;
use vello::peniko::Color;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

pub fn valid_spans(text: &str, spans: impl IntoIterator<Item = Span>) -> Vec<Span> {
    let mut out: Vec<_> = spans
        .into_iter()
        .filter(|span| {
            span.start < span.end
                && span.end <= text.len()
                && text.is_char_boundary(span.start)
                && text.is_char_boundary(span.end)
        })
        .collect();
    out.sort_by_key(|span| span.start);
    out.dedup();
    let mut merged: Vec<Span> = Vec::with_capacity(out.len());
    for span in out {
        if let Some(previous) = merged.last_mut()
            && span.start <= previous.end
        {
            previous.end = previous.end.max(span.end);
        } else {
            merged.push(span);
        }
    }
    merged
}

fn measured_width(engine: &mut TextEngine, text: &str, style: theme::TextStyle) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    // Parley omits trailing whitespace from a standalone run. Put a sentinel
    // after it while measuring, then subtract the sentinel's own advance. The
    // sentinel is never drawn; it only makes the authored spaces internal so
    // the layout engine reports their width.
    if text.chars().last().is_some_and(char::is_whitespace) {
        let sentinel = "|";
        let probe = format!("{text}{sentinel}");
        engine.simple_layout(&probe, style).width() - engine.simple_layout(sentinel, style).width()
    } else {
        engine.simple_layout(text, style).width()
    }
}

fn highlighted_width(
    engine: &mut TextEngine,
    text: &str,
    spans: &[Span],
    style: theme::TextStyle,
) -> f32 {
    let mut width = 0.0;
    let mut at = 0;
    for span in spans {
        width += measured_width(engine, &text[at..span.start], style);
        width += measured_width(
            engine,
            &text[span.start..span.end],
            style.with_weight(theme::FontWeight::Bold),
        );
        at = span.end;
    }
    width + measured_width(engine, &text[at..], style)
}

/// Elide both sides while retaining and translating every explanatory match.
pub fn elide_around_spans(
    engine: &mut TextEngine,
    text: &str,
    spans: impl IntoIterator<Item = Span>,
    style: theme::TextStyle,
    max_width: f32,
) -> (String, Vec<Span>) {
    let spans = valid_spans(text, spans);
    let (Some(first), Some(last)) = (spans.first().copied(), spans.last().copied()) else {
        return (engine.elided(text, style, max_width), Vec::new());
    };
    if highlighted_width(engine, text, &spans, style) <= max_width {
        return (text.to_string(), spans);
    }

    let mut boundaries: Vec<usize> = text.char_indices().map(|(at, _)| at).collect();
    boundaries.push(text.len());
    let match_start = boundaries.binary_search(&first.start).unwrap();
    let match_end = boundaries.binary_search(&last.end).unwrap();
    let before = match_start;
    let after = boundaries.len() - 1 - match_end;

    let candidate = |context: usize| {
        let mut left = (context / 2).min(before);
        let mut right = (context - left).min(after);
        let remaining = context.saturating_sub(left + right);
        left += remaining.min(before - left);
        right += remaining.saturating_sub(before - left).min(after - right);
        let start = boundaries[match_start - left];
        let end = boundaries[match_end + right];
        let prefix = usize::from(start > 0);
        let mut display = String::new();
        if prefix != 0 {
            display.push('…');
        }
        display.push_str(&text[start..end]);
        if end < text.len() {
            display.push('…');
        }
        let translated = spans
            .iter()
            .map(|span| Span {
                start: prefix * '…'.len_utf8() + span.start - start,
                end: prefix * '…'.len_utf8() + span.end - start,
            })
            .collect::<Vec<_>>();
        (display, translated)
    };

    let mut low = 0usize;
    let mut high = before + after;
    let mut best = candidate(0);
    while low <= high {
        let middle = low + (high - low) / 2;
        let next = candidate(middle);
        if highlighted_width(engine, &next.0, &next.1, style) <= max_width {
            best = next;
            low = middle + 1;
        } else if middle == 0 {
            break;
        } else {
            high = middle - 1;
        }
    }
    if highlighted_width(engine, &best.0, &best.1, style) <= max_width {
        return best;
    }

    // The span envelope itself can be wider than the row when terms are far
    // apart. Keep the first match explainable instead of overflowing.
    let matched = engine.elided(&text[first.start..first.end], style, max_width);
    let visible = matched.strip_suffix('…').unwrap_or(&matched).len();
    (
        matched,
        (visible > 0)
            .then_some(Span {
                start: 0,
                end: visible,
            })
            .into_iter()
            .collect(),
    )
}

/// Backwards-compatible single-span preparation.
pub fn elide_around_span(
    engine: &mut TextEngine,
    text: &str,
    span: Span,
    style: theme::TextStyle,
    max_width: f32,
) -> (String, Option<Span>) {
    let (text, spans) = elide_around_spans(engine, text, [span], style, max_width);
    (text, spans.into_iter().next())
}

/// Draw one highlighted line as measured regular/bold runs.
///
/// Drawing the full regular line and then painting bold glyphs over it changes
/// the width without moving the following glyphs. This component instead owns
/// the run sequence, so authored whitespace and every later character keep
/// their correct position.
#[allow(clippy::too_many_arguments)]
pub fn draw_highlighted_line(
    canvas: &mut Scene,
    root: Affine,
    engine: &mut TextEngine,
    text: &str,
    spans: impl IntoIterator<Item = Span>,
    x: f32,
    y: f32,
    max_width: f32,
    style: theme::TextStyle,
    text_color: Color,
    match_color: Color,
    highlight_color: Color,
) {
    let (text, spans) = elide_around_spans(engine, text, spans, style, max_width);
    let bold = style.with_weight(theme::FontWeight::Bold);
    let mut cursor = x;
    let mut at = 0;
    for span in &spans {
        let ordinary = &text[at..span.start];
        let layout = engine.simple_layout(ordinary, style);
        scene::draw_layout_first_line(canvas, root, &layout, cursor, y, Some(text_color));
        cursor += measured_width(engine, ordinary, style);

        let matched = &text[span.start..span.end];
        let width = measured_width(engine, matched, bold);
        scene::fill(
            canvas,
            root,
            highlight_color,
            &scene::rrect(cursor, y - 1.0, width, style.metrics.line_height, 2.0),
        );
        let layout = engine.simple_layout(matched, bold);
        scene::draw_layout_first_line(canvas, root, &layout, cursor, y, Some(match_color));
        cursor += width;
        at = span.end;
    }
    let ordinary = &text[at..];
    let layout = engine.simple_layout(ordinary, style);
    scene::draw_layout_first_line(canvas, root, &layout, cursor, y, Some(text_color));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_span_that_splits_utf8() {
        assert!(valid_spans("計画", [Span { start: 1, end: 3 }]).is_empty());
        assert_eq!(valid_spans("計画", [Span { start: 0, end: 3 }]).len(), 1);
    }

    #[test]
    fn ellipsis_keeps_and_translates_the_match() {
        let mut engine = TextEngine::new();
        let text = "a long prefix before the important needle and a long suffix after it";
        let start = text.find("needle").unwrap();
        let (display, span) = elide_around_span(
            &mut engine,
            text,
            Span {
                start,
                end: start + "needle".len(),
            },
            theme::role::meta(),
            140.0,
        );
        let span = span.unwrap();
        assert_eq!(&display[span.start..span.end], "needle");
        assert!(display.starts_with('…'));
        assert!(display.ends_with('…'));
    }

    #[test]
    fn overlapping_spans_merge_but_separate_terms_remain_separate() {
        assert_eq!(
            valid_spans(
                "spec and decision",
                [
                    Span { start: 0, end: 4 },
                    Span { start: 0, end: 2 },
                    Span { start: 5, end: 8 },
                ]
            ),
            [Span { start: 0, end: 4 }, Span { start: 5, end: 8 }]
        );
    }

    #[test]
    fn multi_span_elision_keeps_all_nearby_terms() {
        let mut engine = TextEngine::new();
        let text = "Specifications and decisions connected to execution";
        let (display, spans) = elide_around_spans(
            &mut engine,
            text,
            [
                Span { start: 0, end: 4 },
                Span { start: 15, end: 18 },
                Span { start: 19, end: 27 },
            ],
            theme::role::meta(),
            360.0,
        );
        let matches: Vec<_> = spans
            .iter()
            .map(|span| &display[span.start..span.end])
            .collect();
        assert_eq!(matches, ["Spec", "and", "decision"]);
    }

    #[test]
    fn measured_run_keeps_authored_trailing_space() {
        let mut engine = TextEngine::new();
        assert!(
            measured_width(&mut engine, "Future ", theme::role::row())
                > measured_width(&mut engine, "Future", theme::role::row())
        );
    }
}
