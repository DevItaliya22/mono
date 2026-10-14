// SPDX-License-Identifier: MPL-2.0

//! **Sync through a real Gateway process, and what its acknowledgements mean.**
//!
//! The host is `koru-agent.exe --worker gateway`, started by
//! [`gateway_process`] — the harness plays the Agent, because the role is
//! supervised-only. Everything below this fixture is the suite as it stood
//! against the standalone `koru-api-gateway.exe`: the composition is the same
//! one, driven from the other launch path.

mod gateway_process;

use std::path::Path;
use std::time::{Duration, Instant};

use koru_api_client::{Client, Connection, Event};
use koru_store_core::ids::DocumentId;
use koru_store_core::manifest::Manifest;

use gateway_process::Gateway;

fn start(root: &Path) -> (Gateway, Client) {
    let mut gateway = Gateway::start(root);
    let found = gateway.published(&gateway_process::hello("gateway-sync-integration"));
    let client = Client::from_gateway(found).expect("build gateway client");
    (gateway, client)
}

/// `gateway.control` is spelled twice on purpose — `koru-api-client` does not
/// link `koru-api-gateway-core`, the same isolation that keeps a prompt box
/// from acquiring a chord table. This package links both, so it is where the
/// two copies are pinned to each other.
#[test]
fn the_control_scope_literals_agree() {
    assert_eq!(
        koru_api_client::CONTROL_SCOPE,
        koru_api_gateway_core::CONTROL_SCOPE
    );
}

/// The same pinning, for the scope a settings caller has to name alongside
/// `settings.write` if it wants to hear that a section changed.
#[test]
fn the_events_read_scope_literals_agree() {
    assert_eq!(
        koru_api_client::EVENTS_READ_SCOPE,
        koru_api_gateway_core::EVENTS_READ_SCOPE
    );
}

/// And for the destructive one, where a drift would be quieter and worse: a
/// window naming a scope the gateway does not know is admitted *without* it,
/// so the manager's Archive and Delete rows would go permanently disabled and
/// nothing would say why. Every other spelling mistake here is a 403; this one
/// is a surface that silently stops offering an action.
#[test]
fn the_destructive_scope_literals_agree() {
    assert_eq!(
        koru_api_client::WORKSPACE_DESTROY_SCOPE,
        koru_api_gateway_core::WORKSPACE_DESTROY_SCOPE
    );
}

/// And for the migration one, which fails the same silent way — and has one
/// more thing to say: the two vouched scopes must stay two. A build in which
/// they had drifted into the same literal would hand every window that may
/// remove a workspace the ability to move one, and nothing would notice.
#[test]
fn the_migration_scope_literals_agree() {
    assert_eq!(
        koru_api_client::WORKSPACE_MIGRATE_SCOPE,
        koru_api_gateway_core::WORKSPACE_MIGRATE_SCOPE
    );
    assert_ne!(
        koru_api_gateway_core::WORKSPACE_MIGRATE_SCOPE,
        koru_api_gateway_core::WORKSPACE_DESTROY_SCOPE
    );
}

fn drain(connection: &mut Connection, manifest: &mut Manifest) {
    loop {
        match connection.next_event() {
            Ok(Some(Event::Updates { updates, .. })) => {
                for update in updates {
                    manifest.import(&update).unwrap();
                }
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => break,
        }
    }
}

#[test]
fn product_gateway_upgrades_sync_and_writes_its_own_log() {
    let root = tempfile::tempdir().unwrap();
    let (mut gateway, client) = start(root.path());
    let profile = client
        .store()
        .create_profile("Gateway test", None, None, Some("en"))
        .unwrap();
    let workspace = client
        .store()
        .create_workspace(profile.id, "Workspace")
        .unwrap()
        .id;
    let manifest_id = client.store().manifest(workspace).unwrap();
    let snapshot = client.store().snapshot(workspace, manifest_id).unwrap();
    let mut a_manifest = Manifest::from_snapshot(&snapshot).unwrap();
    let mut b_manifest = Manifest::from_snapshot(&snapshot).unwrap();
    let room = format!("workspace:{}:manifest", workspace.to_storage());
    let discovery = client.store().discovery();
    let mut a = Connection::open(&discovery).unwrap();
    let mut b = Connection::open(&discovery).unwrap();
    a.set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    b.set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    a.join(&room, &a_manifest.version()).unwrap();
    b.join(&room, &b_manifest.version()).unwrap();
    drain(&mut a, &mut a_manifest);
    drain(&mut b, &mut b_manifest);

    let sent = a_manifest.version();
    let added = DocumentId::new();
    a_manifest
        .add_document(added, "Crossed the gateway", "page", None)
        .unwrap();
    let batch = a
        .send_update(&room, a_manifest.export_updates_since(&sent).unwrap())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut durable = false;
    while Instant::now() < deadline
        && (!durable || !b_manifest.entries().iter().any(|entry| entry.id == added))
    {
        match a.next_event() {
            Ok(Some(Event::Acked { ack, .. })) if ack.batch == batch => durable = ack.durable,
            Ok(Some(Event::Updates { updates, .. })) => {
                for update in updates {
                    a_manifest.import(&update).unwrap();
                }
            }
            _ => {}
        }
        if let Ok(Some(Event::Updates { updates, .. })) = b.next_event() {
            for update in updates {
                b_manifest.import(&update).unwrap();
            }
        }
    }
    assert!(
        durable,
        "the gateway path never returned a durable acknowledgement"
    );
    assert!(
        b_manifest.entries().iter().any(|entry| entry.id == added),
        "the update did not cross gateway -> worker -> sync router"
    );

    let logs = std::fs::read_dir(root.path().join("store").join("logs")).unwrap();
    assert!(logs.filter_map(Result::ok).any(|entry| {
        entry
            .file_name()
            .to_string_lossy()
            .starts_with("koru-api-gateway.log")
    }));

    // The lights-off question. With both sync sockets still joined, an
    // idle-only stop must be declined — the worker's connection count is the
    // answer, and the gateway cannot produce it from its own state.
    let control = Client::open_at_with_scopes(
        koru_channel::segment(),
        root.path(),
        &[koru_api_client::CONTROL_SCOPE],
    )
    .unwrap();
    assert!(
        !control.shutdown_if_idle().unwrap(),
        "a stop was granted while two sync sockets were live"
    );
    assert!(
        gateway.try_wait().is_none(),
        "a declined stop must leave the gateway running"
    );

    // The caller asks after dropping its own sockets; the close reaches the
    // worker's rooms asynchronously, so ask until the count drains.
    drop(a);
    drop(b);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !control.shutdown_if_idle().unwrap_or(false) {
        assert!(
            Instant::now() < deadline,
            "the idle stop was never granted after both sockets closed"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while gateway.try_wait().is_none() {
        assert!(
            Instant::now() < deadline,
            "a granted idle stop must bring the gateway down"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// **A gateway with an attached first-party window is not idle.**
///
/// The failure this pins came off a live machine: the gateway log ended with
/// `POST /v1/shutdown?only_if_idle=true -> 200` and "koru-store-server stopped"
/// while a Koru Terminal window was working. A Terminal joins no sync room —
/// its banner catalog rides `/v1/events/stream` and its session traffic goes
/// straight to the session host — so the Store worker's socket count, which was
/// the whole of the idle test, saw nobody and the gateway stopped underneath it.
///
/// The window here is exactly the terminal's: `watch_catalog`, one SSE
/// subscription and nothing else. It has to be enough to refuse the stop, and
/// dropping it has to be enough to allow it.
#[test]
fn a_client_holding_a_catalog_watch_keeps_the_gateway_from_stopping() {
    let root = tempfile::tempdir().unwrap();
    let (mut gateway, client) = start(root.path());

    // The last window's voice: control only, exactly as the close path opens it.
    let control = Client::open_at_with_scopes(
        koru_channel::segment(),
        root.path(),
        &[koru_api_client::CONTROL_SCOPE],
    )
    .unwrap();
    // Nothing is attached yet and the worker's rooms are empty, so this is the
    // state in which a stop is legitimately granted. Saying so first means the
    // refusal below cannot be a gateway that declines everything.
    assert_eq!(
        client.events().status().unwrap().sse_subscribers,
        0,
        "no window has attached yet"
    );

    let watch = client.events().watch_catalog(|_| {}).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while client.events().status().unwrap().sse_subscribers == 0 {
        assert!(
            Instant::now() < deadline,
            "the catalog watch never reached the events worker"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    assert!(
        !control.shutdown_if_idle().unwrap(),
        "a stop was granted while a window held a catalog watch"
    );
    assert!(
        gateway.try_wait().is_none(),
        "a declined stop must leave the gateway running"
    );

    watch.close();
    let deadline = Instant::now() + Duration::from_secs(10);
    while client.events().status().unwrap().sse_subscribers > 0 {
        assert!(
            Instant::now() < deadline,
            "the watch never left the events worker"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        control.shutdown_if_idle().unwrap(),
        "dropping the last watch must let the stop through"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while gateway.try_wait().is_none() {
        assert!(
            Instant::now() < deadline,
            "a granted idle stop must bring the gateway down"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// **A gateway asked to stop closes its event tails instead of waiting them
/// out, and its exit code carries the Store's completion.**
///
/// Both halves of Plan 5's shutdown truth, in one place, because they are one
/// sequence:
///
/// - **The drain.** An attached window holds a `/v1/events` tail, which the
///   client cuts and resumes on a ninety-second deadline. Axum's graceful
///   shutdown waits for in-flight responses, so a gateway with one window
///   attached took **90.1 s** to exit — measured on this tree before the fix.
///   A server that cannot stop because a client is idling on a tail is the
///   defect, so the shutdown path closes the tails first. A cut tail costs the
///   client nothing: the cursor is the protocol and it resumes.
/// - **The completion.** Exit 0 is set only after the host joined the Store
///   worker and found it `Clean`. It is the Store's own confirmation that its
///   last commit finished, carried across a process boundary — and it is what
///   the Agent's update gate refuses to install without.
///
/// The bound is ten seconds: generous for a real Store closing, and far below
/// the ninety this used to take, so a regression fails here rather than being
/// absorbed by a patient deadline.
#[test]
fn a_stop_closes_the_event_tails_and_the_exit_code_carries_the_store() {
    let root = tempfile::tempdir().unwrap();
    let mut gateway = {
        let (mut gateway, client) = start(root.path());
        let control = Client::open_at_with_scopes(
            koru_channel::segment(),
            root.path(),
            &[koru_api_client::CONTROL_SCOPE],
        )
        .unwrap();

        // One attached window, exactly as a Koru window attaches.
        let watch = client.events().watch_catalog(|_| {}).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while client.events().status().unwrap().sse_subscribers == 0 {
            assert!(
                Instant::now() < deadline,
                "the catalog watch never reached the events worker"
            );
            std::thread::sleep(Duration::from_millis(20));
        }

        let asked = Instant::now();
        control.store().shutdown().unwrap();
        let deadline = asked + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = gateway.try_wait() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "the gateway waited out an idle event tail instead of closing it \
                 ({:?} so far)",
                asked.elapsed()
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        eprintln!(
            "[gateway drain] the gateway exited {} ms after the request",
            asked.elapsed().as_millis()
        );
        assert_eq!(
            status.code(),
            Some(0),
            "the host did not report a clean Store join"
        );
        watch.close();
        gateway
    };
    // Reaped above; keep `Drop` from waiting on a process that is already gone.
    let _ = gateway.try_wait();
}

/// **A durable acknowledgement outlives the host that gave it.**
///
/// `Ack { durable: true }` means the Store committed the bytes, not that a
/// process still holding them says so — the promise
/// [PRINCIPLES](../../../guide/contributing/PRINCIPLES.md) calls "an acknowledgement that was
/// nearly a lie". Moving the Gateway into another process changes who owns that
/// commit's lifetime, so this pins what the acknowledgement is worth across a
/// host that is killed rather than asked: no graceful stop, no chance to finish
/// anything it had already claimed was on the disk.
///
/// **Asserted against a second, freshly discovered client**, because the
/// question is what the *store* still holds, not what the first client's
/// in-memory manifest replica remembers.
#[test]
fn an_acknowledged_update_survives_a_killed_and_replaced_gateway() {
    let root = tempfile::tempdir().unwrap();
    let (gateway, client) = start(root.path());
    let profile = client
        .store()
        .create_profile("Restart test", None, None, Some("en"))
        .unwrap();
    let workspace = client
        .store()
        .create_workspace(profile.id, "Workspace")
        .unwrap()
        .id;
    let manifest_id = client.store().manifest(workspace).unwrap();
    let snapshot = client.store().snapshot(workspace, manifest_id).unwrap();
    let mut manifest = Manifest::from_snapshot(&snapshot).unwrap();
    let room = format!("workspace:{}:manifest", workspace.to_storage());
    let discovery = client.store().discovery();
    let mut connection = Connection::open(&discovery).unwrap();
    connection
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    connection.join(&room, &manifest.version()).unwrap();
    drain(&mut connection, &mut manifest);

    let sent = manifest.version();
    let acknowledged = DocumentId::new();
    manifest
        .add_document(acknowledged, "Acknowledged before the kill", "page", None)
        .unwrap();
    let batch = connection
        .send_update(&room, manifest.export_updates_since(&sent).unwrap())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut durable = false;
    while !durable {
        match connection.next_event() {
            Ok(Some(Event::Acked { ack, .. })) if ack.batch == batch => durable = ack.durable,
            _ => {}
        }
        assert!(
            Instant::now() < deadline,
            "the update was never acknowledged as durable"
        );
    }

    // Nothing is asked of the host: it is killed where it stands, with the
    // sync socket still open.
    drop(connection);
    gateway.terminate();

    let (_replacement, reopened) = start(root.path());
    let manifest_id = reopened.store().manifest(workspace).unwrap();
    let snapshot = reopened.store().snapshot(workspace, manifest_id).unwrap();
    let reloaded = Manifest::from_snapshot(&snapshot).unwrap();
    assert!(
        reloaded
            .entries()
            .iter()
            .any(|entry| entry.id == acknowledged),
        "a durably acknowledged document was not there after the host was replaced"
    );
    // And the workspace it belongs to came back with it, so this is a store
    // that reopened rather than one that started empty.
    assert!(
        reopened
            .store()
            .workspaces()
            .unwrap()
            .iter()
            .any(|listed| listed.id == workspace),
        "the reopened store does not list the workspace the update landed in"
    );
}
