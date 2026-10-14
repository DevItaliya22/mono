// SPDX-License-Identifier: MPL-2.0

use super::*;

fn workspace() -> WorkspaceId {
    WorkspaceId::new()
}

fn hash(seed: &[u8]) -> BlobHash {
    BlobHash::parse(&format!("b3:{}", blake3_hex(seed))).expect("a canonical address")
}

/// The hash spelling this module needs is the address one; the digest itself
/// only has to be a digest, so a fixed-width hex string is enough and this
/// avoids a dependency on the hasher for a test about bookkeeping.
fn blake3_hex(seed: &[u8]) -> String {
    let mut digest = String::with_capacity(64);
    for index in 0..32 {
        digest.push_str(&format!("{:02x}", seed[index % seed.len()] ^ index as u8));
    }
    digest
}

/// **D50, as the four booleans a phone can actually report.** No network beats
/// data saver beats metered, and the person's own allowance answers both of
/// the paid-connection holds — somebody who said "use cellular for pictures"
/// said it about the connection, and reporting low-data at them afterwards
/// would be the switch not working.
#[test]
fn the_network_policy_reads_in_the_order_the_holds_matter() {
    let policy = NetworkPolicy::default();
    assert_eq!(policy.hold_now(), None, "an unknown network is not a hold");

    policy.replace(&wire::BlobPolicy {
        metered: true,
        ..wire::BlobPolicy::default()
    });
    assert_eq!(policy.hold_now(), Some(TransferHold::Metered));

    policy.replace(&wire::BlobPolicy {
        metered: true,
        low_data: true,
        ..wire::BlobPolicy::default()
    });
    assert_eq!(
        policy.hold_now(),
        Some(TransferHold::LowData),
        "data saver is a different sentence from being on cellular",
    );

    policy.replace(&wire::BlobPolicy {
        metered: true,
        low_data: true,
        offline: true,
        ..wire::BlobPolicy::default()
    });
    assert_eq!(
        policy.hold_now(),
        Some(TransferHold::Offline),
        "there is nothing to be metered about with no network",
    );

    policy.replace(&wire::BlobPolicy {
        metered: true,
        low_data: true,
        allow_on_metered: true,
        offline: false,
    });
    assert_eq!(
        policy.hold_now(),
        None,
        "the person's allowance answers both paid-connection holds",
    );

    // …but not the absence of a network, which is not a permission question.
    policy.replace(&wire::BlobPolicy {
        metered: true,
        allow_on_metered: true,
        offline: true,
        low_data: false,
    });
    assert_eq!(policy.hold_now(), Some(TransferHold::Offline));
}

/// **The pause is not a refusal, and the port says which.** Every hold this
/// platform can report carries its own stable code into the engine's status,
/// and none of them is the transport mode's.
#[test]
fn every_platform_hold_reaches_the_engine_with_its_own_code() {
    let workspace = workspace();
    let policy = NetworkPolicy::default();
    for (reported, expected, code) in [
        (
            wire::BlobPolicy {
                metered: true,
                ..wire::BlobPolicy::default()
            },
            TransferHold::Metered,
            "network_metered",
        ),
        (
            wire::BlobPolicy {
                low_data: true,
                ..wire::BlobPolicy::default()
            },
            TransferHold::LowData,
            "network_low_data",
        ),
        (
            wire::BlobPolicy {
                offline: true,
                ..wire::BlobPolicy::default()
            },
            TransferHold::Offline,
            "network_offline",
        ),
    ] {
        policy.replace(&reported);
        assert!(!policy.transfers_permitted(workspace));
        assert_eq!(policy.hold(workspace), expected);
        assert_eq!(policy.hold(workspace).as_str(), code);
        assert_ne!(
            policy.hold(workspace).as_str(),
            TransferHold::TransportMode.as_str(),
            "a network pause was reported as a transport-mode refusal",
        );
    }
}

/// **Document sync continues while blob transfers are paused** (D50), from the
/// value's own side: this is the only place the metered fact enters the host,
/// the only question it answers is the blob port's, and there is nothing here
/// a document pass could ask. `a_paused_workspace_still_takes_document_work`
/// below is the observable half.
#[test]
fn a_metered_connection_pauses_blobs_and_nothing_else() {
    let workspace = workspace();
    let policy = NetworkPolicy::default();
    assert!(policy.transfers_permitted(workspace));

    policy.replace(&wire::BlobPolicy {
        metered: true,
        ..wire::BlobPolicy::default()
    });
    assert!(
        !policy.transfers_permitted(workspace),
        "blob transfers carried on over a metered link",
    );

    // The pause is one the person can waive, which is the other half of D50 —
    // and what makes an explicit request go through in the engine.
    assert!(policy.hold(workspace).overridable());
    assert!(policy.hold(workspace).is_pause());
}

/// Arrivals are a window, not a mailbox: a caller passes back the last
/// sequence it saw and is told exactly what is new.
#[test]
fn arrivals_are_read_forward_from_a_sequence() {
    let arrivals = Arrivals::default();
    let (mine, theirs) = (workspace(), workspace());
    let first = hash(b"one");
    let second = hash(b"two");
    let third = hash(b"three");

    arrivals.arrived(mine, &first);
    arrivals.arrived(theirs, &second);
    arrivals.arrived(mine, &third);

    let (rows, sequence) = arrivals.since(None, 0);
    assert_eq!(rows.len(), 3);
    assert_eq!(sequence, 3);
    assert_eq!(rows[0].hash, first.as_str());

    let (rows, sequence) = arrivals.since(None, 2);
    assert_eq!(rows.len(), 1, "an arrival already seen was sent again");
    assert_eq!(rows[0].hash, third.as_str());
    assert_eq!(sequence, 3);

    // Filtered by Workspace, the sequence still moves: a reader watching one
    // Workspace must not be pinned to an old window by another's arrivals.
    let (rows, sequence) = arrivals.since(Some(theirs), 0);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].hash, second.as_str());
    assert_eq!(sequence, 3);

    assert!(arrivals.since(None, sequence).0.is_empty());
}

/// **Bounded.** A phone that pulled a thousand pictures while nobody was
/// looking holds a ring, not a thousand records — and the newest are the ones
/// kept, because they are the ones a surface still has to react to.
#[test]
fn the_arrival_log_is_a_ring() {
    let arrivals = Arrivals::default();
    let workspace = workspace();
    for index in 0..(ARRIVAL_MEMORY + 50) {
        arrivals.arrived(workspace, &hash(&[index as u8, 7, 9]));
    }
    let (rows, sequence) = arrivals.since(None, 0);
    assert_eq!(rows.len(), ARRIVAL_MEMORY);
    assert_eq!(sequence as usize, ARRIVAL_MEMORY + 50);
    assert_eq!(
        rows.last().map(|arrival| arrival.sequence),
        Some(sequence),
        "the newest arrival was dropped instead of the oldest",
    );
}

/// `watching` is the open document views' own answer, and it decides **order**
/// rather than whether — which is why nothing here gates a transfer.
#[test]
fn watching_is_what_the_open_views_reference() {
    let arrivals = Arrivals::default();
    let workspace = workspace();
    let shown = hash(b"on screen");
    let hidden = hash(b"not on screen");
    arrivals.showing([shown.as_str().to_owned()]);
    assert!(arrivals.watching(workspace, &shown));
    assert!(!arrivals.watching(workspace, &hidden));

    // Replaced wholesale: closing the document stops it being watched, rather
    // than leaving a set that only ever grows.
    arrivals.showing(Vec::<String>::new());
    assert!(!arrivals.watching(workspace, &shown));
}

/// The §10 message is the contract's, field for field, and carries codes
/// rather than prose. An engine with nothing pending sends empty strings, not
/// the word "none".
#[test]
fn the_status_message_is_codes_and_counts() {
    let workspace = workspace();
    let status = koru_blob_sync::BlobTransferStatus {
        referenced: 9,
        present: 4,
        durable: 3,
        missing: 5,
        bytes_complete: 1_024,
        bytes_total: 4_096,
        pending_reason: Some("network_metered"),
        ..koru_blob_sync::BlobTransferStatus::default()
    };
    let message = status_message(workspace, &status);
    assert_eq!(message.workspace_id, workspace.to_string());
    assert_eq!(message.referenced, 9);
    assert_eq!(message.missing, 5);
    assert_eq!(message.bytes_complete, 1_024);
    assert_eq!(message.pending_reason, "network_metered");
    assert_eq!(
        message.last_failure_code, "",
        "a pause is not a failure and must not be reported as one",
    );
}

// ---------------------------------------------------- the host, end to end

use crate::MobileHost;

/// A real host on a private root: the Store worker it spawns is the one that
/// composes the engine, which is the whole claim of Plan B5.
fn host() -> (tempfile::TempDir, std::sync::Arc<MobileHost>) {
    let root = tempfile::tempdir().expect("a private root");
    let host = MobileHost::open(root.path().to_path_buf(), [83; 64], "Test", false)
        .expect("a fresh host opens");
    (root, host)
}

fn at() -> koru_store_core::Timestamp {
    koru_store_core::Timestamp::from_millis(1_770_000_000_000)
}

fn a_workspace(host: &MobileHost) -> WorkspaceId {
    host.store
        .with_store(|store| {
            let profile = store
                .create_profile(
                    "Owner",
                    None,
                    None,
                    koru_store_core::Locale::parse("en").expect("a locale"),
                    at(),
                )
                .expect("a profile");
            store
                .create_workspace(profile.id, "Workspace", at())
                .expect("a workspace")
                .id
        })
        .expect("the store answers")
}

/// Well shaped and far from expiry. **Nothing here dials anything**: every
/// test below stops at a decision the engine takes before its first request.
fn access() -> koru_sync_control::cloud::CloudBlobAccessV1 {
    koru_sync_control::cloud::CloudBlobAccessV1 {
        version: koru_sync_control::cloud::CLOUD_BLOB_ACCESS_VERSION,
        api_origin: "https://us.api.koru.do/".into(),
        region: "us".into(),
        access_token: "an-opaque-bearer".into(),
        expires_at_millis: i64::MAX,
    }
}

fn metered() -> wire::BlobPolicy {
    wire::BlobPolicy {
        metered: true,
        ..wire::BlobPolicy::default()
    }
}

/// **The claim Plan B5 exists to make.** A phone composes the same engine the
/// desktop does — the Store worker it already embeds built it — so asking a
/// fresh host for a Workspace's blob status answers counts rather than
/// "unavailable".
#[test]
fn a_phone_has_the_engine_and_can_be_asked_what_it_is_doing() {
    let (_root, host) = host();
    let workspace = a_workspace(&host);
    let status = host
        .blob_status(&workspace.to_string())
        .expect("the engine answers");
    assert_eq!(status.workspace_id, workspace.to_string());
    assert_eq!(status.referenced, 0);
    assert_eq!(status.pending_reason, "");
    assert_eq!(status.last_failure_code, "");
}

/// **§13, at the surface a person reads.** A metered pause reaches
/// `blob_status` as its own code, it is not filed as a failure, and it is not
/// the transport mode's refusal — the distinction whose absence cost this
/// project a debugging session.
#[test]
fn a_metered_pause_reaches_the_status_as_its_own_code() {
    let (_root, host) = host();
    let workspace = a_workspace(&host);
    host.blobs()
        .expect("an engine")
        .install_access(&host.tokio(), access())
        .expect("a well-shaped value");

    host.set_blob_policy(&metered()).expect("policy accepted");
    host.recover_blobs(&workspace.to_string())
        .expect("a recovery pass runs");

    let status = host
        .blob_status(&workspace.to_string())
        .expect("the engine answers");
    assert_eq!(status.pending_reason, "network_metered");
    assert_ne!(
        status.pending_reason,
        TransferHold::TransportMode.as_str(),
        "a pause was reported with the refusal's code",
    );
    assert_eq!(
        status.last_failure_code, "",
        "a person on a train has had nothing go wrong",
    );
}

/// **Every hold reaches `blob_status` as its own code, and none of them is the
/// refusal's** — §13 through the surface a Kotlin screen actually reads.
///
/// The engine's own tests pin the state machine; what this pins is the last
/// hop, where a phone's screen branches. A pause and a refusal arriving as one
/// string is the defect §13 exists for, and the place it would land is here.
#[test]
fn every_hold_reaches_the_mobile_status_as_its_own_code() {
    let (_root, host) = host();
    let workspace = a_workspace(&host);
    let blobs = host.blobs().expect("an engine");
    blobs
        .install_access(&host.tokio(), access())
        .expect("a well-shaped value");

    let mut seen = std::collections::BTreeSet::new();
    for (policy, expected) in [
        (metered(), "network_metered"),
        (
            wire::BlobPolicy {
                low_data: true,
                ..wire::BlobPolicy::default()
            },
            "network_low_data",
        ),
        (
            wire::BlobPolicy {
                offline: true,
                ..wire::BlobPolicy::default()
            },
            "network_offline",
        ),
    ] {
        host.set_blob_policy(&policy).expect("policy accepted");
        host.recover_blobs(&workspace.to_string()).expect("a pass");
        let status = host.blob_status(&workspace.to_string()).expect("status");
        assert_eq!(status.pending_reason, expected);
        assert_ne!(
            status.pending_reason,
            TransferHold::TransportMode.as_str(),
            "a network pause arrived at the surface as the refusal's code",
        );
        assert_eq!(
            status.last_failure_code, "",
            "a pause was filed as a failure, which is what a person is shown",
        );
        assert!(
            seen.insert(status.pending_reason.clone()),
            "two holds, one code"
        );
    }

    // The person's own pause (D256), which no change of network clears — a
    // fourth sentence, not a spelling of the first three.
    host.set_blob_policy(&wire::BlobPolicy::default())
        .expect("policy accepted");
    assert!(blobs.engine.engine().pause(workspace));
    host.recover_blobs(&workspace.to_string()).expect("a pass");
    let status = host.blob_status(&workspace.to_string()).expect("status");
    assert_eq!(status.pending_reason, "paused_by_person");
    assert!(seen.insert(status.pending_reason.clone()));
    assert_ne!(status.pending_reason, TransferHold::TransportMode.as_str());

    // Four holds, four codes, and the refusal is a fifth this phone cannot
    // reach at all: the transport-mode gate is deliberately off on Android
    // (B8, D254), because a phone has no way to promote a Workspace out of
    // migration 12's seeded `local_only`.
    assert_eq!(seen.len(), 4);
    assert!(!seen.contains(TransferHold::TransportMode.as_str()));
}

/// The other half of D50: allowing cellular transfers clears the pause, and
/// the pass the switch runs has nothing to say — an empty Workspace
/// references no pictures, so nothing is attempted and nothing is pending.
#[test]
fn allowing_cellular_transfers_clears_the_pause() {
    let (_root, host) = host();
    let workspace = a_workspace(&host);
    host.blobs()
        .expect("an engine")
        .install_access(&host.tokio(), access())
        .expect("a well-shaped value");

    host.set_blob_policy(&metered()).expect("policy accepted");
    host.recover_blobs(&workspace.to_string()).expect("a pass");
    assert_eq!(
        host.blob_status(&workspace.to_string())
            .expect("status")
            .pending_reason,
        "network_metered"
    );

    host.set_blob_policy(&wire::BlobPolicy {
        metered: true,
        allow_on_metered: true,
        ..wire::BlobPolicy::default()
    })
    .expect("policy accepted");
    // The switch spawns its pass rather than waiting for one — a gesture must
    // return like a gesture — so the pass this asserts on is run here.
    host.recover_blobs(&workspace.to_string()).expect("a pass");
    assert_eq!(
        host.blob_status(&workspace.to_string())
            .expect("status")
            .pending_reason,
        "",
        "the person allowed it and the pass was still held",
    );
}

/// **Document work continues while blob transfers are paused** (D50), through
/// the host rather than through the policy value alone: a Workspace paused for
/// bytes still takes and commits a durable change, and the read model still
/// answers.
///
/// The two-device proof — a desktop moving documents while a phone on cellular
/// holds its pictures — is a hardware check and is in the acceptance list.
/// What is pinned here is that nothing on the document path can reach the
/// value that holds the bytes.
#[test]
fn a_paused_workspace_still_takes_document_work() {
    let (_root, host) = host();
    let workspace = a_workspace(&host);
    host.blobs()
        .expect("an engine")
        .install_access(&host.tokio(), access())
        .expect("a well-shaped value");
    host.set_blob_policy(&metered()).expect("policy accepted");
    host.recover_blobs(&workspace.to_string()).expect("a pass");
    assert_eq!(
        host.blob_status(&workspace.to_string())
            .expect("status")
            .pending_reason,
        "network_metered"
    );

    host.mutate_workspace(&workspace.to_string(), "rename", "Still writing")
        .expect("a durable change lands while the pictures wait");
    let renamed = host
        .store
        .with_store(move |store| {
            store
                .workspace(workspace)
                .map(|workspace| workspace.name)
                .map_err(|error| error.code().as_str().to_owned())
        })
        .expect("the store answers")
        .expect("the workspace");
    assert_eq!(renamed, "Still writing");
    // …and the Workspace is still paused for bytes afterwards, so the document
    // that landed did not land by the pause having quietly lifted.
    assert_eq!(
        host.blob_status(&workspace.to_string())
            .expect("status")
            .pending_reason,
        "network_metered"
    );
}

/// **A person's pause holds the bytes and nothing else** (D256), through the
/// host rather than through the flag alone: a Workspace whose pictures
/// somebody paused still takes and commits a durable change, and is still
/// paused afterwards — so the document that landed did not land by the pause
/// having quietly lifted.
///
/// The shape is `a_paused_workspace_still_takes_document_work` above, one
/// hold along. It is pinned in this crate because this is where an engine and
/// a real Store meet in one process; the desktop reaches the same engine
/// through `BlobSync::set_paused` and has no second state machine to get
/// this wrong in.
#[test]
fn a_workspace_paused_by_a_person_still_takes_document_work() {
    let (_root, host) = host();
    let workspace = a_workspace(&host);
    let blobs = host.blobs().expect("an engine");
    blobs
        .install_access(&host.tokio(), access())
        .expect("a well-shaped value");

    assert!(blobs.engine.engine().pause(workspace));
    host.recover_blobs(&workspace.to_string()).expect("a pass");
    let status = host.blob_status(&workspace.to_string()).expect("status");
    assert_eq!(status.pending_reason, "paused_by_person");
    assert!(
        status.pending_reason != "network_metered",
        "a pause somebody chose must not read as the network's",
    );

    host.mutate_workspace(&workspace.to_string(), "rename", "Still writing")
        .expect("a durable change lands while the pictures wait");
    let renamed = host
        .store
        .with_store(move |store| {
            store
                .workspace(workspace)
                .map(|workspace| workspace.name)
                .map_err(|error| error.code().as_str().to_owned())
        })
        .expect("the store answers")
        .expect("the workspace");
    assert_eq!(renamed, "Still writing");
    assert_eq!(
        host.blob_status(&workspace.to_string())
            .expect("status")
            .pending_reason,
        "paused_by_person",
    );

    // Lifting it is the person's too, and lifts nothing else.
    assert!(blobs.engine.engine().resume(workspace));
    assert!(!blobs.engine.engine().paused(workspace));
}

/// **A file is counted exactly like a picture** (D259).
///
/// The desktop's surface counted images, silently left an
/// `application/octet-stream` attachment out of its totals, and reported the
/// activity that attachment caused — so a person read "5 of 5 on Koru Cloud"
/// beside "sending" while a 1.9 GiB archive was mid-flight. The engine was
/// never the problem: the canonical walk does not consult a media type. This
/// pins that through the phone's own surface, so the day somebody adds a
/// filter it fails here rather than on hardware.
#[test]
fn a_non_image_blob_is_counted_like_any_other() {
    let (root, host) = host();
    let workspace = a_workspace(&host);
    host.blobs()
        .expect("an engine")
        .install_access(&host.tokio(), access())
        .expect("a well-shaped value");

    // Not a picture by any reading: an opaque type, no image extension, and
    // bytes that decode as nothing.
    let file = root.path().join("quarterly.bin");
    std::fs::write(&file, vec![0x7fu8; 4_096]).expect("a file to import");
    host.import_file(
        &workspace.to_string(),
        "quarterly.bin",
        "application/octet-stream",
        file.to_str().expect("a path"),
        None,
    )
    .expect("the file is imported");

    // The walk reads the projection, and a projection sweep is asked for
    // rather than awaited — `Sweeps::request` documents that a query never
    // waits for its generation and the caller retries. So the pass is run
    // until the read model has caught up, bounded: this is a wait for a
    // read model, never a wait for a network.
    let mut status = host.blob_status(&workspace.to_string()).expect("answers");
    for _ in 0..50 {
        host.recover_blobs(&workspace.to_string())
            .expect("a pass runs");
        status = host
            .blob_status(&workspace.to_string())
            .expect("the engine answers");
        if status.referenced > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(
        status.referenced, 1,
        "an attachment that is not an image was left out of the counts",
    );
    // …and nothing claims it is anywhere else. `current` is the only field a
    // surface may turn into a tick, and no object worker has confirmed a byte:
    // a tick over an unsent file is the other half of D259.
    assert!(!status.current);
    assert_eq!(status.coverage_state, "");
}

/// **The setting without which a phone cannot be tested against a development
/// stack at all**, and the property that keeps it a development setting.
///
/// A phone has no environment: `KORU_CLOUD_BLOB_BYTE_ORIGINS` reaches the
/// desktop's engine and nothing reaches Android's, so a local stack that hands
/// back `http://192.168.1.118:8081/` is refused by the phone's own allowlist —
/// a missing setting that reads exactly like a broken transfer engine. The
/// value arrives from `BuildConfig` instead and lands here.
///
/// **And it is the same allowlist, so a build cannot name a public host.**
/// That is not a detail of this path; it is what the compiled-in per-region
/// set exists for, and a refusal leaves that set standing.
#[test]
fn a_build_may_name_a_local_object_worker_and_may_not_name_a_public_one() {
    let (_root, host) = host();
    let blobs = host.blobs().expect("an engine");

    // The LAN address a phone needs. Loopback would be this phone.
    assert_eq!(
        blobs.install_development_origins("http://192.168.1.118:8081/"),
        Ok(())
    );
    assert_eq!(
        blobs.install_development_origins("http://127.0.0.1:8081, http://[fe80::1]:8081"),
        Ok(())
    );

    for public in [
        "https://evil.example.com",
        "https://us.data.koru.do.evil.example.com",
        "http://93.184.216.34:8081",
        "http://localhost.evil.example.com:8081",
    ] {
        assert_eq!(
            blobs.install_development_origins(public),
            Err(koru_blob_sync::OriginRefused::NotAllowed),
            "{public} was accepted as a development byte origin",
        );
    }
    // Not a URL, and a URL carrying more than an origin: an argument that is
    // not understood is not run.
    assert_eq!(
        blobs.install_development_origins("192.168.1.118:8081"),
        Err(koru_blob_sync::OriginRefused::Unparseable),
    );
    assert_eq!(
        blobs.install_development_origins("http://192.168.1.118:8081/collect"),
        Err(koru_blob_sync::OriginRefused::Shape),
    );
    assert_eq!(
        blobs.install_development_origins("file:///tmp"),
        Err(koru_blob_sync::OriginRefused::Scheme),
    );
}

/// A blob address this build cannot read is refused where it is parsed, and
/// says which argument was wrong — never "something went wrong".
#[test]
fn a_request_for_an_unreadable_address_is_refused_by_name() {
    let (_root, host) = host();
    let workspace = a_workspace(&host);
    assert_eq!(
        host.request_blob(&workspace.to_string(), "not-a-hash"),
        Err("invalid_blob_id".to_owned())
    );
    assert_eq!(
        host.request_blob("not-a-workspace", "b3:00"),
        Err("invalid_workspace_id".to_owned())
    );
}

/// The announcement window starts empty and says where to resume, rather than
/// leaving a reader with nothing to come back with.
#[test]
fn a_fresh_host_announces_nothing_and_says_where_to_resume() {
    let (_root, host) = host();
    let arrivals = host.blob_arrivals("", 0).expect("the host answers");
    assert!(arrivals.arrivals.is_empty());
    assert_eq!(arrivals.sequence, 0);
    assert_eq!(arrivals.error_code, None);
}
