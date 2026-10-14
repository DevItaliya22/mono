// SPDX-License-Identifier: MPL-2.0

//! What this composition decides, as opposed to what it delegates.
//!
//! The transfer itself is `koru-blob-sync`'s and is tested there against a
//! real object worker; the walk's rule is tested against the approved usage
//! corpus. What is only decidable here is the wiring: the account facts read
//! from the control host, the allowlist built from *this device's*
//! environment rather than from the message, and the announcement waking the
//! request that was waiting for it.

use super::*;

use koru_sync_control::{
    CONTROL_PROTOCOL_VERSION, ControlHello, ControlRequest, ControlResponse, ControlSessionId,
    cloud::{CLOUD_BLOB_ACCESS_VERSION, CloudBlobAccessV1},
};

fn access_value(origin: &str, region: &str, expires_at_millis: i64) -> CloudBlobAccessV1 {
    CloudBlobAccessV1 {
        version: CLOUD_BLOB_ACCESS_VERSION,
        api_origin: origin.into(),
        region: region.into(),
        access_token: "an-account-bearer-token".into(),
        expires_at_millis,
    }
}

/// A real address for a fixed set of bytes, without pulling a hasher into
/// this crate: the Store already owns the one that decides a blob's name, and
/// nothing here needs the bytes — only a well-formed, stable address.
fn address(seed: &[u8]) -> BlobHash {
    let mut digest = String::new();
    for byte in seed.iter().cycle().take(32) {
        digest.push_str(&format!("{byte:02x}"));
    }
    BlobHash::from_digest("b3", &digest).expect("an address")
}

fn opened() -> (ControlHost, ControlSessionId) {
    let mut host = ControlHost::default();
    let hello = ControlHello {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        session_id: ControlSessionId::new(),
        local_device: koru_device::DeviceId::new(),
    };
    host.begin_session(&hello).expect("a control session");
    (host, hello.session_id)
}

/// A control host with blob access already applied.
fn host_with(access: CloudBlobAccessV1) -> Arc<Mutex<ControlHost>> {
    let (mut host, session) = opened();
    let account = koru_device::cloud::CloudId::from_bytes(*uuid::Uuid::new_v4().as_bytes());
    let response = host.handle(
        session,
        ControlRequest::ReplaceCloudBlobAccess {
            control_session_id: session,
            sequence: 1,
            account_id: account,
            account_generation: 1,
            access: Box::new(access),
        },
    );
    assert!(
        matches!(response, ControlResponse::CloudBlobAccessApplied { .. }),
        "{response:?}"
    );
    Arc::new(Mutex::new(host))
}

fn access_port(host: Arc<Mutex<ControlHost>>, development: Option<&str>) -> ControlAccess {
    ControlAccess {
        host,
        local: Mutex::new(None),
        development: Mutex::new(development.map(str::to_owned)),
    }
}

/// The account's reach is read from the control host, region and all.
#[test]
fn the_account_facts_come_from_the_control_host() {
    let host = host_with(access_value("https://us.api.koru.do/", "us", i64::MAX));
    let facts = access_port(host, None)
        .access(WorkspaceId::new())
        .expect("access");
    assert_eq!(facts.api_origin.as_str(), "https://us.api.koru.do/");
    assert_eq!(facts.region, Region::Us);
    assert_eq!(facts.bearer, "an-account-bearer-token");
    // The compiled-in US set, and nothing a message could have added to it.
    let origins: Vec<&str> = facts.byte_origins.origins().collect();
    assert_eq!(origins, ["https://data.koru.do", "https://us.data.koru.do"]);
}

/// **Expiry is checked here rather than discovered from a `401`.** A token
/// this device knows is dead is a request it should not make: the refusal
/// would look like a fenced account to everything downstream.
#[test]
fn an_expired_token_is_no_access_at_all() {
    let host = host_with(access_value("https://us.api.koru.do/", "us", 1));
    assert!(
        access_port(host, None).access(WorkspaceId::new()).is_none(),
        "an expired token was handed to a transfer"
    );
}

/// **The allowlist is this device's, never the message's.** A control message
/// that could name a byte origin would be a control message that could name
/// where somebody's photograph goes.
#[test]
fn the_byte_origins_are_built_here_and_a_local_stack_is_explicit() {
    let host = host_with(access_value("http://127.0.0.1:8080/", "us", i64::MAX));
    let facts = access_port(host.clone(), Some("http://127.0.0.1:8081"))
        .access(WorkspaceId::new())
        .expect("access");
    assert!(
        facts
            .byte_origins
            .admit("http://127.0.0.1:8081/v1/objects/abc")
            .is_ok()
    );
    // And the production set still stands beside it.
    assert!(
        facts
            .byte_origins
            .admit("https://us.data.koru.do/v1/objects/abc")
            .is_ok()
    );

    // A value that is not understood is not run: the local allowance is
    // dropped and the production set is what remains.
    let refused = access_port(host, Some("https://evil.example.com"))
        .access(WorkspaceId::new())
        .expect("access");
    assert!(
        refused
            .byte_origins
            .admit("https://evil.example.com/v1/objects/abc")
            .is_err()
    );
    assert!(
        refused
            .byte_origins
            .admit("https://us.data.koru.do/v1/objects/abc")
            .is_ok()
    );
}

/// **A phone names its local stack in process, and it is the same door.**
/// Android has no environment to read `KORU_CLOUD_BLOB_BYTE_ORIGINS` from, so
/// the value arrives from its `BuildConfig` and is installed here — through
/// the one `with_development_origins` the desktop uses, which is why a build
/// that named a public host cannot open a path to one.
///
/// **A refused value stores nothing.** The compiled-in per-region set is what
/// stands, which is the entire reason it is compiled in rather than
/// configured: an allowlist an attacker can configure is one an attacker
/// configures.
#[test]
fn an_in_process_development_origin_is_admitted_only_when_it_is_local() {
    let host = host_with(access_value("http://127.0.0.1:8080/", "us", i64::MAX));
    let port = access_port(host, None);

    // A LAN address is what a phone needs: given loopback it would dial
    // itself, so this is the case a phone actually runs.
    assert_eq!(
        port.install_development_origins("http://192.168.1.118:8081/"),
        Ok(())
    );
    let facts = port.access(WorkspaceId::new()).expect("access");
    assert!(
        facts
            .byte_origins
            .admit("http://192.168.1.118:8081/v1/objects/abc")
            .is_ok()
    );

    // Every shape the allowlist refuses is refused here too — and the public
    // host is the one this whole module exists for.
    for bad in [
        "https://evil.example.com",
        "https://us.data.koru.do.evil.example.com",
        "http://8.8.8.8:8081",
        "http://localhost.example.com:8081",
        "not a url",
        "http://127.0.0.1:8081/collect",
    ] {
        assert!(
            port.install_development_origins(bad).is_err(),
            "{bad} was admitted as a development origin",
        );
        let facts = port.access(WorkspaceId::new()).expect("access");
        assert!(
            facts
                .byte_origins
                .admit(&format!("{bad}/v1/objects/abc"))
                .is_err(),
            "{bad} became somewhere this device would send a file",
        );
        // …and the production set is untouched, so a phone with a mistyped
        // development setting still reaches Koru Cloud.
        assert!(
            facts
                .byte_origins
                .admit("https://us.data.koru.do/v1/objects/abc")
                .is_ok(),
            "a refused development value took the production set with it",
        );
        // The value already installed is still the one in force: a refusal
        // replaces nothing.
        assert!(
            facts
                .byte_origins
                .admit("http://192.168.1.118:8081/v1/objects/abc")
                .is_ok()
        );
    }
}

/// The EU deployment's client holds the EU host and not the US one: a
/// capability minted in one region is refused by the other's worker anyway,
/// and sending the bytes to find that out would already have sent them.
#[test]
fn a_region_selects_its_own_object_hosts() {
    let host = host_with(access_value("https://eu.api.koru.do/", "eu", i64::MAX));
    let facts = access_port(host, None)
        .access(WorkspaceId::new())
        .expect("access");
    assert_eq!(facts.region, Region::Eu);
    let origins: Vec<&str> = facts.byte_origins.origins().collect();
    assert_eq!(origins, ["https://eu.data.koru.do"]);
}

/// A value whose shape could never work is refused at the boundary rather
/// than carried to a service that would refuse it.
#[test]
fn a_control_message_that_could_never_work_is_refused() {
    let (mut host, session) = opened();
    let account = koru_device::cloud::CloudId::from_bytes(*uuid::Uuid::new_v4().as_bytes());
    for (index, bad) in [
        access_value("us.api.koru.do", "us", i64::MAX),
        access_value("http://api.koru.do/", "us", i64::MAX),
        access_value("https://us.api.koru.do/?token=1", "us", i64::MAX),
        access_value("https://us.api.koru.do/", "mars", i64::MAX),
        access_value("https://us.api.koru.do/", "us", 0),
    ]
    .into_iter()
    .enumerate()
    {
        let response = host.handle(
            session,
            ControlRequest::ReplaceCloudBlobAccess {
                control_session_id: session,
                sequence: index as u64 + 1,
                account_id: account,
                account_generation: 1,
                access: Box::new(bad.clone()),
            },
        );
        assert!(
            matches!(response, ControlResponse::Rejected { .. }),
            "{bad:?} was admitted: {response:?}"
        );
    }
}

/// **A bearer token never reaches a log.** The one place a value like this
/// escapes is a `{:?}` somebody added while debugging something else.
#[test]
fn the_token_is_redacted_in_debug() {
    let rendered = format!("{:?}", access_value("https://us.api.koru.do/", "us", 7));
    assert!(rendered.contains("<redacted>"), "{rendered}");
    assert!(!rendered.contains("an-account-bearer-token"), "{rendered}");
}

/// The announcement wakes what was waiting, and a waiter taken **before** the
/// Store was checked cannot miss an arrival that happens in between.
#[tokio::test]
async fn an_arrival_wakes_the_request_that_was_waiting() {
    let arrivals = Arc::new(BlobArrivals::default());
    let workspace = WorkspaceId::new();
    let hash = address(b"a picture");

    let waiter = arrivals.waiter(workspace, &hash);
    let woken = tokio::spawn(async move { waiter.notified().await });
    // Let the waiting task reach its `notified()` before the announcement.
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    arrivals.arrived(workspace, &hash);
    tokio::time::timeout(std::time::Duration::from_secs(5), woken)
        .await
        .expect("the waiting request was never woken")
        .expect("the waiter task");
}

/// `watching` is about what is on screen, and it is answered from the
/// documents a durable change said reference the hash — never from a guess.
#[test]
fn nothing_is_watching_a_hash_no_open_document_references() {
    let arrivals = Arc::new(BlobArrivals::default());
    let workspace = WorkspaceId::new();
    let hash = address(b"a picture");
    assert!(!arrivals.watching(workspace, &hash));
    arrivals.record(workspace, DocumentId::new(), &[hash.clone()]);
    // Recorded, but no rooms handle and therefore no subscribers: the answer
    // is still `false`, and a transfer happens anyway — `watching` decides
    // order, never whether.
    assert!(!arrivals.watching(workspace, &hash));
}

/// A document that stopped referencing a picture stops being a reason to
/// show it.
#[test]
fn a_document_that_dropped_a_reference_no_longer_holds_it() {
    let arrivals = Arc::new(BlobArrivals::default());
    let workspace = WorkspaceId::new();
    let document = DocumentId::new();
    let hash = address(b"a picture");
    arrivals.record(workspace, document, &[hash.clone()]);
    arrivals.record(workspace, document, &[]);
    assert!(
        arrivals
            .documents
            .lock()
            .expect("the recorder's own lock")
            .get(&(workspace, hash.as_str().to_owned()))
            .is_none()
    );
}

/// **A burst is one entry per document, not one per commit.** Six projections
/// of one document drain as one, which is what keeps a burst from becoming a
/// burst of passes without a timer anywhere.
#[test]
fn durable_changes_coalesce_by_document() {
    let changes = BlobChanges::new();
    let workspace = WorkspaceId::new();
    let document = DocumentId::new();
    let other = DocumentId::new();
    for seed in [b"one".as_slice(), b"two".as_slice(), b"three".as_slice()] {
        changes.push(workspace, document, vec![address(seed)]);
    }
    changes.push(workspace, other, vec![address(b"four")]);
    let drained = changes.drain();
    assert_eq!(drained.len(), 2, "{drained:?}");
    // The newest set for a document is the true one, and it keeps the place
    // the document's first change claimed rather than jumping the queue.
    assert_eq!(drained[0].1, document);
    assert_eq!(drained[0].2, vec![address(b"three")]);
    assert_eq!(drained[1].1, other);
    assert!(changes.drain().is_empty(), "a drain left work behind");
}

/// **The in-process seam, for a host that holds the account itself.** A phone
/// has no control session to sequence a `replace_cloud_blob_access` through
/// (D257), so it installs the same value directly — and it wins over the
/// control host, because a host that installed one *is* the account authority
/// in that process.
#[test]
fn an_in_process_host_installs_the_same_value_over_the_control_host() {
    let host = host_with(access_value("https://us.api.koru.do/", "us", i64::MAX));
    let port = access_port(host, None);
    port.install(access_value("https://eu.api.koru.do/", "eu", i64::MAX))
        .expect("a well-shaped value");
    let facts = port.access(WorkspaceId::new()).expect("installed access");
    assert_eq!(facts.api_origin.as_str(), "https://eu.api.koru.do/");
    assert_eq!(facts.region, Region::Eu);
}

/// The same shape check the control command applies, applied here: a value
/// that could never work is refused where it is installed rather than
/// discovered as a `401` on somebody's picture.
#[test]
fn an_in_process_host_cannot_install_a_value_the_control_plane_would_refuse() {
    let port = access_port(
        host_with(access_value("https://us.api.koru.do/", "us", 1)),
        None,
    );
    assert!(
        port.install(access_value("http://us.api.koru.do/", "us", i64::MAX))
            .is_err(),
        "a plaintext non-loopback origin is not an origin",
    );
    assert!(
        port.install(access_value("https://us.api.koru.do/", "mars", i64::MAX))
            .is_err(),
        "a region this build does not know is not a region",
    );
}

/// **Expiry is checked here rather than discovered from a `401`**, for the
/// in-process value exactly as for the control one: an installed token this
/// host knows is dead falls back rather than being sent.
#[test]
fn an_expired_in_process_value_is_not_used() {
    let host = host_with(access_value("https://us.api.koru.do/", "us", i64::MAX));
    let port = access_port(host, None);
    port.install(access_value("https://eu.api.koru.do/", "eu", 1))
        .expect("a well-shaped value");
    let facts = port.access(WorkspaceId::new()).expect("the control host");
    assert_eq!(
        facts.api_origin.as_str(),
        "https://us.api.koru.do/",
        "a dead in-process token was used instead of the live control one",
    );
}

// ------------------------------------------ what a missing picture says (D259)

fn parked_on_a_peer() -> BlobHydrationFailure {
    BlobHydrationFailure {
        code: Code::BlobWaitingForPeer.as_str().to_owned(),
        retryable: true,
        waiting_on: vec!["WIN10-MAIN".to_owned()],
        hold: None,
    }
}

fn pass_held(hold: koru_blob_sync::TransferHold) -> koru_blob_sync::Pass {
    koru_blob_sync::Pass {
        session_refusal: Some(hold.as_str()),
        session_hold: Some(hold),
        ..koru_blob_sync::Pass::default()
    }
}

/// **A hold outranks a peer's silence** (D259).
///
/// On hardware the bytes were on Koru Cloud and this device had never been
/// switched on for the Workspace, so the Cloud pass refused with
/// `mode_refuses_transport` — and the peer path's *"nobody holding this is
/// online"* was returned instead, whole. The placeholder then said so, and a
/// person went and restarted a machine that was not the problem.
#[test]
fn a_transport_mode_hold_outranks_a_peer_that_is_merely_offline() {
    let failure = hydration_failure(
        Some(parked_on_a_peer()),
        &pass_held(koru_blob_sync::TransferHold::TransportMode),
    );
    assert_eq!(
        failure.hold.as_deref(),
        Some("mode_refuses_transport"),
        "the reason a person can act on has to survive the peer's answer",
    );
    // The code is still the peer's: it is what decides whether a caller
    // should come back, and hydration really is still waiting behind it.
    assert_eq!(failure.code, Code::BlobWaitingForPeer.as_str());
    assert_eq!(failure.waiting_on, vec!["WIN10-MAIN".to_owned()]);
}

/// Every hold travels, not only the one that was found.
#[test]
fn each_hold_reaches_the_caller_under_its_own_name() {
    for hold in [
        koru_blob_sync::TransferHold::TransportMode,
        koru_blob_sync::TransferHold::Metered,
        koru_blob_sync::TransferHold::LowData,
        koru_blob_sync::TransferHold::Offline,
        koru_blob_sync::TransferHold::Paused,
    ] {
        let failure = hydration_failure(Some(parked_on_a_peer()), &pass_held(hold));
        assert_eq!(failure.hold.as_deref(), Some(hold.as_str()));
    }
}

/// Cloud sync being off for the Profile is the same kind of answer without
/// being a `TransferHold` — it is the state a person lands in the moment they
/// turn the switch off, and the one they will meet most often.
#[test]
fn cloud_access_being_unavailable_is_carried_as_a_hold_too() {
    let pass = koru_blob_sync::Pass {
        session_refusal: Some(koru_blob_sync::TransferError::NoAccess.code()),
        ..koru_blob_sync::Pass::default()
    };
    let failure = hydration_failure(Some(parked_on_a_peer()), &pass);
    assert_eq!(failure.hold.as_deref(), Some("cloud_access_unavailable"));
}

/// **And a genuine peer wait is still a peer wait.** The placeholder's rung-1
/// sentence is right when it is right; what was wrong was saying it for
/// everything.
#[test]
fn a_peer_that_is_offline_with_no_hold_still_names_the_device() {
    let failure = hydration_failure(Some(parked_on_a_peer()), &koru_blob_sync::Pass::default());
    assert_eq!(failure.hold, None);
    assert_eq!(failure.code, Code::BlobWaitingForPeer.as_str());
    assert_eq!(failure.waiting_on, vec!["WIN10-MAIN".to_owned()]);
}

/// With no peer answer at all, the pass's own refusal is the code as well as
/// the hold — there is nothing else to report.
#[test]
fn with_no_peer_path_the_pass_answers_for_itself() {
    let failure = hydration_failure(None, &pass_held(koru_blob_sync::TransferHold::Metered));
    assert_eq!(failure.code, "network_metered");
    assert_eq!(failure.hold.as_deref(), Some("network_metered"));
    assert!(failure.waiting_on.is_empty());
}

// ------------------------------------------------------ hydration's count

/// A peer that serves the bytes itself, the way a desktop across the room
/// does — straight into the Store, never through the engine.
struct ServingPeer {
    store: Arc<koru_blob_sync::fakes::FakeStore>,
    workspace: WorkspaceId,
    bytes: Vec<u8>,
}

impl BlobHydrationService for ServingPeer {
    fn hydrate(
        &self,
        _workspace: WorkspaceId,
        _hash: BlobHash,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), BlobHydrationFailure>> + Send + 'static>,
    > {
        self.store
            .hold(self.workspace, &self.bytes, "image/png", None);
        Box::pin(async { Ok(()) })
    }
}

/// **Opening a file fetched from a peer moves the count.** Before this the
/// peer path returned before any pass ran, so "0 on this device" stood over a
/// file that had just opened — and since D260 nothing else was going to run
/// a pass and correct it.
#[tokio::test]
async fn a_peer_hydration_is_followed_by_the_pass_that_counts_it() {
    let harness = koru_blob_sync::fakes::Harness::new();
    let bytes: Vec<u8> = (0..3_000u32).map(|i| (i % 251) as u8).collect();
    let hash = koru_blob_sync::fakes::hash_of(&bytes);
    // Cloud holds it (a release left it there); this device does not; a
    // document references it.
    harness
        .plane
        .publish(harness.workspace, &bytes, "image/png");
    harness
        .store
        .document(harness.workspace, &[(&hash, "image/png")]);
    assert_eq!(harness.engine.status(harness.workspace).present, 0);

    let hydration = CloudFallbackHydration {
        peers: Some(Arc::new(ServingPeer {
            store: harness.store.clone(),
            workspace: harness.workspace,
            bytes: bytes.clone(),
        })),
        engine: harness.engine.clone(),
        arrivals: Arc::new(BlobArrivals::default()),
        store: harness.store.clone(),
    };
    hydration
        .hydrate(harness.workspace, hash.clone())
        .await
        .expect("the peer served it");

    // The pass runs as its own task; give it the turns it needs.
    let mut present = 0;
    for _ in 0..200 {
        tokio::task::yield_now().await;
        present = harness.engine.status(harness.workspace).present;
        if present == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(
        present, 1,
        "the count learned about the file the peer served"
    );
    assert_eq!(harness.store.object_bytes(&hash), Some(bytes));
}

/// **A new bearer is an arrival.** The first token a phone pushes after a
/// restart is the stored one, and it may already be dead: the region
/// answers 401, and every Workspace is stamped as having no access. The
/// refreshed token that follows is what lifts those stamps, so installing
/// it must say so even though a live-looking value was already held.
#[test]
fn installing_a_different_bearer_counts_as_access_arriving() {
    let far = now_millis() + 600_000;
    let port = access_port(
        host_with(access_value("https://us.api.koru.do", "us", far)),
        None,
    );
    let mut first = access_value("https://us.api.koru.do", "us", far);
    first.access_token = "token-a".into();
    assert!(
        port.install(first.clone()).unwrap(),
        "nothing was held in process"
    );
    assert!(
        !port.install(first.clone()).unwrap(),
        "the same bearer again is a renewal"
    );
    let mut second = first.clone();
    second.access_token = "token-b".into();
    assert!(
        port.install(second).unwrap(),
        "a different bearer is an arrival"
    );
}
