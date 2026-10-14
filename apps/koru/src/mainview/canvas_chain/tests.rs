// SPDX-License-Identifier: MPL-2.0

//! A canvas's chain, driven end to end through ports that answer from
//! memory: the canvas is an in-memory document the fake store edits exactly
//! as the store's worker does (`CanvasEdit::apply`), runs are created and
//! their results read by a fake run port, and a run's state is the header the
//! run pills' worker would read. No gateway, no store, no disk, no thread of
//! the app's own, no telemetry.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, mpsc};

use koru_core::node::Node;
use koru_core::{BlobHash, CanvasPoint, WorkspaceId};
use koru_delegate_core::{
    ActorRef, DelegationRequest, FailureStage, Objective, ResultReference, RunFailureReason,
    RunHeader, WorkspaceScope,
};
use koru_delegate_document::RunDocument;
use koru_session_protocol::{
    CapabilitySet, ExecutableAvailability, ProviderReadiness, SessionProfileId, capability,
};

use super::*;
use crate::mainview::canvas_runs::{RunPort, Watch};
use crate::mainview::delegate::job::{RunAnswer, RunJob};
use crate::mainview::delegate::pills::RunPillState;

// ------------------------------------------------------------ the canvas

/// The canvas as the store holds it.
struct Board {
    ws: WorkspaceDoc,
    root: NodeId,
}

impl Board {
    fn new() -> Self {
        let mut ws = WorkspaceDoc::create(WorkspaceId::new(), "canvas").unwrap();
        let root = ws
            .insert(Parent::Root, Node::document("Board").unwrap())
            .unwrap();
        Self { ws, root }
    }

    fn add(&mut self, kind: ObjectKind, x: i32, y: i32, text: &str) -> NodeId {
        canvas::add_object(&mut self.ws, self.root, kind, CanvasPoint::new(x, y), text).unwrap()
    }

    fn image(&mut self, x: i32, y: i32) -> NodeId {
        canvas::add_image(
            &mut self.ws,
            self.root,
            CanvasPoint::new(x, y),
            BlobHash(format!("b3:{}", "c".repeat(64))),
            "image/png",
            Some(64),
            Some(64),
            120,
        )
        .unwrap()
    }

    fn arrow(&mut self, from: NodeId, to: NodeId) {
        canvas::add_arrow(&mut self.ws, self.root, from, to).unwrap();
    }

    fn object(&self, id: NodeId) -> canvas::CanvasObject {
        canvas::read(&self.ws, self.root)
            .unwrap()
            .get(id)
            .cloned()
            .expect("on the canvas")
    }
}

// ------------------------------------------------------------ the fake store

#[derive(Default)]
struct StoreSeen {
    edits: Vec<CanvasEdit>,
    pages: Vec<PageWrite>,
    page_answers: Vec<mpsc::Sender<Result<DocumentId, String>>>,
}

/// The store: every edit applied to the board at once, as another writer
/// would; every page write held until the test answers it.
#[derive(Clone)]
struct FakeStore {
    board: Arc<Mutex<Board>>,
    seen: Arc<Mutex<StoreSeen>>,
}

impl FakeStore {
    fn new() -> Self {
        Self {
            board: Arc::new(Mutex::new(Board::new())),
            seen: Arc::default(),
        }
    }

    fn board(&self) -> std::sync::MutexGuard<'_, Board> {
        self.board.lock().unwrap()
    }

    fn seen(&self) -> std::sync::MutexGuard<'_, StoreSeen> {
        self.seen.lock().unwrap()
    }

    fn page_answered(&self, index: usize, answer: Result<DocumentId, String>) {
        self.seen().page_answers[index].send(answer).unwrap();
    }
}

impl ChainPort for FakeStore {
    fn edit(
        &mut self,
        _reach: Reach<'_>,
        _canvas: DocumentId,
        root: Option<NodeId>,
        edit: CanvasEdit,
    ) -> mpsc::Receiver<Result<CanvasRead, String>> {
        let (send, receive) = mpsc::channel();
        let mut board = self.board.lock().unwrap();
        let root = root.unwrap_or(board.root);
        let answer = edit
            .apply(&mut board.ws, root)
            .and_then(|()| read_canvas(&board.ws, root))
            .map_err(|error| error.to_string());
        self.seen().edits.push(edit);
        send.send(answer).unwrap();
        receive
    }

    fn page(
        &mut self,
        _reach: Reach<'_>,
        write: PageWrite,
    ) -> mpsc::Receiver<Result<DocumentId, String>> {
        let (send, receive) = mpsc::channel();
        let mut seen = self.seen();
        seen.pages.push(write);
        seen.page_answers.push(send);
        receive
    }
}

// ------------------------------------------------------------ the fake runs

#[derive(Default)]
struct RunsSeen {
    created: Vec<RunSpec>,
    creations: HashMap<RunId, mpsc::Sender<RunAnswer>>,
    cancels: Vec<RunId>,
    reads: Vec<ArtifactVersionId>,
    read_answers: HashMap<ArtifactVersionId, mpsc::Sender<Result<ResultBody, String>>>,
}

#[derive(Clone, Default)]
struct FakeRuns {
    seen: Arc<Mutex<RunsSeen>>,
}

impl FakeRuns {
    fn seen(&self) -> std::sync::MutexGuard<'_, RunsSeen> {
        self.seen.lock().unwrap()
    }
}

impl RunPort for FakeRuns {
    fn create(&mut self, _reach: Reach<'_>, spec: RunSpec) -> Result<RunJob, RunAnswer> {
        let (send, receive) = mpsc::channel();
        let run = spec.run;
        let mut seen = self.seen();
        seen.created.push(spec);
        seen.creations.insert(run, send);
        Ok(RunJob::from_receiver(run, receive))
    }

    fn cancel(&mut self, _reach: Reach<'_>, run: RunId) -> mpsc::Receiver<Result<(), String>> {
        let (send, receive) = mpsc::channel();
        self.seen().cancels.push(run);
        send.send(Ok(())).unwrap();
        receive
    }

    fn read(
        &mut self,
        _reach: Reach<'_>,
        version: ArtifactVersionId,
    ) -> mpsc::Receiver<Result<ResultBody, String>> {
        let (send, receive) = mpsc::channel();
        let mut seen = self.seen();
        seen.reads.push(version);
        seen.read_answers.insert(version, send);
        receive
    }

    fn watch(
        &mut self,
        _reach: Reach<'_>,
        _workspace: uuid::Uuid,
        _touched: Arc<Mutex<HashSet<uuid::Uuid>>>,
    ) -> Option<Watch> {
        None
    }
}

// ------------------------------------------------------------ the window

fn delegatable(kind: &str) -> ProviderReadiness {
    ProviderReadiness {
        kind: kind.parse().unwrap(),
        availability: ExecutableAvailability::Available { version: None },
        capabilities: CapabilitySet::new(vec![capability::MANAGED_DELEGATION.parse().unwrap()]),
        failure: None,
    }
}

fn all_ready() -> Vec<ProviderReadiness> {
    koru_session_profiles::MANAGED_PROVIDER_REGISTRATIONS
        .iter()
        .map(|entry| delegatable(entry.kind))
        .collect()
}

fn none_ready() -> Vec<ProviderReadiness> {
    let mut all = all_ready();
    for row in &mut all {
        row.availability = ExecutableAvailability::Missing;
        row.capabilities = CapabilitySet::default();
    }
    all
}

fn lang() -> Lang {
    crate::format::lang()
}

const HERE: uuid::Uuid = uuid::Uuid::from_u128(0xd1ce);

struct Rig {
    view: MainView,
    _workspace: koru_workspace::Workspace,
    store: FakeStore,
    runs: FakeRuns,
    canvas: DocumentId,
}

impl Rig {
    fn new() -> Self {
        Self::on(FakeStore::new(), HERE)
    }

    /// A window over `store`'s canvas, on the device `device`.
    fn on(store: FakeStore, device: uuid::Uuid) -> Self {
        let (mut view, workspace) = crate::mainview::harness::container();
        let runs = FakeRuns::default();
        view.canvas_runs = CanvasRuns::with_port(Box::new(runs.clone()));
        view.canvas_chains = CanvasChains::with_port(Box::new(store.clone()));
        view.provider_readiness = Some(all_ready());
        view.this_device = Some(koru_device::DeviceId::from_uuid(device));
        Self {
            view,
            _workspace: workspace,
            store,
            runs,
            canvas: DocumentId::from_uuid(uuid::Uuid::from_u128(0xca_17a5)),
        }
    }

    fn root(&self) -> NodeId {
        self.store.board().root
    }

    /// Passes the way the window makes them, until nothing more moves: runs
    /// first, then the chains they belong to.
    fn settle(&mut self) {
        for _ in 0..12 {
            self.view.maintain_canvas_runs();
            self.view.maintain_canvas_chains();
        }
    }

    fn states(&self) -> HashMap<NodeId, CanvasNodeState> {
        self.view.canvas_node_states(self.canvas)
    }

    fn state(&self, node: NodeId) -> CanvasNodeState {
        self.states()
            .get(&node)
            .cloned()
            .unwrap_or(CanvasNodeState::Idle)
    }

    fn specs(&self) -> Vec<RunSpec> {
        self.runs.seen().created.clone()
    }

    /// The host creates the newest run asked for; answers its Run document.
    fn created(&mut self) -> (RunId, DocumentId) {
        let run = self.specs().last().expect("a run was asked for").run;
        let document = DocumentId::new();
        let send = self.runs.seen().creations.remove(&run).expect("a creation");
        send.send(RunAnswer::Done {
            run,
            document,
            busy: None,
        })
        .unwrap();
        self.settle();
        (run, document)
    }

    /// What the pills' worker reads for `document`.
    fn reads_as(&mut self, document: DocumentId, state: RunPillState) {
        self.view
            .run_pill_states
            .insert(document.to_storage(), state);
        self.settle();
    }

    /// A result read answers.
    fn result_read(&mut self, version: ArtifactVersionId, body: Result<ResultBody, String>) {
        let send = self
            .runs
            .seen()
            .read_answers
            .remove(&version)
            .expect("that result was asked for");
        send.send(body).unwrap();
        self.settle();
    }

    /// The newest run is created, works, and finishes with `markdown`.
    fn finishes_with(&mut self, markdown: &str) -> DocumentId {
        let (_, document) = self.created();
        let result = a_result();
        self.reads_as(
            document,
            read_as(RunStatus::Completed, vec![result.clone()], None),
        );
        self.result_read(result.artifact, Ok(ResultBody::Markdown(markdown.into())));
        document
    }
}

/// A run's header as its executing device commits it, read the way the run
/// pills' worker reads it.
fn read_as(
    status: RunStatus,
    results: Vec<ResultReference>,
    failure: Option<RunFailureReason>,
) -> RunPillState {
    let agent = koru_session_profiles::built_in_profiles()
        .into_iter()
        .find(|profile| {
            profile
                .provider
                .as_ref()
                .and_then(koru_session_profiles::managed_provider)
                .is_some()
        })
        .expect("a delegatable built-in")
        .id;
    let mut header = RunHeader::from_request(
        DelegationRequest {
            requested_model: None,
            run: RunId::mint(),
            requested_by: ActorRef::Person {
                profile: koru_store_core::ProfileId::new(),
            },
            objective: Objective::new("Summarize").unwrap(),
            agent,
            scope: WorkspaceScope::Temporary,
            context: Vec::new(),
            grants: Default::default(),
            executing_device: koru_device::DeviceId::from_uuid(HERE),
            origin: Default::default(),
        },
        1_700_000_000_000,
    );
    header.status = status;
    header.failure_reason = failure;
    let run = RunDocument::create(&header).unwrap();
    for result in &results {
        run.register_result(result).unwrap();
    }
    RunPillState::of(&run, &run.header().unwrap(), 1_700_000_060_000)
}

fn a_result() -> ResultReference {
    ResultReference {
        artifact: ArtifactVersionId::mint(),
        held_by: koru_device::DeviceId::from_uuid(HERE),
        title: koru_delegate_core::ArtifactTitle::new("The summary").unwrap(),
        size_bytes: 64,
        registered_at_ms: 1_700_000_050_000,
    }
}

fn running() -> CanvasNodeState {
    CanvasNodeState::Running {
        activity: None,
        elapsed: None,
    }
}

fn block(rig: &Rig, node: NodeId) -> ContextSource {
    let workspace = rig.view.chain_workspace().expect("a workspace is open");
    ContextSource::Block {
        workspace: koru_delegate_core::WorkspaceId::from_uuid(workspace.as_uuid()),
        document: koru_delegate_core::DocumentId::from_uuid(rig.canvas.as_uuid()),
        node: BlockNodeId::from_uuid(node.as_uuid()),
    }
}

/// start → A → B → output, with two cards and an image into A.
struct Straight {
    start: NodeId,
    first_card: NodeId,
    second_card: NodeId,
    picture: NodeId,
    a: NodeId,
    b: NodeId,
    output: NodeId,
}

fn straight(rig: &Rig) -> Straight {
    let mut board = rig.store.board();
    let first_card = board.add(ObjectKind::Card, 300, -300, "Revenue grew");
    let second_card = board.add(ObjectKind::Card, 600, -300, "Costs fell");
    let picture = board.image(900, -300);
    let start = board.add(ObjectKind::Start, 0, 0, "Go");
    let a = board.add(ObjectKind::Agent, 300, 0, "Summarize\nthe cards");
    let b = board.add(ObjectKind::Agent, 600, 0, "Critique");
    let output = board.add(ObjectKind::Output, 900, 0, "Report");
    for (from, to) in [
        (start, a),
        (first_card, a),
        (second_card, a),
        (picture, a),
        (a, b),
        (b, output),
    ] {
        board.arrow(from, to);
    }
    Straight {
        start,
        first_card,
        second_card,
        picture,
        a,
        b,
        output,
    }
}

// ------------------------------------------------------------ a chain

/// **A straight chain advances a step at a time.** A's run is given its
/// cards and its picture, in reading order; its result is copied into A;
/// B is given A; the output writes one page from B; and every node says
/// where it is along the way.
#[test]
fn a_straight_chain_advances_step_by_step() {
    let mut rig = Rig::new();
    let chain = straight(&rig);
    let root = rig.root();

    rig.view.run_chain(rig.canvas, root, chain.start);
    assert_eq!(rig.state(chain.start), running(), "pressed");
    assert!(
        rig.specs().is_empty(),
        "nothing starts on the calling thread"
    );
    rig.settle();

    // The press is recorded on the start node, with this device.
    let execution = {
        let board = rig.store.board();
        let execution = canvas::execution(&board.ws, chain.start)
            .unwrap()
            .expect("the execution is on the start node");
        assert_eq!(
            execution.device,
            koru_device::DeviceId::from_uuid(HERE).to_storage()
        );
        execution.id
    };

    // A: its instruction, and its inputs in the canvas's reading order.
    let specs = rig.specs();
    assert_eq!(specs.len(), 1, "only A is ready");
    assert_eq!(specs[0].objective, "Summarize\nthe cards");
    let workspace =
        koru_delegate_core::WorkspaceId::from_uuid(rig.view.chain_workspace().unwrap().as_uuid());
    assert_eq!(
        specs[0].sources,
        vec![
            block(&rig, chain.first_card),
            block(&rig, chain.second_card),
            ContextSource::Blob {
                workspace,
                blob: koru_delegate_core::BlobHash::parse(&format!("b3:{}", "c".repeat(64)))
                    .unwrap(),
                blob_kind: BlobKind::Image,
            },
        ]
    );
    let _ = chain.picture;
    assert_eq!(rig.state(chain.a), running());
    assert_eq!(rig.state(chain.b), CanvasNodeState::Idle);
    assert_eq!(rig.state(chain.output), CanvasNodeState::Idle);

    // A is created: its node links the run, in this execution.
    let (_, a_document) = rig.created();
    {
        let board = rig.store.board();
        assert_eq!(board.object(chain.a).run, Some(a_document));
        assert_eq!(
            canvas::step_execution(&board.ws, chain.a)
                .unwrap()
                .as_deref(),
            Some(execution.as_str())
        );
    }

    // A works, and says what it is doing in the run pill's words.
    rig.reads_as(a_document, read_as(RunStatus::running(), Vec::new(), None));
    assert!(matches!(
        rig.state(chain.a),
        CanvasNodeState::Running { .. }
    ));
    assert!(rig.specs().len() == 1, "B waits for A's result");

    // Approval/input pauses remain active steps. They expose a response
    // action without starting B, minting another run, or changing the canvas.
    for marker in [
        koru_delegate_core::Attention::ApprovalRequired,
        koru_delegate_core::Attention::WaitingForInput,
    ] {
        rig.reads_as(
            a_document,
            read_as(
                RunStatus::Running {
                    attention: koru_delegate_core::AttentionSet::new([marker]),
                    control: Default::default(),
                },
                Vec::new(),
                None,
            ),
        );
        assert_eq!(rig.state(chain.a), CanvasNodeState::NeedsResponse);
        assert_eq!(rig.specs().len(), 1);
        assert_eq!(rig.store.board().object(chain.a).run, Some(a_document));
    }
    rig.reads_as(a_document, read_as(RunStatus::running(), Vec::new(), None));
    assert!(matches!(
        rig.state(chain.a),
        CanvasNodeState::Running { .. }
    ));

    // A finishes: its newest result is read and copied into it, and then B
    // starts with A as its input.
    let result = a_result();
    rig.reads_as(
        a_document,
        read_as(RunStatus::Completed, vec![a_result(), result.clone()], None),
    );
    assert_eq!(
        rig.runs.seen().reads,
        vec![result.artifact],
        "the newest result"
    );
    assert_eq!(rig.specs().len(), 1, "not before the copy is written");
    rig.result_read(
        result.artifact,
        Ok(ResultBody::Markdown(
            "\n# Summary\n\nRevenue up, costs down\n\n".into(),
        )),
    );
    assert_eq!(
        rig.store.board().object(chain.a).result,
        vec!["# Summary", "", "Revenue up, costs down"]
    );
    assert_eq!(rig.state(chain.a), CanvasNodeState::Done);
    let specs = rig.specs();
    assert_eq!(specs.len(), 2);
    assert_eq!(specs[1].objective, "Critique");
    assert_eq!(specs[1].sources, vec![block(&rig, chain.a)]);
    assert_eq!(rig.state(chain.b), running());

    // B finishes; the output writes its page from B, once.
    rig.finishes_with("Fair, but thin");
    assert_eq!(rig.state(chain.b), CanvasNodeState::Done);
    assert_eq!(rig.state(chain.output), running());
    let write = {
        let seen = rig.store.seen();
        assert_eq!(seen.pages.len(), 1);
        seen.pages[0].clone()
    };
    assert_eq!(write.title, "Report");
    assert_eq!(write.markdown, "Fair, but thin");
    assert_eq!(write.existing, None);
    assert_eq!(write.execution, execution);
    assert_eq!(
        write.idempotency_key(),
        format!("canvas-output:{}:{}", chain.output.to_storage(), execution)
    );

    let page = DocumentId::new();
    rig.store.page_answered(0, Ok(page));
    rig.settle();
    {
        let board = rig.store.board();
        assert_eq!(board.object(chain.output).output_page, Some(page));
        assert_eq!(
            canvas::step_execution(&board.ws, chain.output)
                .unwrap()
                .as_deref(),
            Some(execution.as_str())
        );
        assert_eq!(
            canvas::execution(&board.ws, chain.start).unwrap(),
            None,
            "an ended run is cleared from the start node"
        );
    }
    for node in [chain.start, chain.a, chain.b, chain.output] {
        assert_eq!(rig.state(node), CanvasNodeState::Done, "{node:?}");
    }
    rig.settle();
    assert_eq!(rig.store.seen().pages.len(), 1, "one page per execution");
    assert_eq!(rig.specs().len(), 2, "each node ran once");
}

/// **The output page is asked for once per execution**, and a second run
/// updates the page the first wrote rather than making another.
#[test]
fn a_second_run_updates_the_page_the_first_wrote() {
    let mut rig = Rig::new();
    let chain = straight(&rig);
    let root = rig.root();

    rig.view.run_chain(rig.canvas, root, chain.start);
    rig.settle();
    rig.finishes_with("First");
    rig.finishes_with("First critique");
    let page = DocumentId::new();
    rig.store.page_answered(0, Ok(page));
    rig.settle();
    assert_eq!(rig.state(chain.start), CanvasNodeState::Done);

    // Pressing again while it runs does nothing; pressing after is a new run.
    rig.view.run_chain(rig.canvas, root, chain.start);
    rig.view.run_chain(rig.canvas, root, chain.start);
    rig.settle();
    assert_eq!(rig.specs().len(), 3, "one new run of A");
    rig.finishes_with("Second");
    rig.finishes_with("Second critique");
    let seen = rig.store.seen();
    assert_eq!(seen.pages.len(), 2);
    let (first, second) = (&seen.pages[0], &seen.pages[1]);
    assert_eq!(second.existing, Some(page), "the same page, in place");
    assert_ne!(first.execution, second.execution);
    assert_ne!(first.idempotency_key(), second.idempotency_key());
    assert_eq!(second.markdown, "Second critique");
    assert!(
        second.before.contains("Report") && second.after.contains("Report"),
        "the History labels name the node"
    );
}

/// **A failed step stops what depends on it, and says why**: B and the
/// output never start, and the start node names where it stopped.
#[test]
fn a_failed_step_halts_what_follows_with_a_reason() {
    let mut rig = Rig::new();
    let chain = straight(&rig);
    let root = rig.root();
    rig.view.run_chain(rig.canvas, root, chain.start);
    rig.settle();
    let (_, document) = rig.created();
    let failed = RunStatus::Failed {
        stage: FailureStage::ProviderStart,
    };
    rig.reads_as(
        document,
        read_as(
            failed.clone(),
            Vec::new(),
            Some(RunFailureReason::AuthenticationRequired),
        ),
    );

    let why = koru_runs_ui::failed_because(
        lang(),
        &failed,
        Some(RunFailureReason::AuthenticationRequired),
    )
    .unwrap();
    assert_eq!(rig.state(chain.a), CanvasNodeState::Failed(why));
    assert_eq!(rig.state(chain.b), CanvasNodeState::NotStarted);
    assert_eq!(rig.state(chain.output), CanvasNodeState::NotStarted);
    assert_eq!(
        rig.state(chain.start),
        CanvasNodeState::Failed(fill(t(lang(), Key::CanvasChainHaltedAt), &["Summarize"]))
    );
    assert_eq!(rig.specs().len(), 1, "nothing after A ran");
    assert!(rig.store.seen().pages.is_empty());
}

/// **A step that finishes with nothing to hand on halts the chain** the same
/// way, in its own words.
#[test]
fn a_result_less_finish_halts() {
    let mut rig = Rig::new();
    let chain = straight(&rig);
    let root = rig.root();
    rig.view.run_chain(rig.canvas, root, chain.start);
    rig.settle();
    let (_, document) = rig.created();
    rig.reads_as(document, read_as(RunStatus::Completed, Vec::new(), None));
    assert_eq!(
        rig.state(chain.a),
        CanvasNodeState::Failed(t(lang(), Key::CanvasChainNoResult).into())
    );
    assert_eq!(rig.state(chain.b), CanvasNodeState::NotStarted);
    assert!(matches!(rig.state(chain.start), CanvasNodeState::Failed(_)));
    assert_eq!(rig.specs().len(), 1);

    // A result whose text is only blank lines is no result either.
    let mut rig = Rig::new();
    let chain = straight(&rig);
    let root = rig.root();
    rig.view.run_chain(rig.canvas, root, chain.start);
    rig.settle();
    rig.finishes_with("\n  \n");
    assert_eq!(
        rig.state(chain.a),
        CanvasNodeState::Failed(t(lang(), Key::CanvasChainNoResult).into())
    );
    assert!(rig.store.board().object(chain.a).result.is_empty());
    assert_eq!(rig.specs().len(), 1);
}

/// **Stop stops the running step and marks the rest not started**: one
/// cancel, *Stopping…* until the run says it stopped, then *Stopped*.
#[test]
fn stop_marks_the_rest_not_started() {
    let mut rig = Rig::new();
    let chain = straight(&rig);
    let root = rig.root();
    rig.view.run_chain(rig.canvas, root, chain.start);
    rig.settle();
    let (run, document) = rig.created();
    rig.reads_as(document, read_as(RunStatus::running(), Vec::new(), None));

    rig.view.stop_chain(rig.canvas, chain.start);
    rig.view.stop_chain(rig.canvas, chain.start);
    rig.settle();
    assert_eq!(rig.runs.seen().cancels, vec![run], "one cancel");
    assert_eq!(rig.state(chain.start), CanvasNodeState::Stopping);
    assert_eq!(rig.state(chain.a), CanvasNodeState::Stopping);
    assert_eq!(rig.state(chain.b), CanvasNodeState::NotStarted);

    rig.reads_as(document, read_as(RunStatus::Cancelled, Vec::new(), None));
    assert_eq!(rig.state(chain.a), CanvasNodeState::Stopped);
    assert_eq!(rig.state(chain.b), CanvasNodeState::NotStarted);
    assert_eq!(rig.state(chain.output), CanvasNodeState::NotStarted);
    assert_eq!(rig.state(chain.start), CanvasNodeState::Stopped);
    assert_eq!(rig.specs().len(), 1, "nothing starts after a stop");
    assert_eq!(
        canvas::execution(&rig.store.board().ws, chain.start).unwrap(),
        None,
        "a stopped run has ended"
    );

    // Stop on a node stops the chain it is a step of, the same way — here
    // while its run is still being created: cancelled the moment it exists.
    let mut rig = Rig::new();
    let chain = straight(&rig);
    let root = rig.root();
    rig.view.run_chain(rig.canvas, root, chain.start);
    rig.settle();
    rig.view.stop_node(rig.canvas, chain.a);
    rig.settle();
    assert_eq!(rig.state(chain.a), CanvasNodeState::Stopping);
    assert!(rig.runs.seen().cancels.is_empty(), "nothing to cancel yet");
    let (run, document) = rig.created();
    assert_eq!(rig.runs.seen().cancels, vec![run]);
    rig.reads_as(document, read_as(RunStatus::Cancelled, Vec::new(), None));
    assert_eq!(rig.state(chain.a), CanvasNodeState::Stopped);
    assert_eq!(rig.state(chain.b), CanvasNodeState::NotStarted);
    assert_eq!(rig.state(chain.start), CanvasNodeState::Stopped);
}

/// **A cycle is refused on the start node, naming a node on it**, and
/// nothing runs or is recorded.
#[test]
fn a_cycle_is_reported_on_the_start_node() {
    let mut rig = Rig::new();
    let (start, a) = {
        let mut board = rig.store.board();
        let start = board.add(ObjectKind::Start, 0, 0, "Go");
        let a = board.add(ObjectKind::Agent, 300, 0, "Draft");
        let b = board.add(ObjectKind::Agent, 600, 0, "Review");
        board.arrow(start, a);
        board.arrow(a, b);
        board.arrow(b, a);
        (start, a)
    };
    let root = rig.root();
    rig.view.run_chain(rig.canvas, root, start);
    rig.settle();
    let CanvasNodeState::NotReady(why) = rig.state(start) else {
        panic!("a cycle is not ready: {:?}", rig.state(start));
    };
    let on_loop = ["Draft", "Review"].map(|name| fill(t(lang(), Key::CanvasChainLoop), &[name]));
    assert!(on_loop.contains(&why), "{why}");
    assert!(rig.specs().is_empty());
    assert_eq!(
        canvas::execution(&rig.store.board().ws, start).unwrap(),
        None
    );
    assert_eq!(rig.state(a), CanvasNodeState::Idle);

    // A start node that reaches nothing says what to do.
    let lonely = rig.store.board().add(ObjectKind::Start, 0, 400, "Alone");
    rig.view.run_chain(rig.canvas, root, lonely);
    rig.settle();
    assert_eq!(
        rig.state(lonely),
        CanvasNodeState::NotReady(t(lang(), Key::CanvasChainEmpty).into())
    );
}

/// **What an agent node cannot run without, it says**: an instruction, and
/// an agent this machine can run. A node's own agent is used while it is
/// ready; otherwise the default.
#[test]
fn an_agent_node_says_what_it_is_missing() {
    // No instruction.
    let mut rig = Rig::new();
    let (start, empty) = {
        let mut board = rig.store.board();
        let start = board.add(ObjectKind::Start, 0, 0, "Go");
        let empty = board.add(ObjectKind::Agent, 300, 0, "  ");
        board.arrow(start, empty);
        (start, empty)
    };
    let root = rig.root();
    rig.view.run_chain(rig.canvas, root, start);
    rig.settle();
    assert_eq!(
        rig.state(empty),
        CanvasNodeState::NotReady(t(lang(), Key::CanvasChainNoInstruction).into())
    );
    assert!(matches!(rig.state(start), CanvasNodeState::Failed(_)));
    assert!(rig.specs().is_empty());

    // No agent set up on this machine.
    let mut rig = Rig::new();
    rig.view.provider_readiness = Some(none_ready());
    let chain = straight(&rig);
    let root = rig.root();
    rig.view.run_chain(rig.canvas, root, chain.start);
    rig.settle();
    assert_eq!(
        rig.state(chain.a),
        CanvasNodeState::NotReady(t(lang(), Key::DelegateNoAgents).into())
    );
    assert!(rig.specs().is_empty());

    // The node's own agent while it is ready; the default otherwise.
    let mut rig = Rig::new();
    let chain = straight(&rig);
    let root = rig.root();
    rig.settle();
    let ready = rig.view.canvas_runs.ready_agents();
    let chosen = ready.last().expect("an agent is ready").0.clone();
    canvas::set_agent(
        &mut rig.store.board().ws,
        chain.a,
        &agent_key_text(&chosen),
        Some("a-model"),
    )
    .unwrap();
    canvas::set_agent(
        &mut rig.store.board().ws,
        chain.b,
        &agent_key_text(&SessionProfileKey::Custom(SessionProfileId::mint())),
        None,
    )
    .unwrap();
    rig.view.run_chain(rig.canvas, root, chain.start);
    rig.settle();
    let spec = rig.specs()[0].clone();
    assert_eq!(spec.agent, chosen);
    assert_eq!(spec.model, Some(ModelName::new("a-model").unwrap()));
    rig.finishes_with("done");
    assert_eq!(
        Some(rig.specs()[1].agent.clone()),
        rig.view.canvas_runs.default_agent(),
        "an agent that is not here falls back to the default"
    );
}

/// **One agent node runs on its own** (UC10): its inputs, its result copied
/// into it, and no start node involved.
#[test]
fn an_agent_node_runs_on_its_own() {
    let mut rig = Rig::new();
    let (card, agent) = {
        let mut board = rig.store.board();
        let card = board.add(ObjectKind::Card, 0, 0, "Notes");
        let agent = board.add(ObjectKind::Agent, 300, 0, "Tidy these");
        board.arrow(card, agent);
        (card, agent)
    };
    let root = rig.root();
    rig.view.run_agent_node(rig.canvas, root, agent);
    assert_eq!(rig.state(agent), running());
    rig.settle();
    let specs = rig.specs();
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].sources, vec![block(&rig, card)]);
    rig.view.run_agent_node(rig.canvas, root, agent);
    rig.settle();
    assert_eq!(rig.specs().len(), 1, "a running node is not run twice");

    rig.finishes_with("Tidied");
    assert_eq!(rig.state(agent), CanvasNodeState::Done);
    assert_eq!(rig.store.board().object(agent).result, vec!["Tidied"]);
    assert!(rig.store.seen().pages.is_empty());

    // An image result is a line naming it, for now.
    rig.view.run_agent_node(rig.canvas, root, agent);
    rig.settle();
    let (_, document) = rig.created();
    let result = a_result();
    rig.reads_as(
        document,
        read_as(RunStatus::Completed, vec![result.clone()], None),
    );
    rig.result_read(
        result.artifact,
        Ok(ResultBody::Image {
            blob: koru_artifacts_ui::BlobHash::from_digest("b3", &"ab".repeat(32)).unwrap(),
            mime: "image/png".into(),
        }),
    );
    assert_eq!(
        rig.store.board().object(agent).result,
        vec![fill(
            t(lang(), Key::CanvasChainResultImage),
            &["The summary"]
        )]
    );
}

// ------------------------------------------------------------ opening again

/// **A chain this device began continues when its canvas is opened again**:
/// A ran in the execution and finished while the window was closed, so its
/// result is copied and B starts. On another device, nothing advances.
#[test]
fn opening_the_canvas_again_continues_only_on_the_device_that_began_it() {
    let store = FakeStore::new();
    let (chain, execution, a_document) = {
        let rig = Rig::on(store.clone(), HERE);
        let chain = straight(&rig);
        let execution = "a-run-begun-before".to_owned();
        let a_document = DocumentId::new();
        let mut board = store.board();
        canvas::set_execution(
            &mut board.ws,
            chain.start,
            &execution,
            &koru_device::DeviceId::from_uuid(HERE).to_storage(),
        )
        .unwrap();
        canvas::set_run(&mut board.ws, chain.a, a_document).unwrap();
        canvas::set_step_execution(&mut board.ws, chain.a, &execution).unwrap();
        (chain, execution, a_document)
    };

    // Another device opens the canvas: nothing is followed or started.
    let mut elsewhere = Rig::on(store.clone(), uuid::Uuid::from_u128(0xfa7));
    elsewhere.states();
    elsewhere.settle();
    assert!(elsewhere.specs().is_empty());
    assert_eq!(
        elsewhere.view.canvas_runs.run_documents().count(),
        0,
        "A's run is not followed here"
    );
    assert_eq!(elsewhere.state(chain.b), CanvasNodeState::Idle);

    // This device opens it: A's run is followed again.
    let mut rig = Rig::on(store, HERE);
    rig.states();
    rig.settle();
    assert_eq!(rig.state(chain.a), running());
    assert!(rig.specs().is_empty(), "A is not run a second time");
    let result = a_result();
    rig.reads_as(
        a_document,
        read_as(RunStatus::Completed, vec![result.clone()], None),
    );
    rig.result_read(result.artifact, Ok(ResultBody::Markdown("Found it".into())));
    assert_eq!(rig.store.board().object(chain.a).result, vec!["Found it"]);
    let specs = rig.specs();
    assert_eq!(specs.len(), 1, "B starts");
    assert_eq!(specs[0].objective, "Critique");
    rig.finishes_with("Critiqued");
    assert_eq!(rig.store.seen().pages.len(), 1);
    assert_eq!(rig.store.seen().pages[0].execution, execution);
}

/// **A press recorded elsewhere takes the chain over**: once the canvas says
/// its start node runs another execution, this window starts nothing more.
#[test]
fn a_chain_pressed_again_elsewhere_stops_advancing_here() {
    let mut rig = Rig::new();
    let chain = straight(&rig);
    let root = rig.root();
    rig.view.run_chain(rig.canvas, root, chain.start);
    rig.settle();
    canvas::set_execution(
        &mut rig.store.board().ws,
        chain.start,
        "pressed-on-the-phone",
        "another-device",
    )
    .unwrap();
    // The next answer from the canvas — A's run link — shows the other press.
    let (_, document) = rig.created();
    rig.reads_as(
        document,
        read_as(RunStatus::Completed, vec![a_result()], None),
    );
    assert!(
        rig.runs.seen().reads.is_empty(),
        "A's result is the other run's to copy"
    );
    assert_eq!(rig.specs().len(), 1, "B is the other run's to start");
    assert_eq!(rig.state(chain.start), CanvasNodeState::Idle);
}

// ------------------------------------------------------------ the pieces

#[test]
fn an_agent_key_reads_back_as_written() {
    let custom = SessionProfileKey::Custom(SessionProfileId::mint());
    assert_eq!(
        parse_agent_key(&agent_key_text(&custom)),
        Some(custom.clone())
    );
    let SessionProfileKey::Custom(id) = &custom else {
        unreachable!()
    };
    assert_eq!(parse_agent_key(&id.to_string()), Some(custom));
    let built_in = koru_session_profiles::built_in_profiles()
        .into_iter()
        .map(|profile| profile.id)
        .find(|key| matches!(key, SessionProfileKey::BuiltIn(_)))
        .expect("a built-in");
    assert_eq!(parse_agent_key(&agent_key_text(&built_in)), Some(built_in));
    assert_eq!(parse_agent_key(""), None);
}

#[test]
fn markdown_keeps_its_lines_and_drops_the_blank_ones_around_them() {
    assert_eq!(
        markdown_lines("\n\n# Title\n\n- one\n- two\n\n"),
        vec!["# Title", "", "- one", "- two"]
    );
    assert!(markdown_lines(" \n\t\n").is_empty());
}

/// A page write's `Debug` carries ids and counts, never its text.
#[test]
fn a_page_write_never_prints_its_content() {
    let write = PageWrite {
        workspace: WorkspaceId::new(),
        node: NodeId::new(),
        execution: "e".into(),
        title: "Secret title".into(),
        markdown: "Secret body".into(),
        existing: None,
        before: "Secret before".into(),
        after: "Secret after".into(),
    };
    let printed = format!("{write:?}");
    assert!(!printed.contains("Secret"), "{printed}");
}
