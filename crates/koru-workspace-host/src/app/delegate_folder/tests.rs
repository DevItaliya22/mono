// SPDX-License-Identifier: MPL-2.0

use super::*;
use std::time::Duration;

#[test]
fn a_blocked_probe_returns_to_the_caller_and_queues_before_waking() {
    let caller = std::thread::current().id();
    let (entered, started) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let (woke, wake) = mpsc::channel();
    let reply = spawn_check(
        move || {
            entered.send(std::thread::current().id()).unwrap();
            wait.recv_timeout(Duration::from_secs(5)).unwrap();
            DelegateFolder::Chosen("chosen-folder".into())
        },
        move || woke.send(()).unwrap(),
    )
    .unwrap();

    assert_ne!(
        started.recv_timeout(Duration::from_secs(5)).unwrap(),
        caller
    );
    assert_eq!(reply.try_recv(), Err(mpsc::TryRecvError::Empty));
    assert_eq!(wake.try_recv(), Err(mpsc::TryRecvError::Empty));
    release.send(()).unwrap();
    wake.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        reply.try_recv().unwrap(),
        DelegateFolder::Chosen("chosen-folder".into())
    );
}

#[test]
fn a_superseded_check_cannot_deliver_or_wake_the_window() {
    let (release, wait) = mpsc::channel();
    let (woke, wake) = mpsc::channel();
    let reply = spawn_check(
        move || {
            wait.recv_timeout(Duration::from_secs(5)).unwrap();
            DelegateFolder::NotWritable
        },
        move || woke.send(()).unwrap(),
    )
    .unwrap();

    // A new picker or a closed window drops its predecessor's reply channel.
    drop(reply);
    release.send(()).unwrap();
    assert_eq!(
        wake.recv_timeout(Duration::from_secs(5)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
}

#[test]
fn a_refused_probe_is_delivered_without_a_timer() {
    let (woke, wake) = mpsc::channel();
    let reply = spawn_check(
        || DelegateFolder::NotWritable,
        move || woke.send(()).unwrap(),
    )
    .unwrap();
    wake.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(reply.try_recv().unwrap(), DelegateFolder::NotWritable);
}
