// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn probing_an_endpoint_nobody_holds_is_false() {
    // The stale-socket check rests on this: connecting is the liveness test,
    // and it must say "no" without blocking when nothing is there.
    //
    // Scoped to a root that does not exist rather than to a channel that
    // happens to be quiet — the listener suite binds channels in a parallel
    // thread of this same process, so a channel-scoped version would pass or
    // fail on scheduling.
    assert!(!probe(&crate::test_support::nowhere("probe")));
}

#[test]
fn nothing_listening_reads_as_not_running_rather_than_as_a_failure() {
    // The distinction matters to every caller: `NotRunning` is the one thing
    // worth starting a service over, and anything else would still be true
    // after a spawn.
    let endpoint = crate::test_support::nowhere("not-running");
    match connect(&endpoint) {
        Err(e) => {
            assert_eq!(e.code(), "ipc_not_running");
            assert!(e.is_retryable());
            // It names *which* endpoint, or a wrong one is undiagnosable.
            assert!(e.to_string().contains(endpoint.name()), "{e}");
        }
        Ok(_) => panic!("something answered on an endpoint that cannot exist"),
    }
}

#[test]
fn a_name_can_be_rendered_for_a_test_that_speaks_the_wire_by_hand() {
    // Public on purpose: a suite that has to send a malformed frame needs the
    // platform-native name, and a `#[cfg(test)]` back door in shipping code is
    // how that need used to be met.
    let endpoint = crate::test_support::nowhere("name");
    assert!(name_of(&endpoint).is_ok());
}
