// SPDX-License-Identifier: MPL-2.0

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
async fn room_interest_negotiates_and_leaves_on_the_same_stream() {
    let (client, mut reader, mut writer, _, scope, budget) =
        super::super::tests::pair_with_workspace(true).await;
    let mut sync = Reconciler::new(
        client,
        |_| Ok(async { Ok(Answer::Ready(VersionVector::default())) }),
        |_| Ok(async { Ok(Answer::Ready(None)) }),
    );
    let unit = Unit::Document {
        workspace_id: koru_device::cloud::CloudId::from_bytes(
            scope.eligibility_workspace.as_uuid().into_bytes(),
        ),
        document_id: koru_device::cloud::CloudId::from_bytes(
            koru_store_core::DocumentId::new().as_uuid().into_bytes(),
        ),
    };
    sync.rooms([unit]).unwrap();
    sync.start(unit).unwrap();
    let peer = async {
        let deadline = Instant::now() + Duration::from_secs(2);
        let request = records::read(&mut reader, deadline, &budget).await.unwrap();
        let Record::Peer(frame) = request.record() else {
            panic!("expected join")
        };
        let mut messages = metadata::decode(scope, frame.clone(), 4).unwrap();
        let Metadata::Request(request) = messages.remove(0) else {
            panic!("expected join")
        };
        assert!(request.live);
        assert_eq!(request.unit, unit);
        let response = request
            .response(scope, VersionVector::default().encode())
            .unwrap()
            .unwrap();
        records::write(&mut writer, &response, deadline, &budget)
            .await
            .unwrap();
        let leave = records::read(&mut reader, deadline, &budget).await.unwrap();
        let Record::Peer(frame) = leave.record() else {
            panic!("expected leave")
        };
        assert!(
            matches!(metadata::decode(scope, frame.clone(), 4).unwrap().as_slice(),
            [Metadata::Leave(left)] if *left == unit)
        );
    };
    let device = async {
        loop {
            if matches!(sync.next().await.unwrap(), Event::Settled { .. }) {
                break;
            }
        }
        sync.rooms([]).unwrap();
        // Drive the retained writer until the peer observes Leave. No second
        // connection, manifest inventory or simulated durable receipt.
        loop {
            sync.next().await.unwrap();
        }
    };
    tokio::time::timeout(Duration::from_secs(3), async {
        tokio::select! { () = peer => {}, () = device => unreachable!() }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_comparison_page_checks_many_documents_without_loro_jobs() {
    use koru_replication::protocol::{CLOUD_COMPARE_LIMIT, CloudDocumentDigest, PeerFrame};
    let (client, mut reader, mut writer, _, scope, budget) =
        super::super::tests::pair_with_workspace(true).await;
    let mut sync = Reconciler::new(
        client,
        |_| -> Result<std::future::Ready<Result<Answer<VersionVector>, Error>>, Error> {
            panic!("inventory must not read document content")
        },
        |_| -> Result<std::future::Ready<Result<Answer<Option<Export>>, Error>>, Error> {
            panic!("inventory must not export document content")
        },
    );
    let documents = (0..CLOUD_COMPARE_LIMIT)
        .map(|_| CloudDocumentDigest {
            document_id: koru_store_core::DocumentId::new(),
            digest: [7; 32],
        })
        .collect::<Vec<_>>();
    let expected = documents
        .iter()
        .take(CLOUD_COMPARE_LIMIT - 1)
        .map(|d| d.document_id)
        .collect::<std::collections::HashSet<_>>();
    let response = expected.iter().copied().collect::<Vec<_>>();
    let peer = async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let request = super::super::records::read(&mut reader, deadline, &budget)
            .await
            .unwrap();
        assert!(
            matches!(request.record(), super::super::Record::Peer(PeerFrame::CloudCompare { documents, .. }) if documents.len() == CLOUD_COMPARE_LIMIT)
        );
        super::super::records::write(
            &mut writer,
            &super::super::Record::Peer(PeerFrame::SyncRequest),
            deadline,
            &budget,
        )
        .await
        .unwrap();
        super::super::records::write(
            &mut writer,
            &super::super::Record::Peer(PeerFrame::CloudCompared {
                workspace_id: scope.eligibility_workspace,
                matching: response,
            }),
            deadline,
            &budget,
        )
        .await
        .unwrap();
    };
    let (answer, ()) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(sync.compare_documents(documents), peer)
    })
    .await
    .unwrap();
    let (matching, hinted) = answer.unwrap();
    assert_eq!(matching, expected);
    assert!(
        hinted,
        "a concurrent remote edit must survive inventory comparison"
    );
}

#[tokio::test]
async fn a_room_push_can_be_deferred_during_comparison_without_losing_the_reply() {
    use koru_device::cloud::CloudId;
    use koru_replication::protocol::{CloudDocumentDigest, PeerFrame};
    let (client, mut reader, mut writer, mut stream, scope, budget) =
        super::super::tests::pair_with_workspace(true).await;
    let mut sync = Reconciler::new(
        client,
        |_| Ok(async { Ok(Answer::Ready(VersionVector::default())) }),
        |_| Ok(async { Ok(Answer::Ready(None)) }),
    );
    let document_id = koru_store_core::DocumentId::new();
    let peer = async {
        let deadline = Instant::now() + Duration::from_secs(2);
        let request = records::read(&mut reader, deadline, &budget).await.unwrap();
        assert!(matches!(
            request.record(),
            Record::Peer(PeerFrame::CloudCompare { .. })
        ));
        let unit = Unit::Document {
            workspace_id: CloudId::from_bytes(scope.eligibility_workspace.as_uuid().into_bytes()),
            document_id: CloudId::from_bytes(document_id.as_uuid().into_bytes()),
        };
        let (start, _) = stream
            .begin_outgoing(
                unit,
                [8; 8],
                vec![1; transfer::RESTRICTED_LIMIT + 1],
                0,
                0,
                |_| Ok(()),
            )
            .unwrap();
        records::write(&mut writer, &Record::Control(start), deadline, &budget)
            .await
            .unwrap();
        records::write(
            &mut writer,
            &Record::Peer(PeerFrame::CloudCompared {
                workspace_id: scope.eligibility_workspace,
                matching: vec![document_id],
            }),
            deadline,
            &budget,
        )
        .await
        .unwrap();
    };
    let (result, ()) = tokio::join!(
        sync.compare_documents(vec![CloudDocumentDigest {
            document_id,
            digest: [3; 32]
        }]),
        peer
    );
    assert!(result.unwrap().0.contains(&document_id));
    assert!(sync.status().transport.transfer.closed.is_none());
}

#[tokio::test]
async fn download_during_retained_verification_is_rechecked_after_original_read() {
    let (client, _reader, _writer, _, scope, budget) = super::super::tests::pair().await;
    let remote = loro::LoroDoc::new();
    remote
        .get_map("root")
        .insert("value", "downloaded")
        .unwrap();
    remote.commit();
    let version = remote.oplog_vv();
    let fresh = version.clone();
    let reads = Arc::new(AtomicUsize::new(0));
    let observed = reads.clone();
    let mut sync = Reconciler::new(
        client,
        move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            let version = fresh.clone();
            Ok(async move { Ok(Answer::Ready(version)) })
        },
        |_| Ok(async { Ok(Answer::Ready(None)) }),
    );
    let unit = Unit::Identity {
        profile_id: scope.profile,
    };
    let (send, receive) = tokio::sync::oneshot::channel();
    sync.pass = Some(Pass {
        unit,
        step: Step::Verify(Box::pin(async move {
            receive.await.unwrap();
            Ok(Answer::Ready(VersionVector::default()))
        })),
        remote: Some(Arc::new(Remote {
            version,
            _messages: Messages {
                values: vec![],
                _charge: budget.reserve(1).unwrap(),
            },
        })),
        upload_deferred: false,
        download_deferred: false,
        verify_again: false,
        uploaded_version: None,
        upload_acked: false,
    });
    sync.transport_event(super::super::Event::Downloaded(unit))
        .unwrap();
    assert_eq!(
        reads.load(Ordering::SeqCst),
        0,
        "do not replace an unfinished Store read"
    );
    assert_eq!(sync.status().phase, Phase::Verifying);
    send.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match sync.next().await.unwrap() {
                Event::Settled {
                    unit: completed,
                    local_changed,
                    ..
                } => {
                    assert_eq!(completed, unit);
                    assert!(!local_changed);
                    break;
                }
                _ => assert_ne!(
                    sync.status().phase,
                    Phase::AwaitingDownload,
                    "the already consumed download cannot be awaited again"
                ),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_edit_after_the_acknowledged_export_is_not_claimed_as_synced() {
    let (client, _reader, _writer, _, scope, budget) = super::super::tests::pair().await;
    let doc = loro::LoroDoc::new();
    doc.get_map("root").insert("value", "uploaded").unwrap();
    doc.commit();
    let acknowledged = doc.oplog_vv();
    doc.get_map("root").insert("value", "edited again").unwrap();
    doc.commit();
    let current = doc.oplog_vv();
    let mut sync = Reconciler::new(
        client,
        |_| Ok(async { Ok(Answer::Ready(VersionVector::default())) }),
        |_| Ok(async { Ok(Answer::Ready(None)) }),
    );
    let unit = Unit::Identity {
        profile_id: scope.profile,
    };
    sync.pass = Some(Pass {
        unit,
        step: Step::Verify(Box::pin(async move { Ok(Answer::Ready(current)) })),
        remote: Some(Arc::new(Remote {
            version: acknowledged.clone(),
            _messages: Messages {
                values: vec![],
                _charge: budget.reserve(1).unwrap(),
            },
        })),
        upload_deferred: false,
        download_deferred: false,
        verify_again: false,
        uploaded_version: Some(acknowledged),
        upload_acked: true,
    });
    let event = tokio::time::timeout(Duration::from_secs(1), sync.next())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        event,
        Event::Settled {
            local_changed: true,
            ..
        }
    ));
}
