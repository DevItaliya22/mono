// SPDX-License-Identifier: MPL-2.0

use super::*;

fn token(n: u8) -> String {
    format!("grant-{n}")
}

/// **The anchor, stated as a test.** A grant is minted for a pid this Agent
/// launched and for nothing else — not for a pid it never started, and not for
/// a connection whose pid the platform could not report. A mechanism that fell
/// back to trusting the caller when it could not check would be a mechanism
/// that never checked.
#[test]
fn a_grant_is_minted_only_for_a_process_this_agent_launched() {
    let mut authority = GrantAuthority::default();
    authority.launched(4321, 1_000);

    assert!(authority.mint(Some(4321), 1_000, token(1)).is_ok());
    assert_eq!(
        authority.mint(Some(9999), 1_000, token(2)),
        Err(Refusal::NotLaunchedHere),
        "a grant was minted for a process this Agent never started"
    );
    assert_eq!(
        authority.mint(None, 1_000, token(3)),
        Err(Refusal::NotLaunchedHere),
        "an unreadable peer pid was treated as trusted"
    );
}

/// A grant is spent by being validated. The Gateway redeems it once at
/// admission; a token that leaks afterwards buys nothing.
#[test]
fn a_grant_is_good_once_and_then_never_again() {
    let mut authority = GrantAuthority::default();
    authority.launched(7, 0);
    let grant = authority.mint(Some(7), 0, token(1)).unwrap();

    assert!(authority.validate(grant.expose(), 1));
    assert!(
        !authority.validate(grant.expose(), 2),
        "a replayed grant was accepted a second time"
    );
    assert!(!authority.validate("never-minted", 3));
}

/// Expiry is enforced on the way in, not by a timer: the authority has no
/// clock of its own, so every answer is computed against the moment it is
/// asked.
#[test]
fn a_grant_stops_working_when_its_window_closes() {
    let mut authority = GrantAuthority::default();
    authority.launched(7, 0);
    let grant = authority.mint(Some(7), 0, token(1)).unwrap();
    assert_eq!(grant.expires_at_millis, GRANT_LIFETIME_MILLIS);

    assert!(!authority.validate(grant.expose(), GRANT_LIFETIME_MILLIS + 1));
    // And an expired one is gone rather than lingering to be swept later.
    assert!(!authority.validate(grant.expose(), 1));
}

/// The launched table is bounded. A person who opens and closes windows all
/// day must not grow it without end, and the oldest launch is the one whose
/// window is least likely to still be asking for anything.
#[test]
fn the_launched_table_forgets_the_oldest_rather_than_growing() {
    let mut authority = GrantAuthority::default();
    for index in 0..LAUNCHED_CAPACITY {
        authority.launched(index as u32 + 1, index as i64);
    }
    assert!(authority.was_launched_here(1));
    authority.launched(u32::MAX, LAUNCHED_CAPACITY as i64);
    assert!(
        !authority.was_launched_here(1),
        "the oldest launch was not the one evicted"
    );
    assert!(authority.was_launched_here(u32::MAX));
    assert!(authority.was_launched_here(LAUNCHED_CAPACITY as u32));
}

/// The token never reaches a log, a trace, or a test failure through `Debug` —
/// the same rule `LinkCode` and the launch bootstrap follow, and for the same
/// reason: whole wire messages get formatted routinely.
#[test]
fn a_grant_does_not_print_its_token() {
    let mut authority = GrantAuthority::default();
    authority.launched(7, 0);
    let grant = authority.mint(Some(7), 0, "the-secret".into()).unwrap();
    let printed = format!("{grant:?}");
    assert!(!printed.contains("the-secret"), "{printed}");
    assert!(printed.contains("expires_at_millis"), "{printed}");
}
