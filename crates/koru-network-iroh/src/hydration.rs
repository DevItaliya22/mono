// SPDX-License-Identifier: MPL-2.0

//! Blob hydration: getting the bytes behind a content hash from linked devices.
//!
//! A document carries a blob's *reference*; the bytes wait to be asked for
//! ([ADR-128](../../../docs/decisions/ADR-126-150.md#adr-128-lazy-files--a-files-reference-syncs-with-the-workspace-its-bytes-wait-to-be-asked-for)).
//! This module is everything that asking involves: the ticket a caller waits
//! on, the in-flight coordinator that deduplicates by content identity, the
//! holder directory that remembers which device advertised what, and the
//! transfer itself.
//!
//! Two rules keep it honest:
//!
//! - **Hints route, digests decide.** The directory and the durable origin
//!   attribute only choose *whom to ask first*. A wrong hint costs one
//!   `blob_not_found` round trip; the bytes are verified against their hash
//!   before publication regardless of who served them.
//! - **Waiting names what it waits for.** A parked hydration carries the
//!   labels of the devices known to hold the blob, so a user reads "waiting
//!   for Pixel 8" instead of "the device holding it", and a park with no
//!   known holder and nobody offline fails rather than waiting forever.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use koru_device::DeviceId;
use koru_store_core::model::BlobHash;
use tokio::sync::{oneshot, watch};

use crate::protocol;
use crate::{
    NetworkStatus, RequestTransport, SYNC_TRANSFER_TIMEOUT, record_state_change, request_frame,
};

pub const BLOB_CHUNK_BYTES: u32 = 1024 * 1024;
pub const MAX_BLOB_HYDRATION_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_CONCURRENT_BLOB_HYDRATIONS: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlobWaitReason {
    NoLinkedDevices,
    /// Nothing connected could serve the bytes. `holders` names the devices
    /// known to hold — or to have originated — the blob among those offline;
    /// empty when all we know is that some linked device may have it.
    Offline {
        holders: Vec<String>,
    },
}

impl BlobWaitReason {
    fn as_str(&self) -> &'static str {
        match self {
            Self::NoLinkedDevices => "no_linked_devices",
            Self::Offline { .. } => "peer_offline",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlobHydrationState {
    CheckingLocal,
    WaitingForPeer {
        reason: BlobWaitReason,
    },
    Fetching {
        received_bytes: u64,
        total_bytes: Option<u64>,
    },
    Verifying,
    Available,
    Failed {
        code: String,
        retryable: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobHydrationError {
    pub code: String,
    pub retryable: bool,
    /// Display names of the devices a parked hydration is waiting on. Empty
    /// for every other failure and for a park whose holder is unknown.
    pub waiting_on: Vec<String>,
}

impl BlobHydrationError {
    pub(crate) fn new(code: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            retryable,
            waiting_on: Vec::new(),
        }
    }

    fn waiting(code: impl Into<String>, holders: Vec<String>) -> Self {
        Self {
            code: code.into(),
            retryable: true,
            waiting_on: holders,
        }
    }
}

pub struct BlobHydrationTicket {
    state: watch::Receiver<BlobHydrationState>,
    completion: oneshot::Receiver<Result<(), BlobHydrationError>>,
}

impl BlobHydrationTicket {
    pub(crate) fn new(
        state: watch::Receiver<BlobHydrationState>,
        completion: oneshot::Receiver<Result<(), BlobHydrationError>>,
    ) -> Self {
        Self { state, completion }
    }

    pub fn state(&self) -> &watch::Receiver<BlobHydrationState> {
        &self.state
    }

    pub fn subscribe(&self) -> watch::Receiver<BlobHydrationState> {
        self.state.clone()
    }

    pub async fn wait(self) -> Result<(), BlobHydrationError> {
        self.completion
            .await
            .unwrap_or_else(|_| Err(BlobHydrationError::new("runtime_closed", true)))
    }

    /// Like [`wait`](Self::wait), but a hydration that **parks** — no
    /// connected peer has the bytes, and the device that does is offline —
    /// answers `blob_waiting_for_peer` instead of holding the caller until
    /// that device returns. The hydration itself keeps waiting: the entry
    /// stays registered and re-runs when a peer connects, so a later read
    /// finds the bytes. This is what a *request* should wait on; `wait` is
    /// for a caller that means "however long it takes".
    pub async fn wait_unless_parked(self) -> Result<(), BlobHydrationError> {
        let mut state = self.state;
        let mut completion = self.completion;
        loop {
            // The two park reasons are different sentences to a person:
            // "the holder is offline, wait" against "nothing is linked, so
            // nothing will ever bring it" — so they keep their own codes.
            let parked = match &*state.borrow() {
                BlobHydrationState::WaitingForPeer {
                    reason: BlobWaitReason::Offline { holders },
                } => Some(BlobHydrationError::waiting(
                    "blob_waiting_for_peer",
                    holders.clone(),
                )),
                BlobHydrationState::WaitingForPeer {
                    reason: BlobWaitReason::NoLinkedDevices,
                } => Some(BlobHydrationError::waiting("no_linked_devices", Vec::new())),
                _ => None,
            };
            if let Some(error) = parked {
                return Err(error);
            }
            tokio::select! {
                result = &mut completion => {
                    return result
                        .unwrap_or_else(|_| Err(BlobHydrationError::new("runtime_closed", true)));
                }
                changed = state.changed() => {
                    if changed.is_err() {
                        // The coordinator dropped the entry: the completion is
                        // the only signal left.
                        return completion.await.unwrap_or_else(|_| {
                            Err(BlobHydrationError::new("runtime_closed", true))
                        });
                    }
                }
            }
        }
    }

    pub fn blocking_wait(self) -> Result<(), BlobHydrationError> {
        self.completion
            .blocking_recv()
            .unwrap_or_else(|_| Err(BlobHydrationError::new("runtime_closed", true)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct BlobHydrationKey {
    pub workspace: koru_store_core::WorkspaceId,
    pub hash: BlobHash,
}

pub(crate) struct BlobHydrationEntry {
    pub state: watch::Sender<BlobHydrationState>,
    pub waiters: Vec<oneshot::Sender<Result<(), BlobHydrationError>>>,
    pub generation: u64,
    pub running: bool,
}

pub(crate) type BlobHydrationCoordinator =
    Arc<Mutex<HashMap<BlobHydrationKey, BlobHydrationEntry>>>;

#[derive(Clone, Debug)]
pub(crate) enum BlobHydrationOutcome {
    Available,
    WaitingForPeer(BlobWaitReason),
    Failed(BlobHydrationError),
}

/// Which connected devices hold which blobs, as observed this session.
///
/// Fed by two direct observations: a peer's inventory advertisement (every
/// sync pass already carries it — the directory exists because that list used
/// to be dropped unread) and a peer successfully serving us the bytes. Never
/// persisted: a restart costs one inventory exchange, and a stale entry costs
/// one `blob_not_found` before the next peer is tried.
pub(crate) struct BlobDirectory {
    holders: HashMap<(koru_store_core::WorkspaceId, BlobHash), BTreeSet<DeviceId>>,
}

impl BlobDirectory {
    pub fn new() -> Self {
        Self {
            holders: HashMap::new(),
        }
    }

    pub fn record(
        &mut self,
        workspace: koru_store_core::WorkspaceId,
        hash: &BlobHash,
        device: DeviceId,
    ) {
        self.holders
            .entry((workspace, hash.clone()))
            .or_default()
            .insert(device);
    }

    pub fn holders(
        &self,
        workspace: koru_store_core::WorkspaceId,
        hash: &BlobHash,
    ) -> Option<&BTreeSet<DeviceId>> {
        self.holders.get(&(workspace, hash.clone()))
    }

    /// The devices seen advertising each of `hashes`, for a person to read.
    /// A hash nobody advertised is absent rather than present and empty.
    pub fn advertised(
        &self,
        workspace: koru_store_core::WorkspaceId,
        hashes: &[BlobHash],
    ) -> HashMap<BlobHash, Vec<DeviceId>> {
        hashes
            .iter()
            .filter_map(|hash| {
                let holders = self.holders(workspace, hash)?;
                Some((hash.clone(), holders.iter().copied().collect()))
            })
            .collect()
    }

    /// Everything this device was recorded as holding is gone — revocation
    /// means its advertisements stop meaning anything.
    #[cfg(test)]
    pub fn forget_device(&mut self, device: DeviceId) {
        self.holders.retain(|_, devices| {
            devices.remove(&device);
            !devices.is_empty()
        });
    }

    /// The whole session's knowledge, gone — what a restart means for this
    /// directory. Test-only, for modelling one.
    #[cfg(test)]
    pub fn clear(&mut self) {
        self.holders.clear();
    }

    /// An authorization snapshot replaces the whole grant at once: a device
    /// no longer granted stops being a holder hint too.
    pub fn forget_except(&mut self, granted: &BTreeSet<DeviceId>) {
        self.holders.retain(|_, devices| {
            devices.retain(|device| granted.contains(device));
            !devices.is_empty()
        });
    }
}

pub(crate) type SharedBlobDirectory = Arc<Mutex<BlobDirectory>>;

/// Round-robin cursor: successive hydrations start at a different peer, so a
/// run of misses does not land every transfer on whoever sorts first in the
/// authorization snapshot.
///
/// **Read here and passed in, never read inside [`order_attempts`].** The
/// ordering is otherwise a pure function of its arguments, and a global read
/// *inside* it made every caller share one counter — including two tests in
/// the same binary, which is how `attempt_order_rotates_the_starting_peer`
/// failed once the harness stopped being pinned to two threads: its four
/// calls saw a cursor three other tests were also advancing. A process global
/// gets a lock or it gets lifted to the edge; this is the second.
static NEXT_ATTEMPT_START: AtomicU64 = AtomicU64::new(0);

/// The next rotation offset. One caller — [`plan_blob_hydration`] — so the
/// cursor advances once per real hydration and never once per test.
fn next_attempt_start() -> usize {
    NEXT_ATTEMPT_START.fetch_add(1, Ordering::Relaxed) as usize
}

/// Who to ask, in order: connected known holders first, then every other
/// connected candidate, each list rotated by `start`. The durable
/// origin counts as known even when the directory has not seen it this
/// session: it is the device that once held the bytes, so of the connected
/// peers it is the most likely to still have them.
///
/// `connected` and `holders` are device ids; the caller maps ids back to its
/// connections. A holder that is not connected is not returned here — it is
/// the parking decision's business, not the attempt list's.
///
/// `start` is the round-robin offset, taken from [`next_attempt_start`] by
/// the one production caller. Pure in every argument, so a test names the
/// rotation it is asserting on rather than racing whatever else is running.
pub(crate) fn order_attempts(
    mut connected: Vec<DeviceId>,
    holders: Option<&BTreeSet<DeviceId>>,
    origin: Option<DeviceId>,
    start: usize,
) -> Vec<DeviceId> {
    if connected.len() < 2 {
        return connected;
    }
    connected.sort();
    let (mut known, mut rest): (Vec<DeviceId>, Vec<DeviceId>) =
        connected.into_iter().partition(|device| {
            holders.is_some_and(|holders| holders.contains(device)) || origin == Some(*device)
        });
    if !known.is_empty() {
        let len = known.len();
        known.rotate_left(start % len);
    }
    if !rest.is_empty() {
        let len = rest.len();
        rest.rotate_left(start % len);
    }
    known.extend(rest);
    known
}

/// The labels a parked hydration can honestly name: the devices among
/// `offline` that the directory or the blob's durable origin say hold the
/// bytes. `label_of` resolves a device to its display name; an unresolvable
/// one is skipped rather than shown as an id.
pub(crate) fn known_offline_holders(
    offline: &[DeviceId],
    directory_holders: Option<&BTreeSet<DeviceId>>,
    origin: Option<DeviceId>,
    label_of: impl Fn(DeviceId) -> Option<String>,
) -> Vec<String> {
    let mut labels = Vec::new();
    for device in offline {
        let known = directory_holders.is_some_and(|holders| holders.contains(device))
            || origin == Some(*device);
        if known
            && let Some(label) = label_of(*device)
            && !labels.contains(&label)
        {
            labels.push(label);
        }
    }
    labels
}

pub(crate) fn set_blob_hydration_waiting(
    coordinator: &BlobHydrationCoordinator,
    key: &BlobHydrationKey,
    reason: BlobWaitReason,
) {
    tracing::info!(
        workspace = %key.workspace.to_storage(),
        reason = reason.as_str(),
        holders = match &reason {
            BlobWaitReason::Offline { holders } if !holders.is_empty() => holders.join(", "),
            _ => "-".to_string(),
        },
        "blob_hydration state=waiting"
    );
    let mut hydrations = coordinator
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(entry) = hydrations.get_mut(key) {
        entry.running = false;
        entry
            .state
            .send_replace(BlobHydrationState::WaitingForPeer { reason });
    }
}

pub(crate) fn complete_blob_hydration(
    coordinator: &BlobHydrationCoordinator,
    key: &BlobHydrationKey,
    result: Result<(), BlobHydrationError>,
) {
    let entry = coordinator
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(key);
    let Some(entry) = entry else {
        return;
    };
    match &result {
        Ok(()) => tracing::info!(
            workspace = %key.workspace.to_storage(),
            "blob_hydration state=available action=durable_commit"
        ),
        Err(error) => tracing::warn!(
            workspace = %key.workspace.to_storage(),
            error = %error.code,
            retryable = error.retryable,
            "blob_hydration state=failed"
        ),
    }
    let final_state = match &result {
        Ok(()) => BlobHydrationState::Available,
        Err(error) => BlobHydrationState::Failed {
            code: error.code.clone(),
            retryable: error.retryable,
        },
    };
    entry.state.send_replace(final_state);
    for waiter in entry.waiters {
        let _ = waiter.send(result.clone());
    }
}

pub(crate) fn fail_all_blob_hydrations(
    coordinator: &BlobHydrationCoordinator,
    error: BlobHydrationError,
) {
    let keys = coordinator
        .lock()
        .unwrap_or_else(|lock_error| lock_error.into_inner())
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for key in keys {
        complete_blob_hydration(coordinator, &key, Err(error.clone()));
    }
}

/// What "nobody connected could serve it" means for one hydration, decided at
/// schedule time while the actor still holds authorization state.
#[derive(Debug)]
pub(crate) struct ParkDecision {
    /// A linked, compatible device that might hold the blob is unreachable,
    /// so waiting can pay off. When false, a failed sweep fails with
    /// `blob_not_found` rather than parking on nobody.
    pub worth_waiting: bool,
    /// The offline devices known to hold the bytes, for the waiting state.
    pub known_holders: Vec<String>,
}

/// the same regardless of which connection type carries the attempt, so it is
/// decided once, here.
#[derive(Debug)]
pub(crate) enum HydrationPlan {
    /// Nobody connected can be asked; publish this waiting state.
    Wait(BlobWaitReason),
    /// Ask these connected devices in order; if none can serve the bytes,
    /// park with this decision.
    Attempt {
        order: Vec<DeviceId>,
        park: ParkDecision,
    },
}

/// The pure half of scheduling a hydration. Given the compatible, authorized
/// candidates, who is connected, and whom the directory and the durable origin
/// name as holders: decide the attempt order, or the honest waiting state.
///
/// "No linked devices" is deliberately not decided here — that answer is about
/// the *authorization snapshot*, before authentication, and the caller still
/// holds it.
pub(crate) fn plan_blob_hydration(
    candidates: &[protocol::AuthenticatedPeer],
    connected: &BTreeSet<DeviceId>,
    directory_holders: Option<&BTreeSet<DeviceId>>,
    origin: Option<DeviceId>,
) -> HydrationPlan {
    let connected_ids: Vec<DeviceId> = candidates
        .iter()
        .filter(|peer| connected.contains(&peer.device_id))
        .map(|peer| peer.device_id)
        .collect();
    let offline_ids: Vec<DeviceId> = candidates
        .iter()
        .filter(|peer| !connected.contains(&peer.device_id))
        .map(|peer| peer.device_id)
        .collect();
    let known_holders = known_offline_holders(&offline_ids, directory_holders, origin, |device| {
        candidates
            .iter()
            .find(|peer| peer.device_id == device)
            .and_then(|peer| peer.display_name.clone())
    });
    let order = order_attempts(
        connected_ids,
        directory_holders,
        origin,
        next_attempt_start(),
    );
    if order.is_empty() {
        HydrationPlan::Wait(BlobWaitReason::Offline {
            holders: known_holders,
        })
    } else {
        HydrationPlan::Attempt {
            order,
            park: ParkDecision {
                // Waiting pays off only if a compatible, authorized device is
                // unreachable; a head count over *all* authorized peers parks
                // on incompatible ones too, and parks forever on blobs no
                // linked device has ever held.
                worth_waiting: !offline_ids.is_empty(),
                known_holders,
            },
        }
    }
}

pub(crate) async fn hydrate_blob_from_peers<C>(
    key: &BlobHydrationKey,
    peers: Vec<(protocol::AuthenticatedPeer, C)>,
    park: ParkDecision,
    store: Arc<dyn protocol::ReplicationStore>,
    state: &watch::Sender<BlobHydrationState>,
    status: Arc<RwLock<NetworkStatus>>,
    directory: SharedBlobDirectory,
) -> BlobHydrationOutcome
where
    C: RequestTransport,
{
    // **§6's second point, before a single `BlobRequest` leaves this device.**
    // Refusing the bytes on arrival would be too late: they would already have
    // crossed the network for a Workspace this device has closed. The Store
    // answers, through the same `admit_transport` the Cloud path asks — see
    // `ReplicationStore::admit_peer_blob_transfer`.
    if let Err(refusal) = store.admit_peer_blob_transfer(key.workspace) {
        return BlobHydrationOutcome::Failed(BlobHydrationError::new(refusal.code, false));
    }
    let mut last_retryable = None;
    for (peer, connection) in peers {
        tracing::info!(
            workspace = %key.workspace.to_storage(),
            peer = %peer.device_id.to_storage(),
            peer_name = %peer.label(),
            "blob_hydration state=fetching"
        );
        state.send_replace(BlobHydrationState::Fetching {
            received_bytes: 0,
            total_bytes: None,
        });
        match hydrate_blob_from_peer(key, &peer, &connection, store.as_ref(), state).await {
            Ok(()) => {
                // A peer that served the bytes holds the bytes: record it, so
                // the next miss here starts from knowledge rather than order.
                directory
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .record(key.workspace, &key.hash, peer.device_id);
                record_state_change(
                    &status,
                    Some(store.as_ref()),
                    peer.device_id,
                    "blob_stored",
                    None,
                    Some(key.workspace),
                    None,
                    None,
                    Some(key.hash.to_string()),
                    None,
                    Some(1),
                );
                return BlobHydrationOutcome::Available;
            }
            Err(error) if error.code == "blob_not_found" => {
                tracing::info!(
                    workspace = %key.workspace.to_storage(),
                    peer = %peer.device_id.to_storage(),
                    peer_name = %peer.label(),
                    "blob_hydration action=peer_fallback error=blob_not_found"
                );
                last_retryable = Some(error);
            }
            Err(error) if error.retryable => {
                last_retryable = Some(error);
            }
            Err(error) => return BlobHydrationOutcome::Failed(error),
        }
    }
    if park.worth_waiting {
        BlobHydrationOutcome::WaitingForPeer(BlobWaitReason::Offline {
            holders: park.known_holders,
        })
    } else {
        BlobHydrationOutcome::Failed(
            last_retryable.unwrap_or_else(|| BlobHydrationError::new("blob_not_found", false)),
        )
    }
}

pub(crate) async fn hydrate_blob_from_peer<C>(
    key: &BlobHydrationKey,
    peer: &protocol::AuthenticatedPeer,
    connection: &C,
    store: &dyn protocol::ReplicationStore,
    state: &watch::Sender<BlobHydrationState>,
) -> Result<(), BlobHydrationError>
where
    C: RequestTransport,
{
    let mut offset = 0u64;
    let mut payload = Vec::new();
    let mut metadata = None;
    // **D246's watcher, held for the whole transfer.** It resolves the moment
    // the connection's selected path stops being an IP path, and the request
    // below is raced against it — so a selection change stops this within one
    // snapshot rather than at the end of the chunk in flight.
    let mut closed = std::pin::pin!(connection.until_blob_path_closes());
    loop {
        // And asked again before every chunk, because a gate checked once at
        // the start of a 40 MB transfer is a gate checked once.
        if !connection.blob_path_admitted() {
            return Err(BlobHydrationError::new(
                crate::blob_path::RELAY_PATH_REFUSED,
                true,
            ));
        }
        let request = protocol::PeerFrame::BlobRequest {
            workspace_id: key.workspace,
            hash: key.hash.clone(),
            offset,
            max_bytes: BLOB_CHUNK_BYTES,
        };
        let response = tokio::select! {
            // Biased so a path that has already closed wins over a response
            // that happens to be ready in the same poll: the refusal is the
            // conservative answer and it must not depend on scheduling luck.
            biased;
            () = &mut closed => {
                return Err(BlobHydrationError::new(
                    crate::blob_path::RELAY_PATH_REFUSED,
                    true,
                ));
            }
            response = tokio::time::timeout(
                SYNC_TRANSFER_TIMEOUT,
                request_frame(connection, &request),
            ) => response,
        };
        let chunk = match response {
            Ok(Ok(protocol::PeerFrame::BlobChunk(chunk))) => chunk,
            Ok(Ok(protocol::PeerFrame::Error { code })) => {
                let retryable = code == "internal";
                return Err(BlobHydrationError::new(code, retryable));
            }
            Ok(Ok(_)) => {
                return Err(BlobHydrationError::new("unexpected_blob_frame", false));
            }
            Ok(Err(error)) => return Err(BlobHydrationError::new(error.stage, true)),
            Err(_) => return Err(BlobHydrationError::new("timeout", true)),
        };
        if chunk.workspace_id != key.workspace {
            return Err(BlobHydrationError::new("blob_workspace_mismatch", false));
        }
        if chunk.blob.hash != key.hash {
            return Err(BlobHydrationError::new("blob_hash_mismatch", false));
        }
        if chunk.offset != offset {
            return Err(BlobHydrationError::new("blob_offset_mismatch", false));
        }
        if chunk.bytes.is_empty() && !chunk.complete {
            return Err(BlobHydrationError::new("empty_blob_chunk", false));
        }
        // Size is the only thing bounding a transfer now. The MIME type used to
        // bound it as well — the first slice of this coordinator served image
        // nodes and refused everything else — which meant a shared PDF or `.url`
        // could never arrive on a second device, however small it was. Content
        // type is not a safety property here: the bytes are content-addressed
        // and verified against their own hash before publication, so what makes
        // a transfer safe is the digest and the cap, not the label on it.
        if chunk.blob.size > MAX_BLOB_HYDRATION_BYTES {
            return Err(BlobHydrationError::new("blob_too_large", false));
        }
        if metadata
            .as_ref()
            .is_some_and(|blob: &koru_store_core::model::Blob| blob != &chunk.blob)
        {
            return Err(BlobHydrationError::new("blob_metadata_changed", false));
        }
        metadata.get_or_insert_with(|| chunk.blob.clone());
        offset = offset
            .checked_add(chunk.bytes.len() as u64)
            .ok_or_else(|| BlobHydrationError::new("blob_size_overflow", false))?;
        if offset > chunk.blob.size || offset > MAX_BLOB_HYDRATION_BYTES {
            return Err(BlobHydrationError::new("blob_length_mismatch", false));
        }
        payload.extend_from_slice(&chunk.bytes);
        state.send_replace(BlobHydrationState::Fetching {
            received_bytes: offset,
            total_bytes: Some(chunk.blob.size),
        });
        if chunk.complete {
            if offset != chunk.blob.size {
                return Err(BlobHydrationError::new("blob_length_mismatch", false));
            }
            state.send_replace(BlobHydrationState::Verifying);
            return store
                .store_remote_blob(peer, key.workspace, &chunk.blob, payload)
                .map_err(|error| {
                    BlobHydrationError::new(
                        error.code,
                        matches!(error.code, "internal" | "store_lock"),
                    )
                });
        }
    }
}

#[cfg(test)]
mod tests;
