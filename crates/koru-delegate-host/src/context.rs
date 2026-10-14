// SPDX-License-Identifier: MPL-2.0

//! What a run was given, over the wire (§7.2, P1.7).
//!
//! `koru-delegate-context` states the five questions a capture asks a
//! store and implements them for the in-process `Store`. A delegate worker
//! has no store in its process — it reaches one through the gateway, the
//! way `documents.rs` reaches the Run documents — so [`WireContextStore`]
//! answers the same five questions over `koru-api-client`, and
//! `StoreContext<WireContextStore>` becomes the orchestrator's real
//! `ContextCapture`.
//!
//! **Nothing is decided here.** Which bytes a pin names, what is written
//! where, and what a failure is called are the context crate's; this file
//! is the transport under it and the two joins the runtime's port shape
//! leaves to a host:
//!
//! - **the checksums.** The runtime's `CapturedContext` is the pinned
//!   items and nothing else, while materialization verifies each pin
//!   against the checksum capture read. [`HostCapture`] keeps the full
//!   capture per run and re-verifies from the ledger's workspace when a
//!   restart materializes a run this process never captured.
//! - **the delivery mode.** The context crate takes it as an argument and
//!   the runtime's port does not carry one, because materialization
//!   happens before a session exists (§7.2 step 4). It is read from the
//!   run's launch profile — the mode P1.1 declares on the profile — with
//!   `QuotedPath` for a profile that declares none, which is what every
//!   CLI provider can read.
//!
//! The `.artifacts/<RunId>` folder the preamble names is created here too:
//! it is where a run writes results, not part of what it was given, so the
//! context crate does not know about it.

use std::collections::HashMap;
use std::path::Path;

use koru_api_client::Client;
use koru_delegate_context::{ContextStore, Fault, PinLiveFailure, StoreContext};
use koru_delegate_core::{
    BlobHash, DocumentId, DocumentVersionId, RunContextItem, RunId, WorkspaceId,
};
use koru_delegate_runtime::{
    CaptureFailure, CaptureStage, CapturedContext, ContextCapture, MaterializeFailure,
    MaterializeStage, MaterializedContext, SourceRefusal, SourceRefusalReason,
};
use koru_delegate_schedule::ScheduleContextPointer;
use koru_session_protocol::{AttachmentDeliveryPolicy, DeliveryMode};
use koru_store_core::model::{Blob, DocumentEntry, Timestamp};
use koru_store_core::ports::DocumentVersion;

use crate::ledger::SharedLedger;

/// The folder a run writes its results under, inside its working scope
/// (D37). The orchestrator names it in the first submission, so it exists
/// before the provider starts.
const ARTIFACTS_FOLDER: &str = ".artifacts";

/// `koru_delegate_context::ContextStore` over the gateway.
pub struct WireContextStore {
    client: Client,
}

impl WireContextStore {
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

impl ContextStore for WireContextStore {
    /// The store stamps a marker's moment itself over the wire, so `at` is
    /// not sent. No frontiers: the live frontier is what the in-process
    /// call captures with `None`, and it is what a person delegating means
    /// by "this document". No label and no message — both are free text a
    /// person would read.
    fn capture_version(
        &mut self,
        workspace: WorkspaceId,
        document: DocumentId,
        _at: Timestamp,
    ) -> Result<DocumentVersionId, Fault> {
        let entry = self
            .client
            .store()
            .capture_history(
                workspace,
                document,
                &koru_store_client::http::CaptureHistory {
                    reason: koru_store_core::ports::HistoryReason::Manual
                        .as_str()
                        .into(),
                    frontiers: None,
                    label: None,
                    message: None,
                    boundary: None,
                },
            )
            .map_err(|error| Fault::from_store(&error))?;
        Ok(entry.version)
    }

    fn version(
        &mut self,
        workspace: WorkspaceId,
        version: DocumentVersionId,
    ) -> Result<Option<DocumentVersion>, Fault> {
        let answered = self
            .client
            .store()
            .version_snapshot(workspace, version)
            .map_err(|error| {
                // The client verified the bytes against the checksum the
                // store sent beside them; a mismatch is a fact about *this*
                // version, not a store that is down.
                if error.code == koru_store_core::errors::Code::CorruptState {
                    Fault::VersionCorrupt
                } else {
                    Fault::from_store(&error)
                }
            })?;
        Ok(answered.map(|answered| DocumentVersion {
            id: answered.id,
            document: answered.document,
            snapshot: answered.snapshot,
            // The two vectors are the store's own bookkeeping for a
            // rollback; a run reads the snapshot and the checksum, and the
            // route does not carry them.
            version_vector: Vec::new(),
            frontiers: Vec::new(),
            checksum: answered.checksum,
            captured_at: answered.captured_at,
        }))
    }

    fn entries(&mut self, workspace: WorkspaceId) -> Result<Vec<DocumentEntry>, Fault> {
        self.client
            .store()
            .documents(workspace)
            .map_err(|error| Fault::from_store(&error))
    }

    /// **The workspace's blob records, filtered here.** The API serves the
    /// bytes behind one hash but has no route for one record; rather than
    /// add a route for a lookup a run performs once per pinned file, this
    /// reads the list the API already serves. If a workspace ever holds
    /// enough blobs for that to matter, the route is the fix — see
    /// `docs/contributing/techdebt/README.md`.
    fn blob(&mut self, workspace: WorkspaceId, hash: &BlobHash) -> Result<Option<Blob>, Fault> {
        let blobs = self
            .client
            .store()
            .blobs(workspace)
            .map_err(|error| Fault::from_store(&error))?;
        Ok(blobs.into_iter().find(|blob| blob.hash == *hash))
    }

    fn blob_bytes(&mut self, workspace: WorkspaceId, hash: &BlobHash) -> Result<Vec<u8>, Fault> {
        self.client
            .store()
            .get_blob(workspace, hash)
            .map_err(|error| Fault::from_store(&error))
    }
}

/// The orchestrator's `ContextCapture`, over the wire.
pub struct HostCapture {
    store: WireContextStore,
    ledger: SharedLedger,
    /// The full capture per run — the pins *with* the checksums the
    /// runtime's port shape does not carry.
    captured: HashMap<RunId, koru_delegate_context::CapturedContext>,
}

impl HostCapture {
    pub fn new(client: Client, ledger: SharedLedger) -> Self {
        Self {
            store: WireContextStore::new(client),
            ledger,
            captured: HashMap::new(),
        }
    }

    /// How this run's provider is handed files. The launch profile's
    /// declared policy; `QuotedPath` when it declares none, and when the
    /// ledger cannot name the profile at all — a path in the prompt is
    /// what every CLI provider this milestone launches can read, and the
    /// layout on disk is the same either way.
    fn delivery_for(&self, run: RunId) -> DeliveryMode {
        delivery_of(
            self.ledger
                .intent_facts(run)
                .ok()
                .flatten()
                .and_then(|facts| koru_session_profiles::resolve(&facts.agent).ok())
                .map(|profile| profile.attachment_delivery),
        )
    }

    /// The capture this run's pins verified to. Re-verified against the
    /// store when this process did not perform the capture — a restart
    /// materializes from the Run document's pins, and the checksums are
    /// the store's to answer, not something to remember across a crash.
    fn full_capture(
        &mut self,
        run: RunId,
        pins: &[RunContextItem],
    ) -> Result<koru_delegate_context::CapturedContext, MaterializeFailure> {
        if let Some(known) = self.captured.get(&run) {
            return Ok(known.clone());
        }
        let workspace = self
            .ledger
            .intent_facts(run)
            .ok()
            .flatten()
            .map(|facts| facts.workspace)
            .or_else(|| pins.first().map(workspace_of))
            .ok_or(MaterializeFailure {
                item: 0,
                stage: MaterializeStage::Read,
            })?;
        let captured = StoreContext::new(&mut self.store)
            .capture(run, pins, workspace)
            .map_err(|failure| MaterializeFailure {
                item: failure.item,
                stage: MaterializeStage::Read,
            })?;
        self.captured.insert(run, captured.clone());
        Ok(captured)
    }
}

/// Schedule-only context resolver. It owns no Run cache and no ledger: its
/// sole job is the fresh all-or-nothing live pin performed before a firing is
/// admitted. That makes it `Send` and keeps Store work on the scheduler
/// thread.
pub struct ScheduleCapture {
    store: WireContextStore,
}

impl ScheduleCapture {
    pub fn new(client: Client) -> Self {
        Self {
            store: WireContextStore::new(client),
        }
    }

    pub fn resolve_and_pin(
        &mut self,
        run: RunId,
        pointers: &[ScheduleContextPointer],
        workspace: WorkspaceId,
    ) -> Result<Vec<RunContextItem>, CaptureFailure> {
        let sources = pointers.iter().map(schedule_source).collect::<Vec<_>>();
        StoreContext::new(&mut self.store)
            .pin_live(run, &sources, workspace)
            .map_err(|failure| CaptureFailure {
                item: failure.item(),
                stage: match failure {
                    PinLiveFailure::Capture(failure) => capture_stage(failure.stage),
                    PinLiveFailure::Refused(_) => CaptureStage::Resolve,
                },
            })
    }
}

/// The declared policy as a delivery mode. `Unsupported` is the policy's
/// `Default`, so a profile that says nothing arrives here as `None` or as
/// `Unsupported` and means the same thing: undeclared. Both become
/// `QuotedPath` rather than refusing the run — a path in the prompt is
/// what every CLI provider this milestone launches can read, and the
/// layout on disk is identical under either mode.
fn delivery_of(declared: Option<AttachmentDeliveryPolicy>) -> DeliveryMode {
    match declared {
        Some(AttachmentDeliveryPolicy::NativeStructured) => DeliveryMode::NativeStructured,
        Some(AttachmentDeliveryPolicy::QuotedPath { .. }) => DeliveryMode::QuotedPath,
        Some(AttachmentDeliveryPolicy::Unsupported) | None => DeliveryMode::QuotedPath,
    }
}

fn workspace_of(item: &RunContextItem) -> WorkspaceId {
    match item {
        RunContextItem::Document { workspace, .. }
        | RunContextItem::Block { workspace, .. }
        | RunContextItem::Blob { workspace, .. } => *workspace,
    }
}

fn schedule_source(pointer: &ScheduleContextPointer) -> koru_delegate_context::ContextSource {
    match pointer {
        ScheduleContextPointer::Document {
            workspace,
            document,
        } => koru_delegate_context::ContextSource::Document {
            workspace: *workspace,
            document: *document,
        },
        ScheduleContextPointer::Block {
            workspace,
            document,
            node,
        } => koru_delegate_context::ContextSource::Block {
            workspace: *workspace,
            document: *document,
            node: *node,
        },
        ScheduleContextPointer::Blob {
            workspace,
            blob,
            blob_kind,
        } => koru_delegate_context::ContextSource::Blob {
            workspace: *workspace,
            blob: blob.clone(),
            blob_kind: *blob_kind,
        },
    }
}

/// §7.2's stages, as the runtime's narrower port names them: a commit that
/// did not happen and a verification that failed are one refusal to the
/// orchestrator, which decides the same thing about both.
fn capture_stage(stage: koru_delegate_context::CaptureStage) -> CaptureStage {
    match stage {
        koru_delegate_context::CaptureStage::Resolve => CaptureStage::Resolve,
        koru_delegate_context::CaptureStage::Capture => CaptureStage::Commit,
        koru_delegate_context::CaptureStage::Retention => CaptureStage::Retention,
    }
}

/// The four places materialization can stop, as the runtime's two: laying
/// the directory out and writing a file are both writing, and reading the
/// pinned bytes and rendering them are both reading what the pin points at.
fn materialize_stage(stage: koru_delegate_context::MaterializeStage) -> MaterializeStage {
    match stage {
        koru_delegate_context::MaterializeStage::Layout
        | koru_delegate_context::MaterializeStage::Write => MaterializeStage::Write,
        koru_delegate_context::MaterializeStage::Read
        | koru_delegate_context::MaterializeStage::Render => MaterializeStage::Read,
    }
}

impl ContextCapture for HostCapture {
    fn resolve_and_pin(
        &mut self,
        run: RunId,
        pointers: &[ScheduleContextPointer],
        workspace: WorkspaceId,
    ) -> Result<Vec<RunContextItem>, CaptureFailure> {
        let sources = pointers.iter().map(schedule_source).collect::<Vec<_>>();
        StoreContext::new(&mut self.store)
            .pin_live(run, &sources, workspace)
            .map_err(|failure| CaptureFailure {
                item: failure.item(),
                stage: match failure {
                    PinLiveFailure::Capture(failure) => capture_stage(failure.stage),
                    PinLiveFailure::Refused(_) => CaptureStage::Resolve,
                },
            })
    }

    /// D48: may these pins start something new? Read-only, and it reads
    /// only each pin's item — the checksum a `ContextPin` also carries is
    /// what `capture` verifies, not what the trash answers — so a pin this
    /// process never captured is asked about by item alone.
    fn admit(
        &mut self,
        run: RunId,
        pins: &[koru_delegate_runtime::ContextPin],
    ) -> Result<(), SourceRefusal> {
        let known = self.captured.get(&run);
        let asked: Vec<koru_delegate_context::ContextPin> = pins
            .iter()
            .map(|item| koru_delegate_context::ContextPin {
                item: item.clone(),
                checksum: known
                    .and_then(|captured| {
                        captured
                            .pins
                            .iter()
                            .find(|pin| &pin.item == item)
                            .map(|pin| pin.checksum.clone())
                    })
                    .unwrap_or_default(),
            })
            .collect();
        StoreContext::new(&mut self.store)
            .admit(&asked)
            .map_err(|refusal| SourceRefusal {
                item: refusal.item(),
                reason: match refusal {
                    koru_delegate_context::SourceRefusal::InTrash { .. } => {
                        SourceRefusalReason::InTrash
                    }
                    koru_delegate_context::SourceRefusal::Unavailable { .. } => {
                        SourceRefusalReason::Unavailable
                    }
                    koru_delegate_context::SourceRefusal::Unanswered { .. } => {
                        SourceRefusalReason::Unanswered
                    }
                },
            })
    }

    fn capture(
        &mut self,
        run: RunId,
        items: &[RunContextItem],
        workspace: WorkspaceId,
    ) -> Result<CapturedContext, CaptureFailure> {
        let captured = StoreContext::new(&mut self.store)
            .capture(run, items, workspace)
            .map_err(|failure| CaptureFailure {
                item: failure.item,
                stage: capture_stage(failure.stage),
            })?;
        let pins = captured.pins.iter().map(|pin| pin.item.clone()).collect();
        self.captured.insert(run, captured);
        Ok(CapturedContext { pins })
    }

    fn materialize(
        &mut self,
        run: RunId,
        captured: &CapturedContext,
        run_workspace: &Path,
    ) -> Result<MaterializedContext, MaterializeFailure> {
        // Where results go. Created before the provider starts because the
        // first submission names it (§7.2 step 4, P1.12).
        std::fs::create_dir_all(run_workspace.join(ARTIFACTS_FOLDER).join(run.to_storage()))
            .map_err(|_| MaterializeFailure {
                item: 0,
                stage: MaterializeStage::Write,
            })?;
        let full = self.full_capture(run, &captured.pins)?;
        let delivery = self.delivery_for(run);
        let laid_out = StoreContext::new(&mut self.store)
            .materialize(run, &full, delivery, run_workspace)
            .map_err(|failure| MaterializeFailure {
                item: failure.item.unwrap_or(0),
                stage: materialize_stage(failure.stage),
            })?;
        Ok(MaterializedContext {
            directory: laid_out.root,
            items: u32::try_from(laid_out.files.len()).unwrap_or(u32::MAX),
        })
    }

    /// The materialized copies go; the captured versions, the blobs and the
    /// results stay (D37, D47).
    fn cleanup(&mut self, run: RunId, run_workspace: &Path) {
        self.captured.remove(&run);
        let _ = koru_delegate_context::cleanup(run, run_workspace);
    }
}

#[cfg(test)]
mod tests;
