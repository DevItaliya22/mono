// SPDX-License-Identifier: MPL-2.0

//! Test fixtures: a ledger under a temporary root, a clock moved by hand,
//! deterministic ids, and a request whose objective is a sentinel no
//! rendering may ever contain.
//!
//! Every ledger a test opens lives under `tempfile::tempdir()`; nothing
//! here resolves the real channel root, and the root's `store/` child is
//! passed as the store root so `runs_db_path` places the file the way a
//! `--root R` stack would (`R/delegate/runs.db` beside `R/store`).

use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
};

use koru_delegate_core::{
    ActorRef, AttemptGeneration, DelegationRequest, DocumentId, DocumentVersionId, EventId,
    HostInstanceId, IdempotencyKey, Objective, ProfileId, ProviderSessionKey, RelativeFolder,
    RunContextItem, RunId, ScheduleId, SessionId, SessionProfileKey, WorkspaceId, WorkspaceScope,
};
use uuid::Uuid;

use crate::{AttemptPhase, AttemptRow, Clock, RunsDb, runs_db_path};

pub(crate) const OBJECTIVE_SENTINEL: &str = "OBJECTIVE-SENTINEL rewrite the intro";
pub(crate) const FOLDER_SENTINEL: &str = "FOLDER-SENTINEL/src/private";
pub(crate) const BODY_SENTINEL: &str = "BODY-SENTINEL";

pub(crate) struct FakeClock(pub Arc<AtomicI64>);

impl Clock for FakeClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// A temporary channel root and the clock every ledger opened under it
/// shares, so a "restart" (drop, reopen) keeps the same notion of time.
pub(crate) struct Root {
    dir: tempfile::TempDir,
    clock: Arc<AtomicI64>,
}

impl Root {
    pub(crate) fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            clock: Arc::new(AtomicI64::new(1_000)),
        }
    }

    pub(crate) fn path(&self) -> &std::path::Path {
        self.dir.path()
    }

    pub(crate) fn store_root(&self) -> PathBuf {
        self.dir.path().join("store")
    }

    pub(crate) fn db_path(&self) -> PathBuf {
        runs_db_path(&self.store_root())
    }

    pub(crate) fn open(&self) -> RunsDb {
        RunsDb::open_with_clock(&self.db_path(), Box::new(FakeClock(self.clock.clone()))).unwrap()
    }

    pub(crate) fn now_ms(&self) -> i64 {
        self.clock.load(Ordering::SeqCst)
    }

    pub(crate) fn tick(&self, by_ms: i64) {
        self.clock.fetch_add(by_ms, Ordering::SeqCst);
    }
}

/// A deterministic v4 UUID — version nibble 4, variant 8 — so strict ids
/// accept it and a fixture is the same on every run.
pub(crate) fn v4(n: u8) -> Uuid {
    format!("00000000-0000-4000-8000-0000000000{n:02x}")
        .parse()
        .unwrap()
}

pub(crate) fn run_id(n: u8) -> RunId {
    RunId::from_uuid(v4(n))
}

pub(crate) fn workspace(n: u8) -> WorkspaceId {
    WorkspaceId::from_uuid(v4(n))
}

pub(crate) fn document(n: u8) -> DocumentId {
    DocumentId::from_uuid(v4(n))
}

pub(crate) fn host(n: u8) -> HostInstanceId {
    HostInstanceId::from_uuid(v4(n))
}

pub(crate) fn key(n: u8) -> IdempotencyKey {
    v4(n).hyphenated().to_string().parse().unwrap()
}

pub(crate) fn session(n: u8) -> SessionId {
    v4(n).hyphenated().to_string().parse().unwrap()
}

pub(crate) fn event(n: u8) -> EventId {
    EventId(v4(n))
}

pub(crate) fn schedule(n: u8) -> ScheduleId {
    ScheduleId::from_uuid(v4(n))
}

/// A key reserved but not yet answered by the host.
pub(crate) fn reserved(n: u8) -> ProviderSessionKey {
    ProviderSessionKey {
        idempotency: key(n),
        session: None,
    }
}

/// A request for `run_id(n)`. Its objective and folder are sentinels the
/// privacy tests grep every rendering for.
pub(crate) fn request(n: u8) -> DelegationRequest {
    DelegationRequest {
        run: run_id(n),
        requested_by: ActorRef::Person {
            profile: ProfileId::from_uuid(v4(0xa0)),
        },
        objective: Objective::new(OBJECTIVE_SENTINEL).unwrap(),
        agent: SessionProfileKey::BuiltIn("codex".parse().unwrap()),
        requested_model: None,
        scope: WorkspaceScope::ProjectFolder {
            project: v4(0xa1).hyphenated().to_string().parse().unwrap(),
            folder: RelativeFolder::new(FOLDER_SENTINEL).unwrap(),
        },
        context: vec![RunContextItem::Document {
            workspace: workspace(0xa2),
            document: document(0xa3),
            version: DocumentVersionId::from_uuid(v4(0xa4)),
        }],
        grants: BTreeSet::from([workspace(0xa2)]),
        executing_device: koru_delegate_core::DeviceId::from_uuid(v4(0xa5)),
        origin: Default::default(),
    }
}

/// A run that exists: intent recorded and satisfied.
pub(crate) fn created_run(db: &mut RunsDb, n: u8) -> RunId {
    let run = run_id(n);
    db.record_intent(&request(n), workspace(0xa2)).unwrap();
    db.satisfy_intent(run, document(n.wrapping_add(0x40)))
        .unwrap();
    run
}

/// A running attempt on `run`, bound to `key(n)` under `host(n)`.
pub(crate) fn running_attempt(db: &mut RunsDb, run: RunId, n: u8) -> AttemptRow {
    let opened = db.open_attempt(run).unwrap();
    let generation: AttemptGeneration = opened.generation;
    db.advance(run, generation, AttemptPhase::Starting).unwrap();
    db.bind_session(run, generation, reserved(n), host(n))
        .unwrap();
    db.bind_session(
        run,
        generation,
        ProviderSessionKey {
            idempotency: key(n),
            session: Some(session(n.wrapping_add(0x20))),
        },
        host(n),
    )
    .unwrap();
    db.advance(run, generation, AttemptPhase::Running).unwrap()
}
