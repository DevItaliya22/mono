// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_store_api::cloud_changes::{Changes, Dataset};
use koru_store_core::DocumentId;
use std::sync::Arc;

#[tokio::test]
async fn a_commit_during_delivery_is_not_lost_and_drop_releases_interest() {
    let hub = Arc::new(Changes::default());
    let key = Dataset::DocumentView(DocumentId::from_uuid(uuid::Uuid::from_u128(1)));
    let watch = hub.subscribe(key);
    // Registration happens before the initial read/task starts. A racing
    // commit must survive that gap, and repeated wakes may coalesce.
    for _ in 0..100 {
        hub.changed(key);
    }
    let (sent, mut received) = tokio::sync::mpsc::unbounded_channel();
    let callback_hub = hub.clone();
    let count = std::sync::atomic::AtomicUsize::new(0);
    let observer =
        DocumentObserver::start(&tokio::runtime::Handle::current(), watch, None, move || {
            if count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0 {
                callback_hub.changed(key);
            }
            sent.send(()).is_ok()
        });
    for _ in 0..2 {
        tokio::time::timeout(std::time::Duration::from_secs(1), received.recv())
            .await
            .unwrap()
            .unwrap();
    }
    assert!(received.try_recv().is_err());
    drop(observer);
    // Dropping the task releases its callback and watch, without another edit
    // or polling timeout being needed to let cancellation finish.
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(1), received.recv())
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn manifest_changes_refresh_an_open_document_without_body_changes() {
    let hub = Changes::default();
    let document = Dataset::DocumentView(DocumentId::from_uuid(uuid::Uuid::from_u128(1)));
    let manifest = Dataset::DocumentView(DocumentId::from_uuid(uuid::Uuid::from_u128(2)));
    let (sent, mut received) = tokio::sync::mpsc::unbounded_channel();
    let _observer = DocumentObserver::start(
        &tokio::runtime::Handle::current(),
        hub.subscribe(document),
        Some(hub.subscribe(manifest)),
        move || sent.send(()).is_ok(),
    );
    hub.changed(manifest);
    tokio::time::timeout(std::time::Duration::from_secs(1), received.recv())
        .await
        .unwrap()
        .unwrap();
}
