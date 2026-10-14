// SPDX-License-Identifier: MPL-2.0

//! What can be proved without a host: the bootstrap's shape and what its
//! `Debug` withholds, the endpoint's root qualification, the tool set a
//! delegated launch allows, and the dependency fence. The composition
//! itself is proved by `apps/koru-agent/tests/delegate_worker_process.rs`,
//! with a real Agent on a temporary root — including a run that registers
//! a result through its own endpoint and completes.

use std::path::{Path, PathBuf};

use crate::*;

mod dependency;

fn bootstrap() -> LaunchBootstrap {
    LaunchBootstrap::new(
        "launch-1".into(),
        koru_channel::segment().into(),
        PathBuf::from("/tmp/koru-delegate-root"),
        PathBuf::from("/tmp/koru-delegate-root/store"),
        true,
        "00000000-0000-4000-8000-0000000000e1".into(),
        "AUTHORITY-SENTINEL".into(),
    )
}

#[test]
fn the_bootstrap_round_trips_and_its_debug_withholds_the_authority() {
    let bootstrap = bootstrap();
    let json = serde_json::to_string(&bootstrap).unwrap();
    assert!(
        json.contains("AUTHORITY-SENTINEL"),
        "the Agent hands it down"
    );
    let back: LaunchBootstrap = serde_json::from_str(&json).unwrap();
    assert_eq!(back.authority(), "AUTHORITY-SENTINEL");
    assert_eq!(back.launch_id, "launch-1");
    assert!(back.explicit_root);
    let rendered = format!("{back:?}");
    assert!(!rendered.contains("AUTHORITY-SENTINEL"), "{rendered}");
    assert!(
        !rendered.contains("koru-delegate-root"),
        "a path is private: {rendered}"
    );
}

#[test]
fn the_endpoint_is_rooted_and_distinct_from_the_session_hosts() {
    let a = endpoint(
        koru_channel::segment(),
        Some(Path::new("/tmp/koru-delegate-a")),
    )
    .unwrap();
    let b = endpoint(
        koru_channel::segment(),
        Some(Path::new("/tmp/koru-delegate-b")),
    )
    .unwrap();
    assert_ne!(a.name(), b.name());
    assert_eq!(a.service(), koru_ipc::DELEGATE);
    let sessions = sessions::endpoint(
        koru_channel::segment(),
        Some(Path::new("/tmp/koru-delegate-a")),
    )
    .unwrap();
    assert_ne!(a.name(), sessions.name());
}

/// The six tool names a delegated launch allows are exactly the six the
/// run endpoint publishes. They are spelled in two places — here, and in
/// the gateway's `run_mcp::tools` — because the two crates do not see each
/// other; a name that drifts is a tool the model is offered and the
/// endpoint refuses, or one the preamble names and the launch forbids.
#[test]
fn the_launch_allows_the_six_tools_the_run_endpoint_publishes() {
    assert_eq!(
        authority::RUN_TOOLS.to_vec(),
        vec![
            "attach_to_run",
            "create",
            "create_version",
            "get",
            "read",
            "search"
        ]
    );
}

#[test]
fn the_offer_negotiates_the_delegation_family() {
    let offer = sessions::offer();
    assert_eq!(
        offer.versions.max,
        koru_session_protocol::MAX_PROTOCOL_VERSION
    );
    assert!(
        offer
            .capabilities
            .supports(koru_session_protocol::capability::DELEGATION)
    );
    assert!(
        offer
            .capabilities
            .supports(koru_session_protocol::capability::MANAGED_SESSIONS)
    );
}
