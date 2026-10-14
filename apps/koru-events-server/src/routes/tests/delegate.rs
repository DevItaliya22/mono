// SPDX-License-Identifier: MPL-2.0

//! **A delegated run through the real activity route with its real
//! parameters** (ROADMAP §10, P1.8's verification): the rows the builders
//! make are published through `POST /v1/events`, and
//! `GET /v1/events/activity?kind=document&id=<run document id>&after=&limit=`
//! answers exactly that run's rows, in `seq` order, with `retention_days`
//! — and never another run's, even in the same workspace.

use koru_delegate_core::{
    AttemptGeneration, CancelReason, DocumentId, FailureStage, RunAttemptId, RunId, TurnIdentity,
    TurnOriginator, WorkspaceId,
};
use koru_delegate_events::{
    RunCancelled, RunChain, RunCompleted, RunCreated, RunFailed, RunRef, RunStarted, Stamp,
    ToolsCompleted, TurnCompleted, TurnStarted, TurnUsage,
};
use koru_events::{ActorKind, AttrValue};

use super::*;

const T: i64 = 1_756_000_000_000;

/// A deterministic v4 UUID, so the strict ids accept it.
fn v4(n: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 | n)
}

fn stamp(n: u128) -> Stamp {
    Stamp {
        event_id: EventId(v4(0xee00 + n)),
        occurred_at_ms: T + n as i64,
    }
}

/// Run `n`: its own run id and its own Run document, in the one shared
/// workspace — so the only thing that tells two runs apart is the subject.
fn run(n: u128) -> RunRef {
    RunRef {
        run: RunId::from_uuid(v4(0x100 + n)),
        document: DocumentId::from_uuid(v4(0x200 + n)),
        workspace: WorkspaceId::from_uuid(v4(0x300)),
    }
}

fn attempt(n: u128) -> RunAttemptId {
    RunAttemptId::from_uuid(v4(0x400 + n))
}

fn turn(n: u128) -> TurnIdentity {
    TurnIdentity {
        attempt: attempt(n),
        turn: v4(0x500 + n).hyphenated().to_string().parse().unwrap(),
        originator: TurnOriginator::Orchestrator,
    }
}

fn created(n: u128) -> (Envelope, RunChain) {
    RunChain::created(
        &EmitContext::user_session(),
        run(n),
        stamp(n * 100),
        RunCreated {
            requested_by: koru_delegate_core::ActorRef::Person {
                profile: koru_delegate_core::ProfileId::from_uuid(v4(0x600)),
            },
            agent: koru_delegate_events::DelegatedAgent::Codex,
            scope: koru_delegate_events::ScopeKind::ProjectFolder,
            context_items: 2,
        },
    )
}

/// The route's real parameters: `kind=document`, `id=<the Run document>`.
fn activity_for(document: DocumentId, after: u64, limit: u32) -> String {
    format!(
        "/v1/events/activity?kind={}&id={}&after={after}&limit={limit}",
        koru_events::SubjectKind::Document.token(),
        document.to_storage(),
    )
}

async fn page(handle: &EventsHandle, document: DocumentId, after: u64, limit: u32) -> ActivityPage {
    body_json(
        handle
            .request(get(&activity_for(document, after, limit)))
            .await,
    )
    .await
}

fn names(page: &ActivityPage) -> Vec<&str> {
    page.entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect()
}

/// Run 1's whole lifecycle interleaved with run 2's, both in one workspace.
/// The route answers each run with exactly its own rows, oldest first by
/// `seq`, every row correlated to that run's `created`, with the retention
/// window on the page.
#[tokio::test]
async fn a_runs_lifecycle_is_read_back_by_the_real_route_and_isolated_from_another_run() {
    let (_dir, handle) = handle();
    let (created_1, first) = created(1);
    let (created_2, second) = created(2);
    let generation = AttemptGeneration::FIRST;

    let interleaved = vec![
        created_1.clone(),
        created_2.clone(),
        first.started(
            stamp(101),
            RunStarted {
                attempt: attempt(1),
                generation,
                session: v4(0x700).hyphenated().to_string().parse().unwrap(),
            },
        ),
        second.started(
            stamp(201),
            RunStarted {
                attempt: attempt(2),
                generation,
                session: v4(0x701).hyphenated().to_string().parse().unwrap(),
            },
        ),
        first.turn_started(stamp(102), TurnStarted { turn: turn(1) }),
        second.failed(
            stamp(202),
            RunFailed {
                attempt: Some(attempt(2)),
                generation,
                stage: FailureStage::Turn,
            },
        ),
        first.tools_completed(
            stamp(103),
            ToolsCompleted {
                attempt: attempt(1),
                turn: turn(1).turn,
                calls: 4,
                failed: 0,
            },
        ),
        first.turn_completed(
            stamp(104),
            TurnCompleted {
                turn: turn(1),
                artifacts: 1,
            },
        ),
        first.completed(
            stamp(105),
            RunCompleted {
                attempt: attempt(1),
                generation,
                artifacts: 1,
            },
        ),
    ];
    let receipt = publish(&handle, interleaved).await;
    assert_eq!((receipt.accepted, receipt.duplicates), (9, 0));
    assert!(receipt.rejected.is_empty(), "{:?}", receipt.rejected);

    let one = page(&handle, run(1).document, 0, 50).await;
    assert_eq!(
        names(&one),
        [
            "delegate.run.created",
            "delegate.run.started",
            "delegate.turn.started",
            "delegate.tool.completed",
            "delegate.turn.completed",
            "delegate.run.completed",
        ]
    );
    assert_eq!(one.retention_days, crate::journal::RETENTION_DAYS);
    assert!(
        one.entries.windows(2).all(|pair| pair[0].seq < pair[1].seq),
        "oldest first, by seq"
    );
    assert_eq!(one.next_after, one.entries.last().unwrap().seq);
    let run_1 = AttrValue::Id(run(1).run.as_uuid());
    for entry in &one.entries {
        assert_eq!(entry.correlation_id, created_1.event_id, "{}", entry.name);
        assert_eq!(entry.attrs.get("run"), Some(&run_1), "{}", entry.name);
    }
    // The root reports its own id as the group key, resolved for the reader.
    assert_eq!(one.entries[0].correlation_id, created_1.event_id);
    // Who acted: the person asked; the orchestrator did the rest.
    assert_eq!(one.entries[0].actor, ActorKind::User);
    assert_eq!(one.entries[1].actor, ActorKind::Agent);
    assert_eq!(one.entries[1].actor_token.as_deref(), Some("delegate"));

    let two = page(&handle, run(2).document, 0, 50).await;
    assert_eq!(
        names(&two),
        [
            "delegate.run.created",
            "delegate.run.started",
            "delegate.run.failed",
        ]
    );
    for entry in &two.entries {
        assert_eq!(entry.correlation_id, created_2.event_id);
        assert_ne!(entry.attrs.get("run"), Some(&run_1), "subject isolation");
    }

    // Paging with the real cursor walks run 1 once, nothing repeated or
    // skipped, and run 2 never appears in it.
    let mut walked = Vec::new();
    let mut after = 0;
    loop {
        let page = page(&handle, run(1).document, after, 2).await;
        if page.entries.is_empty() {
            assert_eq!(page.next_after, after);
            break;
        }
        assert!(page.entries.len() <= 2);
        walked.extend(page.entries.iter().map(|entry| entry.seq));
        after = page.next_after;
    }
    assert_eq!(
        walked,
        one.entries
            .iter()
            .map(|entry| entry.seq)
            .collect::<Vec<_>>()
    );
}

/// §7.1 and §17.2 through the route: a re-publication under the stored
/// stamp is a duplicate and one row; an amendment under a fresh stamp is a
/// second row with the same fact identity and a higher revision.
#[tokio::test]
async fn a_retry_is_one_row_and_an_amendment_is_two() {
    let (_dir, handle) = handle();
    let (created_3, chain) = created(3);
    let usage = |input: u64, revision: u32| TurnUsage {
        fact: koru_delegate_core::UsageFact {
            identity: koru_delegate_core::UsageFactId::from_uuid(v4(0x800)),
            attempt: attempt(3),
            turn: turn(3).turn,
            revision: koru_delegate_core::AmendmentRevision(revision),
            input: Some(input),
            output: Some(10),
            cached_input: None,
            model: koru_delegate_core::ModelRef::from_uuid(v4(0x900)),
            completeness: koru_delegate_core::Completeness::Complete,
        },
        cached_relationship: koru_delegate_events::CachedInputRelationship::Included,
    };
    let first = chain.turn_usage(stamp(301), &usage(100, 0));
    publish(&handle, vec![created_3, first.clone()]).await;

    // The pending list re-publishes the same envelope.
    let retried = publish(&handle, vec![first]).await;
    assert_eq!((retried.accepted, retried.duplicates), (0, 1));
    let page_1 = page(&handle, run(3).document, 0, 50).await;
    assert_eq!(
        names(&page_1),
        ["delegate.run.created", "delegate.turn.usage"]
    );

    // A correction: same fact, higher revision, fresh identity.
    let amended = chain.turn_usage(Stamp::fresh(T + 5_000), &usage(140, 1));
    let receipt = publish(&handle, vec![amended]).await;
    assert_eq!((receipt.accepted, receipt.duplicates), (1, 0));
    let page_2 = page(&handle, run(3).document, 0, 50).await;
    assert_eq!(
        names(&page_2),
        [
            "delegate.run.created",
            "delegate.turn.usage",
            "delegate.turn.usage"
        ]
    );
    let revisions: Vec<_> = page_2.entries[1..]
        .iter()
        .map(|entry| entry.attrs.get("revision").cloned())
        .collect();
    assert_eq!(
        revisions,
        [Some(AttrValue::Count(0)), Some(AttrValue::Count(1))]
    );
    assert_eq!(
        page_2.entries[1].attrs.get("usage"),
        page_2.entries[2].attrs.get("usage"),
        "one fact identity"
    );
    assert_eq!(
        page_2.entries[2].attrs.get("input"),
        Some(&AttrValue::Count(140))
    );
}

/// Retention through the route: a run's rows past the domain window are
/// pruned by the write path, the page says how far back it could reach, and
/// a row published afterwards takes a sequence no pruned row ever had.
#[tokio::test]
async fn a_runs_rows_past_retention_are_gone_and_a_later_row_never_reuses_a_seq() {
    let (_dir, handle) = handle();
    let (created_4, chain) = created(4);
    let receipt = publish(
        &handle,
        vec![
            created_4,
            chain.cancelled(
                stamp(401),
                RunCancelled {
                    attempt: None,
                    generation: AttemptGeneration::FIRST,
                    reason: CancelReason::Run,
                },
            ),
        ],
    )
    .await;
    let high_water = receipt.last_seq;
    let before = page(&handle, run(4).document, 0, 50).await;
    assert_eq!(before.entries.len(), 2);

    let later = crate::state::now_ms() + (crate::journal::RETENTION_DAYS as i64 + 1) * 86_400_000;
    handle
        .state
        .with_journal(|journal| journal.prune_now(later), || ())
        .unwrap();

    let after = page(&handle, run(4).document, 0, 50).await;
    assert!(after.entries.is_empty(), "nothing outlives the window");
    assert_eq!(after.next_after, 0);
    assert_eq!(after.retention_days, crate::journal::RETENTION_DAYS);

    // A late rating lands after the prune, on a sequence past the high-water
    // mark — never one a pruned row held under a live subscriber's cursor.
    publish(
        &handle,
        vec![chain.run_rated(
            stamp(402),
            koru_delegate_events::RunRated {
                rated_by: koru_delegate_core::ActorRef::Person {
                    profile: koru_delegate_core::ProfileId::from_uuid(v4(0x600)),
                },
                rating: koru_delegate_core::Rating::NotGood,
                has_note: false,
            },
        )],
    )
    .await;
    let rated = page(&handle, run(4).document, 0, 50).await;
    assert_eq!(names(&rated), ["delegate.run.rated"]);
    assert!(rated.entries[0].seq > high_water);
}
