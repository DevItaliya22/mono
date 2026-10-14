// SPDX-License-Identifier: MPL-2.0

use super::*;

fn ack(status: UpdateStatusCode) -> Ack {
    let Some(Event::Acked { ack, .. }) = translate(ProtocolMessage::Ack {
        crdt: CrdtType::Loro,
        room_id: "document:00000000-0000-4000-8000-000000000000".into(),
        ref_id: BatchId([7; 8]),
        status,
    }) else {
        panic!("expected an Acked event");
    };
    ack
}

#[test]
fn only_ok_counts_as_durable() {
    // Treating any other status as an acknowledgement is how a client discards
    // work the store never wrote.
    assert!(ack(UpdateStatusCode::Ok).durable);
    for status in [
        UpdateStatusCode::Unknown,
        UpdateStatusCode::PermissionDenied,
        UpdateStatusCode::InvalidUpdate,
        UpdateStatusCode::PayloadTooLarge,
        UpdateStatusCode::RateLimited,
        UpdateStatusCode::FragmentTimeout,
        UpdateStatusCode::AppError,
    ] {
        assert!(!ack(status).durable, "{status:?} was treated as durable");
    }
}

#[test]
fn an_ack_carries_the_batch_it_answers() {
    // Without this a client cannot tell which of several outstanding updates
    // was acknowledged.
    assert_eq!(ack(UpdateStatusCode::Ok).batch, BatchId([7; 8]));
}

#[test]
fn every_message_a_store_sends_becomes_an_event() {
    let room = "document:00000000-0000-4000-8000-000000000000".to_string();
    let sent = [
        ProtocolMessage::JoinResponseOk {
            crdt: CrdtType::Loro,
            room_id: room.clone(),
            permission: Permission::Write,
            version: vec![1],
            extra: None,
        },
        ProtocolMessage::JoinError {
            crdt: CrdtType::Loro,
            room_id: room.clone(),
            code: JoinErrorCode::AppError,
            message: "no such room".into(),
            receiver_version: None,
            app_code: Some("document_not_found".into()),
        },
        ProtocolMessage::DocUpdate {
            crdt: CrdtType::Loro,
            room_id: room.clone(),
            updates: vec![vec![1, 2]],
            batch_id: BatchId([0; 8]),
        },
        ProtocolMessage::RoomError {
            crdt: CrdtType::Loro,
            room_id: room.clone(),
            code: RoomErrorCode::RejoinSuggested,
            message: "rejoin".into(),
        },
    ];
    for message in sent {
        assert!(
            translate(message.clone()).is_some(),
            "{message:?} was dropped"
        );
    }
}

#[test]
fn a_request_is_not_an_event() {
    // A store does not send these to a client; producing an event for one
    // would invite a client to act on its own traffic.
    assert!(
        translate(ProtocolMessage::JoinRequest {
            crdt: CrdtType::Loro,
            room_id: "x".into(),
            auth: Vec::new(),
            version: Vec::new(),
        })
        .is_none()
    );
    assert!(
        translate(ProtocolMessage::Leave {
            crdt: CrdtType::Loro,
            room_id: "x".into(),
        })
        .is_none()
    );
}

#[test]
fn a_refusal_carries_koru_s_token_when_the_wire_allows_it() {
    let Some(Event::JoinRefused { app_code, .. }) = translate(ProtocolMessage::JoinError {
        crdt: CrdtType::Loro,
        room_id: "x".into(),
        code: JoinErrorCode::AppError,
        message: "no".into(),
        receiver_version: None,
        app_code: Some("document_not_found".into()),
    }) else {
        panic!("expected JoinRefused");
    };
    assert_eq!(app_code.as_deref(), Some("document_not_found"));
}
