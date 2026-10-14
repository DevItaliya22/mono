// SPDX-License-Identifier: MPL-2.0

//! Everything this crate reaches the world through.
//!
//! Five ports, and the rule that decides whether something is one: a port is a
//! fact the *host* owns — where bytes live, what the account is, whether the
//! network may be used, what is on screen. Everything else is this crate's own
//! and is not made swappable, because a seam nobody needs is a seam somebody
//! will put policy behind.
//!
//! The Store ports are the signatures the contract's §9 froze for B3, so B1
//! can move the local layout underneath them without rewriting anything here.

use std::{path::PathBuf, pin::Pin, sync::Arc};

use koru_account_core::workspace_authorization::Region;
use koru_cloud_blob::BlobCode;
use koru_store_core::{WorkspaceId, errors::Code, model::BlobHash};

use crate::{allowlist::OriginRefused, coverage::DocumentSample, referenced::Walk};

/// The shape every async port answers in.
///
/// Boxed rather than `impl Future`, because the composition holds these behind
/// `dyn` — the desktop's Store lives in another task and Android's lives in
/// process, and one engine has to talk to both.
pub type PortFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// What stopped a transfer.
///
/// The contract's codes are carried through rather than flattened: a client
/// branches on them, and [`crate::retry::classify`] turns one into an answer
/// about whether to try again and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferError {
    /// A service answered with one of the contract's §7 codes.
    Refused(BlobCode),
    /// A returned byte URL was not somewhere this device sends files.
    Origin(OriginRefused),
    /// The request never got an answer: no route, a reset, a timeout, or a
    /// body that would not decode. Retryable, and never confused with a
    /// refusal — "the server said no" and "there was no server" are different
    /// facts and a client that merges them either spins or gives up.
    Unreachable,
    /// A local Store or filesystem refusal, with the Store's own code.
    Local(Code),
    /// The Workspace's policy does not permit this transfer now, with the
    /// reason. Not a failure: the engine parks the blob and waits for the
    /// trigger that changes it.
    ///
    /// **The reason travels with it** rather than being flattened to one
    /// code. §13 of the plan says a silent pause is indistinguishable from
    /// being refused, and that has already cost this project a debugging
    /// session; a person on a train and a person on a Local-only Workspace
    /// are owed different sentences.
    NotPermitted(TransferHold),
    /// This device holds no Cloud access for the Workspace — no account, no
    /// token, or a region it has not learned yet.
    NoAccess,
    /// The volume cannot hold what this download would write. **Bounded and
    /// visible**: the blob defers with a code a person can read, its
    /// neighbours carry on, and nothing is written towards a full disk.
    InsufficientStorage,
}

impl TransferError {
    /// The stable code a caller branches on and a status surface shows.
    ///
    /// **Always a code, never prose and never absent.** A failure with no name
    /// is a failure nobody can search a log for, and the three conditions that
    /// are not on the contract's §7 registry — a transport failure, a refused
    /// byte origin, and an account this device does not hold — each get one
    /// here rather than being reported as "something went wrong".
    pub fn code(&self) -> &'static str {
        match self {
            Self::Refused(code) => code.as_str(),
            Self::Local(code) => code.as_str(),
            Self::Origin(_) => "byte_origin_refused",
            Self::Unreachable => "service_unreachable",
            Self::NotPermitted(hold) => hold.as_str(),
            Self::NoAccess => "cloud_access_unavailable",
            Self::InsufficientStorage => "insufficient_storage",
        }
    }
}

/// One blob as the local Store knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalBlob {
    pub hash: BlobHash,
    pub size: u64,
    pub mime: String,
    /// The name the person's file had. **Metadata only** — never identity,
    /// never a path component, never a header without encoding.
    pub filename: Option<String>,
}

/// The Store, as a transfer needs it. The contract's §9, frozen.
pub trait BlobStorePort: Send + Sync {
    /// **One read of this Workspace's durable state**, with each document's
    /// exact sampled state beside its content.
    ///
    /// This is the read the canonical walk runs on and the read coverage
    /// samples — the same one, because [`crate::coverage`]'s first step says so
    /// in as many words. A coverage claim over state B built from a document
    /// check over state A is a claim about a Workspace that never existed, and
    /// the only way to make that unreachable is for there to be nothing else
    /// to call.
    ///
    /// Off the UI thread, and never on a timer.
    ///
    /// Every document the Workspace's durable listing names appears exactly
    /// once, whether or not it references a blob: a document with no pictures
    /// still has a state, and coverage has to be able to say whether Cloud
    /// holds it.
    fn sample_documents(
        &self,
        workspace: WorkspaceId,
    ) -> PortFuture<'_, Result<DocumentSample, TransferError>>;

    /// The one canonical walk ([`crate::referenced`]), over that sample.
    ///
    /// A provided method rather than a second port, so the signature B3 was
    /// written against still exists and there is still exactly one read
    /// behind it. An implementation that overrode this would be a second
    /// definition of *referenced*, which is the one thing this crate does not
    /// have.
    fn referenced_blobs(
        &self,
        workspace: WorkspaceId,
    ) -> PortFuture<'_, Result<Walk, TransferError>> {
        Box::pin(async move { Ok(self.sample_documents(workspace).await?.walk()) })
    }

    /// Bytes exist locally, anywhere on this device. B1 makes this one lookup
    /// in the global object store; today it is whatever the Store can answer.
    fn object_present(&self, hash: BlobHash) -> PortFuture<'_, Result<bool, TransferError>>;

    /// The Workspace is associated **and** the bytes are present, with the
    /// metadata an allocation declares. `None` means one or the other is
    /// missing — and no caller infers this from [`Self::object_present`].
    fn workspace_blob(
        &self,
        workspace: WorkspaceId,
        hash: BlobHash,
    ) -> PortFuture<'_, Result<Option<LocalBlob>, TransferError>>;

    /// Re-hash the local object and quarantine it if the bytes have rotted.
    ///
    /// **Before any network request** (plan §10). Uploading bytes that no
    /// longer hash to their own name would publish something else entirely
    /// under an address a document believes it knows.
    fn verify_object(
        &self,
        workspace: WorkspaceId,
        hash: BlobHash,
    ) -> PortFuture<'_, Result<(), TransferError>>;

    /// A reader over the local object. **Uploads stream**: `get_blob` returns
    /// a `Vec` and a 2 GiB `Vec` on the Store dispatcher is the same mistake
    /// the announcement seam avoids.
    ///
    /// No `Seek`, because §9 froze this signature without one. A resume from a
    /// non-zero committed offset therefore reads and discards the prefix once
    /// — one local pass on the rare path, rather than a wider port.
    fn open_blob(
        &self,
        workspace: WorkspaceId,
        hash: BlobHash,
    ) -> PortFuture<'_, Result<Box<dyn std::io::Read + Send>, TransferError>>;

    /// Publish a verified staging file as the object, **and only then** write
    /// the Workspace association.
    ///
    /// One call rather than two because the order is the invariant (S10): two
    /// calls would be two places for a future caller to get it the wrong way
    /// round, and the wrong way round is a successful metadata lookup pointing
    /// at partial bytes.
    fn install_downloaded(
        &self,
        workspace: WorkspaceId,
        blob: LocalBlob,
        staged: PathBuf,
    ) -> PortFuture<'_, Result<(), TransferError>>;

    /// Where this device keeps transfer staging: `<root>/blobs/staging`.
    /// A path, not a policy — the engine names files inside it.
    fn staging_root(&self) -> PathBuf;

    /// Drop this device's copy of one blob and keep the association.
    ///
    /// **The one destructive call on this port, and the engine owns its
    /// safety.** The Store refuses nothing here on durability grounds — it
    /// cannot know where else the bytes live — so this is called only after
    /// the control plane has answered *durable* for the address, with a size
    /// that matches what is held. Answers the bytes freed; zero when nothing
    /// was held, which is the caller's goal already being true.
    fn release_blob(
        &self,
        workspace: WorkspaceId,
        hash: BlobHash,
    ) -> PortFuture<'_, Result<u64, TransferError>>;
}

/// What the account says about one Workspace's Cloud.
#[derive(Clone)]
pub struct CloudAccessFacts {
    /// The control plane's origin, already validated by whoever holds the
    /// account. The five control routes hang off it.
    pub api_origin: url::Url,
    /// The account bearer token. Short-lived and refreshed by the account
    /// owner; this crate only ever sends it to [`Self::api_origin`].
    pub bearer: String,
    pub region: Region,
    /// Where bytes may be sent. Held by the device, never learned from a
    /// response.
    pub byte_origins: crate::allowlist::ByteOrigins,
}

impl std::fmt::Debug for CloudAccessFacts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudAccessFacts")
            .field("api_origin", &self.api_origin.as_str())
            .field("bearer", &"<redacted>")
            .field("region", &self.region)
            .finish()
    }
}

/// Who holds the account. The engine asks per pass rather than caching,
/// because a token expires and an account is switched while a transfer runs.
pub trait CloudAccess: Send + Sync {
    fn access(&self, workspace: WorkspaceId) -> Option<CloudAccessFacts>;
}

/// Whether a transfer may use the network at all right now.
///
/// **This is not the transport-mode gate** — [`TransportModes`] is. What this
/// port answers is the network policy of §10's sixth trigger: metered, low
/// data, offline. That is the host's own fact about the link it is holding,
/// and it is a different question from what a person chose for this Workspace.
pub trait BlobPolicy: Send + Sync {
    fn transfers_permitted(&self, workspace: WorkspaceId) -> bool;

    /// **Why**, when [`Self::transfers_permitted`] says no — and whether a
    /// person asking for one picture gets past it.
    ///
    /// A default rather than a required method, because a gate that cannot
    /// distinguish has exactly one honest answer and should not have to write
    /// it out: the transport mode refused, and no request overrides that. A
    /// host whose reason is the *network* says so here, and that is the whole
    /// of why §13's "a silent pause is indistinguishable from being refused"
    /// does not happen a second time.
    fn hold(&self, _workspace: WorkspaceId) -> TransferHold {
        TransferHold::TransportMode
    }
}

/// Why automatic transfers are held, and whether a person may override it.
///
/// Every variant's [`Self::as_str`] is a stable code a status surface shows
/// and a client branches on. `mode_refuses_transport` is the contract's §7
/// spelling; the other three are this crate's own, in the same shape and the
/// same registry as `service_unreachable` and `byte_origin_refused` — because
/// a pause with no name of its own is a pause a person reads as a refusal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransferHold {
    /// The Workspace's transport mode forbids this (D245). **Never
    /// overridable**: Local-only means no bytes leave, and a person asking
    /// for one picture has not changed what the Workspace is.
    #[default]
    TransportMode,
    /// The connection is metered (D50). **Automatic** transfers pause;
    /// document sync is untouched, an explicit request goes through, and so
    /// does everything once the person allows cellular transfers.
    Metered,
    /// The platform's data saver is on. The same shape as [`Self::Metered`]
    /// and a different word, because "I am on cellular" and "I asked this
    /// phone to use less data" are two sentences a person answers
    /// differently.
    LowData,
    /// There is no usable network. Not overridable — there is nothing to ask.
    Offline,
    /// **A person pressed pause for this Workspace** (D256). Nothing about
    /// the network, nothing about the mode, and nothing that ends on its own
    /// — which is why it is a hold of its own rather than a fifth meaning
    /// given to one of the four above. Documents are untouched: this lives in
    /// the blob engine and the document pass never asks it.
    Paused,
}

impl TransferHold {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TransportMode => BlobCode::ModeRefusesTransport.as_str(),
            Self::Metered => "network_metered",
            Self::LowData => "network_low_data",
            Self::Offline => "network_offline",
            Self::Paused => "paused_by_person",
        }
    }

    /// Whether an explicit request for one blob carries on through this hold.
    ///
    /// D50's sentence exactly: *automatic* blob transfers pause on a metered
    /// network **unless the user explicitly initiates the transfer**. The
    /// other two are not policy a person can waive — one is what the
    /// Workspace *is*, and the other is that there is no network.
    ///
    /// A person's own pause is waivable for the same reason the metered one
    /// is, and more plainly: opening a picture is a newer sentence than the
    /// pause, and it is about one picture rather than about the Workspace.
    pub const fn overridable(self) -> bool {
        matches!(self, Self::Metered | Self::LowData | Self::Paused)
    }

    /// Whether this is a **pause** — something that ends without anything
    /// being repaired — rather than a refusal a person has to act on.
    pub const fn is_pause(self) -> bool {
        !matches!(self, Self::TransportMode)
    }

    /// Whether a person put this hold there themselves.
    ///
    /// A status surface needs it because *"you paused this"* and *"waiting
    /// for a better connection"* are two sentences a person answers
    /// differently, and §13's lesson is that a pause without a name of its
    /// own is read as a refusal.
    pub const fn chosen_by_person(self) -> bool {
        matches!(self, Self::Paused)
    }
}

/// **§6's transport-mode gate, asked of the Store.**
///
/// `Store::admit_transport` is the one question both transports ask: it applies
/// the stored mode, the Profile ceiling and the in-memory fence in that order,
/// at the live policy generation. This port is how this crate asks it, and an
/// implementation does nothing but forward — the rule is not re-derived here,
/// because a second copy of a rule is a second answer waiting to happen.
///
/// It is asked **per pass and again per transfer**. A pass that asked once and
/// remembered would carry a 2 GiB file to completion under a mode somebody
/// turned off a minute into it; a restrictive choice closes admission in memory
/// before its durable write precisely so an in-flight transfer stops.
///
/// The path is [`koru_store_core::TransportPath`] because this crate already
/// links the Store's vocabulary. A second enum meaning the same two things
/// would be one more place for the two to disagree.
pub trait TransportModes: Send + Sync {
    /// `Ok(())` means bytes may move for this Workspace, on this path, now.
    ///
    /// Async like every other port here, and for the same reason: the Store
    /// lives behind a worker on the desktop, and a task that answers a person
    /// never blocks a runtime thread waiting on one.
    fn admit(
        &self,
        workspace: WorkspaceId,
        path: koru_store_core::TransportPath,
    ) -> PortFuture<'_, Result<(), TransferError>>;
}

/// A host with no transport modes to consult.
///
/// **Named rather than implied.** A composition that cannot ask a Store says so
/// here, in one line a reader can find, rather than by the question being
/// absent from the file. Tests use it; a shipped host whose mode rows are
/// promoted must not.
pub struct AlwaysAdmitted;

impl TransportModes for AlwaysAdmitted {
    fn admit(
        &self,
        _workspace: WorkspaceId,
        _path: koru_store_core::TransportPath,
    ) -> PortFuture<'_, Result<(), TransferError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Permissive: what a desktop on mains power and Ethernet means.
pub struct AlwaysPermitted;

impl BlobPolicy for AlwaysPermitted {
    fn transfers_permitted(&self, _workspace: WorkspaceId) -> bool {
        true
    }
}

/// One ranged read from a peer that holds these bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerChunk {
    pub bytes: Vec<u8>,
    /// The blob's whole size, as the serving peer declares it. The staging file
    /// is written against this and the hash at the end is what actually
    /// decides — a lying peer costs a quarantine, never a wrong picture.
    pub total: u64,
    /// The advertised metadata, carried so a completed peer download can be
    /// installed without a Cloud round trip for facts a peer already sent.
    pub mime: String,
    pub filename: Option<String>,
}

/// **Cloud mode tries the peer first** (§8, D245).
///
/// Relay bandwidth and Cloud egress are both ours to pay for while a LAN
/// transfer costs neither, so reconciliation orders its candidates as *an
/// authorized peer with a direct path that advertises the hash, then Cloud
/// HTTP*.
///
/// This port answers only the first of those, and it answers it **ranged**,
/// because that is what makes the progress shared: the peer's bytes go into the
/// same staging file, keyed by hash, so a download that got 300 MB from a peer
/// resumes at 300 MB from Cloud rather than starting again.
///
/// The host owns the D246 fence behind it: this crate never sees a connection,
/// a peer or a path, and a host that cannot find an admitted direct peer
/// answers `None` rather than explaining why.
pub trait PeerBlobs: Send + Sync {
    /// `Ok(None)` means no authorized peer on a direct path is offering these
    /// bytes right now — which is not a failure, it is the fall-through.
    fn read_range(
        &self,
        workspace: WorkspaceId,
        hash: BlobHash,
        offset: u64,
        max: u64,
    ) -> PortFuture<'_, Result<Option<PeerChunk>, TransferError>>;
}

/// A host with no peer path — Android today, and every test that is about the
/// Cloud plane.
pub struct NoPeerBlobs;

impl PeerBlobs for NoPeerBlobs {
    fn read_range(
        &self,
        _workspace: WorkspaceId,
        _hash: BlobHash,
        _offset: u64,
        _max: u64,
    ) -> PortFuture<'_, Result<Option<PeerChunk>, TransferError>> {
        Box::pin(async { Ok(None) })
    }
}

/// The announcement seam (plan §11).
///
/// **Identity, never bytes.** `Downloads::arrived(unit, Vec<u8>)` exists for
/// documents, where a payload is an edit; at blob sizes the equivalent is up
/// to a 2 GiB `Vec` on the Store dispatcher, which is a memory event, a
/// latency event, and on a phone an out-of-memory kill.
pub trait BlobDownloads: Send + Sync {
    /// Is anything on this device showing a document that references this
    /// blob?
    ///
    /// Asked **before** bytes move, and since D260 it is one of the three
    /// things that decide *whether* an absent blob is fetched at all — beside
    /// an explicit request and a download resumed from before a crash. A
    /// referenced blob nobody is showing and nobody asked for stays on
    /// Cloud. Among the ones that are fetched it still decides *order*: a
    /// picture somebody is staring at a placeholder for goes first.
    ///
    /// Coverage does not depend on it: coverage measures what Cloud holds,
    /// never what this device does.
    fn watching(&self, workspace: WorkspaceId, hash: &BlobHash) -> bool;

    /// The bytes are durable and associated. The image service invalidates its
    /// decoded-pixel entry for the hash and redraws the affected documents,
    /// reading the bytes from the object store as it always does.
    ///
    /// **After durability, never before**: a view woken by bytes a power cut
    /// could still erase is showing something that may not exist.
    fn arrived(&self, workspace: WorkspaceId, hash: &BlobHash);
}

/// A host with nothing on screen — a background pass, a test, Android today.
pub struct NoBlobDownloads;

impl BlobDownloads for NoBlobDownloads {
    fn watching(&self, _workspace: WorkspaceId, _hash: &BlobHash) -> bool {
        false
    }
    fn arrived(&self, _workspace: WorkspaceId, _hash: &BlobHash) {}
}

/// Everything one engine is composed of.
#[derive(Clone)]
pub struct BlobPorts {
    pub store: Arc<dyn BlobStorePort>,
    pub access: Arc<dyn CloudAccess>,
    pub policy: Arc<dyn BlobPolicy>,
    /// §6's gate. A field rather than an `Option`, so a host has to say what it
    /// means — [`AlwaysAdmitted`] is a sentence somebody can read, and a
    /// missing question is not.
    pub modes: Arc<dyn TransportModes>,
    /// §8's peer-first candidate, for the same reason: [`NoPeerBlobs`] says
    /// "this host has no peer path" out loud.
    pub peers: Arc<dyn PeerBlobs>,
    pub downloads: Arc<dyn BlobDownloads>,
}
