// SPDX-License-Identifier: MPL-2.0

use super::*;

fn ids() -> (ProfileId, WorkspaceId) {
    (ProfileId::new(), WorkspaceId::new())
}

#[test]
fn a_late_authenticated_pid_is_attached_and_the_entry_ends_at_eof() {
    let (profile, workspace) = ids();
    let mut registry = Registry::new();
    registry.hello(ConnectionId(1), None, profile, workspace);
    registry.pid_arrived(ConnectionId(1), Some(84));
    assert_eq!(registry.route(profile, workspace).unwrap().pid, Some(84));
    assert!(registry.gone(ConnectionId(1)));
    assert!(registry.route(profile, workspace).is_none());
    assert!(registry.is_empty());
}

#[test]
fn a_hello_becomes_an_entry_and_an_open_finds_it() {
    let (profile, workspace) = ids();
    let mut registry = Registry::new();
    registry.hello(ConnectionId(1), Some(42), profile, workspace);

    let found = registry.route(profile, workspace).expect("the only shell");
    assert_eq!(found.connection, ConnectionId(1));
    assert_eq!(found.pid, Some(42));
}

#[test]
fn a_workspace_nobody_holds_routes_nowhere() {
    // Exact-workspace routing stays strict. Profile-level reuse is a separate
    // call so a new-window open cannot steal another workspace's shell.
    let (profile, workspace) = ids();
    let mut registry = Registry::new();
    registry.hello(ConnectionId(1), None, profile, workspace);
    assert!(registry.route(profile, WorkspaceId::new()).is_none());
    assert!(registry.route(ProfileId::new(), workspace).is_none());
}

#[test]
fn route_profile_picks_the_most_recently_active_shell() {
    let (profile, a) = ids();
    let b = WorkspaceId::new();
    let mut registry = Registry::new();
    registry.hello(ConnectionId(1), None, profile, a);
    registry.hello(ConnectionId(2), None, profile, b);
    registry.active(ConnectionId(1));
    assert_eq!(
        registry.route_profile(profile).unwrap().connection,
        ConnectionId(1)
    );
    registry.active(ConnectionId(2));
    assert_eq!(
        registry.route_profile(profile).unwrap().connection,
        ConnectionId(2)
    );
}

#[test]
fn changing_workspace_updates_the_entry_and_never_adds_a_second() {
    // A shell switching workspace is still one shell. Adding would leave a
    // ghost holding a workspace nothing is showing, and an `Open` for it would
    // be written into a window that had moved on.
    let (profile, workspace) = ids();
    let elsewhere = WorkspaceId::new();
    let mut registry = Registry::new();
    registry.hello(ConnectionId(1), None, profile, workspace);
    registry.workspace_changed(ConnectionId(1), profile, elsewhere);

    assert_eq!(registry.len(), 1);
    assert!(registry.route(profile, workspace).is_none());
    assert!(registry.route(profile, elsewhere).is_some());
}

#[test]
fn a_workspace_change_from_a_connection_that_never_said_hello_is_ignored() {
    // It has told us nothing routable. Inventing an entry from a partial
    // introduction would put a shell in the table with no pid and no history.
    let (profile, workspace) = ids();
    let mut registry = Registry::new();
    registry.workspace_changed(ConnectionId(9), profile, workspace);
    assert!(registry.is_empty());
}

#[test]
fn the_shell_that_was_active_last_wins() {
    // **The tie-break a person expects.** Two windows on one workspace, and the
    // result belongs in the one they were just looking at.
    let (profile, workspace) = ids();
    let mut registry = Registry::new();
    registry.hello(ConnectionId(1), None, profile, workspace);
    registry.hello(ConnectionId(2), None, profile, workspace);

    registry.active(ConnectionId(1));
    assert_eq!(
        registry.route(profile, workspace).unwrap().connection,
        ConnectionId(1)
    );

    registry.active(ConnectionId(2));
    assert_eq!(
        registry.route(profile, workspace).unwrap().connection,
        ConnectionId(2)
    );

    // ...and back again, because the order is observed rather than remembered
    // once.
    registry.active(ConnectionId(1));
    assert_eq!(
        registry.route(profile, workspace).unwrap().connection,
        ConnectionId(1)
    );
}

#[test]
fn with_nobody_active_the_newest_connection_wins() {
    // Two windows opened and neither focused since. The second key settles it;
    // without one the answer would be whatever the hash map felt like, and the
    // same press would open different windows on different runs.
    let (profile, workspace) = ids();
    let mut registry = Registry::new();
    registry.hello(ConnectionId(1), None, profile, workspace);
    registry.hello(ConnectionId(2), None, profile, workspace);
    assert_eq!(
        registry.route(profile, workspace).unwrap().connection,
        ConnectionId(2)
    );
}

#[test]
fn an_active_shell_beats_a_newer_one_that_has_never_been_focused() {
    // The keys are ordered, not summed: activity outranks recency of
    // connection, however new the other window is.
    let (profile, workspace) = ids();
    let mut registry = Registry::new();
    registry.hello(ConnectionId(1), None, profile, workspace);
    registry.active(ConnectionId(1));
    registry.hello(ConnectionId(2), None, profile, workspace);
    assert_eq!(
        registry.route(profile, workspace).unwrap().connection,
        ConnectionId(1)
    );
}

#[test]
fn a_re_hello_keeps_the_connections_place_in_the_order() {
    // A shell that re-introduces itself has not just connected. Re-stamping
    // `connected_seq` would let a chatty client jump the queue ahead of a
    // window that had been open longer.
    let (profile, workspace) = ids();
    let mut registry = Registry::new();
    registry.hello(ConnectionId(1), None, profile, workspace);
    registry.hello(ConnectionId(2), None, profile, workspace);
    registry.hello(ConnectionId(1), None, profile, workspace);
    assert_eq!(
        registry.route(profile, workspace).unwrap().connection,
        ConnectionId(2)
    );
}

#[test]
fn a_re_hello_keeps_the_connections_activity() {
    // The same argument on the other key: saying hello again is not being
    // brought to the front.
    let (profile, workspace) = ids();
    let mut registry = Registry::new();
    registry.hello(ConnectionId(1), None, profile, workspace);
    registry.active(ConnectionId(1));
    registry.hello(ConnectionId(2), None, profile, workspace);
    registry.hello(ConnectionId(1), None, profile, workspace);
    assert_eq!(
        registry.route(profile, workspace).unwrap().connection,
        ConnectionId(1)
    );
}

#[test]
fn eof_removes_the_entry_and_the_next_open_falls_through() {
    // **The whole of the disconnect story.** No heartbeat, no poll: the reader
    // thread ending is the signal, and after it the workspace routes nowhere —
    // which is what sends an `Open` down the launch path instead of into a
    // socket nobody is holding.
    let (profile, workspace) = ids();
    let mut registry = Registry::new();
    registry.hello(ConnectionId(1), None, profile, workspace);
    assert!(registry.gone(ConnectionId(1)));
    assert!(registry.route(profile, workspace).is_none());
    // Idempotent: a failed write and the reader thread ending both report it,
    // and whichever arrives second must not be an error.
    assert!(!registry.gone(ConnectionId(1)));
}

#[test]
fn one_shell_leaving_does_not_take_the_other_with_it() {
    let (profile, workspace) = ids();
    let mut registry = Registry::new();
    registry.hello(ConnectionId(1), None, profile, workspace);
    registry.hello(ConnectionId(2), None, profile, workspace);
    registry.gone(ConnectionId(2));
    assert_eq!(
        registry.route(profile, workspace).unwrap().connection,
        ConnectionId(1)
    );
}

#[test]
fn activity_on_a_connection_that_has_gone_changes_nothing() {
    // The two arrive on different threads, so `Active` can land just after the
    // socket ended. It must not resurrect an entry.
    let (profile, workspace) = ids();
    let mut registry = Registry::new();
    registry.hello(ConnectionId(1), None, profile, workspace);
    registry.gone(ConnectionId(1));
    registry.active(ConnectionId(1));
    assert!(registry.is_empty());
}

#[test]
fn open_workspaces_are_named_sorted_and_deduplicated() {
    let profile = ProfileId::new();
    let alpha = WorkspaceId::new();
    let beta = WorkspaceId::new();
    let mut registry = Registry::new();
    registry.hello_named(ConnectionId(1), Some(10), profile, beta, "Personal", "Beta");
    registry.hello_named(
        ConnectionId(2),
        Some(20),
        profile,
        alpha,
        "Personal",
        "Alpha",
    );
    registry.hello_named(
        ConnectionId(3),
        Some(30),
        profile,
        alpha,
        "Personal",
        "Alpha",
    );
    registry.active(ConnectionId(2));

    assert_eq!(
        registry.open_workspaces(),
        vec![
            OpenWorkspace {
                connection: ConnectionId(2),
                label: "Alpha — Personal".into(),
            },
            OpenWorkspace {
                connection: ConnectionId(1),
                label: "Beta — Personal".into(),
            },
        ]
    );
}

#[test]
fn foreground_pid_matching_is_unique_or_explicitly_ambiguous() {
    let (profile, workspace) = ids();
    let mut registry = Registry::new();
    assert_eq!(registry.workspace_for_pid(42), WorkspacePidMatch::None);

    registry.hello(ConnectionId(1), Some(42), profile, workspace);
    assert_eq!(
        registry.workspace_for_pid(42),
        WorkspacePidMatch::Unique(ConnectionId(1))
    );

    registry.hello(ConnectionId(2), Some(42), profile, WorkspaceId::new());
    assert_eq!(registry.workspace_for_pid(42), WorkspacePidMatch::Ambiguous);

    registry.gone(ConnectionId(2));
    assert_eq!(
        registry.workspace_for_pid(42),
        WorkspacePidMatch::Unique(ConnectionId(1))
    );
}
