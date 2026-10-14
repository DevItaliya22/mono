// SPDX-License-Identifier: MPL-2.0

use super::*;

fn machine() -> Machine {
    Machine::starting(
        RestartBudget {
            max_restarts: 3,
            window: Duration::from_secs(60),
        },
        Backoff {
            initial: Duration::from_millis(100),
            max: Duration::from_secs(2),
        },
    )
}

#[test]
fn a_ready_probe_moves_starting_to_ready() {
    let mut m = machine();
    assert_eq!(m.on(Event::Ready, Instant::now()), Action::None);
    assert_eq!(m.state(), State::Ready);
}

#[test]
fn a_crash_schedules_a_backoff_and_the_elapse_respawns() {
    let mut m = machine();
    let now = Instant::now();
    m.on(Event::Ready, now);
    let action = m.on(Event::Exited, now);
    assert_eq!(action, Action::ScheduleBackoff(Duration::from_millis(100)));
    assert_eq!(m.state(), State::Backoff);
    assert!(!m.child_alive());
    assert_eq!(m.on(Event::BackoffElapsed, now), Action::Spawn);
    assert_eq!(m.state(), State::Starting);
}

#[test]
fn consecutive_crashes_grow_the_delay_and_a_ready_resets_it() {
    let mut m = machine();
    let now = Instant::now();
    assert_eq!(
        m.on(Event::Exited, now),
        Action::ScheduleBackoff(Duration::from_millis(100))
    );
    m.on(Event::BackoffElapsed, now);
    assert_eq!(
        m.on(Event::Exited, now),
        Action::ScheduleBackoff(Duration::from_millis(200))
    );
    m.on(Event::BackoffElapsed, now);
    m.on(Event::Ready, now);
    // A successful Ready reset the growth: the next crash backs off from
    // the initial delay again.
    assert_eq!(
        m.on(Event::Exited, now),
        Action::ScheduleBackoff(Duration::from_millis(100))
    );
}

#[test]
fn the_budget_exhausts_visibly_inside_its_window() {
    let mut m = machine();
    let now = Instant::now();
    for _ in 0..3 {
        m.on(Event::Exited, now);
        m.on(Event::BackoffElapsed, now);
    }
    assert_eq!(m.on(Event::Exited, now), Action::None);
    assert_eq!(m.state(), State::Exhausted);
}

#[test]
fn crashes_outside_the_window_do_not_count_against_the_budget() {
    let mut m = machine();
    let start = Instant::now();
    for round in 0..5 {
        let now = start + Duration::from_secs(round * 120);
        assert!(matches!(
            m.on(Event::Exited, now),
            Action::ScheduleBackoff(_)
        ));
        m.on(Event::BackoffElapsed, now);
        m.on(Event::Ready, now);
    }
    assert_eq!(m.state(), State::Ready);
}

#[test]
fn an_unhealthy_child_is_killed_and_the_exit_drives_the_restart() {
    let mut m = machine();
    let now = Instant::now();
    m.on(Event::Ready, now);
    assert_eq!(m.on(Event::Unhealthy, now), Action::Kill);
    // Still Ready until the corpse is reaped — no replacement can race it.
    assert_eq!(m.state(), State::Ready);
    assert!(matches!(
        m.on(Event::Exited, now),
        Action::ScheduleBackoff(_)
    ));
}

#[test]
fn stop_beats_a_pending_backoff() {
    let mut m = machine();
    let now = Instant::now();
    m.on(Event::Exited, now);
    assert_eq!(m.state(), State::Backoff);
    assert_eq!(m.on(Event::Stop, now), Action::None);
    assert_eq!(m.state(), State::Stopped);
    // The stale timer firing later does nothing.
    assert_eq!(m.on(Event::BackoffElapsed, now), Action::None);
    assert_eq!(m.state(), State::Stopped);
}

#[test]
fn stop_on_a_live_child_stops_it_and_the_exit_finishes() {
    let mut m = machine();
    let now = Instant::now();
    m.on(Event::Ready, now);
    assert_eq!(m.on(Event::Stop, now), Action::StopChild);
    assert_eq!(m.state(), State::Stopping);
    assert_eq!(m.on(Event::Exited, now), Action::None);
    assert_eq!(m.state(), State::Stopped);
}

#[test]
fn a_hold_keeps_the_child_and_a_resume_returns_to_ready() {
    let mut m = machine();
    let now = Instant::now();
    m.on(Event::Ready, now);
    assert_eq!(m.on(Event::Hold, now), Action::None);
    assert_eq!(m.state(), State::Stopping);
    assert!(m.child_alive());
    assert_eq!(m.on(Event::Resume, now), Action::None);
    assert_eq!(m.state(), State::Ready);
}

#[test]
fn a_child_that_dies_inside_a_hold_is_not_restarted_until_resume() {
    let mut m = machine();
    let now = Instant::now();
    m.on(Event::Ready, now);
    m.on(Event::Hold, now);
    assert_eq!(m.on(Event::Exited, now), Action::None);
    assert_eq!(m.state(), State::Stopping);
    assert!(!m.child_alive());
    assert_eq!(m.on(Event::Resume, now), Action::Spawn);
    assert_eq!(m.state(), State::Starting);
}

#[test]
fn a_stop_during_a_hold_ends_the_held_child() {
    let mut m = machine();
    let now = Instant::now();
    m.on(Event::Ready, now);
    m.on(Event::Hold, now);
    assert_eq!(m.on(Event::Stop, now), Action::StopChild);
    assert_eq!(m.on(Event::Exited, now), Action::None);
    assert_eq!(m.state(), State::Stopped);
}

/// **C1, first reproducing order: `Ready → Hold → Exited → Stop`.**
///
/// The child crashed inside the update window. The old machine emitted
/// `StopChild` for a child that had already been reaped, so nothing happened,
/// no `Exited` followed, `Stopped` was never reached, the acknowledgement
/// timed out and the public stop's control-thread join blocked for good.
#[test]
fn a_held_stop_with_no_live_child_is_finished_already() {
    let mut m = machine();
    let now = Instant::now();
    m.on(Event::Ready, now);
    m.on(Event::Hold, now);
    m.on(Event::Exited, now);
    assert!(!m.child_alive());
    assert_eq!(
        m.on(Event::Stop, now),
        Action::None,
        "a stop asked the control thread to stop a child that had been reaped"
    );
    assert_eq!(
        m.state(),
        State::Stopped,
        "the stop never completed, so its acknowledgement could only time out"
    );
}

/// **C1, second reproducing order: `Exited → Backoff → Hold → Stop`.**
///
/// No child died inside the window here — the hold arrived while the machine
/// was already in backoff, so the respawn was suppressed and the stop found
/// nothing running. Same wedge, different route in.
#[test]
fn a_held_stop_during_a_suppressed_backoff_is_finished_already() {
    let mut m = machine();
    let now = Instant::now();
    m.on(Event::Exited, now);
    assert_eq!(m.state(), State::Backoff);
    m.on(Event::Hold, now);
    assert_eq!(m.state(), State::Stopping);
    assert!(!m.child_alive());
    assert_eq!(m.on(Event::Stop, now), Action::None);
    assert_eq!(m.state(), State::Stopped);
}

/// **C1's abort/resume continuation.** Both orders have to be revivable: an
/// install that stopped a held worker and then aborted must bring the service
/// back rather than leave it suppressed for the rest of the run.
#[test]
fn a_held_stop_with_no_live_child_still_resumes_after_an_abort() {
    for order in ["ready-hold-exited", "exited-hold"] {
        let mut m = machine();
        let now = Instant::now();
        match order {
            "ready-hold-exited" => {
                m.on(Event::Ready, now);
                m.on(Event::Hold, now);
                m.on(Event::Exited, now);
            }
            _ => {
                m.on(Event::Exited, now);
                m.on(Event::Hold, now);
            }
        }
        m.on(Event::Stop, now);
        assert_eq!(m.state(), State::Stopped, "{order}");
        assert_eq!(m.on(Event::Resume, now), Action::Spawn, "{order}");
        assert_eq!(m.state(), State::Starting, "{order}");
        assert!(m.child_alive(), "{order}");
        // The budget started over with the resume, so the fresh child gets a
        // full allowance rather than inheriting the crash that preceded it.
        assert!(matches!(
            m.on(Event::Exited, now),
            Action::ScheduleBackoff(_)
        ));
    }
}

#[test]
fn resume_out_of_exhausted_starts_a_fresh_budget() {
    let mut m = machine();
    let now = Instant::now();
    for _ in 0..3 {
        m.on(Event::Exited, now);
        m.on(Event::BackoffElapsed, now);
    }
    m.on(Event::Exited, now);
    assert_eq!(m.state(), State::Exhausted);
    assert_eq!(m.on(Event::Resume, now), Action::Spawn);
    assert_eq!(m.state(), State::Starting);
    // The budget genuinely restarted: three more crashes fit again.
    assert!(matches!(
        m.on(Event::Exited, now),
        Action::ScheduleBackoff(_)
    ));
}

#[test]
fn resume_after_a_stop_revives_the_worker_with_a_fresh_budget() {
    // The stop-then-abort install path: sessions were stopped for the
    // swap, the swap failed, and the service must come back rather than
    // stay suppressed until the next Agent restart.
    let mut m = machine();
    let now = Instant::now();
    m.on(Event::Ready, now);
    m.on(Event::Stop, now);
    m.on(Event::Exited, now);
    assert_eq!(m.state(), State::Stopped);
    assert_eq!(m.on(Event::Resume, now), Action::Spawn);
    assert_eq!(m.state(), State::Starting);
}

#[test]
fn a_hold_while_nothing_runs_suppresses_the_pending_respawn() {
    let mut m = machine();
    let now = Instant::now();
    m.on(Event::Exited, now);
    assert_eq!(m.state(), State::Backoff);
    assert_eq!(m.on(Event::Hold, now), Action::None);
    assert_eq!(m.state(), State::Stopping);
    assert_eq!(m.on(Event::BackoffElapsed, now), Action::None);
    assert_eq!(m.state(), State::Stopping);
    assert_eq!(m.on(Event::Resume, now), Action::Spawn);
}
