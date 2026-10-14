// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_replication::{cloud::transfer::Control, protocol::PeerFrame};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Retained {
    record: Record,
    dropped: Arc<AtomicUsize>,
}
impl Retained {
    fn record(&self) -> &Record {
        &self.record
    }
}
impl Drop for Retained {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}
fn policy() -> Record {
    Record::Control(Control::policy(1, 200_000))
}

#[tokio::test]
async fn write_only_wait_preserves_the_original_partial_read() {
    use tokio::io::AsyncWriteExt;
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let deadline = Instant::now() + Duration::from_secs(20);
    let (reader, mut peer) = tokio::io::duplex(4096);
    let mut io = Duplex::new(reader, Vec::<u8>::new(), deadline, budget.clone());
    let encoded = koru_replication::cloud::transfer::encode_record(&policy()).unwrap();
    peer.write_all(&encoded[..2]).await.unwrap();
    poll_pending(io.next()).await;
    io.send(policy(), |value| value, deadline).unwrap();
    assert!(matches!(io.next_write().await.unwrap(), Event::Written));
    assert!(io.status().reading);
    peer.write_all(&encoded[2..]).await.unwrap();
    let Event::Received(received) = io.next().await.unwrap() else {
        panic!("received")
    };
    assert!(matches!(
        received.record(),
        Record::Control(Control::TransferPolicy {
            policy_generation: 1,
            ..
        })
    ));
    drop((received, io));
    assert_eq!(budget.used(), 0);
}
fn retained(dropped: &Arc<AtomicUsize>) -> Retained {
    Retained {
        record: policy(),
        dropped: dropped.clone(),
    }
}
async fn poll_pending(future: impl Future) {
    tokio::pin!(future);
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    // Deliberately cancel this event waiter, not the retained I/O owner.
}

#[tokio::test]
async fn simultaneous_large_records_cross_under_backpressure() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let deadline = Instant::now() + Duration::from_secs(20);
    let (a, b) = tokio::io::duplex(1024);
    let (ar, aw) = tokio::io::split(a);
    let (br, bw) = tokio::io::split(b);
    let mut a = Duplex::new(ar, aw, deadline, budget.clone());
    let mut b = Duplex::new(br, bw, deadline, budget.clone());
    // The record codec is content-opaque. Large generated metadata here tests
    // bidirectional framing, not semantic admission or durable document sync.
    let left = "left".repeat(70_000);
    let right = "right".repeat(60_000);
    a.send(
        Record::Peer(PeerFrame::Error { code: left.clone() }),
        |v| v,
        deadline,
    )
    .unwrap();
    b.send(
        Record::Peer(PeerFrame::Error {
            code: right.clone(),
        }),
        |v| v,
        deadline,
    )
    .unwrap();
    async fn exchange<'a, R, W>(io: &mut Duplex<'a, R, W>, expected: &str)
    where
        R: AsyncRead + Unpin + Send + 'a,
        W: AsyncWrite + Unpin + Send + 'a,
    {
        let (mut read, mut written) = (false, false);
        while !read || !written {
            match io.next().await.unwrap() {
                Event::Received(value) => {
                    assert!(!read);
                    let Record::Peer(PeerFrame::Error { code }) = value.record() else {
                        panic!("wrong frame")
                    };
                    assert_eq!(code, expected);
                    read = true;
                }
                Event::Written => {
                    assert!(!written);
                    written = true;
                }
            }
        }
        assert!(io.can_send());
    }
    tokio::time::timeout_at(deadline, async {
        tokio::join!(exchange(&mut a, &right), exchange(&mut b, &left));
    })
    .await
    .unwrap();
    assert_eq!(budget.used(), 0);
}

#[tokio::test]
async fn canceled_event_wait_preserves_partial_read_write_and_original_receipt() {
    let budget = MemoryBudget::new(1024 * 1024);
    let deadline = Instant::now() + Duration::from_secs(10);
    let (local, peer) = tokio::io::duplex(8);
    let (reader, writer) = tokio::io::split(local);
    let (mut peer_reader, mut peer_writer) = tokio::io::split(peer);
    let mut io = Duplex::new(reader, writer, deadline, budget.clone());
    let dropped = Arc::new(AtomicUsize::new(0));
    let encoded = encode_record(&policy()).unwrap();
    peer_writer.write_all(&encoded[..2]).await.unwrap();
    io.send(retained(&dropped), Retained::record, deadline)
        .unwrap();
    poll_pending(io.next()).await;
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert!(budget.used() > 0);
    assert!(io.status().reading && io.status().writing);

    // A storage completion could have won the outer select. Continue the SAME
    // prefix and write, not a new read interpreting the remaining prefix as one.
    let (event, sent) = tokio::join!(io.next(), peer_writer.write_all(&encoded[2..]));
    sent.unwrap();
    let Event::Received(value) = event.unwrap() else {
        panic!("outgoing is still blocked")
    };
    assert_eq!(encode_record(value.record()).unwrap(), encoded);
    drop(value);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert!(!io.can_send());

    let (event, received) = tokio::join!(io.next(), read(&mut peer_reader, deadline, &budget));
    assert!(matches!(event.unwrap(), Event::Written));
    assert_eq!(encode_record(received.unwrap().record()).unwrap(), encoded);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(budget.used(), 0);
    assert!(io.can_send());
    // Keep both peer halves alive until the test ends; split read-half Drop
    // alone does not close a DuplexStream and is not used as EOF evidence.
    drop((peer_reader, peer_writer));
}

#[tokio::test(start_paused = true)]
async fn repeated_cancellation_never_refreshes_partial_control_deadline() {
    let budget = MemoryBudget::new(1024 * 1024);
    let deadline = Instant::now() + Duration::from_secs(600);
    let (local, mut peer) = tokio::io::duplex(16);
    let (reader, writer) = tokio::io::split(local);
    let mut io = Duplex::new(reader, writer, deadline, budget.clone());
    let started = Instant::now();
    peer.write_all(&[0]).await.unwrap();
    poll_pending(io.next()).await;
    for _ in 0..9 {
        tokio::time::advance(Duration::from_secs(1)).await;
        poll_pending(io.next()).await;
    }
    assert_eq!(io.next().await.err(), Some(Error::Timeout));
    assert_eq!(Instant::now() - started, Duration::from_secs(10));
    assert_eq!(budget.used(), 0);
    assert_eq!(
        io.status(),
        Status {
            reading: false,
            writing: false,
            closed: Some(Error::Timeout)
        }
    );
    assert_eq!(io.next().await.err(), Some(Error::Timeout));
}

#[tokio::test(start_paused = true)]
async fn write_deadline_survives_canceled_wait_and_is_capped_by_session() {
    for requested_secs in [3, 900] {
        let budget = MemoryBudget::new(1024 * 1024);
        let started = Instant::now();
        let (local, _peer) = tokio::io::duplex(1);
        let (reader, writer) = tokio::io::split(local);
        let mut io = Duplex::new(
            reader,
            writer,
            started + Duration::from_secs(8),
            budget.clone(),
        );
        let dropped = Arc::new(AtomicUsize::new(0));
        io.send(
            retained(&dropped),
            Retained::record,
            started + Duration::from_secs(requested_secs),
        )
        .unwrap();
        poll_pending(io.next()).await;
        tokio::time::advance(Duration::from_secs(1)).await;
        poll_pending(io.next()).await;
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        assert_eq!(io.next().await.err(), Some(Error::Timeout));
        assert_eq!(
            Instant::now() - started,
            Duration::from_secs(requested_secs.min(8))
        );
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        assert_eq!(budget.used(), 0);
    }
}

#[tokio::test]
async fn busy_refusal_does_not_replace_the_original_write_and_drop_releases_it() {
    let budget = MemoryBudget::new(1024 * 1024);
    let deadline = Instant::now() + Duration::from_secs(10);
    let (local, _peer) = tokio::io::duplex(1);
    let (reader, writer) = tokio::io::split(local);
    let mut io = Duplex::new(reader, writer, deadline, budget.clone());
    let first = Arc::new(AtomicUsize::new(0));
    let second = Arc::new(AtomicUsize::new(0));
    io.send(retained(&first), Retained::record, deadline)
        .unwrap();
    poll_pending(io.next()).await;
    assert_eq!(
        io.send(retained(&second), Retained::record, deadline),
        Err(Error::Unavailable)
    );
    assert_eq!(first.load(Ordering::SeqCst), 0);
    assert_eq!(second.load(Ordering::SeqCst), 1);
    assert!(budget.used() > 0);
    drop(io);
    assert_eq!(first.load(Ordering::SeqCst), 1);
    assert_eq!(budget.used(), 0);
}

#[tokio::test]
async fn explicit_fence_and_eof_release_pending_values_without_a_write_event() {
    for eof in [false, true] {
        let budget = MemoryBudget::new(1024 * 1024);
        let deadline = Instant::now() + Duration::from_secs(10);
        let (local, mut peer) = tokio::io::duplex(8);
        let (reader, writer) = tokio::io::split(local);
        let mut io = Duplex::new(reader, writer, deadline, budget.clone());
        let dropped = Arc::new(AtomicUsize::new(0));
        peer.write_all(&[0, 0, 0, 100, 1, b'{']).await.unwrap();
        io.send(retained(&dropped), Retained::record, deadline)
            .unwrap();
        poll_pending(io.next()).await;
        assert!(budget.used() > 0);
        if eof {
            drop(peer);
            // A peer that hangs up mid-record is Closed, not Invalid: the
            // prefix it did send was well formed and nothing was refused.
            assert_eq!(io.next().await.err(), Some(Error::Closed));
        } else {
            assert_eq!(io.cancel(Error::PolicyChanged), Error::PolicyChanged);
            assert_eq!(io.next().await.err(), Some(Error::PolicyChanged));
        }
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        assert_eq!(budget.used(), 0);
        assert!(!io.can_send());
    }
}

#[tokio::test(start_paused = true)]
async fn buffered_control_after_deadline_is_not_rescued_by_ready_io() {
    let budget = MemoryBudget::new(1024 * 1024);
    let (local, mut peer) = tokio::io::duplex(4096);
    let (reader, writer) = tokio::io::split(local);
    let started = Instant::now();
    let mut io = Duplex::new(
        reader,
        writer,
        started + Duration::from_secs(600),
        budget.clone(),
    );
    let encoded = encode_record(&policy()).unwrap();
    peer.write_all(&encoded[..1]).await.unwrap();
    poll_pending(io.next()).await;
    tokio::time::advance(Duration::from_secs(10)).await;
    peer.write_all(&encoded[1..]).await.unwrap();
    assert_eq!(io.next().await.err(), Some(Error::Timeout));
    assert_eq!(budget.used(), 0);
}

#[tokio::test(start_paused = true)]
async fn write_first_polled_after_deadline_emits_no_record() {
    let budget = MemoryBudget::new(1024 * 1024);
    let (reader, _peer) = tokio::io::duplex(4096);
    let mut emitted = Vec::new();
    let started = Instant::now();
    let mut io = Duplex::new(
        reader,
        &mut emitted,
        started + Duration::from_secs(600),
        budget.clone(),
    );
    let dropped = Arc::new(AtomicUsize::new(0));
    io.send(
        retained(&dropped),
        Retained::record,
        started + Duration::from_secs(3),
    )
    .unwrap();
    tokio::time::advance(Duration::from_secs(3)).await;
    assert_eq!(io.next().await.err(), Some(Error::Timeout));
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(budget.used(), 0);
    drop(io);
    // The writer is an explicit memory buffer: inspect bytes, not whether a
    // service/connection happens to be available in the test environment.
    assert!(emitted.is_empty());
}
