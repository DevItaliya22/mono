// SPDX-License-Identifier: MPL-2.0

//! **The delegated Run document through the real store.** What
//! `koru-store-core`'s `run_storage.rs` measures in memory over a stand-in
//! shape, this proves on disk over the shape `koru-delegate-document`
//! writes — the same `Store<SqliteCatalog, SqliteBackend>` the desktop worker
//! and the Android host (`koru-mobile-host`) both open — under temporary roots.
//!
//! P1.0 committed the first five properties (an unknown manifest kind is
//! preserved by the store, a restart, an older reader and a second root; a
//! cold open replays a bounded journal; two roots hold separate runs). P1.4
//! builds on them: two-root sync asserting the header a second device reads
//! (D16, D27), two replicas that each append and converge to one timeline and
//! one economics fold, the source paragraph trashed with the run intact
//! (D31), and **the criterion of proposal §1.6 in bytes and ratios** — never
//! milliseconds — over a deliberately long run.
//!
//! The figures are an ignored test:
//!
//! ```sh
//! cargo test --release -p koru-store-sqlite --test run_document -- --ignored --nocapture
//! ```

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use koru_delegate_core::{
    ActorRef, AmendmentRevision, Completeness, DelegationRequest, DeviceId, ModelRef, Objective,
    Rating, RatingNote, ResultReference, RunAttemptId, RunHeader, RunId, RunRating, RunStatus,
    SessionProfileKey, StepKind, TimelineEntry, TimelineRefs, UsageFact, WorkspaceScope,
};
use koru_delegate_document::{RUN_KIND, RunDocument, TimelineItem};
use koru_store_core::ids::{DocumentId, WorkspaceId};
use koru_store_core::manifest::Manifest;
use koru_store_core::model::{DocumentKind, Timestamp};
use koru_store_core::ports::WorkspaceRepo;
use koru_store_core::service::{Backend, Store};
use koru_store_core::text::Locale;
use koru_store_core::version::CRDT_SCHEMA_VERSION;
use koru_store_sqlite::{SqliteBackend, SqliteCatalog};
use loro::VersionVector;

type TestStore = Store<SqliteCatalog, SqliteBackend>;

/// The proposal's criterion for one document (§1.6), as constants so the
/// assertions below read as the budget they are.
const OPEN_BYTES_PER_ENTRY: usize = 256;
const OPEN_BYTES_HEADER: usize = 4 * 1024;
const STEP_BYTES: usize = 512;

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millis(1_770_000_000_000 + ms)
}

struct Root {
    dir: tempfile::TempDir,
    store: TestStore,
}

impl Root {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        Self {
            store: Self::open_in(dir.path()),
            dir,
        }
    }

    fn open_in(root: &std::path::Path) -> TestStore {
        let catalog = SqliteCatalog::open(&root.join("catalog.db")).unwrap();
        Store::open(catalog, SqliteBackend::new(root)).unwrap()
    }

    /// Close and reopen, as a restart would. Nothing is cached across it.
    fn restart(&mut self) {
        self.store = Self::open_in(self.dir.path());
    }

    fn a_workspace(&mut self) -> WorkspaceId {
        let profile = self
            .store
            .create_profile("Owner", None, None, Locale::parse("en").unwrap(), at(0))
            .unwrap();
        self.store
            .create_workspace(profile.id, "Workspace", at(0))
            .unwrap()
            .id
    }

    /// The repository beneath the store, for the bytes the store itself does
    /// not report: checkpoint size and journal size.
    fn repo(&self, workspace: WorkspaceId) -> <SqliteBackend as Backend>::Workspace {
        SqliteBackend::new(self.dir.path())
            .open_workspace(workspace)
            .unwrap()
    }

    /// The bytes a cold open of `run` reads: its newest checkpoint and the
    /// journal above it.
    fn cold_read_bytes(&self, workspace: WorkspaceId, run: DocumentId) -> usize {
        let repo = self.repo(workspace);
        let state = repo.document(run).unwrap().unwrap();
        let checkpoint = repo
            .latest_sound_checkpoint(run)
            .unwrap()
            .map_or(0, |c| c.snapshot.len());
        let journal = repo
            .journal_since(run, state.latest_checkpoint_seq)
            .unwrap()
            .bytes as usize;
        checkpoint + journal
    }

    fn listed(
        &mut self,
        workspace: WorkspaceId,
        document: DocumentId,
    ) -> koru_store_core::model::DocumentEntry {
        self.store
            .documents(workspace)
            .unwrap()
            .into_iter()
            .find(|entry| entry.id == document)
            .expect("the document is listed")
    }

    /// Open a run the way a surface does: fork, adopt.
    fn open(&mut self, workspace: WorkspaceId, run: DocumentId) -> RunDocument {
        RunDocument::adopt(self.store.fork_document(workspace, run).unwrap()).unwrap()
    }

    /// Bring `run` on this root level with `from`'s copy: the bytes since
    /// this root's version, applied and flushed — what `koru-replication`
    /// sends after the first sync. Returns the bytes moved.
    fn pull(
        &mut self,
        from: &mut Root,
        workspace: WorkspaceId,
        run: DocumentId,
        at_ms: i64,
    ) -> usize {
        let mine = self.store.version(workspace, run).unwrap();
        let theirs = from.open(workspace, run);
        let since = theirs.export_updates_since(&mine).unwrap();
        let bytes = since.len();
        self.store
            .apply_update(workspace, run, since, at(at_ms), None)
            .unwrap();
        self.store.flush(workspace, run, at(at_ms)).unwrap();
        bytes
    }
}

// ------------------------------------------------------------------ fixtures

fn a_header(device: DeviceId) -> RunHeader {
    RunHeader::from_request(
        DelegationRequest {
            run: RunId::mint(),
            requested_by: ActorRef::Person {
                profile: koru_store_core::ProfileId::new(),
            },
            objective: Objective::new("tidy the release notes").unwrap(),
            agent: SessionProfileKey::BuiltIn("codex".parse().unwrap()),
            requested_model: None,
            scope: WorkspaceScope::Temporary,
            context: Vec::new(),
            grants: BTreeSet::new(),
            executing_device: device,
            origin: Default::default(),
        },
        1_770_000_000_000,
    )
}

/// A fresh turn id. `AgentTurnId` is a v4 UUID in text; a `DocumentId` mints
/// one of the same spelling, and this suite links no `uuid` of its own.
fn a_turn() -> koru_delegate_core::AgentTurnId {
    DocumentId::new().to_storage().parse().unwrap()
}

/// The cursor of one run's activity: which attempt and turn the next entry
/// belongs to.
struct Cycle {
    attempt: RunAttemptId,
    turn: koru_delegate_core::AgentTurnId,
}

impl Cycle {
    fn new() -> Self {
        Self {
            attempt: RunAttemptId::mint(),
            turn: a_turn(),
        }
    }

    /// Append the `i`-th entry of a run in **P1.0's six-step cycle**, in the
    /// vocabulary: a turn starts (referencing the turn), three coalesced
    /// tool batches each pinning one document (two ids — the pin of D46),
    /// the turn completes, and its usage fact. The same mix the proposal
    /// measured its 170 bytes against, so the figures compare; every id is
    /// fresh, so nothing deduplicates.
    fn append(&mut self, run: &RunDocument, i: usize) {
        let at = 1_770_000_000_000 + i as i64 * 1_500;
        match i % 6 {
            0 => {
                self.turn = a_turn();
                run.append_step(&TimelineEntry::new(
                    StepKind::TurnStarted {
                        originator: koru_delegate_core::TurnOriginator::Orchestrator,
                    },
                    TimelineRefs::new([koru_delegate_core::TimelineRef::Turn { turn: self.turn }])
                        .unwrap(),
                    at,
                ))
                .unwrap();
            }
            1..=3 => run
                .append_step(&TimelineEntry::new(
                    StepKind::ToolsCompleted {
                        calls: (i % 5) as u32 + 1,
                        failed: 0,
                        failures: Default::default(),
                    },
                    TimelineRefs::new([koru_delegate_core::TimelineRef::Document {
                        document: DocumentId::new(),
                        version: koru_delegate_core::DocumentVersionId::new(),
                    }])
                    .unwrap(),
                    at,
                ))
                .unwrap(),
            4 => run
                .append_step(&TimelineEntry::new(
                    StepKind::TurnCompleted {
                        originator: koru_delegate_core::TurnOriginator::Orchestrator,
                        artifacts_registered: 0,
                    },
                    TimelineRefs::new([koru_delegate_core::TimelineRef::Turn { turn: self.turn }])
                        .unwrap(),
                    at,
                ))
                .unwrap(),
            _ => run
                .record_usage(&UsageFact {
                    identity: koru_delegate_core::UsageFactId::mint(),
                    attempt: self.attempt,
                    turn: self.turn,
                    revision: AmendmentRevision(0),
                    input: Some((i % 900) as u64),
                    output: Some((i % 300) as u64),
                    cached_input: None,
                    model: ModelRef::from_uuid(
                        "00000000-0000-4000-8000-000000000001".parse().unwrap(),
                    ),
                    completeness: Completeness::Complete,
                })
                .unwrap(),
        }
    }
}

fn usage(attempt: RunAttemptId, turn: u8, revision: u32, input: u64) -> UsageFact {
    UsageFact {
        // Stable per turn, so a retry carries the identity it had.
        identity: format!("00000000-0000-4000-8000-0000000001{turn:02x}")
            .parse()
            .unwrap(),
        attempt,
        turn: format!("00000000-0000-4000-8000-0000000000{turn:02x}")
            .parse()
            .unwrap(),
        revision: AmendmentRevision(revision),
        input: Some(input),
        output: Some(1),
        cached_input: None,
        model: ModelRef::mint(),
        completeness: Completeness::Complete,
    }
}

fn a_result(device: DeviceId) -> ResultReference {
    ResultReference {
        artifact: koru_delegate_core::ArtifactVersionId::mint(),
        held_by: device,
        title: koru_delegate_core::ArtifactTitle::new("notes.md").unwrap(),
        size_bytes: 2_048,
        registered_at_ms: 1_770_000_000_500,
    }
}

/// The executing device's replica of a run: it writes locally and sends the
/// store the update since its last send, one flush per fact — the sequence
/// P1.6 performs and `koru-delegate-document`'s crate docs spell.
struct Executing {
    workspace: WorkspaceId,
    id: DocumentId,
    run: RunDocument,
    sent: VersionVector,
    cycle: Cycle,
}

impl Executing {
    /// `create_document(ws, title, RUN_KIND, ..)`, then the header, sent.
    fn create(root: &mut Root, workspace: WorkspaceId, device: DeviceId) -> Self {
        let (id, _) = root
            .store
            .create_document(workspace, "Run", RUN_KIND, None, None, at(1))
            .unwrap();
        let run = RunDocument::create(&a_header(device)).unwrap();
        let mut this = Self {
            workspace,
            id,
            run,
            sent: VersionVector::default(),
            cycle: Cycle::new(),
        };
        this.send(root, 2);
        this
    }

    /// A replica over what the store holds, so a later attempt — or a second
    /// root's writer — continues the same history.
    fn joining(root: &mut Root, workspace: WorkspaceId, id: DocumentId) -> Self {
        let run = RunDocument::from_snapshot(&root.store.snapshot(workspace, id).unwrap()).unwrap();
        let sent = run.version();
        Self {
            workspace,
            id,
            run,
            sent,
            cycle: Cycle::new(),
        }
    }

    fn send(&mut self, root: &mut Root, at_ms: i64) -> usize {
        let delta = self.run.export_updates_since(&self.sent).unwrap();
        self.sent = self.run.version();
        let bytes = delta.len();
        root.store
            .apply_update(self.workspace, self.id, delta, at(at_ms), None)
            .unwrap();
        root.store
            .flush(self.workspace, self.id, at(at_ms))
            .unwrap();
        bytes
    }

    fn append(&mut self, root: &mut Root, i: usize) -> usize {
        self.cycle.append(&self.run, i);
        self.send(root, 3 + i as i64)
    }
}

/// Create a run with `steps` steps, each committed and flushed on its own.
fn a_run_with(root: &mut Root, workspace: WorkspaceId, steps: usize) -> DocumentId {
    let mut executing = Executing::create(root, workspace, DeviceId::new());
    executing.run.set_status(&RunStatus::running()).unwrap();
    executing.send(root, 2);
    for i in 0..steps {
        executing.append(root, i);
    }
    executing.id
}

/// A run in memory, the way `run_storage.rs` builds one: no store, one
/// commit per step, for the properties the store adds nothing to.
fn a_run_in_memory(steps: usize) -> RunDocument {
    let run = RunDocument::create(&a_header(DeviceId::new())).unwrap();
    let mut cycle = Cycle::new();
    for i in 0..steps {
        cycle.append(&run, i);
    }
    run
}

fn timeline_len(root: &mut Root, workspace: WorkspaceId, run: DocumentId) -> usize {
    root.open(workspace, run).timeline_len()
}

fn steps_of(run: &RunDocument) -> Vec<TimelineEntry> {
    run.timeline_all()
        .into_iter()
        .filter_map(|item| match item {
            TimelineItem::Step(entry) => Some(entry),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------ properties

/// The manifest's `kind` is an open token the store never interprets, so a
/// run registers under its own kind with no change to the manifest's shape
/// (`CRDT_SCHEMA_VERSION` stays 2), and the listing still says `koru-run`
/// — and the header still reads — after the store has been closed and
/// reopened.
#[test]
fn a_run_kind_survives_the_store_and_a_restart_with_the_manifest_schema_unchanged() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let run = a_run_with(&mut root, ws, 3);
    assert_eq!(root.listed(ws, run).kind, RUN_KIND);

    root.restart();
    assert_eq!(root.listed(ws, run).kind, RUN_KIND);
    let opened = root.open(ws, run);
    assert_eq!(opened.timeline_len(), 3);
    assert_eq!(opened.header().unwrap().status, RunStatus::running());
    assert!(opened.compatibility().writable());

    // To the store's own table it is an ordinary document — the closed
    // `DocumentKind` gains no value.
    let state = root.repo(ws).document(run).unwrap().unwrap();
    assert_eq!(state.kind, DocumentKind::Document);

    // The manifest that lists it is still a version-2 manifest.
    let manifest = root.store.manifest_id(ws).unwrap();
    let manifest = Manifest::from_snapshot(&root.store.snapshot(ws, manifest).unwrap()).unwrap();
    assert_eq!(manifest.schema_version(), Some(CRDT_SCHEMA_VERSION));
    assert!(manifest.compatibility().writable());
}

/// An older reader — any build before the run kind existed — is the same
/// manifest code, because nothing in it branches on the kind. It lists the
/// entry, keeps the token, and a write of its own (a rename) leaves the token
/// exactly as it found it: preserve and ignore, never drop or rewrite.
#[test]
fn an_older_reader_preserves_a_run_kind_it_does_not_know() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let run = a_run_with(&mut root, ws, 1);
    let manifest_id = root.store.manifest_id(ws).unwrap();

    let mut older =
        Manifest::from_snapshot(&root.store.snapshot(ws, manifest_id).unwrap()).unwrap();
    let entry = older
        .entries()
        .into_iter()
        .find(|entry| entry.id == run)
        .unwrap();
    assert_eq!(
        entry.kind, RUN_KIND,
        "listed with the token it does not know"
    );

    // The older writer edits something it does understand.
    let before = older.version();
    older
        .rename_document(run, "Renamed by an older build")
        .unwrap();
    let delta = older.export_updates_since(&before).unwrap();
    root.store
        .apply_update(ws, manifest_id, delta, at(50), None)
        .unwrap();
    root.store.flush(ws, manifest_id, at(50)).unwrap();

    let entry = root.listed(ws, run);
    assert_eq!(entry.title, "Renamed by an older build");
    assert_eq!(entry.kind, RUN_KIND, "the write left the kind alone");
    assert_eq!(older.schema_version(), Some(CRDT_SCHEMA_VERSION));
}

/// Over sync: a second root receives the manifest and the run the way
/// `koru-replication` delivers them — a snapshot for a replica with nothing,
/// then updates since its version — and reads the status, every step, the
/// result reference (D27) and the executing device (D16: the id in the
/// header, named elsewhere). Two temporary roots stand in for two devices.
#[test]
fn a_run_reaches_a_second_root_with_status_timeline_results_and_executing_device() {
    let mut a = Root::new();
    let ws = a.a_workspace();
    let device = DeviceId::new();
    let mut executing = Executing::create(&mut a, ws, device);
    executing.run.set_status(&RunStatus::running()).unwrap();
    for i in 0..20 {
        executing.append(&mut a, i);
    }
    executing.run.register_result(&a_result(device)).unwrap();
    executing.send(&mut a, 40);
    let run = executing.id;
    let manifest = a.store.manifest_id(ws).unwrap();
    let workspace_record = a.store.workspace(ws).unwrap();
    let profile = a.store.profile(workspace_record.owner).unwrap();

    let mut b = Root::new();
    b.store
        .provision_replica(&profile, &workspace_record, manifest, at(100))
        .unwrap();
    let manifest_snapshot = a.store.snapshot(ws, manifest).unwrap();
    b.store
        .apply_update(ws, manifest, manifest_snapshot, at(101), None)
        .unwrap();
    b.store.flush(ws, manifest, at(101)).unwrap();
    b.store
        .provision_replica_document(ws, run, at(102))
        .unwrap();
    let run_snapshot = a.store.snapshot(ws, run).unwrap();
    let first_sync_bytes = run_snapshot.len();
    b.store
        .apply_update(ws, run, run_snapshot, at(103), None)
        .unwrap();
    b.store.flush(ws, run, at(103)).unwrap();

    assert_eq!(b.listed(ws, run).kind, RUN_KIND);
    let on_b = b.open(ws, run);
    let header = on_b.header().unwrap();
    assert_eq!(header.status, RunStatus::running());
    assert_eq!(header.executing_device(), Some(device));
    assert_eq!(
        header.artifacts,
        vec![a_result_like(&header.artifacts[0], device)]
    );
    assert_eq!(
        header.artifacts[0].held_by, device,
        "D27: the holding device is named"
    );
    assert_eq!(on_b.timeline_len(), 20);
    assert_eq!(steps_of(&on_b), steps_of(&a.open(ws, run)));
    assert_eq!(
        b.store.version(ws, run).unwrap(),
        a.store.version(ws, run).unwrap()
    );

    // One more step on A travels as updates since B's version: bounded,
    // not another snapshot — and under the criterion's step budget.
    let mut executing = Executing::joining(&mut a, ws, run);
    executing.append(&mut a, 20);
    let since = b.pull(&mut a, ws, run, 105);
    assert!(
        since <= STEP_BYTES && since * 10 < first_sync_bytes,
        "one step ({since} bytes) is bounded and a fraction of the first sync ({first_sync_bytes} bytes)"
    );
    assert_eq!(timeline_len(&mut b, ws, run), 21);
    assert_eq!(
        b.store.version(ws, run).unwrap(),
        a.store.version(ws, run).unwrap()
    );
}

/// The result on B is A's result, field for field, except that `a_result`
/// mints a fresh artifact id each call — so compare against the one B holds.
fn a_result_like(held: &ResultReference, device: DeviceId) -> ResultReference {
    ResultReference {
        artifact: held.artifact,
        ..a_result(device)
    }
}

/// Two replicas of one run each append — steps and usage facts — and a
/// rating is written on the second; merged through their stores in both
/// directions, they hold the same timeline, the same economics fold, and
/// the rating. Convergence is Loro's: the list is append-only, the header
/// last-writer-wins; the fold is order-independent by construction.
#[test]
fn two_replicas_that_each_append_converge_to_one_timeline_and_one_fold() {
    let mut a = Root::new();
    let ws = a.a_workspace();
    let device = DeviceId::new();
    let mut on_a = Executing::create(&mut a, ws, device);
    on_a.run.set_status(&RunStatus::running()).unwrap();
    on_a.run
        .append_step(&TimelineEntry::new(
            StepKind::Started,
            TimelineRefs::default(),
            1_770_000_000_000,
        ))
        .unwrap();
    on_a.send(&mut a, 2);
    let run = on_a.id;

    let mut b = Root::new();
    let manifest = a.store.manifest_id(ws).unwrap();
    let workspace_record = a.store.workspace(ws).unwrap();
    let profile = a.store.profile(workspace_record.owner).unwrap();
    b.store
        .provision_replica(&profile, &workspace_record, manifest, at(100))
        .unwrap();
    b.store
        .apply_update(
            ws,
            manifest,
            a.store.snapshot(ws, manifest).unwrap(),
            at(101),
            None,
        )
        .unwrap();
    b.store.flush(ws, manifest, at(101)).unwrap();
    b.store
        .provision_replica_document(ws, run, at(102))
        .unwrap();
    b.store
        .apply_update(ws, run, a.store.snapshot(ws, run).unwrap(), at(103), None)
        .unwrap();
    b.store.flush(ws, run, at(103)).unwrap();
    let mut on_b = Executing::joining(&mut b, ws, run);

    // Each side appends without hearing from the other: A's attempt
    // reports a turn twice (a retry) and corrects it; B — a later attempt
    // after a handover, say — appends its own steps and a fact of its own.
    let attempt_a = RunAttemptId::mint();
    let attempt_b = RunAttemptId::mint();
    for i in 0..5 {
        on_a.append(&mut a, i);
    }
    on_a.run.record_usage(&usage(attempt_a, 1, 0, 100)).unwrap();
    on_a.run.record_usage(&usage(attempt_a, 1, 1, 140)).unwrap();
    on_a.run.record_usage(&usage(attempt_a, 1, 0, 100)).unwrap();
    on_a.send(&mut a, 200);
    for i in 5..8 {
        on_b.append(&mut b, i);
    }
    on_b.run.record_usage(&usage(attempt_b, 2, 0, 7)).unwrap();
    on_b.run
        .set_rating(&RunRating {
            rating: Rating::Good,
            note: Some(RatingNote::new("fine").unwrap()),
            rated_by: ActorRef::Person {
                profile: koru_store_core::ProfileId::new(),
            },
            rated_at_ms: 1_770_000_009_000,
        })
        .unwrap();
    on_b.send(&mut b, 300);

    // Merge both ways, through the stores.
    a.pull(&mut b, ws, run, 400);
    b.pull(&mut a, ws, run, 401);

    let read_a = a.open(ws, run);
    let read_b = b.open(ws, run);
    assert_eq!(
        a.store.version(ws, run).unwrap(),
        b.store.version(ws, run).unwrap()
    );
    assert_eq!(read_a.timeline_len(), 1 + 5 + 3 + 3 + 1);
    let set = |run: &RunDocument| -> BTreeSet<String> {
        run.timeline_all()
            .into_iter()
            .map(|item| format!("{item:?}"))
            .collect()
    };
    assert_eq!(set(&read_a), set(&read_b), "the same timeline set");
    assert_eq!(
        read_a.timeline_all(),
        read_b.timeline_all(),
        "in the same order"
    );

    let fold_a = read_a.economics(1_770_000_010_000);
    let fold_b = read_b.economics(1_770_000_010_000);
    assert_eq!(fold_a, fold_b, "the same fold");
    // A's corrected turn, B's turn, and the one usage fact the cycle itself
    // appended (entry 5, on A, whose input is 5).
    let cycle_input = 5;
    assert_eq!(fold_a.turns_with_usage, 3);
    assert_eq!(
        fold_a.input.complete(),
        Some(140 + 7 + cycle_input),
        "highest revision per key, on both"
    );
    assert!(fold_a.tool_calls > 0);

    let header_a = read_a.header().unwrap();
    assert_eq!(
        header_a.rating.as_ref().map(|r| r.rating),
        Some(Rating::Good),
        "B's rating reached A"
    );
    assert_eq!(header_a, read_b.header().unwrap());
}

/// **D31.** A pill in a paragraph references the run; the run is a
/// free-standing manifest entry. Trashing the page — through the store's
/// own trash operation — trashes the page and nothing else: the run stays
/// listed, untrashed, and opens with its header and every step.
#[test]
fn trashing_the_source_paragraphs_document_leaves_the_run_listed_and_openable() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let run = a_run_with(&mut root, ws, 4);

    // The page a person delegated from, holding the pill that names the run.
    let mut page =
        koru_core::schema::WorkspaceDoc::create(koru_core::WorkspaceId::new(), "W").unwrap();
    let doc_root = page
        .insert(
            koru_core::schema::Parent::Root,
            koru_core::node::Node::document("Plan").unwrap(),
        )
        .unwrap();
    let line = page
        .insert(
            koru_core::schema::Parent::Node(doc_root),
            koru_core::node::Node::paragraph("Delegated").unwrap(),
        )
        .unwrap();
    page.insert_pill(
        line,
        0,
        &koru_core::node::Attr::new(
            "koru.run",
            koru_core::node::AttrValue::Document(koru_core::DocumentReference::new(
                koru_core::DocumentId::parse(&run.to_storage()).unwrap(),
            )),
        ),
    )
    .unwrap();
    let (page_id, _) = root
        .store
        .import_document(
            ws,
            "Plan",
            "page",
            None,
            page.export_snapshot().unwrap(),
            at(60),
        )
        .unwrap();

    root.store
        .trash_document_entry(ws, page_id, at(70), Some("Tester"))
        .unwrap();

    assert!(
        root.listed(ws, page_id).trashed_at.is_some(),
        "the page is in the trash"
    );
    let entry = root.listed(ws, run);
    assert_eq!(entry.kind, RUN_KIND);
    assert_eq!(entry.trashed_at, None, "the run is not");
    assert_eq!(entry.parent, None, "and never was the page's child");
    let opened = root.open(ws, run);
    assert_eq!(opened.timeline_len(), 4);
    assert_eq!(opened.header().unwrap().status, RunStatus::running());

    // And after a restart, the same.
    root.restart();
    assert_eq!(root.listed(ws, run).trashed_at, None);
    assert_eq!(timeline_len(&mut root, ws, run), 4);
}

/// A cold open reads one checkpoint and the journal above it, and the
/// checkpoint policy keeps that journal bounded: after many steps the pending
/// journal never exceeds the policy's update count. The rest of the history
/// is in the checkpoint, read once.
#[test]
fn a_cold_open_replays_a_bounded_journal_whatever_the_history() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let policy = root.store.checkpoints;
    let steps = (policy.every_updates as usize) * 3 + 7;
    let run = a_run_with(&mut root, ws, steps);
    root.restart();

    let report = root.store.document_report(ws, run).unwrap();
    assert!(
        report.pending_updates <= policy.every_updates,
        "{} journal updates pending against a policy of {}",
        report.pending_updates,
        policy.every_updates
    );
    assert!(report.latest_checkpoint.is_some());
    assert_eq!(timeline_len(&mut root, ws, run), steps);
}

/// Two roots never see each other's runs: the same run id created under one
/// root is unknown to the other.
#[test]
fn two_roots_hold_separate_runs() {
    let mut a = Root::new();
    let ws = a.a_workspace();
    let run = a_run_with(&mut a, ws, 1);
    let mut b = Root::new();
    let other = b.a_workspace();
    assert_eq!(b.store.locate(run).unwrap(), None);
    assert!(b.store.documents(other).unwrap().is_empty());
}

// ------------------------------------------------------------------ the criterion (§1.6)

/// **Criterion 1.** The bytes to open a run are its snapshot, at most 256
/// per retained entry plus 4 KiB of header — measured as what a cold open
/// reads from the store (checkpoint plus journal) and as what a joining
/// replica imports (`snapshot`), at a hundred steps and at a thousand.
#[test]
fn bytes_to_open_a_run_are_within_256_per_entry_plus_4_kib() {
    for steps in [100usize, 1_000] {
        let mut root = Root::new();
        let ws = root.a_workspace();
        let policy = root.store.checkpoints;
        let run = a_run_with(&mut root, ws, steps);
        root.restart();
        let budget = OPEN_BYTES_PER_ENTRY * steps + OPEN_BYTES_HEADER;
        let cold = root.cold_read_bytes(ws, run);
        let snapshot = root.store.snapshot(ws, run).unwrap().len();
        assert!(
            snapshot <= budget,
            "{steps} steps: a joining replica imports {snapshot} bytes against a budget of {budget}"
        );
        if root
            .store
            .document_report(ws, run)
            .unwrap()
            .latest_checkpoint
            .is_some()
        {
            // Past the first checkpoint a cold open is the checkpoint — a
            // snapshot — plus a journal the policy bounds.
            assert!(
                cold <= budget,
                "{steps} steps: a cold open reads {cold} bytes against a budget of {budget}"
            );
        } else {
            // Before it, a cold open replays the journal verbatim: every
            // step as the update the executing device sent, each carrying
            // Loro's per-update envelope on top of the entry — about 330
            // bytes a step against 200 in a snapshot. Bounded by the policy
            // and by criterion 2, never by the history.
            let replay_budget = OPEN_BYTES_HEADER + policy.every_updates as usize * STEP_BYTES;
            assert!(
                cold <= replay_budget,
                "{steps} steps: a cold open before the first checkpoint replays {cold} bytes against {replay_budget}"
            );
            assert!(
                cold <= OPEN_BYTES_HEADER + steps * STEP_BYTES,
                "{steps} steps: the verbatim journal ({cold} bytes) is within what criterion 2 allows a step"
            );
        }
        assert_eq!(timeline_len(&mut root, ws, run), steps);
    }
}

/// **Criterion 2.** One appended step syncs in at most 512 bytes whatever
/// the history: the bytes a second root imports for one step on a
/// thousand-step run through the store, and — in memory, where the store
/// adds nothing — on a five-thousand-step run within 2× of the same step on
/// a hundred-step run.
#[test]
fn one_appended_step_syncs_within_512_bytes_whatever_the_history() {
    // Through the store, root to root.
    let mut a = Root::new();
    let ws = a.a_workspace();
    let run = a_run_with(&mut a, ws, 1_000);
    let mut b = Root::new();
    let manifest = a.store.manifest_id(ws).unwrap();
    let workspace_record = a.store.workspace(ws).unwrap();
    let profile = a.store.profile(workspace_record.owner).unwrap();
    b.store
        .provision_replica(&profile, &workspace_record, manifest, at(100))
        .unwrap();
    b.store
        .apply_update(
            ws,
            manifest,
            a.store.snapshot(ws, manifest).unwrap(),
            at(101),
            None,
        )
        .unwrap();
    b.store.flush(ws, manifest, at(101)).unwrap();
    b.store
        .provision_replica_document(ws, run, at(102))
        .unwrap();
    b.store
        .apply_update(ws, run, a.store.snapshot(ws, run).unwrap(), at(103), None)
        .unwrap();
    b.store.flush(ws, run, at(103)).unwrap();
    let mut executing = Executing::joining(&mut a, ws, run);
    let sent = executing.append(&mut a, 1_000);
    let received = b.pull(&mut a, ws, run, 200);
    assert!(
        sent <= STEP_BYTES && received <= STEP_BYTES,
        "one step after 1,000: {sent} bytes sent to the store, {received} bytes to the second root"
    );
    assert_eq!(timeline_len(&mut b, ws, run), 1_001);

    // In memory, short against long.
    let on_short = appended_step_bytes(&a_run_in_memory(100), 100);
    let on_long = appended_step_bytes(&a_run_in_memory(5_000), 5_000);
    assert!(
        on_long <= STEP_BYTES && on_long <= on_short * 2,
        "one step cost {on_long} bytes after 5,000 entries and {on_short} after 100"
    );
}

/// The bytes a replica that already has everything before it imports for
/// one more step.
fn appended_step_bytes(run: &RunDocument, i: usize) -> usize {
    let before = run.version();
    // The cycle's heaviest entry: a tool batch pinning one document.
    Cycle::new().append(run, i - i % 6 + 1);
    run.export_updates_since(&before).unwrap().len()
}

/// **Criterion 3.** A snapshot grows no worse than linearly: bytes per
/// entry at five thousand within 1.25× of bytes per entry at a hundred.
#[test]
fn a_snapshot_grows_no_worse_than_linearly_with_its_entries() {
    let per_entry =
        |n: usize| a_run_in_memory(n).export_snapshot().unwrap().len() as f64 / n as f64;
    let small = per_entry(100);
    let large = per_entry(5_000);
    assert!(
        large <= small * 1.25,
        "{large:.1} bytes per entry at 5,000 against {small:.1} at 100"
    );
    assert!(
        large <= OPEN_BYTES_PER_ENTRY as f64,
        "{large:.1} bytes per entry at 5,000 against the criterion's 256"
    );
}

/// **Criterion 4.** Import is no worse than linear above a thousand
/// entries: opening ten times the entries costs at most twenty times the
/// time — a ratio, and a loose one, measured in the same process on the
/// same machine, because the fixed cost below a thousand entries still
/// shows. Medians of three; never a wall-clock budget.
#[test]
fn import_is_no_worse_than_linear_above_a_thousand_entries() {
    let small = a_run_in_memory(1_000).export_snapshot().unwrap();
    let large = a_run_in_memory(10_000).export_snapshot().unwrap();
    let import = |bytes: &[u8]| {
        let run = RunDocument::from_snapshot(bytes).unwrap();
        assert!(run.timeline_len() >= 1_000);
    };
    let one = timed(|| import(&small));
    let ten = timed(|| import(&large));
    let ratio = ten.as_secs_f64() / one.as_secs_f64().max(f64::EPSILON);
    assert!(
        ratio <= 20.0,
        "importing 10,000 entries cost {ratio:.1}x importing 1,000 ({ten:?} against {one:?})"
    );
}

fn timed(f: impl Fn()) -> Duration {
    let mut samples: Vec<Duration> = (0..3)
        .map(|_| {
            let began = Instant::now();
            f();
            began.elapsed()
        })
        .collect();
    samples.sort();
    samples[1]
}

/// **Criterion 6.** A window read costs no store bytes once the document is
/// open: the store is closed before the first window is read, and every
/// window across the whole history is served from the fork.
#[test]
fn a_window_read_costs_no_store_bytes() {
    let mut root = Root::new();
    let ws = root.a_workspace();
    let run = a_run_with(&mut root, ws, 300);
    let opened = root.open(ws, run);
    drop(root);

    let mut seen = 0;
    let mut from = 0;
    while from < opened.timeline_len() {
        let window = opened.timeline_window(from, 50);
        assert!(!window.is_empty());
        assert!(
            window
                .iter()
                .all(|item| !matches!(item, TimelineItem::Unrecognized))
        );
        seen += window.len();
        from += 50;
    }
    assert_eq!(seen, 300);
    assert_eq!(opened.header().unwrap().status, RunStatus::running());
}

// ------------------------------------------------------------------ figures

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

/// What a cold open, a warm open and a first replication cost through the
/// store, per history length, over the real shape. ASCII only.
#[ignore = "P1.4's measurement: cargo test --release -p koru-store-sqlite --test run_document -- --ignored --nocapture"]
#[test]
fn run_document_store_figures() {
    eprintln!();
    eprintln!(
        "run document through Store<SqliteCatalog, SqliteBackend>, one flush per step, koru-delegate-document shape"
    );
    eprintln!(
        "  {:>8} {:>10} {:>8} {:>10} {:>10} {:>8} {:>10} {:>10} {:>10}",
        "steps",
        "checkpoint",
        "journal",
        "cold read",
        "snapshot",
        "B/entry",
        "cold open",
        "warm fork",
        "append+fs"
    );
    for steps in [100usize, 1_000, 10_000] {
        let mut root = Root::new();
        let ws = root.a_workspace();
        let began = Instant::now();
        let run = a_run_with(&mut root, ws, steps);
        let append = began.elapsed() / steps as u32;
        root.restart();

        let repo = root.repo(ws);
        let state = repo.document(run).unwrap().unwrap();
        let checkpoint = repo
            .latest_sound_checkpoint(run)
            .unwrap()
            .map_or(0, |c| c.snapshot.len());
        let journal = repo
            .journal_since(run, state.latest_checkpoint_seq)
            .unwrap()
            .bytes as usize;
        drop(repo);

        let cold = {
            let began = Instant::now();
            let _ = root.store.fork_document(ws, run).unwrap();
            began.elapsed()
        };
        let warm = median(
            (0..5)
                .map(|_| {
                    let began = Instant::now();
                    let _ = root.store.fork_document(ws, run).unwrap();
                    began.elapsed()
                })
                .collect(),
        );
        let snapshot = root.store.snapshot(ws, run).unwrap().len();
        eprintln!(
            "  {:>8} {:>10} {:>8} {:>10} {:>10} {:>8.1} {:>8.2}ms {:>8.3}ms {:>8.2}ms",
            steps,
            checkpoint,
            journal,
            checkpoint + journal,
            snapshot,
            snapshot as f64 / steps as f64,
            cold.as_secs_f64() * 1_000.0,
            warm.as_secs_f64() * 1_000.0,
            append.as_secs_f64() * 1_000.0,
        );
    }
    eprintln!();
    eprintln!("in memory, one commit per step");
    eprintln!(
        "  {:>8} {:>10} {:>8} {:>10} {:>7}",
        "entries", "snapshot", "B/entry", "import", "step B"
    );
    for n in [100usize, 1_000, 5_000, 10_000] {
        let run = a_run_in_memory(n);
        let snapshot = run.export_snapshot().unwrap();
        let import = timed(|| {
            let _ = RunDocument::from_snapshot(&snapshot).unwrap();
        });
        let step_bytes = appended_step_bytes(&run, n);
        eprintln!(
            "  {:>8} {:>10} {:>8.1} {:>8.2}ms {:>7}",
            n,
            snapshot.len(),
            snapshot.len() as f64 / n as f64,
            import.as_secs_f64() * 1_000.0,
            step_bytes
        );
    }
}
