// SPDX-License-Identifier: MPL-2.0

//! One connection per device/worker, with independently authorized streams.
//! Renewal replaces a stream, never its siblings' transport. The handshake
//! generation belongs to this connection and survives individual retirements.
use super::*;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct Worker {
    connection: Option<Connection>,
    generations: BTreeMap<CloudReplicaPrincipalV1, u64>,
}

#[derive(Default)]
pub struct Pool(Mutex<BTreeMap<ReplicationEndpointId, Arc<tokio::sync::Mutex<Worker>>>>);

impl Pool {
    #[allow(clippy::too_many_arguments)]
    pub async fn open(
        &self,
        runtime: &RuntimeHandle,
        route: CloudRoute,
        credential: String,
        dataset: CloudReplicaPrincipalV1,
        local_endpoint: ReplicationEndpointId,
        verifier: &TokenCapabilityVerifier,
        device_key: &DeviceSigningKey,
    ) -> Result<Dialed, DialError> {
        let endpoint = route.endpoint_id;
        let slot = {
            let mut workers = self.0.lock().unwrap_or_else(|e| e.into_inner());
            // Discard closed routes without disturbing a live handshake.
            workers.retain(|_, worker| {
                Arc::strong_count(worker) > 1
                    || worker.try_lock().map_or(true, |w| {
                        w.connection
                            .as_ref()
                            .is_some_and(|c| c.close_reason().is_none())
                    })
            });
            workers.entry(endpoint).or_default().clone()
        };
        let mut worker = slot.lock().await;
        if worker
            .connection
            .as_ref()
            .is_none_or(|c| c.close_reason().is_some())
        {
            worker.connection = Some(
                runtime
                    .connect_cloud(route)
                    .await
                    .map_err(DialError::Connect)?,
            );
            worker.generations.clear();
        }
        let connection = worker
            .connection
            .as_ref()
            .expect("connected worker")
            .clone();
        let previous = worker.generations.get(&dataset).copied().unwrap_or(0);
        let dialed = open_on(
            connection,
            credential,
            dataset,
            local_endpoint,
            endpoint,
            verifier,
            device_key,
            previous,
        )
        .await?;
        worker
            .generations
            .insert(dataset, dialed.prepared.generation);
        Ok(dialed)
    }
}
