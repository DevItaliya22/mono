// SPDX-License-Identifier: MPL-2.0

use super::*;

use koru_session_protocol::ColumnIndex;

/// The one frame a hit test needs behind it: a grid of the requested size with
/// its viewport at row 0.
fn snapshot(session: SessionId, size: GridSize) -> koru_session_protocol::RenderUpdate {
    koru_session_protocol::RenderUpdate::Snapshot {
        snapshot: koru_session_protocol::RenderSnapshot {
            session_id: session,
            terminal_generation: koru_session_protocol::TerminalGeneration::new(1).unwrap(),
            viewport_generation: koru_session_protocol::ViewportGeneration::new(1).unwrap(),
            sequence: koru_session_protocol::StreamSequence::new(1).unwrap(),
            viewport: RowRange {
                first: RowIndex(0),
                count: size.rows,
            },
            size,
            rows: Vec::new(),
            cursor: koru_session_protocol::CursorState {
                row: RowIndex(0),
                column: ColumnIndex(0),
                shape: koru_session_protocol::CursorShape::Block,
                blinking: false,
            },
            modes: koru_session_protocol::TerminalModes::default(),
            attention: koru_session_protocol::AttentionState::default(),
        },
    }
}

/// A surface over the null interaction: an intent one of these tests provokes
/// is dropped rather than refused, and no backend needs to exist behind it.
fn surface() -> SessionSurface<()> {
    SessionSurface::new(
        &(),
        std::sync::Arc::new(SessionStats::from_env()),
        Lang::En,
        true,
        TerminalCursorPreferences::default(),
    )
}

#[test]
fn a_switch_that_opens_raw_keeps_the_previous_frame_and_names_its_target_grid() {
    let mut surface = surface();
    let size = GridSize {
        columns: 80,
        rows: 24,
    };
    surface.begin_switch(true, true, Some(size));
    assert!(surface.raw_switch_pending);
    assert!(surface.raw_switch_previous_managed);
    assert_eq!(surface.pending_raw_size, Some(size));
    assert_eq!(
        surface.pending_raw_attachment, None,
        "the attachment is only known when the host answers"
    );

    surface.clear_raw_switch();
    assert!(!surface.raw_switch_pending);
    assert!(!surface.raw_switch_previous_managed);
    assert_eq!(surface.pending_raw_size, None);
    assert_eq!(surface.pending_raw_attachment, None);
}

#[test]
fn a_switch_that_does_not_open_raw_drops_the_previous_presentation() {
    let mut surface = surface();
    surface.begin_switch(false, true, None);
    assert!(!surface.raw_switch_pending);
    assert!(
        !surface.raw_switch_previous_managed,
        "there is no wait to keep a previous frame for"
    );
    assert!(surface.view.model().is_none());
}

#[test]
fn a_pending_raw_switch_negotiates_no_viewport_and_answers_no_cell() {
    let mut surface = surface();
    let size = GridSize {
        columns: 80,
        rows: 24,
    };
    surface.begin_switch(true, false, Some(size));
    assert!(!surface.resize_view(false, size));
    assert_eq!(
        surface.terminal_point(Rect::new(0.0, 0.0, 800.0, 600.0), 10.0, 10.0),
        None
    );
}

#[test]
fn an_ineligible_window_leaves_the_caret_visible_with_no_deadline() {
    let mut surface = surface();
    let now = Instant::now();
    assert!(!surface.cursor_blink_active(false));
    let _ = surface.reset_cursor_blink(false, now);
    assert!(surface.cursor_blink_visible);
    assert_eq!(surface.cursor_blink_deadline, None);
    let _ = surface.sync_cursor_blink(false, now);
    assert_eq!(
        surface.cursor_blink_deadline, None,
        "an inactive surface costs no timer wakeups"
    );
}

#[test]
fn stream_gaps_ride_the_frame_that_presented_them_and_only_a_managed_one() {
    let mut surface = surface();
    surface.record_stream_gap();
    surface.record_stream_gap();

    surface.record_frame(None, 1, false);
    let raw = surface
        .take_experience_sample()
        .expect("a drawn frame stages a sample");
    assert_eq!(raw.stream_gaps, None, "a raw frame reports no transcript");
    assert_eq!(surface.stream_gaps_since_present, 2);

    surface.record_frame(None, 1, true);
    let managed = surface
        .take_experience_sample()
        .expect("a drawn frame stages a sample");
    assert_eq!(managed.stream_gaps, Some(2));
    assert_eq!(surface.stream_gaps_since_present, 0);
}

#[test]
fn a_switch_timer_survives_every_frame_that_kept_the_previous_session() {
    let mut surface = surface();
    surface.begin_switch(true, false, None);
    surface.switch_started = Some(Instant::now());

    surface.record_frame(None, 1, false);
    let held = surface.take_experience_sample().expect("a staged sample");
    assert_eq!(
        held.switch_present_ms, None,
        "nothing of the new session has been presented yet"
    );
    assert!(surface.switch_started.is_some());

    surface.clear_raw_switch();
    surface.record_frame(None, 1, false);
    let presented = surface.take_experience_sample().expect("a staged sample");
    assert!(presented.switch_present_ms.is_some());
    assert!(surface.switch_started.is_none());
}

/// **The grid a session is created in is the grid it runs in.**
///
/// `split` takes the composer band off the pane only once the session says
/// `Running`, so `grid_size` answers a taller pane for every moment before
/// that — which is every moment a launch or an attach is measured in. A
/// session created at *that* size has `COMPOSER_HEIGHT` of rows taken away
/// from it the instant it starts, and a shell that has already printed its
/// banner repaints into the smaller grid with the difference in scrollback:
/// the leading gap and the live scrollbar on a session that has printed once.
///
/// `running_grid_size` is the one that does not move, and it is what a launch,
/// an attach and a retarget all measure. This row is the whole rule in two
/// assertions, so a future rewrite that collapses the two back into one fails
/// here rather than on somebody's screen.
#[test]
fn the_grid_a_session_is_created_in_is_the_grid_it_runs_in() {
    let mut surface = surface();
    surface.bounds = Rect::new(0.0, 0.0, 960.0, 720.0);
    assert!(surface.composer_visible);

    let launched = surface.running_grid_size();
    assert_eq!(
        surface.status,
        PaneStatus::Empty,
        "measured before anything has been named, which is when a launch is"
    );
    assert!(
        surface.grid_size().rows > launched.rows,
        "the pane a launch is measured from really is the taller one"
    );

    // The one transition the session makes on its own, and the only thing that
    // moves the band.
    surface.status = PaneStatus::Running;
    assert_eq!(
        surface.running_grid_size(),
        launched,
        "the grid the session was created in did not move under it"
    );
    assert_eq!(
        surface.grid_size(),
        launched,
        "and once it is running the two questions have one answer, so every \
         later resize agrees with the creation size"
    );
    assert_eq!(
        surface.grid_size_for(surface.split().0),
        launched,
        "the pane it draws in is the pane it was sized for"
    );
}

/// **The pane's padding is grid arithmetic, not a draw offset.**
///
/// The first column used to touch the pane divider. A small inset on each side
/// fixes that only if the columns the session is *created and resized* with are
/// measured from the width that is left: a PTY told it owns the pane's full
/// width wraps its lines behind the padding, and the two columns it thinks it
/// has are pixels the surface never draws. Both halves of the two-size rule ask
/// the same inset question, so a later change that insets only the drawing
/// fails here rather than on somebody's screen.
#[test]
fn the_grid_is_measured_from_the_padded_content_and_not_from_the_pane() {
    let mut surface = surface();
    surface.bounds = Rect::new(0.0, 0.0, 960.0, 720.0);
    let inset = koru_session_view::CONTENT_INSET_X;
    let (cell_w, cell_h) = surface.view.cell_size().unwrap_or((8.0, 18.0));

    for pane in [surface.split().0, surface.running_split().0] {
        assert_eq!(
            pane.w, surface.bounds.w,
            "the pane itself is not inset — the background still reaches the divider"
        );
        let content = SessionSurface::<()>::content_rect(pane);
        assert_eq!(content.x, pane.x + inset);
        assert_eq!(content.w, pane.w - inset * 2.0);
        assert_eq!(
            surface.grid_size_for(pane).columns,
            ((content.w / cell_w).floor() as u16).max(1),
            "the columns come from the padded content"
        );
        assert_eq!(
            surface.grid_size_for(pane).rows,
            ((pane.h / cell_h).floor() as u16).max(1),
            "and the inset is horizontal only, so the rows are the pane's"
        );
    }

    assert!(
        surface.running_grid_size().columns < (surface.bounds.w / cell_w).floor() as u16,
        "the padded grid is genuinely narrower than the pane's own, so this row \
         would catch an inset that never reached the arithmetic"
    );

    // A pane narrower than its own padding still names a grid rather than
    // underflowing one: `max(0.0)` on the width, `max(1)` on the columns.
    surface.bounds = Rect::new(0.0, 0.0, inset, 40.0);
    assert_eq!(SessionSurface::<()>::content_rect(surface.split().0).w, 0.0);
    assert_eq!(surface.grid_size().columns, 1);
}

/// **A click lands on the glyph under the pointer.** The grid draws one inset
/// right of the pane, so a hit test measured from the pane would select the
/// column left of the one clicked, and a press in the padding would select
/// column 0 instead of nothing.
#[test]
fn pointer_hit_testing_starts_at_the_padded_content_edge() {
    let mut surface = surface();
    let mut engine = TextEngine::new();
    surface.bounds = Rect::new(0.0, 0.0, 960.0, 720.0);
    surface.view.prepare_style(&mut engine, 1.0);
    let (cell_w, _) = surface.view.cell_size().expect("the cell was measured");
    let pane = surface.running_split().0;
    let size = surface.running_grid_size();
    surface
        .view
        .apply(snapshot(SessionId::mint(), size))
        .expect("a first snapshot");

    let inset = koru_session_view::CONTENT_INSET_X;
    assert_eq!(
        surface.terminal_point(pane, pane.x + inset - 1.0, pane.y + 1.0),
        None,
        "the padding belongs to no cell"
    );
    let first = surface
        .terminal_point(pane, pane.x + inset + cell_w * 0.5, pane.y + 1.0)
        .expect("the first column begins at the padded edge");
    assert_eq!(first.column, ColumnIndex(0));
    assert_eq!(first.row, RowIndex(0));
    let second = surface
        .terminal_point(pane, pane.x + inset + cell_w * 1.5, pane.y + 1.0)
        .expect("the second column is one cell further right");
    assert_eq!(second.column, ColumnIndex(1));
}

/// **A composer toggled under a switch in flight retargets it.** The band is
/// part of the grid a launch was measured for, so moving it has to move what
/// the sized-snapshot gate is waiting for — or the tab waits forever for a
/// frame at a size nobody asked for.
#[test]
fn toggling_the_composer_mid_switch_retargets_the_gate_instead_of_stranding_it() {
    let mut surface = surface();
    surface.bounds = Rect::new(0.0, 0.0, 960.0, 720.0);
    let requested = surface.running_grid_size();
    surface.begin_switch(true, false, Some(requested));
    surface.pending_raw_attachment = Some((SessionId::mint(), 1));

    surface.composer_visible = false;
    assert!(surface.renegotiate_grid(), "the target moved");
    let retargeted = surface
        .pending_raw_size
        .expect("a pending switch always names a size");
    assert_eq!(retargeted, surface.running_grid_size());
    assert!(
        retargeted.rows > requested.rows,
        "the band it gave back is rows the session may now use"
    );
}

/// **Watching replaces the composer, not sits beside it** — and the caret
/// goes with it.
///
/// The band and the composer are drawn from the same rect, so a surface can
/// never present a person a place to type on a session another connection
/// controls. Read-only is *said* rather than merely enforced: the sentence is
/// in the band, and the transcript is told it may not be acted on.
#[test]
fn a_watched_session_swaps_the_composer_for_the_read_only_band() {
    let mut surface = surface();
    surface.bounds = Rect::new(0.0, 0.0, 800.0, 600.0);
    surface.status = PaneStatus::Running;
    surface.active = true;
    surface.window_focused = true;
    surface.composer_visible = true;

    let (_, composer) = surface.split();
    assert_eq!(composer.h, koru_session_view::COMPOSER_HEIGHT);
    assert!(surface.blink_eligible(), "an ordinary session has a caret");

    assert!(surface.set_watching(crate::Watching::Yes));
    let (grid, band) = surface.split();
    assert_eq!(band.h, crate::watching::FOOTER_H);
    assert_eq!(
        grid.h + band.h,
        600.0,
        "the band is carved out of the pane, never drawn over it"
    );
    assert!(
        !surface.blink_eligible(),
        "no caret on a session this window is only reading"
    );
    assert!(
        surface.managed.read_only(),
        "and no approval or cancel control either"
    );
    assert_eq!(surface.focus, SurfaceFocus::Terminal, "focus left the band");

    // And leaving read-only gives the composer back, untouched.
    assert!(surface.set_watching(crate::Watching::No));
    assert_eq!(surface.split().1.h, koru_session_view::COMPOSER_HEIGHT);
    assert!(!surface.managed.read_only());
    assert!(surface.blink_eligible());
}

/// **The surface asks; it never claims.** A press on *Take the session*
/// records a request for the host to perform on the connection that owns the
/// session — and the surface stays read-only until the host says otherwise
/// (§7.4). Once asked there is no control left to press (ADR-354).
#[test]
fn taking_the_session_is_a_request_and_the_surface_claims_nothing() {
    let mut surface = surface();
    surface.bounds = Rect::new(0.0, 0.0, 800.0, 600.0);
    surface.status = PaneStatus::Running;
    surface.set_watching(crate::Watching::Yes);

    let band = surface.split().1;
    let mut engine = koru_draw::text::TextEngine::new();
    let laid = crate::watching::layout(band, &mut engine, Lang::En, surface.watching());
    let control = laid.action.expect("a control to take the session");

    assert!(
        !surface.press_watch_band(band.x + 4.0, band.centre().1),
        "a press on the sentence asks for nothing"
    );
    assert!(!surface.take_takeover_request());

    let (x, y) = control.centre();
    assert!(surface.press_watch_band(x, y));
    assert_eq!(
        surface.watching(),
        crate::Watching::Yes,
        "the surface does not promote itself; the host reports the handover"
    );
    assert!(surface.take_takeover_request(), "the host drains it once");
    assert!(!surface.take_takeover_request(), "and only once");

    // Waiting: still read-only, and nothing left to press.
    assert!(surface.set_watching(crate::Watching::Asked));
    assert_eq!(surface.split().1.h, crate::watching::FOOTER_H);
    assert!(!surface.press_watch_band(x, y), "the control is gone");
    assert!(!surface.activate_watch_control());
    assert!(!surface.take_takeover_request());
}

/// The band's control is reachable from the keyboard, and only while there
/// is one: focus cannot be parked on a control that is not laid out.
#[test]
fn the_bands_control_is_keyboard_reachable_only_while_it_exists() {
    let mut surface = surface();
    surface.bounds = Rect::new(0.0, 0.0, 800.0, 600.0);
    surface.status = PaneStatus::Running;

    assert!(
        !surface.set_watch_focused(true),
        "there is no band on an ordinary session"
    );
    assert!(!surface.watch_focused());

    surface.set_watching(crate::Watching::Yes);
    assert!(surface.set_watch_focused(true));
    assert!(surface.watch_focused());
    assert!(surface.activate_watch_control());
    assert!(surface.take_takeover_request());

    surface.set_watching(crate::Watching::Asked);
    assert!(
        !surface.watch_focused(),
        "focus does not survive the control it was on"
    );
}
