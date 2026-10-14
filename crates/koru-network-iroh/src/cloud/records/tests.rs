// SPDX-License-Identifier: MPL-2.0

use super::*;

#[tokio::test]
async fn occupied_budget_waits_and_cancellation_does_not_leak_a_charge() {
    let budget = MemoryBudget::new(100);
    let held = budget.reserve(100).unwrap();
    let mut waiting = Box::pin(budget.reserve_wait(80));
    assert!(
        tokio::time::timeout(Duration::from_millis(1), &mut waiting)
            .await
            .is_err()
    );
    assert_eq!(budget.used(), 100);
    drop(waiting);
    drop(held);
    assert_eq!(budget.used(), 0);
    let held = budget.reserve_wait(100).await.unwrap();
    assert_eq!(budget.used(), 100);
    drop(held);
    assert_eq!(budget.used(), 0);
}

#[tokio::test]
async fn releasing_memory_wakes_a_waiting_transport_reservation() {
    let budget = MemoryBudget::new(100);
    let held = budget.reserve(100).unwrap();
    let waiter_budget = budget.clone();
    let waiter = tokio::spawn(async move { waiter_budget.reserve_wait(100).await.unwrap() });
    tokio::task::yield_now().await;
    assert!(!waiter.is_finished());
    drop(held);
    let received = tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(budget.used(), 100);
    drop(received);
    assert_eq!(budget.used(), 0);
}
use koru_replication::{cloud::transfer::Control, protocol::PeerFrame};
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn record_roundtrip_keeps_budget_until_dispatch() {
    let budget = MemoryBudget::new(1024 * 1024);
    let source = Record::Control(Control::policy(1, 200000));
    let mut bytes = Vec::new();
    write(
        &mut bytes,
        &source,
        Instant::now() + Duration::from_secs(1),
        &budget,
    )
    .await
    .unwrap();
    assert_eq!(budget.used(), 0);
    let record = read(
        &mut bytes.as_slice(),
        Instant::now() + Duration::from_secs(1),
        &budget,
    )
    .await
    .unwrap();
    assert!(budget.used() > 0);
    assert!(matches!(
        record.record(),
        Record::Control(Control::TransferPolicy {
            automatic_limit_bytes: 200000,
            ..
        })
    ));
    drop(record);
    assert_eq!(budget.used(), 0);
}

#[tokio::test]
async fn pending_write_keeps_borrowed_record_and_releases_output_charge_on_cancellation() {
    let budget = MemoryBudget::new(1024 * 1024);
    let encoded = koru_replication::cloud::transfer::encode_record(&Record::Control(
        Control::policy(1, 200000),
    ))
    .unwrap();
    let received = read(
        &mut encoded.as_slice(),
        Instant::now() + Duration::from_secs(10),
        &budget,
    )
    .await
    .unwrap();
    let input_charge = budget.used();
    assert!(input_charge > 0);
    let (mut writer, _reader) = tokio::io::duplex(1);
    let mut pending = Box::pin(write(
        &mut writer,
        received.record(),
        Instant::now() + Duration::from_secs(10),
        &budget,
    ));
    std::future::poll_fn(|cx| {
        assert!(pending.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert!(budget.used() > input_charge);
    drop(pending);
    assert_eq!(budget.used(), input_charge);
    drop(received);
    assert_eq!(budget.used(), 0);
}

#[tokio::test]
async fn invalid_header_and_budget_refusal_consume_no_payload() {
    for (length, tag, maximum, expected) in [
        (16777218u32, 0, 1024usize, "sync_transfer_invalid"),
        (4098, 1, 1024, "sync_transfer_invalid"),
        (100, 2, 1024, "sync_transfer_invalid"),
        (100, 1, 0, "sync_unavailable"),
    ] {
        let mut bytes = length.to_be_bytes().to_vec();
        bytes.push(tag);
        bytes.extend([1, 2, 3]);
        let mut cursor = std::io::Cursor::new(bytes);
        let budget = MemoryBudget::new(maximum);
        let result = read(
            &mut cursor,
            Instant::now() + Duration::from_secs(1),
            &budget,
        )
        .await;
        assert_eq!(result.err().unwrap().code(), expected);
        assert_eq!(cursor.position(), 5);
        assert_eq!(budget.used(), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn start_prefix_and_control_json_must_arrive_within_ten_seconds() {
    let (mut writer, mut reader) = tokio::io::duplex(16);
    writer.write_all(&[0]).await.unwrap();
    let budget = MemoryBudget::new(1024 * 1024);
    let now = Instant::now();
    assert_eq!(
        read(&mut reader, now + Duration::from_secs(600), &budget)
            .await
            .err()
            .unwrap()
            .code(),
        "sync_transfer_timeout"
    );
    assert_eq!(Instant::now() - now, Duration::from_secs(10));
    assert_eq!(budget.used(), 0);

    let (mut writer, mut reader) = tokio::io::duplex(16);
    writer.write_all(&[0, 0, 0, 100, 1, b'{']).await.unwrap();
    let now = Instant::now();
    assert_eq!(
        read(&mut reader, now + Duration::from_secs(600), &budget)
            .await
            .err()
            .unwrap()
            .code(),
        "sync_transfer_timeout"
    );
    assert_eq!(Instant::now() - now, Duration::from_secs(10));
    assert_eq!(budget.used(), 0);
}

#[tokio::test(start_paused = true)]
async fn data_payload_obeys_absolute_transfer_deadline_not_ten_second_control_wait() {
    let (mut writer, mut reader) = tokio::io::duplex(16);
    writer.write_all(&[0, 0, 0, 100, 0, b'{']).await.unwrap();
    let budget = MemoryBudget::new(1024 * 1024);
    let now = Instant::now();
    assert_eq!(
        read(&mut reader, now + Duration::from_secs(542), &budget)
            .await
            .err()
            .unwrap()
            .code(),
        "sync_transfer_timeout"
    );
    assert_eq!(Instant::now() - now, Duration::from_secs(542));
    assert_eq!(budget.used(), 0);
}

#[tokio::test]
async fn eof_and_malformed_json_release_all_reserved_bytes() {
    // The two are deliberately different answers. A payload that stops early
    // is a peer that went away; JSON we read and refused is a peer at fault.
    // Both used to be `sync_transfer_invalid`, which said the second about the
    // first and sent two investigations into the codec for no reason.
    for (bytes, expected) in [
        (vec![0, 0, 0, 100, 0, b'{'], "sync_peer_closed"),
        (vec![0, 0, 0, 3, 1, b'{', b'}'], "sync_transfer_invalid"),
    ] {
        let budget = MemoryBudget::new(1024 * 1024);
        let result = read(
            &mut bytes.as_slice(),
            Instant::now() + Duration::from_secs(1),
            &budget,
        )
        .await;
        assert_eq!(result.err().unwrap().code(), expected);
        assert_eq!(budget.used(), 0);
    }
}

#[tokio::test]
async fn a_peer_that_never_writes_a_byte_is_closed_not_invalid() {
    let budget = MemoryBudget::new(1024 * 1024);
    let result = read(
        &mut [].as_slice(),
        Instant::now() + Duration::from_secs(1),
        &budget,
    )
    .await;
    assert_eq!(result.err().unwrap().code(), "sync_peer_closed");
    assert_eq!(budget.used(), 0);
}

#[tokio::test]
async fn write_uses_actual_json_bound_not_only_raw_byte_count() {
    let budget = MemoryBudget::new(1024 * 1024 * 1024);
    let frame = Record::Peer(PeerFrame::Loro {
        workspace_id: koru_store_core::WorkspaceId::new(),
        message: vec![255; MAX_PAYLOAD / 3],
    });
    let mut target = Vec::new();
    assert_eq!(
        write(
            &mut target,
            &frame,
            Instant::now() + Duration::from_secs(10),
            &budget
        )
        .await
        .err()
        .unwrap()
        .code(),
        "sync_transfer_invalid"
    );
    assert!(target.is_empty());
    assert_eq!(budget.used(), 0);
}
