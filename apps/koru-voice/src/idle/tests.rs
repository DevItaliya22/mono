// SPDX-License-Identifier: MPL-2.0

use super::*;

fn policy(unload: u64, exit: u64) -> IdlePolicy {
    IdlePolicy {
        unload_after: Duration::from_secs(unload),
        exit_after: Duration::from_secs(exit),
    }
}

#[test]
fn the_defaults_are_five_minutes_and_half_an_hour() {
    let default = IdlePolicy::default();
    assert_eq!(default.unload_after, Duration::from_secs(300));
    assert_eq!(default.exit_after, Duration::from_secs(1800));
}

#[test]
fn a_loaded_model_is_dropped_at_the_unload_deadline() {
    let policy = policy(300, 1800);
    assert!(matches!(
        policy.decide(Duration::from_secs(299), true),
        IdleAction::Wait(_)
    ));
    assert_eq!(
        policy.decide(Duration::from_secs(300), true),
        IdleAction::UnloadModel
    );
    // Nothing to unload: the deadline passes without an action.
    assert!(matches!(
        policy.decide(Duration::from_secs(300), false),
        IdleAction::Wait(_)
    ));
}

#[test]
fn the_process_ends_at_the_exit_deadline_whether_or_not_a_model_is_loaded() {
    let policy = policy(300, 1800);
    assert_eq!(
        policy.decide(Duration::from_secs(1800), false),
        IdleAction::Exit
    );
    assert_eq!(
        policy.decide(Duration::from_secs(1800), true),
        IdleAction::Exit
    );
}

/// Exit is checked first, so a policy whose exit deadline is inside its unload
/// deadline behaves as its numbers say rather than unloading a model out of a
/// process that is about to end.
#[test]
fn exit_wins_over_unload_when_the_numbers_overlap() {
    assert_eq!(
        policy(600, 60).decide(Duration::from_secs(120), true),
        IdleAction::Exit
    );
}

/// Zero disables, which is what a developer keeping a loaded worker alive
/// across a morning of experiments sets.
#[test]
fn zero_means_never() {
    let never = policy(0, 0);
    let a_day = Duration::from_secs(86_400);
    assert!(matches!(never.decide(a_day, true), IdleAction::Wait(_)));
    assert!(matches!(
        policy(0, 1800).decide(Duration::from_secs(600), true),
        IdleAction::Wait(_)
    ));
    assert_eq!(
        policy(300, 0).decide(Duration::from_secs(600), true),
        IdleAction::UnloadModel
    );
}

/// **The wait is the exact remaining time, which is what makes this a deadline
/// rather than a timer.** The inference thread blocks for precisely this, so an
/// idle worker wakes once at the moment something is due.
#[test]
fn the_wait_is_the_time_until_the_next_thing_is_due() {
    let policy = policy(300, 1800);
    assert_eq!(
        policy.decide(Duration::from_secs(290), true),
        IdleAction::Wait(Duration::from_secs(10))
    );
    // With nothing loaded the unload deadline is not a reason to wake, so the
    // wait is capped only by the coarse ceiling below.
    assert_eq!(
        policy.decide(Duration::from_secs(290), false),
        IdleAction::Wait(Duration::from_secs(60))
    );
    assert_eq!(
        policy.decide(Duration::from_secs(1780), false),
        IdleAction::Wait(Duration::from_secs(20))
    );
}

/// Never zero: a zero wait turns `recv_timeout` into a spin, which is the one
/// failure this module is arranged to avoid.
#[test]
fn the_wait_is_never_zero() {
    let policy = policy(300, 1800);
    // A nanosecond short of the exit deadline, with nothing loaded, so the
    // remaining time rounds to nothing.
    let almost = Duration::from_secs(1800) - Duration::from_nanos(1);
    match policy.decide(almost, false) {
        IdleAction::Wait(wait) => assert!(wait >= Duration::from_millis(1)),
        other => panic!("expected a wait, got {other:?}"),
    }
}

/// The two variables are read, and a value that is not a whole number of
/// seconds is ignored rather than fatal — this decides when memory is released,
/// and refusing to start a worker over it would be the worse failure.
#[test]
fn a_variable_that_is_not_a_number_falls_back_to_the_default() {
    let name = koru_env::name::KORU_VOICE_IDLE_UNLOAD;
    let fallback = IdlePolicy::DEFAULT_UNLOAD;
    assert_eq!(parse_seconds(name, None, fallback), fallback);
    assert_eq!(parse_seconds(name, Some("   "), fallback), fallback);
    assert_eq!(parse_seconds(name, Some("5m"), fallback), fallback);
    assert_eq!(parse_seconds(name, Some("-1"), fallback), fallback);
    assert_eq!(
        parse_seconds(name, Some(" 42 "), fallback),
        Duration::from_secs(42)
    );
    assert_eq!(parse_seconds(name, Some("0"), fallback), Duration::ZERO);
}
