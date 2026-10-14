// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::{
    RelayProviderConfig,
    cloud::worker_endpoint::{WorkerBindError, bind},
};
use koru_device::ReplicationEndpointSecret;
use std::{net::UdpSocket, time::Duration};

#[tokio::test]
async fn invalid_worker_configuration_refuses_before_binding() {
    let secret = ReplicationEndpointSecret::from_secret_bytes([4; 32]);
    let valid = "127.0.0.1:12345".parse().unwrap();
    let disabled = RelayProviderConfig::Disabled;
    for address in ["127.0.0.1:0", "224.0.0.1:12345"] {
        assert_eq!(
            bind(&secret, address.parse().unwrap(), valid, &disabled)
                .await
                .err(),
            Some(WorkerBindError::InvalidBindAddress)
        );
    }
    for address in [
        "127.0.0.1:0",
        "0.0.0.0:12345",
        "224.0.0.1:12345",
        "255.255.255.255:12345",
        "[::1]:12345",
    ] {
        assert_eq!(
            bind(&secret, valid, address.parse().unwrap(), &disabled)
                .await
                .err(),
            Some(WorkerBindError::InvalidAdvertisedAddress)
        );
    }
    for relays in [
        RelayProviderConfig::DefaultNetwork,
        RelayProviderConfig::CustomUrls(vec![]),
        RelayProviderConfig::CustomUrls(vec!["not a URL".into()]),
    ] {
        assert_eq!(
            bind(&secret, valid, valid, &relays).await.err(),
            Some(WorkerBindError::InvalidRelayConfiguration)
        );
    }
}

#[tokio::test]
async fn fixed_worker_socket_cannot_fall_back_to_an_ephemeral_port() {
    // Own the conflict rather than assuming anything about local services.
    let occupied = UdpSocket::bind("127.0.0.1:0").unwrap();
    let address = occupied.local_addr().unwrap();
    let secret = ReplicationEndpointSecret::from_secret_bytes([4; 32]);
    assert_eq!(
        bind(&secret, address, address, &RelayProviderConfig::Disabled)
            .await
            .err(),
        Some(WorkerBindError::BindFailed)
    );
}

#[tokio::test]
async fn fixed_worker_udp_endpoint_runs_signed_workspace_and_profile_admission() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let reserved = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = reserved.local_addr().unwrap();
        drop(reserved);
        let worker_secret = ReplicationEndpointSecret::from_secret_bytes([4; 32]);
        let worker = bind(
            &worker_secret,
            address,
            address,
            &RelayProviderConfig::Disabled,
        )
        .await
        .unwrap();
        assert_eq!(worker.bound_sockets(), vec![address]);
        assert_eq!(
            worker.id().as_bytes(),
            worker_secret.endpoint_id().as_bytes()
        );
        assert!(worker.addr().ip_addrs().any(|addr| *addr == address));
        assert_eq!(worker.addr().relay_urls().count(), 0);
        let client_secret = ReplicationEndpointSecret::from_secret_bytes([5; 32]);
        let client = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .clear_ip_transports()
            .bind_addr("127.0.0.1:0")
            .unwrap()
            .relay_mode(iroh::RelayMode::Disabled)
            .portmapper_config(iroh::endpoint::PortmapperConfig::Disabled)
            .secret_key(iroh::SecretKey::from_bytes(&client_secret.secret_bytes()))
            .bind()
            .await
            .unwrap();
        // Only the exact configured direct address is handed to the client.
        let target = iroh::EndpointAddr::new(worker.id()).with_ip_addr(address);
        let (outgoing, incoming) = tokio::join!(client.connect(target, CLOUD_ALPN_V1), async {
            worker.accept().await.unwrap().await.unwrap()
        });
        let outgoing = outgoing.unwrap();
        assert_eq!(incoming.alpn(), CLOUD_ALPN_V1);
        assert_eq!(incoming.remote_id(), client.id());
        assert_eq!(outgoing.remote_id(), worker.id());
        let vectors = vectors();
        let verifier = verifier();
        let key = DeviceSigningKey::from_secret_bytes([3; 32]);
        let mut generations = ConnectionGenerations::default();
        for round in 0..2 {
            for index in 0..vectors["sessions"].as_array().unwrap().len() {
                // The worker's assignment rows name the endpoint it is bound to,
                // which is also the endpoint_id of the route the API issued.
                let mut provider = provider(&vectors);
                provider.facts.dataset = serde_json::from_value(
                    vectors["sessions"][index]["assignment"]["dataset"].clone(),
                )
                .unwrap();
                provider.facts.worker_endpoint = worker_secret.endpoint_id();
                provider.facts.registered_endpoint = client_secret.endpoint_id();
                let server = async {
                    let (mut writer, mut reader) = incoming.accept_bi().await.unwrap();
                    accept(
                        &incoming,
                        &mut writer,
                        &mut reader,
                        &verifier,
                        &provider,
                        worker_secret.endpoint_id(),
                        &mut generations,
                        || 1_800_000_000_000,
                    )
                    .await
                };
                let device = async {
                    let (mut writer, mut reader) = outgoing.open_bi().await.unwrap();
                    authenticate(
                        &outgoing,
                        &mut writer,
                        &mut reader,
                        &verifier,
                        vectors["credential"].as_str().unwrap().into(),
                        provider.facts.dataset,
                        client_secret.endpoint_id(),
                        worker_secret.endpoint_id(),
                        &key,
                        round,
                        || 1_800_000_000_000,
                    )
                    .await
                };
                let (server, device) = tokio::join!(server, device);
                let server = server.unwrap();
                let device = device.unwrap();
                assert_eq!(server.generation, round + 1);
                assert_eq!(device.generation, server.generation);
                assert_eq!(server.admit_transfer(), Err(Refusal::SyncUnavailable));
                assert_eq!(device.admit_transfer(), Err(Refusal::SyncUnavailable));
            }
        }
        client.close().await;
        worker.close().await;
        assert!(worker.is_closed());
        assert!(worker.accept().await.is_none());
        let worker_id = worker.id();
        drop(incoming);
        drop(outgoing);
        drop(client);
        drop(worker);
        // Same configured private secret and socket survive a fresh endpoint,
        // without reading/writing any key file or sharing a payload volume.
        let restarted = bind(
            &worker_secret,
            address,
            address,
            &RelayProviderConfig::Disabled,
        )
        .await
        .unwrap();
        assert_eq!(restarted.id(), worker_id);
        assert_eq!(restarted.bound_sockets(), vec![address]);
        restarted.close().await;
    })
    .await
    .expect("isolated worker transport deadline");
}

#[tokio::test]
async fn configured_local_relay_carries_worker_traffic_without_client_udp() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let relay = koru_relay_test_support::TestRelayServer::start()
            .await
            .unwrap();
        let reserved = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = reserved.local_addr().unwrap();
        drop(reserved);
        let secret = ReplicationEndpointSecret::from_secret_bytes([4; 32]);
        let worker = bind(
            &secret,
            address,
            address,
            &RelayProviderConfig::CustomUrls(vec![relay.url().to_string()]),
        )
        .await
        .unwrap();
        let client = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .clear_ip_transports()
            .relay_mode(iroh::RelayMode::Custom(iroh::RelayMap::from(
                relay.url().clone(),
            )))
            .portmapper_config(iroh::endpoint::PortmapperConfig::Disabled)
            .bind()
            .await
            .unwrap();
        worker.online().await;
        client.online().await;
        assert!(client.bound_sockets().is_empty());
        assert_eq!(worker.bound_sockets(), vec![address]);
        assert!(worker.addr().relay_urls().any(|url| url == relay.url()));
        let target = iroh::EndpointAddr::new(worker.id()).with_relay_url(relay.url().clone());
        let (outgoing, incoming) = tokio::join!(client.connect(target, CLOUD_ALPN_V1), async {
            worker.accept().await.unwrap().await.unwrap()
        });
        let outgoing = outgoing.unwrap();
        let server = async {
            let (mut writer, mut reader) = incoming.accept_bi().await.unwrap();
            let mut bytes = [0; 18];
            reader.read_exact(&mut bytes).await.unwrap();
            assert_eq!(&bytes, b"worker relay probe");
            writer.write_all(&bytes).await.unwrap();
            writer.finish().unwrap();
        };
        let device = async {
            let (mut writer, mut reader) = outgoing.open_bi().await.unwrap();
            writer.write_all(b"worker relay probe").await.unwrap();
            writer.finish().unwrap();
            assert_eq!(reader.read_to_end(18).await.unwrap(), b"worker relay probe");
        };
        tokio::join!(server, device);
        client.close().await;
        worker.close().await;
        relay.shutdown().await.unwrap();
    })
    .await
    .expect("isolated worker relay deadline");
}
