// SPDX-License-Identifier: MPL-2.0

//! Creating one run, asserted over a gateway that lives in memory: no
//! client, no thread, no store.

use std::collections::BTreeSet;

use koru_delegate_context::{CaptureFailure, CaptureStage, ContextSource, Fault};
use koru_delegate_core::{
    ActorRef, BlockNodeId, DocumentId, DocumentVersionId, ModelName, RunContextItem, RunId,
    WorkspaceId, WorkspaceScope,
};

use super::*;

/// What `create_run` asked, and what it is answered.
struct Recorder {
    available: bool,
    pinned: Result<Vec<RunContextItem>, CaptureFailure>,
    busy: Option<RunId>,
    answer: Result<DocumentId, String>,
    asked_to_pin: Vec<Vec<ContextSource>>,
    created: Vec<CreateRunRequest>,
}

impl Recorder {
    fn answering(pinned: Vec<RunContextItem>) -> Self {
        Self {
            available: true,
            pinned: Ok(pinned),
            busy: None,
            answer: Ok(DocumentId::new()),
            asked_to_pin: Vec::new(),
            created: Vec::new(),
        }
    }
}

impl RunGateway for Recorder {
    fn available(&self) -> bool {
        self.available
    }

    fn pin(&mut self, sources: &[ContextSource]) -> Result<Vec<RunContextItem>, CaptureFailure> {
        self.asked_to_pin.push(sources.to_vec());
        self.pinned.clone()
    }

    fn busy_holder(&mut self, _workspace: WorkspaceId, _scope: &WorkspaceScope) -> Option<RunId> {
        self.busy
    }

    fn create(&mut self, body: &CreateRunRequest) -> Result<(RunId, DocumentId), String> {
        self.created.push(body.clone());
        self.answer.clone().map(|document| (body.run, document))
    }
}

fn an_agent() -> SessionProfileKey {
    koru_session_profiles::built_in_profiles()
        .into_iter()
        .find(|profile| {
            profile
                .provider
                .as_ref()
                .and_then(koru_session_profiles::managed_provider)
                .is_some()
        })
        .expect("a delegatable built-in")
        .id
}

/// A spec with every field set to something a default would not produce.
fn a_full_spec() -> (RunSpec, Vec<RunContextItem>) {
    let workspace = WorkspaceId::new();
    let other = WorkspaceId::new();
    let canvas = DocumentId::new();
    let card = BlockNodeId::from_uuid(uuid::Uuid::new_v4());
    let mut spec = RunSpec::new(
        workspace,
        koru_store_core::ProfileId::new(),
        "Summarise these cards",
        an_agent(),
    );
    spec.model = Some(ModelName::new("a-model").unwrap());
    spec.sources = vec![
        ContextSource::Block {
            workspace,
            document: canvas,
            node: card,
        },
        ContextSource::Document {
            workspace,
            document: canvas,
        },
    ];
    spec.grants = BTreeSet::from([workspace, other]);
    let version = DocumentVersionId::new();
    let pinned = vec![
        RunContextItem::Block {
            workspace,
            document: canvas,
            version,
            node: card,
        },
        RunContextItem::Document {
            workspace,
            document: canvas,
            version,
        },
    ];
    (spec, pinned)
}

/// **The request is exactly the spec**, with the context the pin returned —
/// every field, and the run id the press minted rather than a new one.
#[test]
fn create_run_builds_exactly_the_request_the_spec_describes() {
    let (spec, pinned) = a_full_spec();
    let mut gateway = Recorder::answering(pinned.clone());
    let document = DocumentId::new();
    gateway.answer = Ok(document);

    let answer = create_run(&mut gateway, spec.clone());

    assert_eq!(gateway.asked_to_pin, vec![spec.sources.clone()]);
    assert_eq!(
        gateway.created,
        vec![CreateRunRequest {
            run: spec.run,
            workspace: spec.workspace,
            requested_by: ActorRef::Person {
                profile: spec.profile,
            },
            objective: Objective::new("Summarise these cards").unwrap(),
            agent: spec.agent.clone(),
            requested_model: spec.model.clone(),
            scope: WorkspaceScope::Temporary,
            context: pinned,
            grants: spec.grants.clone(),
        }]
    );
    assert_eq!(
        answer,
        RunAnswer::Done {
            run: spec.run,
            document: koru_core::DocumentId::from_uuid(document.as_uuid()),
            busy: None,
        }
    );
}

/// A fresh spec is a fresh press: a new run id every time, a temporary
/// folder, and this workspace granted (D28's default).
#[test]
fn every_new_spec_mints_its_own_run_and_grants_its_workspace() {
    let workspace = WorkspaceId::new();
    let profile = koru_store_core::ProfileId::new();
    let first = RunSpec::new(workspace, profile, "a", an_agent());
    let second = RunSpec::new(workspace, profile, "a", an_agent());
    assert_ne!(first.run, second.run);
    assert_eq!(first.scope, WorkspaceScope::Temporary);
    assert_eq!(first.grants, BTreeSet::from([workspace]));
    assert_eq!(first.model, None);
    assert!(first.sources.is_empty());
    // The objective is content: never in a Debug.
    let printed = format!(
        "{:?}",
        RunSpec::new(workspace, profile, "secret plan", an_agent())
    );
    assert!(!printed.contains("secret plan"), "{printed}");
}

/// Nothing is created when the transport, the objective or the pin says no
/// — and each says which, in the codes the modal already shows.
#[test]
fn a_refusal_before_the_create_creates_nothing() {
    let (spec, pinned) = a_full_spec();

    let mut gateway = Recorder::answering(pinned.clone());
    gateway.available = false;
    assert_eq!(
        create_run(&mut gateway, spec.clone()),
        RunAnswer::Refused {
            code: "delegate_unavailable".into()
        }
    );
    assert!(gateway.created.is_empty() && gateway.asked_to_pin.is_empty());

    let mut gateway = Recorder::answering(pinned.clone());
    let mut blank = spec.clone();
    blank.objective = "   ".into();
    assert_eq!(
        create_run(&mut gateway, blank),
        RunAnswer::Refused {
            code: "objective_refused".into()
        }
    );
    assert!(gateway.created.is_empty() && gateway.asked_to_pin.is_empty());

    let mut gateway = Recorder::answering(pinned);
    gateway.pinned = Err(CaptureFailure {
        item: 1,
        stage: CaptureStage::Resolve,
        fault: Fault::DocumentUnavailable,
    });
    assert_eq!(
        create_run(&mut gateway, spec.clone()),
        RunAnswer::Refused {
            code: "context_document_unavailable".into()
        }
    );
    assert!(gateway.created.is_empty());

    let mut gateway = Recorder::answering(Vec::new());
    gateway.answer = Err("forbidden".into());
    assert_eq!(
        create_run(&mut gateway, spec),
        RunAnswer::Refused {
            code: "forbidden".into()
        }
    );
}

/// D34's warning travels with the answer and does not stop the create.
#[test]
fn a_busy_folder_warns_and_still_creates() {
    let (spec, pinned) = a_full_spec();
    let mut gateway = Recorder::answering(pinned);
    let holder = RunId::mint();
    gateway.busy = Some(holder);
    match create_run(&mut gateway, spec) {
        RunAnswer::Done { busy, .. } => assert_eq!(busy, Some(holder)),
        other => panic!("{other:?}"),
    }
    assert_eq!(gateway.created.len(), 1);
}

/// **The modal's Run once is the same path**: its request becomes a spec
/// field for field, under the id its press minted.
#[test]
fn the_modals_request_becomes_the_same_spec() {
    let (spec, _) = a_full_spec();
    let request = Request {
        token: super::super::modal::RequestToken::mint(),
        submission: super::super::modal::Submission::RunOnce { run: spec.run },
        workspace: spec.workspace,
        profile: spec.profile,
        objective: spec.objective.clone(),
        agent: spec.agent.clone(),
        requested_model: spec.model.clone(),
        scope: spec.scope.clone(),
        sources: spec.sources.clone(),
        grants: spec.grants.clone(),
        pill_anchor: None,
    };
    assert_eq!(RunSpec::of_request(request, spec.run), spec);
}

/// No client, no thread: the refusal comes back at once.
#[test]
fn starting_with_no_client_refuses_at_once() {
    let (spec, _) = a_full_spec();
    match start_delegation(None, spec, None) {
        Err(RunAnswer::Refused { code }) => assert_eq!(code, "delegate_unavailable"),
        Err(other) => panic!("{other:?}"),
        Ok(_) => panic!("a job with no client"),
    }
}

/// A worker that went away without answering is a refusal, so a node never
/// says *Starting…* for ever.
#[test]
fn a_worker_that_vanished_is_a_refusal() {
    let (send, receive) = std::sync::mpsc::channel();
    let job = RunJob::from_receiver(RunId::mint(), receive);
    assert_eq!(job.try_take(), None);
    drop(send);
    assert_eq!(
        job.try_take(),
        Some(RunAnswer::Refused {
            code: "worker_unavailable".into()
        })
    );
}
