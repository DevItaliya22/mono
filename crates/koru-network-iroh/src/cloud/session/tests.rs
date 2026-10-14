// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::{RelayProviderConfig, cloud::worker_endpoint};
use koru_device::ReplicationEndpointSecret;
use koru_replication::cloud::CLOUD_ALPN_V1;
use std::{net::UdpSocket, time::Duration};
use tokio::time::timeout;

async fn endpoint(secret: u8) -> iroh::Endpoint {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap();
    drop(socket);
    worker_endpoint::bind(
        &ReplicationEndpointSecret::from_secret_bytes([secret; 32]),
        address,
        address,
        &RelayProviderConfig::Disabled,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn fence_error_and_caller_cancellation_reset_only_the_owned_stream() {
    timeout(Duration::from_secs(30), async {
        let server = endpoint(111).await;
        let client = endpoint(112).await;
        let (outgoing, incoming) =
            tokio::join!(client.connect(server.addr(), CLOUD_ALPN_V1), async {
                server.accept().await.unwrap().await.unwrap()
            });
        let outgoing = outgoing.unwrap();
        for mode in 0..3 {
            let connection = incoming.clone();
            let (ready, observed) = tokio::sync::oneshot::channel();
            let (fence, fenced) = tokio::sync::oneshot::channel();
            let task = tokio::spawn(async move {
                let (mut writer, mut reader) = connection.accept_bi().await.unwrap();
                with_fence::<(), _, _>(
                    &mut writer,
                    &mut reader,
                    async { fenced.await.unwrap() },
                    move |_, reader| {
                        Box::pin(async move {
                            let mut byte = [0];
                            reader.read_exact(&mut byte).await.unwrap();
                            ready.send(()).unwrap();
                            if mode == 1 {
                                return Err(Refusal::Malformed);
                            }
                            std::future::pending().await
                        })
                    },
                )
                .await
            });
            let (mut writer, mut reader) = outgoing.open_bi().await.unwrap();
            writer.write_all(b"x").await.unwrap();
            observed.await.unwrap();
            match mode {
                0 => {
                    fence.send(Refusal::CloudDisabled).unwrap();
                    assert_eq!(task.await.unwrap(), Err(Refusal::CloudDisabled));
                }
                1 => assert_eq!(task.await.unwrap(), Err(Refusal::Malformed)),
                _ => {
                    task.abort();
                    assert!(task.await.unwrap_err().is_cancelled());
                }
            }
            // A deliberate reset, not an assumption that a service is absent.
            assert!(reader.read_to_end(1).await.is_err());
        }
        // All three streams traversed the same still-live QUIC connection.
        assert!(incoming.close_reason().is_none());
        assert!(outgoing.close_reason().is_none());
        client.close().await;
        server.close().await;
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn successful_handler_leaves_finishing_to_its_caller() {
    timeout(Duration::from_secs(30), async {
        let server = endpoint(113).await;
        let client = endpoint(114).await;
        let (outgoing, incoming) =
            tokio::join!(client.connect(server.addr(), CLOUD_ALPN_V1), async {
                server.accept().await.unwrap().await.unwrap()
            });
        let outgoing = outgoing.unwrap();
        let serve = async {
            let (mut writer, mut reader) = incoming.accept_bi().await.unwrap();
            with_fence::<(), _, _>(
                &mut writer,
                &mut reader,
                std::future::pending(),
                |writer, reader| {
                    Box::pin(async move {
                        let mut byte = [0];
                        reader.read_exact(&mut byte).await.unwrap();
                        writer.write_all(&byte).await.unwrap();
                        Ok(())
                    })
                },
            )
            .await
            .unwrap();
            writer.finish().unwrap();
        };
        let receive = async {
            let (mut writer, mut reader) = outgoing.open_bi().await.unwrap();
            writer.write_all(b"x").await.unwrap();
            writer.finish().unwrap();
            assert_eq!(reader.read_to_end(1).await.unwrap(), b"x");
        };
        tokio::join!(serve, receive);
        client.close().await;
        server.close().await;
    })
    .await
    .unwrap();
}
