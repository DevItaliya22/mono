// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_store_core::manifest::Manifest;
use koru_store_core::service::Store;
use koru_store_core::text::Locale;
use koru_store_sqlite::{SqliteBackend, SqliteCatalog};

fn at() -> Timestamp {
    Timestamp::from_millis(1_770_000_000_000)
}

fn caller(scopes: &[Scope]) -> Caller {
    Caller {
        scopes: scopes.to_vec(),
        label: "test".into(),
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: KoruStore,
    workspace: WorkspaceId,
    manifest_room: String,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let catalog = SqliteCatalog::open(&dir.path().join("catalog.db")).unwrap();
    let mut store: KoruStore = Store::open(catalog, SqliteBackend::new(dir.path())).unwrap();
    let profile = store
        .create_profile("Owner", None, None, Locale::parse("en").unwrap(), at())
        .unwrap();
    let workspace = store.create_workspace(profile.id, "W", at()).unwrap().id;
    let manifest_room = Room::Manifest(workspace).to_id();
    Fixture {
        _dir: dir,
        store,
        workspace,
        manifest_room,
    }
}

fn join_request(room_id: &str, version: Vec<u8>) -> ProtocolMessage {
    ProtocolMessage::JoinRequest {
        crdt: CrdtType::Loro,
        room_id: room_id.into(),
        auth: Vec::new(),
        version,
    }
}

/// A client's replica, and the update it would send after an edit.
fn an_update(f: &mut Fixture, title: &str) -> (Vec<u8>, Manifest) {
    let document = f.store.manifest_id(f.workspace).unwrap();
    let snapshot = f.store.snapshot(f.workspace, document).unwrap();
    let mut manifest = Manifest::from_snapshot(&snapshot).unwrap();
    let before = manifest.version();
    manifest
        .add_document(DocumentId::new(), title, "page", None)
        .unwrap();
    (manifest.export_updates_since(&before).unwrap(), manifest)
}

fn doc_update(room_id: &str, bytes: Vec<u8>) -> ProtocolMessage {
    ProtocolMessage::DocUpdate {
        crdt: CrdtType::Loro,
        room_id: room_id.into(),
        updates: vec![bytes],
        batch_id: BatchId([1, 2, 3, 4, 5, 6, 7, 8]),
    }
}

// ------------------------------------------------------------------ rooms

#[test]
fn a_manifest_room_resolves_to_its_workspace() {
    let mut f = fixture();
    let room = Room::parse(&f.manifest_room).unwrap();
    let target = resolve(&mut f.store, &room)
        .expect("resolve")
        .expect("target");
    assert_eq!(target.workspace, f.workspace);
    assert_eq!(target.document, f.store.manifest_id(f.workspace).unwrap());
}

#[test]
fn a_document_room_resolves_through_the_location_index() {
    // A client should be able to name a document without knowing which
    // workspace it is in.
    let mut f = fixture();
    let (document, _) = f
        .store
        .create_document(f.workspace, "四半期計画", "page", None, None, at())
        .unwrap();

    let room = Room::Document(document);
    let target = resolve(&mut f.store, &room)
        .expect("resolve")
        .expect("target");
    assert_eq!(target.workspace, f.workspace);
    assert_eq!(target.document, document);
}

#[test]
fn an_unknown_document_room_resolves_to_nothing() {
    let mut f = fixture();
    assert!(
        resolve(&mut f.store, &Room::Document(DocumentId::new()))
            .expect("resolve")
            .is_none()
    );
}

#[test]
fn a_workspace_open_failure_is_not_flattened_into_an_unknown_room() {
    let mut f = fixture();
    let error = resolve(&mut f.store, &Room::Manifest(WorkspaceId::new())).unwrap_err();
    assert_eq!(error.code(), Code::WorkspaceNotFound);
}

// ------------------------------------------------------------------ joining

#[test]
fn joining_reports_the_stores_position_and_what_the_client_is_missing() {
    let mut f = fixture();
    // Put something in the manifest so there is history to catch up on.
    f.store
        .create_document(f.workspace, "Existing", "page", None, None, at())
        .unwrap();

    let room = f.manifest_room.clone();
    let outcome = handle(
        &mut f.store,
        &caller(Scope::ALL),
        // An empty version vector: a client with no history asks for
        // everything, which is the only form that applies to an empty replica.
        join_request(&room, Vec::new()),
        at(),
    );

    let reply = outcome.reply.expect("a response");
    let ProtocolMessage::JoinResponseOk {
        permission,
        version,
        ..
    } = reply
    else {
        panic!("expected JoinResponseOk, got {reply:?}");
    };
    assert_eq!(permission, Permission::Write);
    assert!(!version.is_empty(), "the store's position must be reported");
    assert_eq!(outcome.joined, Some(Room::parse(&room).unwrap()));

    // The catch-up follows, and it is a real one.
    let catch_up = outcome.broadcast.expect("a catch-up");
    let ProtocolMessage::DocUpdate { updates, .. } = catch_up else {
        panic!("expected a DocUpdate");
    };
    let mut fresh = Manifest::from_snapshot(
        &Manifest::create(f.workspace, "x", at())
            .unwrap()
            .export_snapshot()
            .unwrap(),
    )
    .unwrap();
    let _ = &mut fresh;
    assert!(!updates[0].is_empty());
}

#[test]
fn a_client_that_is_already_up_to_date_gets_no_catch_up() {
    let mut f = fixture();
    let document = f.store.manifest_id(f.workspace).unwrap();
    let ours = f.store.version(f.workspace, document).unwrap();

    let outcome = handle(
        &mut f.store,
        &caller(Scope::ALL),
        join_request(&f.manifest_room.clone(), ours.encode()),
        at(),
    );
    assert!(outcome.reply.is_some());
    assert!(outcome.broadcast.is_none(), "an empty delta was sent");
}

#[test]
fn a_read_only_credential_joins_with_read_permission() {
    let mut f = fixture();
    let outcome = handle(
        &mut f.store,
        &caller(&[Scope::ReadDocuments]),
        join_request(&f.manifest_room.clone(), Vec::new()),
        at(),
    );
    let ProtocolMessage::JoinResponseOk { permission, .. } = outcome.reply.unwrap() else {
        panic!("expected JoinResponseOk");
    };
    assert_eq!(permission, Permission::Read);
}

#[test]
fn a_credential_that_cannot_read_is_refused_with_koru_s_code() {
    let mut f = fixture();
    let outcome = handle(
        &mut f.store,
        &caller(&[]),
        join_request(&f.manifest_room.clone(), Vec::new()),
        at(),
    );
    let ProtocolMessage::JoinError { code, app_code, .. } = outcome.reply.unwrap() else {
        panic!("expected JoinError");
    };
    assert_eq!(code, JoinErrorCode::AuthFailed);
    // **No `app_code` here, deliberately.** The codec only writes that field
    // beside `AppError`; setting it with `AuthFailed` compiles, looks right and
    // reaches the client as nothing at all. `AuthFailed` already says
    // everything a client needs, including one that has never heard of Koru.
    assert_eq!(app_code, None);
    assert!(outcome.joined.is_none());
}

#[test]
fn joining_a_room_that_does_not_exist_is_refused() {
    let mut f = fixture();
    for room in [
        Room::Document(DocumentId::new()).to_id(),
        "not a room".to_string(),
    ] {
        let outcome = handle(
            &mut f.store,
            &caller(Scope::ALL),
            join_request(&room, Vec::new()),
            at(),
        );
        assert!(
            matches!(outcome.reply, Some(ProtocolMessage::JoinError { .. })),
            "{room} was joined"
        );
        assert!(outcome.joined.is_none());
    }
}

// ------------------------------------------------------------------ updating

#[test]
fn an_update_is_acknowledged_only_after_it_is_durable() {
    let mut f = fixture();
    let room = f.manifest_room.clone();
    let (update, _) = an_update(&mut f, "Acknowledged");

    let outcome = handle(
        &mut f.store,
        &caller(Scope::ALL),
        doc_update(&room, update),
        at(),
    );
    let ProtocolMessage::Ack { status, ref_id, .. } = outcome.reply.expect("an ack") else {
        panic!("expected an Ack");
    };
    assert_eq!(status, UpdateStatusCode::Ok);
    assert!(
        outcome.durable_change,
        "a committed update did not request peer reconciliation"
    );
    assert_eq!(
        ref_id,
        BatchId([1, 2, 3, 4, 5, 6, 7, 8]),
        "the batch id must match"
    );

    // Durable means durable: the document is on disk, not merely in memory.
    let listed = f.store.documents(f.workspace).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].title, "Acknowledged");
}

#[test]
fn an_acknowledged_update_is_broadcast_to_the_room() {
    let mut f = fixture();
    let room = f.manifest_room.clone();
    let (update, _) = an_update(&mut f, "Shared");

    let outcome = handle(
        &mut f.store,
        &caller(Scope::ALL),
        doc_update(&room, update.clone()),
        at(),
    );
    let broadcast = outcome.broadcast.expect("a broadcast");
    let ProtocolMessage::DocUpdate {
        updates, room_id, ..
    } = broadcast
    else {
        panic!("expected a DocUpdate");
    };
    assert_eq!(room_id, room);
    // Verbatim: what the room receives is what the sender sent.
    assert_eq!(updates, vec![update]);
}

#[test]
fn a_malformed_update_is_refused_and_never_broadcast() {
    let mut f = fixture();
    let outcome = handle(
        &mut f.store,
        &caller(Scope::ALL),
        doc_update(&f.manifest_room.clone(), b"not an update".to_vec()),
        at(),
    );
    let ProtocolMessage::Ack { status, .. } = outcome.reply.expect("an ack") else {
        panic!("expected an Ack");
    };
    assert_eq!(status, UpdateStatusCode::InvalidUpdate);
    assert!(
        outcome.broadcast.is_none(),
        "a refused update reached the room"
    );
    assert!(
        !outcome.durable_change,
        "a refused update requested peer reconciliation"
    );
    assert!(f.store.documents(f.workspace).unwrap().is_empty());
}

#[test]
fn a_read_only_credential_cannot_write() {
    let mut f = fixture();
    let room = f.manifest_room.clone();
    let (update, _) = an_update(&mut f, "Refused");

    let outcome = handle(
        &mut f.store,
        &caller(&[Scope::ReadDocuments]),
        doc_update(&room, update),
        at(),
    );
    let ProtocolMessage::Ack { status, .. } = outcome.reply.expect("an ack") else {
        panic!("expected an Ack");
    };
    assert_eq!(status, UpdateStatusCode::PermissionDenied);
    assert!(outcome.broadcast.is_none());
    assert!(f.store.documents(f.workspace).unwrap().is_empty());
}

#[test]
fn an_update_to_an_unknown_room_is_refused() {
    let mut f = fixture();
    let outcome = handle(
        &mut f.store,
        &caller(Scope::ALL),
        doc_update(&Room::Document(DocumentId::new()).to_id(), vec![1]),
        at(),
    );
    assert!(matches!(
        outcome.reply,
        Some(ProtocolMessage::RoomError { .. })
    ));
    assert!(outcome.broadcast.is_none());
}

// ------------------------------------------------------------------ leaving

#[test]
fn leaving_reports_the_room() {
    let mut f = fixture();
    let room = f.manifest_room.clone();
    let outcome = handle(
        &mut f.store,
        &caller(Scope::ALL),
        ProtocolMessage::Leave {
            crdt: CrdtType::Loro,
            room_id: room.clone(),
        },
        at(),
    );
    assert_eq!(outcome.left, Some(Room::parse(&room).unwrap()));
    assert!(outcome.reply.is_none());
}

// ------------------------------------------------------------------ refusals

#[test]
fn a_message_the_store_sends_is_not_one_it_accepts() {
    // A client sending a response is confused; saying so is more use than
    // ignoring it.
    let mut f = fixture();
    let room = f.manifest_room.clone();
    for message in [
        ProtocolMessage::Ack {
            crdt: CrdtType::Loro,
            room_id: room.clone(),
            ref_id: BatchId([0; 8]),
            status: UpdateStatusCode::Ok,
        },
        ProtocolMessage::JoinResponseOk {
            crdt: CrdtType::Loro,
            room_id: room.clone(),
            permission: Permission::Write,
            version: Vec::new(),
            extra: None,
        },
    ] {
        let outcome = handle(&mut f.store, &caller(Scope::ALL), message, at());
        assert!(matches!(
            outcome.reply,
            Some(ProtocolMessage::RoomError { .. })
        ));
    }
}

#[test]
fn a_fragment_is_refused_explicitly_rather_than_ignored() {
    // Silently ignoring one would look exactly like a dropped edit.
    let mut f = fixture();
    let outcome = handle(
        &mut f.store,
        &caller(Scope::ALL),
        ProtocolMessage::DocUpdateFragment {
            crdt: CrdtType::Loro,
            room_id: f.manifest_room.clone(),
            batch_id: BatchId([0; 8]),
            index: 0,
            fragment: vec![1],
        },
        at(),
    );
    let Some(ProtocolMessage::RoomError { message, .. }) = outcome.reply else {
        panic!("a fragment was accepted or ignored");
    };
    assert!(message.contains("fragmented"), "{message}");
}

// ------------------------------------------------------------------ codec

#[test]
fn every_message_the_store_sends_survives_the_official_codec() {
    // The constraint that keeps this a standard protocol: if the published
    // encoder cannot carry a message, it is not a protocol message.
    use loro_protocol::encoding;
    let room = "workspace:00000000-0000-4000-8000-000000000000:manifest".to_string();
    for message in [
        ProtocolMessage::JoinResponseOk {
            crdt: CrdtType::Loro,
            room_id: room.clone(),
            permission: Permission::Write,
            version: vec![1, 2, 3],
            extra: None,
        },
        ProtocolMessage::JoinError {
            crdt: CrdtType::Loro,
            room_id: room.clone(),
            code: JoinErrorCode::AuthFailed,
            message: "no".into(),
            receiver_version: None,
            app_code: None,
        },
        ProtocolMessage::JoinError {
            crdt: CrdtType::Loro,
            room_id: room.clone(),
            code: JoinErrorCode::AppError,
            message: "no such room".into(),
            receiver_version: None,
            app_code: Some("document_not_found".into()),
        },
        ProtocolMessage::Ack {
            crdt: CrdtType::Loro,
            room_id: room.clone(),
            ref_id: BatchId([9; 8]),
            status: UpdateStatusCode::Ok,
        },
        ProtocolMessage::RoomError {
            crdt: CrdtType::Loro,
            room_id: room.clone(),
            code: RoomErrorCode::RejoinSuggested,
            message: "rejoin".into(),
        },
        ProtocolMessage::DocUpdate {
            crdt: CrdtType::Loro,
            room_id: room.clone(),
            updates: vec![vec![1, 2, 3]],
            batch_id: BatchId([4; 8]),
        },
    ] {
        let encoded = encoding::encode(&message).expect("the official encoder must carry it");
        let mut decoded = encoding::try_decode(&encoded).expect("and decode it again");

        // **A codec quirk worth knowing.** `JoinResponseOk::extra` is an
        // `Option<Vec<u8>>`, and the wire format has no way to tell "absent"
        // from "present and empty": `None` encodes as a zero-length byte string
        // and comes back as `Some([])`. Nothing in the store reads `extra`, so
        // it costs nothing here — but code that ever does must treat the two as
        // the same, not test for `None`.
        if let ProtocolMessage::JoinResponseOk { extra, .. } = &mut decoded
            && extra.as_deref() == Some(&[][..])
        {
            *extra = None;
        }
        assert_eq!(decoded, message);
    }
}

#[test]
fn an_app_code_only_survives_the_wire_beside_app_error() {
    // The codec guards the field with `if matches!(code, AppError)`. Anywhere
    // else it is written by nobody and read as `None` — so a refusal that
    // relies on it must use `AppError`, and one that does not must leave it
    // unset rather than set something that vanishes.
    use loro_protocol::encoding;
    let room = "document:00000000-0000-4000-8000-000000000000".to_string();

    let carried = ProtocolMessage::JoinError {
        crdt: CrdtType::Loro,
        room_id: room.clone(),
        code: JoinErrorCode::AppError,
        message: "no such room".into(),
        receiver_version: None,
        app_code: Some("document_not_found".into()),
    };
    let back = encoding::try_decode(&encoding::encode(&carried).unwrap()).unwrap();
    let ProtocolMessage::JoinError { app_code, .. } = back else {
        panic!("expected JoinError");
    };
    assert_eq!(app_code.as_deref(), Some("document_not_found"));

    let dropped = ProtocolMessage::JoinError {
        crdt: CrdtType::Loro,
        room_id: room,
        code: JoinErrorCode::AuthFailed,
        message: "no".into(),
        receiver_version: None,
        app_code: Some("forbidden".into()),
    };
    let back = encoding::try_decode(&encoding::encode(&dropped).unwrap()).unwrap();
    let ProtocolMessage::JoinError { app_code, .. } = back else {
        panic!("expected JoinError");
    };
    assert_eq!(
        app_code, None,
        "the codec started carrying this; the rule can relax"
    );
}
