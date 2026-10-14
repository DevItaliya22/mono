// SPDX-License-Identifier: MPL-2.0

//! **What the host publishes and composes, asserted against the process that
//! ships it.**
//!
//! Every case here is a property of the *host process*, not of the router: it
//! survives a `cargo test` of `koru-api-gateway-core` unchanged and would still
//! break if the composition, the published record, or the process boundary
//! moved. That is exactly the class of regression the Gateway's move into
//! `koru-agent.exe --worker gateway` could have introduced, which is what this
//! file was written to catch — it was the before-picture while the standalone
//! `koru-api-gateway.exe` still existed, and it is now the after-picture, run
//! against the only host there is.
//!
//! Nothing new is built here. The fixture is [`gateway_process`], the one the
//! other four gateway integration files use, because root isolation is a
//! property of the rendezvous endpoint name and needs no harness of its own.

mod gateway_process;

use koru_api_client::Client;
use koru_api_discovery::GatewayHello;
use std::path::Path;

use gateway_process::Gateway;

fn hello() -> koru_api_discovery::ClientHello {
    gateway_process::hello("gateway-host-parity")
}

/// Start a real host on `root` and return it with the record it published.
fn start(root: &Path) -> (Gateway, GatewayHello) {
    let mut gateway = Gateway::start(root);
    let found = gateway.published(&hello());
    (gateway, found)
}

/// **The published record's contract half survives a replaced child; its
/// identity half does not, and must not.**
///
/// A client finds the Gateway by dialling the root-qualified rendezvous and
/// reading one `GatewayHello`. Two halves of that record mean different things
/// and a rehost can break them in opposite directions:
///
/// - the *contract* — API version, channel, build version, capabilities — is
///   what a client decides it can talk to. It must be identical across a
///   restart, or a window that reconnected would find a different product.
/// - the *identity* — instance, endpoint, credential — is what says this is a
///   different process. It must change, or a client could not tell a
///   replacement from the host it was already talking to, and would keep using
///   an admission the new host never issued.
///
/// `supervisor_launch_id` is the third thing, and it belongs to the identity
/// half: a supervised child publishes the launch it was started *for*, so a
/// replacement started for a new launch must publish a new one. It was pinned
/// as absent while the standalone host was the one under test; the host that
/// remains is supervised, and this is the same assertion said about it.
#[test]
fn the_published_record_keeps_its_contract_and_replaces_its_identity_across_a_restart() {
    let root = tempfile::tempdir().unwrap();
    let (gateway, first) = start(root.path());
    let serving = Client::from_gateway(first.clone()).expect("build a client from the record");
    serving.store().status().expect("the first host serves");

    gateway.terminate();
    let (_replacement, second) = start(root.path());

    assert_eq!(first.api_version, second.api_version);
    assert_eq!(first.segment, second.segment);
    assert_eq!(first.gateway_version, second.gateway_version);
    assert_eq!(
        first.capabilities, second.capabilities,
        "the replacement published a different capability set"
    );
    let first_launch = first
        .supervisor_launch_id
        .as_deref()
        .expect("a supervised child publishes the launch it was started for");
    let second_launch = second
        .supervisor_launch_id
        .as_deref()
        .expect("the replacement publishes its own launch");
    assert_ne!(
        first_launch, second_launch,
        "the replaced child was still the one being published"
    );

    assert_ne!(
        first.instance_id, second.instance_id,
        "a replacement that reuses the instance id is indistinguishable from the host it replaced"
    );
    assert_ne!(
        first.session.expose(),
        second.session.expose(),
        "the replacement re-served its predecessor's admission"
    );

    // The predecessor's whole record is dead, and it does not matter which
    // half refuses first: the address may be gone or the bearer may be
    // unknown. The invariant is that an admission does not carry across a
    // replacement, not which error says so.
    assert!(
        serving.store().status().is_err(),
        "a client holding the replaced host's record was still served"
    );
    Client::from_gateway(second.clone())
        .expect("build a client from the replacement's record")
        .store()
        .status()
        .expect("the replacement serves");
}

/// **The Tasks query port answers through the host.**
///
/// The port lives in the Store worker this process embeds, and the Gateway
/// reaches it through the fallback that forwards everything it does not own
/// itself. Two things a rehost could break are on that path and neither is
/// visible to an in-process router test: whether the host's Store worker
/// installed a task service at all, and whether the Gateway still forwards to
/// it under the caller's ordinary scopes.
///
/// So the assertion is about the *answer's provenance*, not about task
/// semantics — those are pinned against the Store in
/// `apps/koru-store-server/tests/tasks_query.rs`, over a corpus this test has
/// no business building. A store with no projector answers `limit: 0` and no
/// definitions; a real port echoes the clamped limit and ships the system
/// attribute registry, which is what tells the two apart.
#[test]
fn the_tasks_query_port_answers_through_the_host() {
    let root = tempfile::tempdir().unwrap();
    let (_gateway, found) = start(root.path());
    let client = Client::from_gateway(found).expect("build gateway client");
    let profile = client
        .store()
        .create_profile("Tasks host", None, None, Some("en"))
        .unwrap();
    let workspace = client
        .store()
        .create_workspace(profile.id, "Workspace")
        .unwrap()
        .id;

    let query = koru_store_wire::tasks::TaskQuery {
        workspaces: vec![workspace],
        limit: 25,
        ..Default::default()
    };
    let response = client
        .store()
        .query_tasks(&query)
        .expect("the host answered the tasks query");
    assert_eq!(
        response.limit, 25,
        "the answer did not come from an installed task port"
    );
    assert!(
        !response.definitions.is_empty(),
        "the system attribute registry did not come back with the answer"
    );
    assert!(
        response.rows.is_empty(),
        "an empty workspace answered with task rows"
    );
}

/// **The desktop host composes no account service, and says so.**
///
/// The account service is the hosted Koru Cloud's, which is not in this
/// repository, and `GatewayState` has no slot for one (the optional
/// `AccountService` it once held, which no host ever filled, was removed). The
/// capability list is where that shows, so a machine-readable "no" is
/// published on every hello. The route-level half — that the gateway does
/// not mount the account routes at all (docs/developers/API.md) —
/// is `account_paths_are_not_served_and_the_capability_is_never_published`
/// in `koru-api-gateway-core`.
#[test]
fn the_desktop_host_publishes_no_account_capability() {
    let root = tempfile::tempdir().unwrap();
    let (_gateway, found) = start(root.path());
    assert!(
        !found
            .capabilities
            .iter()
            .any(|capability| capability == koru_account_wire::CAPABILITY),
        "the desktop host composed the hosted account service: {:?}",
        found.capabilities
    );
    // Said against a host that is otherwise fully composed, so this is an
    // absence rather than a host that published nothing.
    assert!(found.capabilities.iter().any(|c| c == "store"));
    assert!(found.capabilities.iter().any(|c| c == "events"));
}

/// **The host opens `artifacts.db` and says so.** The Artifact Store is
/// composed in the gateway worker (D13) beside the AI worker, and the one
/// machine-readable sign that the composition kept it is the `artifacts`
/// capability on the published hello — the same place the account
/// capability's absence is asserted above. Under `--root` the database
/// lands beside the root's `store/`, so a rehearsal never touches a real
/// installation's artifacts.
#[test]
fn the_desktop_host_publishes_the_artifacts_capability_and_keeps_its_database_under_the_root() {
    let root = tempfile::tempdir().unwrap();
    let (_gateway, found) = start(root.path());
    assert!(
        found.capabilities.iter().any(|c| c == "artifacts"),
        "artifacts.db did not open in the host: {:?}",
        found.capabilities
    );
    assert!(
        root.path().join("artifacts").join("artifacts.db").exists(),
        "the database is under the rooted channel root"
    );
}
