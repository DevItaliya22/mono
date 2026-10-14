// SPDX-License-Identifier: MPL-2.0

//! Bounded async adapter over the blocking private session client.
//!
//! HTTP handlers never touch IPC, disk, image decode, process spawn, or a
//! subscription condition variable. Controller connections are retained by
//! lease; metadata calls use short private connections; streams use dedicated
//! connections so cancellation cannot disturb input.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use koru_session_api::{
    CreateSessionRequest, SessionStreamItem, SessionStreamSource, SubscribeRequest,
};
use koru_session_client::{ClientError, SessionClient, Subscription};
use koru_session_protocol::{
    Attachment, AttachmentId, AttachmentSource, ControllerLease, ControllerLeaseToken,
    PendingInput, ProtocolOffer, RawCommand, ScrollbackPage, SessionCommand, SessionFailure,
    SessionId, SessionObservation, SessionSummary,
};
use koru_session_wire::{OutputUpdate, SubscriptionUpdate};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

const UNARY_LIMIT: usize = 16;
const ATTACHMENT_LIMIT: usize = 2;
const CONTROLLER_LIMIT: usize = 128;
const STREAM_LIMIT: usize = 32;
const STREAM_QUEUE: usize = 32;

#[derive(Debug)]
pub enum BridgeError {
    Session(SessionFailure),
    Saturated,
    TimedOut,
    Unavailable,
    Internal,
}

impl From<ClientError> for BridgeError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Session(error) => Self::Session(error),
            ClientError::TimedOut => Self::TimedOut,
            ClientError::Disconnected
            | ClientError::Ipc(_)
            | ClientError::Handshake(_)
            | ClientError::Protocol(_) => Self::Unavailable,
            ClientError::UnexpectedResponse => Self::Internal,
        }
    }
}

struct Controller {
    session_id: SessionId,
    client: Arc<SessionClient>,
    _slot: OwnedSemaphorePermit,
}

struct Inner {
    endpoint: koru_ipc::Endpoint,
    offer: ProtocolOffer,
    /// The launch-scoped secret the Agent handed this Gateway down its
    /// bootstrap pipe, and the only thing that lets the workspace-lifecycle
    /// verb through. `None` when this Gateway was not supervised, or when the
    /// Agent supervises no session host.
    lifecycle_authority: Option<String>,
    unary: Arc<Semaphore>,
    attachments: Arc<Semaphore>,
    controllers: Arc<Semaphore>,
    streams: Arc<Semaphore>,
    by_lease: Mutex<HashMap<ControllerLeaseToken, Arc<Controller>>>,
}

#[derive(Clone)]
pub struct SessionBridge {
    inner: Arc<Inner>,
}

impl SessionBridge {
    /// Connect and prove the exact root-qualified host before the Gateway
    /// advertises the public capability.
    pub async fn connect(
        endpoint: koru_ipc::Endpoint,
        offer: ProtocolOffer,
    ) -> Result<Self, BridgeError> {
        Self::connect_supervised(endpoint, offer, None).await
    }

    /// Connect carrying the Agent's narrow session-lifecycle secret.
    ///
    /// Separate from [`Self::connect`] rather than an optional argument on it,
    /// so a host that has no secret cannot pass one by accident and every call
    /// site says which it is.
    pub async fn connect_supervised(
        endpoint: koru_ipc::Endpoint,
        offer: ProtocolOffer,
        lifecycle_authority: Option<String>,
    ) -> Result<Self, BridgeError> {
        let bridge = Self {
            inner: Arc::new(Inner {
                endpoint,
                offer,
                lifecycle_authority,
                unary: Arc::new(Semaphore::new(UNARY_LIMIT)),
                attachments: Arc::new(Semaphore::new(ATTACHMENT_LIMIT)),
                controllers: Arc::new(Semaphore::new(CONTROLLER_LIMIT)),
                streams: Arc::new(Semaphore::new(STREAM_LIMIT)),
                by_lease: Mutex::new(HashMap::new()),
            }),
        };
        bridge.fresh(|client| client.health()).await?;
        Ok(bridge)
    }

    /// Whether this Gateway can perform the supervised lifecycle verb at all.
    pub fn lifecycle_supervised(&self) -> bool {
        self.inner.lifecycle_authority.is_some()
    }

    /// Close every live session one Koru workspace owns.
    ///
    /// Step 3 of the workspace management operation. Idempotent: a retry after
    /// the sessions are already gone answers with an empty list, which is why
    /// the operation can be retried whole after a failed Store mutation.
    pub async fn terminate_workspace_sessions(
        &self,
        scope: koru_session_protocol::WorkspaceScope,
    ) -> Result<Vec<SessionId>, BridgeError> {
        let Some(authority) = self.inner.lifecycle_authority.clone() else {
            return Err(BridgeError::Unavailable);
        };
        self.fresh(move |client| client.terminate_workspace_sessions(authority, scope))
            .await
    }

    /// **Make every project that names a workspace name its new owner.**
    ///
    /// The repair half of the migration operation, and the reason it is a verb
    /// rather than something the host notices for itself: ADR-264 puts every
    /// project mutation here as a typed operation over the latest registry
    /// state, and makes `ProjectsChanged` the invalidation *after* an
    /// authoritative mutation rather than the thing that triggers one. The host
    /// holds no store client and reads no catalog; giving it one so it could
    /// discover the move would be the second sync mechanism that ADR rejected.
    ///
    /// Idempotent, because the request names the state to be in rather than the
    /// move that happened: a retry, a repair after the host was down, and a
    /// repair nobody needed are all the same call.
    pub async fn rescope_workspace_projects(
        &self,
        workspace: koru_session_protocol::CatalogWorkspaceId,
        profile: koru_session_protocol::CatalogProfileId,
    ) -> Result<Vec<koru_session_protocol::ProjectId>, BridgeError> {
        let Some(authority) = self.inner.lifecycle_authority.clone() else {
            return Err(BridgeError::Unavailable);
        };
        self.fresh(move |client| client.rescope_workspace_projects(authority, workspace, profile))
            .await
    }

    /// **What a workspace's removal would close, without closing it.**
    ///
    /// The consequence a confirmation dialog names (S13). It is a *preview* and
    /// says so: [`Self::terminate_workspace_sessions`] resolves and closes in
    /// one host operation precisely so nothing can be listed and then closed a
    /// moment later, and that ordering is not weakened by anybody reading the
    /// list beforehand. A terminal started while the dialog is on screen is why
    /// the token this feeds is bound to the list it was issued with.
    ///
    /// **The join is the host's rule, mirrored.** Projects carrying this scope,
    /// sessions placed in one of them, alive rather than reaped — the three
    /// clauses of `ProjectState::sessions_in_scope`, including the one that
    /// matters most: a session in no project, or in a project with no scope, is
    /// never named. Both calls ride one connection, so the two halves are at
    /// least each other's contemporaries.
    ///
    /// Unsupervised is not an error here. A gateway that could not perform the
    /// operation can still say a workspace has no terminals it would close, and
    /// the operation itself refuses in its own words.
    pub async fn sessions_in_scope(
        &self,
        scope: koru_session_protocol::WorkspaceScope,
    ) -> Result<Vec<SessionSummary>, BridgeError> {
        self.fresh(move |client| {
            let owned: HashSet<_> = client
                .list_projects()?
                .into_iter()
                .filter(|project| project.workspace_scope == Some(scope))
                .map(|project| project.id)
                .collect();
            if owned.is_empty() {
                return Ok(Vec::new());
            }
            let mut affected: Vec<_> = client
                .list()?
                .into_iter()
                .filter(|session| {
                    matches!(
                        session.process,
                        koru_session_protocol::ProcessState::Starting
                            | koru_session_protocol::ProcessState::Running
                    )
                })
                .filter(|session| {
                    session
                        .project_id
                        .is_some_and(|project| owned.contains(&project))
                })
                .collect();
            affected.sort_by_key(|session| session.id.as_uuid());
            Ok(affected)
        })
        .await
    }

    pub async fn list(&self) -> Result<Vec<SessionSummary>, BridgeError> {
        let sessions = self.fresh(|client| client.list()).await?;
        self.retain_live_controllers(&sessions);
        Ok(sessions)
    }

    pub async fn get(&self, session_id: SessionId) -> Result<SessionSummary, BridgeError> {
        self.fresh(move |client| {
            client
                .list()?
                .into_iter()
                .find(|session| session.id == session_id)
                .ok_or_else(|| session_error(koru_session_protocol::ErrorCode::SessionNotFound))
        })
        .await
    }

    pub async fn create(
        &self,
        request: CreateSessionRequest,
    ) -> Result<SessionSummary, BridgeError> {
        self.fresh(move |client| {
            client.create_profile_mode(
                request.idempotency_key,
                request.profile,
                request.parent_session_id,
                request.mode,
                request.working_directory.into(),
                request.size,
            )
        })
        .await
    }

    pub async fn attach(
        &self,
        session_id: SessionId,
        viewport_generation: koru_session_protocol::ViewportGeneration,
        columns: u16,
        overscan_rows: u16,
    ) -> Result<ControllerLease, BridgeError> {
        // A process can exit without its HTTP controller explicitly detaching.
        // Reap those retained private clients before applying the hard cap so
        // natural exits can never consume all future controller slots.
        self.list().await?;
        let slot = Arc::clone(&self.inner.controllers)
            .try_acquire_owned()
            .map_err(|_| BridgeError::Saturated)?;
        let endpoint = self.inner.endpoint.clone();
        let offer = self.inner.offer.clone();
        let (client, lease) = tokio::task::spawn_blocking(move || {
            let client = SessionClient::connect(&endpoint, "koru-api-gateway", offer)?;
            let (lease, render) =
                client.attach_live(session_id, viewport_generation, columns, overscan_rows)?;
            client.suspend_render(session_id, lease.token)?;
            drop(render);
            Ok::<_, ClientError>((Arc::new(client), lease))
        })
        .await
        .map_err(|_| BridgeError::Internal)??;
        self.inner
            .by_lease
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .insert(
                lease.token,
                Arc::new(Controller {
                    session_id,
                    client,
                    _slot: slot,
                }),
            );
        Ok(lease)
    }

    pub async fn detach(
        &self,
        session_id: SessionId,
        lease: ControllerLeaseToken,
    ) -> Result<(), BridgeError> {
        let controller = self.controller(session_id, lease)?;
        self.with_client(controller.client.clone(), move |client| {
            client.detach(session_id, lease)
        })
        .await?;
        self.inner
            .by_lease
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .remove(&lease);
        Ok(())
    }

    pub async fn snapshot(
        &self,
        session_id: SessionId,
        request: koru_session_api::SnapshotRequest,
    ) -> Result<SessionObservation, BridgeError> {
        self.fresh(move |client| {
            client.observe_snapshot(
                session_id,
                request.viewport_generation,
                request.range,
                request.columns,
            )
        })
        .await
    }

    pub async fn scrollback(
        &self,
        session_id: SessionId,
        request: koru_session_api::ScrollbackRequest,
    ) -> Result<ScrollbackPage, BridgeError> {
        self.fresh(move |client| {
            client.observe_scrollback_page(
                session_id,
                request.terminal_generation,
                request.viewport_generation,
                request.range,
            )
        })
        .await
    }

    pub async fn command(
        &self,
        idempotency_key: koru_session_protocol::IdempotencyKey,
        session_id: SessionId,
        lease: ControllerLeaseToken,
        action: RawCommand,
    ) -> Result<(), BridgeError> {
        let controller = self.controller(session_id, lease)?;
        self.with_client(controller.client.clone(), move |client| {
            client.command(SessionCommand {
                idempotency_key,
                session_id,
                controller_lease: lease,
                action,
            })
        })
        .await
    }

    /// Decode the bounded public representation on the blocking unary lane,
    /// never on a Tokio request worker.
    pub async fn write_bytes(
        &self,
        idempotency_key: koru_session_protocol::IdempotencyKey,
        session_id: SessionId,
        lease: ControllerLeaseToken,
        data_base64: String,
    ) -> Result<(), BridgeError> {
        let controller = self.controller(session_id, lease)?;
        self.with_client(controller.client.clone(), move |client| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data_base64)
                .map_err(|_| {
                    ClientError::Session(failure(koru_session_protocol::ErrorCode::InvalidRequest))
                })?;
            client.command(SessionCommand {
                idempotency_key,
                session_id,
                controller_lease: lease,
                action: RawCommand::WriteBytes { bytes },
            })
        })
        .await
    }

    pub fn forget_controller(&self, session_id: SessionId, lease: ControllerLeaseToken) {
        let mut controllers = self
            .inner
            .by_lease
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        if controllers
            .get(&lease)
            .is_some_and(|controller| controller.session_id == session_id)
        {
            controllers.remove(&lease);
        }
    }

    pub async fn upload(
        &self,
        session_id: SessionId,
        lease: ControllerLeaseToken,
        attachment_id: AttachmentId,
        data_base64: String,
    ) -> Result<Attachment, BridgeError> {
        let controller = self.controller(session_id, lease)?;
        self.with_lane(
            Arc::clone(&self.inner.attachments),
            controller.client.clone(),
            move |client| {
                client.ingest_attachment_base64(
                    session_id,
                    lease,
                    attachment_id,
                    AttachmentSource::PublicUpload,
                    data_base64,
                )
            },
        )
        .await
    }

    pub async fn attachments(&self, session_id: SessionId) -> Result<Vec<Attachment>, BridgeError> {
        self.fresh(move |client| client.attachments(session_id))
            .await
    }

    pub async fn remove_attachment(
        &self,
        session_id: SessionId,
        lease: ControllerLeaseToken,
        attachment_id: AttachmentId,
    ) -> Result<Attachment, BridgeError> {
        let controller = self.controller(session_id, lease)?;
        self.with_lane(
            Arc::clone(&self.inner.attachments),
            controller.client.clone(),
            move |client| client.remove_attachment(session_id, lease, attachment_id),
        )
        .await
    }

    pub async fn send_pending(
        &self,
        session_id: SessionId,
        lease: ControllerLeaseToken,
        input: PendingInput,
    ) -> Result<koru_session_protocol::SendPendingInputResult, BridgeError> {
        let controller = self.controller(session_id, lease)?;
        self.with_client(controller.client.clone(), move |client| {
            client.send_pending_input(session_id, lease, input)
        })
        .await
    }

    pub async fn subscribe(
        &self,
        session_id: SessionId,
        request: SubscribeRequest,
    ) -> Result<BridgeSubscription, BridgeError> {
        let slot = Arc::clone(&self.inner.streams)
            .try_acquire_owned()
            .map_err(|_| BridgeError::Saturated)?;
        let endpoint = self.inner.endpoint.clone();
        let offer = self.inner.offer.clone();
        let (client, events, output) = tokio::task::spawn_blocking(move || {
            let client = Arc::new(SessionClient::connect(
                &endpoint,
                "koru-api-gateway-stream",
                offer,
            )?);
            let events = client.subscribe_events_from(session_id, request.after_event)?;
            let output = client.subscribe_output_from(session_id, request.after_output)?;
            Ok::<_, ClientError>((client, events, output))
        })
        .await
        .map_err(|_| BridgeError::Internal)??;
        let (send, receive) = mpsc::channel(STREAM_QUEUE);
        let readers = Arc::new(AtomicUsize::new(2));
        spawn_reader(
            "koru-gateway-session-events",
            SessionStreamSource::Event,
            request.after_event,
            events,
            send.clone(),
            Arc::clone(&client),
            Arc::clone(&readers),
        )?;
        if let Err(error) = spawn_reader(
            "koru-gateway-session-output",
            SessionStreamSource::Output,
            request.after_output,
            output,
            send,
            Arc::clone(&client),
            readers,
        ) {
            // The first reader may already be blocked on this connection.
            // Wake and reclaim it if the second OS thread could not start.
            client.disconnect();
            return Err(error);
        }
        Ok(BridgeSubscription {
            receive,
            client,
            _slot: slot,
        })
    }

    fn controller(
        &self,
        session_id: SessionId,
        lease: ControllerLeaseToken,
    ) -> Result<Arc<Controller>, BridgeError> {
        self.inner
            .by_lease
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .get(&lease)
            .filter(|controller| controller.session_id == session_id)
            .cloned()
            .ok_or_else(|| {
                BridgeError::Session(failure(koru_session_protocol::ErrorCode::StaleLease))
            })
    }

    fn retain_live_controllers(&self, sessions: &[SessionSummary]) {
        let live = sessions
            .iter()
            .map(|session| session.id)
            .collect::<HashSet<_>>();
        self.inner
            .by_lease
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .retain(|_, controller| live.contains(&controller.session_id));
    }

    async fn fresh<T, F>(&self, action: F) -> Result<T, BridgeError>
    where
        T: Send + 'static,
        F: FnOnce(&SessionClient) -> Result<T, ClientError> + Send + 'static,
    {
        let endpoint = self.inner.endpoint.clone();
        let offer = self.inner.offer.clone();
        self.with_fresh_lane(Arc::clone(&self.inner.unary), move || {
            let client = SessionClient::connect(&endpoint, "koru-api-gateway", offer)?;
            action(&client)
        })
        .await
    }

    async fn with_client<T, F>(
        &self,
        client: Arc<SessionClient>,
        action: F,
    ) -> Result<T, BridgeError>
    where
        T: Send + 'static,
        F: FnOnce(&SessionClient) -> Result<T, ClientError> + Send + 'static,
    {
        self.with_lane(Arc::clone(&self.inner.unary), client, action)
            .await
    }

    async fn with_lane<T, F>(
        &self,
        lane: Arc<Semaphore>,
        client: Arc<SessionClient>,
        action: F,
    ) -> Result<T, BridgeError>
    where
        T: Send + 'static,
        F: FnOnce(&SessionClient) -> Result<T, ClientError> + Send + 'static,
    {
        self.with_fresh_lane(lane, move || action(&client)).await
    }

    async fn with_fresh_lane<T, F>(&self, lane: Arc<Semaphore>, action: F) -> Result<T, BridgeError>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, ClientError> + Send + 'static,
    {
        let permit = lane
            .try_acquire_owned()
            .map_err(|_| BridgeError::Saturated)?;
        tokio::task::spawn_blocking(move || {
            let result = action();
            drop(permit);
            result
        })
        .await
        .map_err(|_| BridgeError::Internal)?
        .map_err(Into::into)
    }
}

pub struct BridgeSubscription {
    receive: mpsc::Receiver<SessionStreamItem>,
    client: Arc<SessionClient>,
    _slot: OwnedSemaphorePermit,
}

impl BridgeSubscription {
    pub async fn next(&mut self) -> Option<SessionStreamItem> {
        self.receive.recv().await
    }
}

impl Drop for BridgeSubscription {
    fn drop(&mut self) {
        self.client.disconnect();
    }
}

fn spawn_reader(
    name: &'static str,
    source: SessionStreamSource,
    after: Option<koru_session_protocol::StreamSequence>,
    subscription: Subscription,
    send: mpsc::Sender<SessionStreamItem>,
    client: Arc<SessionClient>,
    readers: Arc<AtomicUsize>,
) -> Result<(), BridgeError> {
    std::thread::Builder::new()
        .name(name.into())
        .stack_size(128 * 1024)
        .spawn(move || {
            let mut last = after;
            loop {
                let update = match subscription.recv() {
                    Ok(update) => update,
                    Err(error) => {
                        finish_reader(&send, &client, &readers, Some(client_failure(error)));
                        return;
                    }
                };
                let (item, sequence) = match (source, update) {
                    (SessionStreamSource::Event, SubscriptionUpdate::Event { event }) => (
                        SessionStreamItem::Event {
                            event: event.clone(),
                        },
                        Some(event.sequence),
                    ),
                    (
                        SessionStreamSource::Event,
                        SubscriptionUpdate::EventGap { expected, actual },
                    ) => {
                        let item = SessionStreamItem::Gap {
                            source,
                            expected,
                            actual,
                            resnapshot_required: true,
                        };
                        last = Some(actual);
                        if send.blocking_send(item).is_err() {
                            client.disconnect();
                            return;
                        }
                        continue;
                    }
                    (
                        SessionStreamSource::Output,
                        SubscriptionUpdate::Output {
                            update: OutputUpdate::Chunk { chunk },
                        },
                    ) => (
                        SessionStreamItem::Output {
                            chunk: chunk.clone(),
                        },
                        Some(chunk.sequence),
                    ),
                    (
                        SessionStreamSource::Output,
                        SubscriptionUpdate::Output {
                            update: OutputUpdate::Gap { expected, actual },
                        },
                    ) => {
                        let item = SessionStreamItem::Gap {
                            source,
                            expected,
                            actual,
                            resnapshot_required: true,
                        };
                        last = Some(actual);
                        if send.blocking_send(item).is_err() {
                            client.disconnect();
                            return;
                        }
                        continue;
                    }
                    (_, SubscriptionUpdate::Closed { error }) => {
                        finish_reader(&send, &client, &readers, error);
                        return;
                    }
                    _ => {
                        finish_reader(
                            &send,
                            &client,
                            &readers,
                            Some(failure(koru_session_protocol::ErrorCode::Internal)),
                        );
                        return;
                    }
                };
                if let (Some(previous), Some(actual)) = (last, sequence)
                    && previous.next() != Some(actual)
                {
                    let expected = previous.next().unwrap_or(previous);
                    if send
                        .blocking_send(SessionStreamItem::Gap {
                            source,
                            expected,
                            actual,
                            resnapshot_required: true,
                        })
                        .is_err()
                    {
                        client.disconnect();
                        return;
                    }
                }
                last = sequence;
                if send.blocking_send(item).is_err() {
                    client.disconnect();
                    return;
                }
            }
        })
        .map(|_| ())
        .map_err(|_| BridgeError::Internal)
}

fn finish_reader(
    send: &mpsc::Sender<SessionStreamItem>,
    client: &SessionClient,
    readers: &AtomicUsize,
    error: Option<SessionFailure>,
) {
    if error.is_some() {
        client.disconnect();
    }
    if readers.fetch_sub(1, Ordering::AcqRel) == 1 {
        let _ = send.blocking_send(SessionStreamItem::Closed { error });
        client.disconnect();
    }
}

fn session_error(code: koru_session_protocol::ErrorCode) -> ClientError {
    ClientError::Session(failure(code))
}

fn client_failure(error: ClientError) -> SessionFailure {
    match error {
        ClientError::Session(failure) => failure,
        ClientError::TimedOut => failure(koru_session_protocol::ErrorCode::HostUnavailable),
        _ => failure(koru_session_protocol::ErrorCode::HostUnavailable),
    }
}

fn failure(code: koru_session_protocol::ErrorCode) -> SessionFailure {
    SessionFailure {
        message: code.as_str().to_owned(),
        code,
    }
}
