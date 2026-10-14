// SPDX-License-Identifier: MPL-2.0

//! What the crate root decides: where the agent lives, and what it binds.

use super::*;

#[test]
fn an_explicit_root_is_carried_through_rather_than_resolved() {
    let paths = paths_for(Some(std::path::PathBuf::from("/tmp/koru-agent"))).unwrap();
    assert_eq!(paths.root(), std::path::Path::new("/tmp/koru-agent"));
    assert!(!paths.owns_the_machine());
}

#[test]
fn no_root_resolves_the_channels_own_and_owns_the_machine() {
    let Ok(paths) = paths_for(None) else {
        return; // no local data directory on this machine; nothing to compare
    };
    assert!(paths.owns_the_machine());
    // **The channel's root, one level above the store's.** Asserting
    // `store_root()` here is what let the agent's device identity live inside
    // the store's data directory: both sides of the comparison were wrong
    // together, which is the shape of every bug an equality test cannot catch.
    assert_eq!(Some(paths.root()), koru_channel::channel_root().as_deref());
    assert_ne!(Some(paths.root()), koru_channel::store_root().as_deref());
}

#[test]
fn the_agent_and_the_private_store_claim_are_two_endpoints_on_one_channel() {
    // **The reason `koru-ipc` is parameterized over a service at all.** If the
    // name did not carry it, whichever bound first would be dialled by clients
    // of the other — and the handshake, not the transport, would be the only
    // thing that noticed.
    let Ok(paths) = paths_for(None) else {
        return;
    };
    let agent = endpoint(&paths).unwrap();
    let store = koru_ipc::Endpoint::for_service(koru_ipc::STORE, segment()).unwrap();
    assert_ne!(agent.name(), store.name());
    assert!(agent.name().contains("koru-agent"), "{}", agent.name());
}

#[test]
fn an_explicit_root_gets_an_endpoint_of_its_own() {
    // So the gate can run several agents at once. Without it a `--root` agent
    // would have no discoverable endpoint at all, which is the one
    // configuration no client could find.
    let plain = paths_for(None).ok().map(|p| endpoint(&p).unwrap());
    let a = endpoint(&paths_for(Some("/tmp/koru-a".into())).unwrap()).unwrap();
    let b = endpoint(&paths_for(Some("/tmp/koru-b".into())).unwrap()).unwrap();
    assert_ne!(a.name(), b.name());
    if let Some(plain) = plain {
        assert_ne!(a.name(), plain.name());
    }
}

#[test]
fn resolving_the_ordinary_channel_root_meets_unrooted_product_clients() {
    let Some(channel_root) = koru_channel::channel_root() else {
        return;
    };
    let ordinary = paths_for(None).unwrap();
    let resolved = paths_for(Some(channel_root)).unwrap();
    assert_eq!(endpoint(&ordinary).unwrap(), endpoint(&resolved).unwrap());
    assert_eq!(
        session_endpoint(&ordinary).unwrap(),
        session_endpoint(&resolved).unwrap()
    );
}

#[test]
fn the_session_host_endpoint_is_rooted_and_distinct_from_the_agent() {
    let a = paths_for(Some("/tmp/koru-session-a".into())).unwrap();
    let b = paths_for(Some("/tmp/koru-session-b".into())).unwrap();
    let a_session = session_endpoint(&a).unwrap();
    let b_session = session_endpoint(&b).unwrap();

    assert_ne!(a_session.name(), b_session.name());
    assert_ne!(a_session.name(), endpoint(&a).unwrap().name());
    assert_eq!(a_session.service(), koru_ipc::SESSION_HOST);
}

#[test]
fn the_delegate_endpoint_is_rooted_and_distinct_from_the_session_host_and_the_agent() {
    let a = paths_for(Some("/tmp/koru-delegate-a".into())).unwrap();
    let b = paths_for(Some("/tmp/koru-delegate-b".into())).unwrap();
    let a_delegate = delegate_endpoint(&a).unwrap();
    let b_delegate = delegate_endpoint(&b).unwrap();
    assert_ne!(a_delegate.name(), b_delegate.name());
    assert_ne!(a_delegate.name(), endpoint(&a).unwrap().name());
    assert_ne!(a_delegate.name(), session_endpoint(&a).unwrap().name());
    assert_eq!(a_delegate.service(), koru_ipc::DELEGATE);
}

#[test]
fn the_segment_is_this_builds_own() {
    // Asked rather than written down: the agent, the shell and the store are
    // built by one cargo invocation, so a `--release` build moves all three
    // together and a hardcoded string would move none of them.
    assert_eq!(segment(), koru_channel::segment());
    assert!(koru_channel::SEGMENTS.contains(&segment()));
}
