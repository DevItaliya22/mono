// SPDX-License-Identifier: MPL-2.0

use std::net::{Shutdown, TcpListener};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::*;

fn server(frame: Message) -> (Discovery, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        socket.send(frame).unwrap();
        // Keep the peer healthy and idle until the client explicitly closes.
        let _ = socket.read();
    });
    (
        Discovery {
            port,
            token: "test-token".into(),
            expires_at_millis: i64::MAX,
        },
        server,
    )
}

#[test]
fn a_generation_is_the_only_payload() {
    let (discovery, server) = server(Message::Binary(41_u64.to_le_bytes().to_vec().into()));
    let mut watch = CatalogWatch::open(&discovery).unwrap();

    assert_eq!(watch.next_generation().unwrap(), 41);
    watch
        .abort_handle()
        .unwrap()
        .shutdown(Shutdown::Both)
        .unwrap();
    server.join().unwrap();
}

#[test]
fn an_invalid_generation_fails_instead_of_becoming_a_wake() {
    let (discovery, server) = server(Message::Binary(vec![1, 2, 3].into()));
    let mut watch = CatalogWatch::open(&discovery).unwrap();

    let error = watch.next_generation().unwrap_err();
    assert_eq!(error.code(), Code::CorruptState);
    watch
        .abort_handle()
        .unwrap()
        .shutdown(Shutdown::Both)
        .unwrap();
    server.join().unwrap();
}

#[test]
fn healthy_idle_blocks_until_the_abort_handle_closes_it() {
    let (discovery, server) = server(Message::Binary(0_u64.to_le_bytes().to_vec().into()));
    let mut watch = CatalogWatch::open(&discovery).unwrap();
    assert_eq!(watch.next_generation().unwrap(), 0);
    let abort = watch.abort_handle().unwrap();
    let (finished, completion) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || finished.send(watch.next_generation()).unwrap());

    assert_eq!(
        completion.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout),
        "a healthy watch returned without a catalog change"
    );
    abort.shutdown(Shutdown::Both).unwrap();
    assert!(
        completion
            .recv_timeout(Duration::from_secs(2))
            .expect("the aborted read unblocks")
            .is_err()
    );
    reader.join().unwrap();
    server.join().unwrap();
}
