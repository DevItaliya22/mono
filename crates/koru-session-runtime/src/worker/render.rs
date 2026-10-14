// SPDX-License-Identifier: MPL-2.0

use std::time::Instant;

use koru_session_observability::{Counter, SessionStats};
use koru_session_protocol::{
    AttentionReason, AttentionState, ErrorCode, RawOutputChunk, RenderDelta, RenderSnapshot,
    RenderUpdate, RowIndex, RowRange, SequencedSessionEvent, SessionEvent, SessionId, StreamGap,
    StreamSequence, ViewportRequest,
};
use koru_terminal_emulator::{EmulatorEvent, EmulatorUpdate, MAX_PAGE_ROWS};

use crate::stream::PushError;

use super::{RawOutputUpdate, SessionEventUpdate, State};

pub(super) fn apply_emulator_update(
    state: &mut State,
    session_id: SessionId,
    old_visible: RowRange,
    update: EmulatorUpdate,
    force_snapshot: bool,
    stats: &SessionStats,
) {
    let mut attention_changed = false;
    for event in update.events {
        match event {
            EmulatorEvent::TitleChanged(title) => publish_event_locked(
                state,
                session_id,
                SessionEvent::TitleChanged {
                    title: title.unwrap_or_default(),
                },
                stats,
            ),
            EmulatorEvent::Bell => {
                state.summary.attention = AttentionState {
                    unread_count: state.summary.attention.unread_count.saturating_add(1),
                    requires_user: true,
                    reason: Some(AttentionReason::Bell),
                };
                attention_changed = true;
                publish_event_locked(state, session_id, SessionEvent::Bell, stats);
                publish_event_locked(
                    state,
                    session_id,
                    SessionEvent::AttentionChanged {
                        attention: state.summary.attention.clone(),
                    },
                    stats,
                );
            }
            EmulatorEvent::PtyWrite(_)
            | EmulatorEvent::CursorBlinkingChanged
            | EmulatorEvent::ActionBlocked(_)
            | EmulatorEvent::EventsTruncated => {}
        }
    }

    if update.retention_rebased {
        let terminal = state
            .raw_terminal_mut()
            .expect("emulator updates require a raw terminal");
        terminal.terminal_generation = terminal
            .terminal_generation
            .next()
            .unwrap_or(terminal.terminal_generation);
    }
    let Some(mut controller) = state.controller.take() else {
        mark_background_attention(state, session_id, stats);
        return;
    };
    let Some(mut render) = controller.render.take() else {
        state.controller = Some(controller);
        mark_background_attention(state, session_id, stats);
        return;
    };
    if !render.queue.is_alive() {
        controller.render = None;
        state.controller = Some(controller);
        mark_background_attention(state, session_id, stats);
        return;
    }

    let following_tail = render.request.rows == old_visible;
    let needs_snapshot = force_snapshot
        || update.retention_rebased
        || (following_tail
            && update
                .visible_range
                .is_some_and(|range| range != old_visible));
    if needs_snapshot {
        if force_snapshot {
            // Raw resize changes the grid and the live serialization bound as
            // one operation. Keeping the attachment-time column count makes
            // growth look successful by size while every row and later delta
            // is still silently truncated to the old width.
            render.request.columns = state
                .raw_terminal()
                .expect("emulator updates require a raw terminal")
                .emulator
                .size()
                .columns;
        }
        render.request.terminal_generation = state
            .raw_terminal()
            .expect("emulator updates require a raw terminal")
            .terminal_generation;
        if following_tail {
            render.request.rows = state
                .raw_terminal()
                .expect("emulator updates require a raw terminal")
                .emulator
                .visible_range();
        }
        match snapshot(state, render.request, session_id, stats) {
            Ok(snapshot) => {
                render.queue.clear();
                let _ = render.queue.try_push(RenderUpdate::Snapshot { snapshot });
                render.gapped = false;
            }
            Err(_) => render.gapped = true,
        }
        controller.render = Some(render);
        state.controller = Some(controller);
        return;
    }
    if render.gapped {
        controller.render = Some(render);
        state.controller = Some(controller);
        return;
    }

    let viewport = effective_range(
        render.request,
        state
            .raw_terminal()
            .expect("emulator updates require a raw terminal")
            .emulator
            .retained_range(),
    );
    let mut rows = Vec::new();
    for row in update.damaged_rows {
        if in_range(row, viewport)
            && let Ok(window) = state
                .raw_terminal()
                .expect("emulator updates require a raw terminal")
                .emulator
                .scrollback_page(RowRange {
                    first: row,
                    count: 1,
                })
            && let Some(mut terminal_row) = window.rows.into_iter().next()
        {
            terminal_row
                .cells
                .truncate(usize::from(render.request.columns));
            rows.push(terminal_row);
        }
    }
    let has_change =
        !rows.is_empty() || update.cursor.is_some() || update.modes.is_some() || attention_changed;
    if has_change {
        let (terminal_generation, next_render_sequence) = {
            let terminal = state
                .raw_terminal_mut()
                .expect("emulator updates require a raw terminal");
            terminal.render_sequence = next(terminal.render_sequence);
            (terminal.terminal_generation, terminal.render_sequence)
        };
        let delta = RenderDelta {
            session_id,
            terminal_generation,
            viewport_generation: render.request.viewport_generation,
            sequence: next_render_sequence,
            rows,
            cursor: update.cursor,
            modes: update.modes,
            attention: attention_changed.then(|| state.summary.attention.clone()),
        };
        stats.add(Counter::ViewportRowsSent, delta.rows.len() as u64);
        stats.add(Counter::Deltas, 1);
        let actual = delta.sequence;
        match render.queue.try_push(RenderUpdate::Delta { delta }) {
            Ok(depth) => stats.observe_high_water(Counter::QueueHighWater, depth as u64),
            Err(PushError::Full(_)) => {
                let expected = render
                    .queue
                    .first()
                    .and_then(render_sequence)
                    .unwrap_or(actual);
                let gap = RenderUpdate::Gap {
                    gap: StreamGap {
                        session_id,
                        terminal_generation,
                        viewport_generation: render.request.viewport_generation,
                        expected,
                        actual,
                        resnapshot_required: true,
                    },
                };
                let _ = render.queue.replace_with(gap);
                render.gapped = true;
                stats.add(Counter::GappedSubscribers, 1);
            }
            Err(PushError::Closed(_)) => {
                controller.render = None;
                state.controller = Some(controller);
                stats.add(Counter::DroppedSubscribers, 1);
                return;
            }
        }
    }
    controller.render = Some(render);
    state.controller = Some(controller);
}

pub(super) fn snapshot(
    state: &mut State,
    request: ViewportRequest,
    session_id: SessionId,
    stats: &SessionStats,
) -> Result<RenderSnapshot, ErrorCode> {
    let started = Instant::now();
    let terminal = state.raw_terminal()?;
    let range = effective_range(request, terminal.emulator.retained_range());
    let mut rows = terminal
        .emulator
        .scrollback_page(range)
        .map_err(map_emulator)?
        .rows;
    for row in &mut rows {
        row.cells.truncate(usize::from(request.columns));
    }
    stats.add(Counter::SnapshotPageRows, rows.len() as u64);
    stats.record_timing_micros(
        koru_session_observability::Timing::SnapshotEncode,
        micros(started.elapsed()),
    );
    Ok(RenderSnapshot {
        session_id,
        terminal_generation: terminal.terminal_generation,
        viewport_generation: request.viewport_generation,
        sequence: terminal.render_sequence,
        viewport: range,
        size: terminal.emulator.size(),
        rows,
        cursor: terminal.emulator.cursor(),
        modes: terminal.emulator.modes(),
        attention: state.summary.attention.clone(),
    })
}

pub(super) fn validate_viewport(
    state: &State,
    request: ViewportRequest,
    allow_newer: bool,
) -> Result<(), ErrorCode> {
    request.validate()?;
    let terminal = state.raw_terminal()?;
    if request.terminal_generation != terminal.terminal_generation {
        return Err(ErrorCode::StaleGeneration);
    }
    if usize::from(request.rows.count).saturating_add(usize::from(request.overscan_rows) * 2)
        > usize::from(MAX_PAGE_ROWS)
        || request.columns > terminal.emulator.size().columns
    {
        return Err(ErrorCode::InvalidSize);
    }
    let retained = terminal.emulator.retained_range();
    let end = request
        .rows
        .first
        .0
        .checked_add(u64::from(request.rows.count))
        .ok_or(ErrorCode::InvalidSize)?;
    if request.rows.first < retained.first || end > retained.end_exclusive.0 {
        return Err(if allow_newer {
            ErrorCode::StaleGeneration
        } else {
            ErrorCode::InvalidRequest
        });
    }
    Ok(())
}

fn effective_range(
    request: ViewportRequest,
    retained: koru_terminal_emulator::RetainedRange,
) -> RowRange {
    let before = u64::from(request.overscan_rows).min(request.rows.first.0 - retained.first.0);
    let first = RowIndex(request.rows.first.0 - before);
    let requested_end = request.rows.first.0 + u64::from(request.rows.count);
    let after = u64::from(request.overscan_rows)
        .min(retained.end_exclusive.0.saturating_sub(requested_end));
    RowRange {
        first,
        count: u16::try_from(before + u64::from(request.rows.count) + after)
            .unwrap_or(MAX_PAGE_ROWS),
    }
}

pub(super) fn publish_event_locked(
    state: &mut State,
    session_id: SessionId,
    event: SessionEvent,
    stats: &SessionStats,
) {
    state.event_sequence = next(state.event_sequence);
    let event = SequencedSessionEvent {
        session_id,
        sequence: state.event_sequence,
        event,
    };
    state.event_subscribers.retain_mut(|subscriber| {
        if subscriber.gapped || !subscriber.queue.is_alive() {
            return subscriber.queue.is_alive();
        }
        match subscriber
            .queue
            .try_push(SessionEventUpdate::Event(event.clone()))
        {
            Ok(depth) => {
                stats.observe_high_water(Counter::QueueHighWater, depth as u64);
                true
            }
            Err(PushError::Full(_)) => {
                let expected = subscriber
                    .queue
                    .first()
                    .and_then(event_sequence)
                    .unwrap_or(event.sequence);
                let _ = subscriber.queue.replace_with(SessionEventUpdate::Gap {
                    expected,
                    actual: event.sequence,
                });
                subscriber.gapped = true;
                stats.add(Counter::GappedSubscribers, 1);
                true
            }
            Err(PushError::Closed(_)) => false,
        }
    });
}

pub(super) fn publish_output(
    state: &mut State,
    session_id: SessionId,
    bytes: &[u8],
    stats: &SessionStats,
) {
    // The sequence describes process output, not subscriber activity. It must
    // advance while nobody listens so a later cursor-based subscription can
    // prove that its preceding snapshot went stale.
    state.output_sequence = next(state.output_sequence);
    if state.output_subscribers.is_empty() {
        return;
    }
    let actual = state.output_sequence;
    state.output_subscribers.retain_mut(|subscriber| {
        if subscriber.gapped || !subscriber.queue.is_alive() {
            return subscriber.queue.is_alive();
        }
        let update = RawOutputUpdate::Chunk(RawOutputChunk {
            session_id,
            sequence: actual,
            bytes: bytes.to_vec(),
        });
        match subscriber.queue.try_push(update) {
            Ok(depth) => {
                stats.observe_high_water(Counter::QueueHighWater, depth as u64);
                true
            }
            Err(PushError::Full(_)) => {
                let expected = subscriber
                    .queue
                    .first()
                    .and_then(output_sequence)
                    .unwrap_or(actual);
                let _ = subscriber
                    .queue
                    .replace_with(RawOutputUpdate::Gap { expected, actual });
                subscriber.gapped = true;
                stats.add(Counter::GappedSubscribers, 1);
                true
            }
            Err(PushError::Closed(_)) => false,
        }
    });
}

fn mark_background_attention(state: &mut State, session_id: SessionId, stats: &SessionStats) {
    let changed = state.summary.attention.reason != Some(AttentionReason::BackgroundOutput);
    state.summary.attention.unread_count = state.summary.attention.unread_count.saturating_add(1);
    state.summary.attention.reason = Some(AttentionReason::BackgroundOutput);
    if changed {
        publish_event_locked(
            state,
            session_id,
            SessionEvent::AttentionChanged {
                attention: state.summary.attention.clone(),
            },
            stats,
        );
    }
}

fn in_range(row: RowIndex, range: RowRange) -> bool {
    row.0 >= range.first.0 && row.0 < range.first.0 + u64::from(range.count)
}

fn render_sequence(update: RenderUpdate) -> Option<StreamSequence> {
    match update {
        RenderUpdate::Snapshot { snapshot } => Some(snapshot.sequence),
        RenderUpdate::Delta { delta } => Some(delta.sequence),
        RenderUpdate::Gap { gap } => Some(gap.expected),
    }
}

fn output_sequence(update: RawOutputUpdate) -> Option<StreamSequence> {
    match update {
        RawOutputUpdate::Chunk(chunk) => Some(chunk.sequence),
        RawOutputUpdate::Gap { expected, .. } => Some(expected),
    }
}

fn event_sequence(update: SessionEventUpdate) -> Option<StreamSequence> {
    match update {
        SessionEventUpdate::Event(event) => Some(event.sequence),
        SessionEventUpdate::Gap { expected, .. } => Some(expected),
    }
}

fn next(value: StreamSequence) -> StreamSequence {
    value.next().unwrap_or(value)
}

pub(super) fn map_emulator(error: koru_terminal_emulator::EmulatorError) -> ErrorCode {
    use koru_terminal_emulator::EmulatorError;
    match error {
        EmulatorError::InvalidSize
        | EmulatorError::EmptyRange
        | EmulatorError::PageTooLarge { .. } => ErrorCode::InvalidSize,
        EmulatorError::RowOutsideRetention => ErrorCode::StaleGeneration,
    }
}

pub(super) fn micros(duration: std::time::Duration) -> u64 {
    duration.as_micros().try_into().unwrap_or(u64::MAX)
}
