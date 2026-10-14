// SPDX-License-Identifier: MPL-2.0

//! What a run pill says, asserted without a window, a store or a thread.

use std::collections::HashMap;

use koru_delegate_core::{Attention, AttentionSet, FailureStage, PriorAttempt, RunStatus};
use koru_device::DeviceId;

use super::*;

fn device(byte: u8) -> DeviceId {
    DeviceId::from_uuid(uuid::Uuid::from_bytes([byte; 16]))
}

fn state(status: RunStatus, on: Option<DeviceId>) -> RunPillState {
    RunPillState {
        run: koru_delegate_core::RunId::from_uuid(uuid::Uuid::from_u128(0x9a)),
        status,
        device: on,
        results: Vec::new(),
        latest_step: None,
        elapsed_ms: None,
        failure_reason: None,
    }
}

fn no_names(_: DeviceId) -> Option<String> {
    None
}

/// **Every state a run can be in has a word**, in every language.
///
/// The exhaustive match is the point: a status added later cannot reach a pill
/// without somebody choosing what it says, and eight locales are what
/// `koru-i18n`'s own suite then holds to.
#[test]
fn every_status_says_something_in_every_language() {
    let every = [
        RunStatus::Created,
        RunStatus::Starting,
        RunStatus::running(),
        RunStatus::Running {
            attention: AttentionSet::new([Attention::ApprovalRequired]),
            control: Default::default(),
        },
        RunStatus::Interrupted {
            prior_attempt: PriorAttempt::Ended,
        },
        RunStatus::Completed,
        RunStatus::Failed {
            stage: FailureStage::ProviderStart,
        },
        RunStatus::Cancelled,
        RunStatus::AwaitingReview,
        RunStatus::WaitingForParticipant,
    ];
    for status in every {
        for lang in koru_i18n::Lang::ALL {
            let said = state(status.clone(), None).caption(lang, None, no_names);
            assert!(!said.trim().is_empty(), "{status:?} in {lang:?}");
        }
    }
}

/// **A run that wants something says so, whatever it wants.** An approval, a
/// wait for input, a child session and a turn that finished having registered
/// nothing (D29) are four different questions with one answer at this size.
#[test]
fn any_attention_marker_reads_as_needing_a_person() {
    let needs_you = koru_i18n::t(koru_i18n::Lang::En, koru_i18n::Key::DelegateRunNeedsYou);
    for marker in [
        Attention::ApprovalRequired,
        Attention::WaitingForInput,
        Attention::ChildSessionDetected,
        Attention::FinishedNeedingAttention,
    ] {
        let status = RunStatus::Running {
            attention: AttentionSet::new([marker]),
            control: Default::default(),
        };
        assert_eq!(
            state(status, None).caption(koru_i18n::Lang::En, None, no_names),
            needs_you,
            "{marker:?}"
        );
    }
    // And a run with nothing outstanding is just working.
    assert_eq!(
        state(RunStatus::running(), None).caption(koru_i18n::Lang::En, None, no_names),
        koru_i18n::t(koru_i18n::Lang::En, koru_i18n::Key::DelegateRunWorking)
    );
}

/// **D16: the executing device is named where it is not running.**
///
/// One stored record, two devices, two different sentences — which is the
/// whole reason the pill stores an id and nothing else.
#[test]
fn the_executing_device_is_named_only_somewhere_else() {
    let executor = device(1);
    let names = |asked: DeviceId| (asked == executor).then(|| "Studio".to_owned());
    let running = state(RunStatus::running(), Some(executor));
    let working = koru_i18n::t(koru_i18n::Lang::En, koru_i18n::Key::DelegateRunWorking);

    // On the machine doing the work, saying so would be noise.
    assert_eq!(
        running.caption(koru_i18n::Lang::En, Some(executor), names),
        working
    );
    // Anywhere else, it is named.
    let elsewhere = running.caption(koru_i18n::Lang::En, Some(device(2)), names);
    assert!(elsewhere.contains(working), "{elsewhere}");
    assert!(elsewhere.contains("Studio"), "{elsewhere}");
    // A device this window has never been introduced to shows the state
    // alone rather than a uuid a person could not place.
    assert_eq!(
        running.caption(koru_i18n::Lang::En, Some(device(2)), no_names),
        working
    );
}

/// **A run that has finished is never asked about again.** Nothing can move a
/// terminal run, so a document full of old delegations costs one request each
/// and then nothing at all.
#[test]
fn a_terminal_run_drops_out_of_what_is_asked_for() {
    let finished = "a".to_owned();
    let working = "b".to_owned();
    let unknown = "c".to_owned();
    let wanted: HashMap<String, koru_store_core::ids::DocumentId> = [
        (finished.clone(), koru_store_core::ids::DocumentId::new()),
        (working.clone(), koru_store_core::ids::DocumentId::new()),
        (unknown.clone(), koru_store_core::ids::DocumentId::new()),
    ]
    .into_iter()
    .collect();
    let known: HashMap<String, RunPillState> = [
        (finished.clone(), state(RunStatus::Completed, None)),
        (working.clone(), state(RunStatus::running(), None)),
    ]
    .into_iter()
    .collect();

    let asking = unsettled(&wanted, &known);
    assert!(!asking.contains_key(&finished));
    assert!(asking.contains_key(&working));
    assert!(
        asking.contains_key(&unknown),
        "a run nothing is known about is asked about"
    );
}

/// **An idle surface contributes no clock**, and one with a run still moving
/// does. The rule the roadmap spells as "idle surfaces do no timer work".
#[test]
fn nothing_on_screen_means_no_deadline() {
    let pills = RunPills::default();
    assert!(pills.deadline(false).is_none());
    assert!(pills.deadline(true).is_some());
}

/// **A window with no client asks nothing and does not wedge.** Every test
/// container is such a window, and so is a machine whose gateway is down.
#[test]
fn no_client_means_no_request_and_no_state() {
    let mut pills = RunPills::default();
    let wanted: HashMap<String, koru_store_core::ids::DocumentId> =
        [("a".to_owned(), koru_store_core::ids::DocumentId::new())]
            .into_iter()
            .collect();
    assert!(!pills.refresh(
        std::time::Instant::now(),
        None,
        koru_store_core::ids::WorkspaceId::new(),
        &wanted,
        None,
    ));
    assert!(pills.take().is_none());
    // Nothing was asked, so nothing is due: the pill keeps its stored label.
    assert!(pills.deadline(false).is_none());
}

/// Nothing is asked for when nothing is on screen — the empty case is not a
/// request with an empty body.
#[test]
fn an_empty_screen_starts_no_worker() {
    let mut pills = RunPills::default();
    assert!(!pills.refresh(
        std::time::Instant::now(),
        None,
        koru_store_core::ids::WorkspaceId::new(),
        &HashMap::new(),
        None,
    ));
}

/// **A finished run's pill still resolves long after the session is gone.**
///
/// The roadmap's last verification row for P1.15, and the reason the caption
/// is read from the Run *document* rather than from anything that outlives a
/// session: there is no worker, no attempt, no provider process and no runs
/// transport in this test. A document written months ago, exported, and
/// re-opened by a reader that never saw the run start still says *Finished*
/// and still names the device that did the work.
#[test]
fn a_completed_run_still_reads_from_its_document_with_no_session_anywhere() {
    use koru_delegate_core::{
        ActorRef, DelegationRequest, Objective, RunHeader, RunId, WorkspaceScope,
    };
    use koru_delegate_document::RunDocument;

    let executor = device(7);
    let mut header = RunHeader::from_request(
        DelegationRequest {
            requested_model: None,
            run: RunId::mint(),
            requested_by: ActorRef::Person {
                profile: koru_store_core::ProfileId::new(),
            },
            objective: Objective::new("tidy the release notes").unwrap(),
            agent: koru_session_profiles::built_in_profiles()
                .into_iter()
                .find(|profile| {
                    profile
                        .provider
                        .as_ref()
                        .and_then(koru_session_profiles::managed_provider)
                        .is_some()
                })
                .expect("a delegatable built-in")
                .id,
            scope: WorkspaceScope::Temporary,
            context: Vec::new(),
            grants: Default::default(),
            executing_device: executor,
            origin: Default::default(),
        },
        1_700_000_000_000,
    );
    header.status = RunStatus::Completed;

    // Written, closed, and re-opened by a reader with nothing else in hand.
    let written = RunDocument::create(&header).unwrap();
    let bytes = written.export_snapshot().unwrap();
    drop(written);
    let months_later = RunDocument::from_snapshot(&bytes).unwrap();
    let read = months_later.header().unwrap();

    let state = RunPillState {
        run: koru_delegate_core::RunId::from_uuid(uuid::Uuid::from_u128(0x9b)),
        status: read.status.clone(),
        device: read.executing_device(),
        results: Vec::new(),
        latest_step: None,
        elapsed_ms: None,
        failure_reason: None,
    };
    assert!(state.settled(), "nothing can move it again");
    assert_eq!(
        state.caption(koru_i18n::Lang::En, Some(executor), no_names),
        koru_i18n::t(koru_i18n::Lang::En, koru_i18n::Key::DelegateRunFinished)
    );
    // And on the device that did not run it, named (D16).
    let elsewhere = state.caption(koru_i18n::Lang::En, Some(device(8)), |asked| {
        (asked == executor).then(|| "Studio".to_owned())
    });
    assert!(elsewhere.contains("Studio"), "{elsewhere}");
    // The objective never reaches the pill: it is in the paragraph the pill
    // sits in, and a caption is not where a prompt belongs.
    assert!(!elsewhere.contains("tidy the release notes"));
}

// ---------------------------------------------------- the crumb's own menu

/// A result as the run's header records it (D14, D27).
fn a_result(n: u128, title: &str, held_by: DeviceId) -> koru_delegate_core::ResultReference {
    koru_delegate_core::ResultReference {
        artifact: koru_delegate_core::ArtifactVersionId::from_uuid(uuid::Uuid::from_u128(n)),
        held_by,
        title: koru_delegate_core::ArtifactTitle::new(title).unwrap(),
        size_bytes: 4_096,
        registered_at_ms: 1_700_000_000_000,
    }
}

/// **What a run produced is already in the header the pill reads.** No second
/// read, and nothing that could disagree with the run's own record.
#[test]
fn the_results_ride_along_with_the_state_the_pill_is_already_reading() {
    use koru_delegate_core::{
        ActorRef, DelegationRequest, Objective, RunHeader, RunId, WorkspaceScope,
    };

    let executor = device(7);
    let mut header = RunHeader::from_request(
        DelegationRequest {
            requested_model: None,
            run: RunId::mint(),
            requested_by: ActorRef::Person {
                profile: koru_store_core::ProfileId::new(),
            },
            objective: Objective::new("tidy the release notes").unwrap(),
            agent: koru_session_profiles::built_in_profiles()
                .into_iter()
                .find(|profile| {
                    profile
                        .provider
                        .as_ref()
                        .and_then(koru_session_profiles::managed_provider)
                        .is_some()
                })
                .expect("a delegatable built-in")
                .id,
            scope: WorkspaceScope::Temporary,
            context: Vec::new(),
            grants: Default::default(),
            executing_device: executor,
            origin: Default::default(),
        },
        1_700_000_000_000,
    );
    header.artifacts = vec![
        a_result(0x90, "The quarterly summary", executor),
        a_result(0x91, "The regional detail", device(8)),
    ];

    let results = results_of(&header);
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].title.as_str(), "The quarterly summary");
    assert_eq!(results[1].held_by, device(8));

    // **A title is content** (standing requirement 7): it is drawn, and it is
    // not in a `Debug` anything could log.
    let printed = format!("{results:?}");
    assert!(!printed.contains("quarterly"), "{printed}");
    assert!(!printed.contains("regional"), "{printed}");
}

/// A run pill's state, as the window holds it once the worker has answered.
fn resolved(
    document: koru_core::DocumentId,
    results: Vec<koru_delegate_core::ResultReference>,
) -> (String, RunPillState) {
    (
        document.to_storage(),
        RunPillState {
            run: koru_delegate_core::RunId::from_uuid(uuid::Uuid::from_u128(0x9a)),
            status: RunStatus::Completed,
            device: Some(device(7)),
            results,
            latest_step: None,
            elapsed_ms: None,
            failure_reason: None,
        },
    )
}

fn pill_result(n: u128, title: &str, held_by: DeviceId) -> koru_delegate_core::ResultReference {
    a_result(n, title, held_by)
}

/// **What the maintainers asked for.** Right-clicking a run's crumb lists what it
/// produced, by name.
#[test]
fn a_run_pills_menu_lists_what_the_run_produced() {
    use crate::mainview::context_menu::{MenuAction, MenuSubject};

    let (mut v, _w) = crate::mainview::harness::container();
    let document = koru_core::DocumentId::new();
    let (key, state) = resolved(
        document,
        vec![
            pill_result(0x90, "The quarterly summary", device(7)),
            pill_result(0x91, "The regional detail", device(7)),
        ],
    );
    v.run_pill_states.insert(key, state);

    assert!(v.open_run_pill_menu(document, (120.0, 240.0)));
    let menu = v.menu.as_ref().expect("a menu");
    assert_eq!(menu.subject, MenuSubject::RunPill { document });
    assert_eq!(
        menu.items(),
        &[MenuAction::OpenRunResult(0), MenuAction::OpenRunResult(1)]
    );
    assert_eq!(
        menu.items()
            .iter()
            .map(|action| action.label().into_owned())
            .collect::<Vec<_>>(),
        ["The quarterly summary", "The regional detail"]
    );
}

/// **A run with nothing to show says so.** An empty panel reads as a bug
/// rather than as an answer, and the sentence is the run views' own.
#[test]
fn a_run_that_registered_nothing_says_so_rather_than_opening_an_empty_menu() {
    use crate::mainview::context_menu::MenuAction;

    let (mut v, _w) = crate::mainview::harness::container();
    let document = koru_core::DocumentId::new();
    let (key, state) = resolved(document, Vec::new());
    v.run_pill_states.insert(key, state);

    assert!(v.open_run_pill_menu(document, (120.0, 240.0)));
    let menu = v.menu.as_ref().expect("a menu");
    assert_eq!(menu.items(), &[MenuAction::RunHasNoResults]);
    assert_eq!(
        menu.items()[0].label(),
        koru_i18n::t(crate::format::lang(), koru_i18n::Key::RunResultNone)
    );
    assert_eq!(
        v.enabled_items(menu),
        vec![false],
        "a statement is not an offer"
    );
}

/// Picking one opens it as a tab — through the one primitive, so a result
/// reached from a crumb lands where a result reached from the run does.
#[test]
fn picking_a_result_opens_it_as_a_tab() {
    let (mut v, _w) = crate::mainview::harness::container();
    let document = koru_core::DocumentId::new();
    let (key, state) = resolved(
        document,
        vec![
            pill_result(0x90, "The quarterly summary", device(7)),
            pill_result(0x91, "The regional detail", device(7)),
        ],
    );
    v.run_pill_states.insert(key, state);

    assert!(v.open_run_pill_result(document, 1));
    let active = v.tabs().active();
    assert_eq!(
        v.tabs().get(active).expect("a tab").label,
        "The regional detail"
    );
    assert!(matches!(
        v.tabs().get(active).expect("a tab").view,
        crate::mainview::TabView::Result(_)
    ));

    // **One tab per version**, whichever surface opened it.
    let opened = v.tabs().len();
    v.select(0);
    assert!(v.open_run_pill_result(document, 1));
    assert_eq!(v.tabs().len(), opened);
    assert_eq!(v.tabs().active(), active);
}

/// **D27 survives the crumb.** A result whose bytes are on another device is
/// opened all the same, and the tab is told which device rather than being
/// left to ask a gateway that would refuse.
#[test]
fn a_result_held_on_another_device_names_it_rather_than_refusing() {
    let (mut v, _w) = crate::mainview::harness::container();
    let document = koru_core::DocumentId::new();
    let (key, state) = resolved(document, vec![pill_result(0x92, "Elsewhere", device(8))]);
    v.run_pill_states.insert(key, state);
    v.this_device = Some(device(7));
    v.device_names.insert(device(8), "Studio".to_owned());

    assert!(v.open_run_pill_result(document, 0));
    let active = v.tabs().active();
    assert!(matches!(
        v.tabs().get(active).expect("a tab").view,
        crate::mainview::TabView::Result(_)
    ));
}

/// A run this window has never resolved offers nothing rather than guessing.
#[test]
fn a_run_the_window_knows_nothing_about_opens_nothing() {
    let (mut v, _w) = crate::mainview::harness::container();
    let document = koru_core::DocumentId::new();
    assert!(!v.open_run_pill_result(document, 0));
    assert!(v.open_run_pill_menu(document, (10.0, 10.0)));
    assert_eq!(
        v.menu.as_ref().expect("a menu").items(),
        &[crate::mainview::context_menu::MenuAction::RunHasNoResults]
    );
}

// ------------------------------------------- what a canvas node reads too

/// A run header as the executing device commits it, with a status.
fn a_header(status: RunStatus) -> koru_delegate_core::RunHeader {
    use koru_delegate_core::{
        ActorRef, DelegationRequest, Objective, RunHeader, RunId, WorkspaceScope,
    };
    let mut header = RunHeader::from_request(
        DelegationRequest {
            requested_model: None,
            run: RunId::mint(),
            requested_by: ActorRef::Person {
                profile: koru_store_core::ProfileId::new(),
            },
            objective: Objective::new("tidy the release notes").unwrap(),
            agent: koru_session_profiles::built_in_profiles()
                .into_iter()
                .find(|profile| {
                    profile
                        .provider
                        .as_ref()
                        .and_then(koru_session_profiles::managed_provider)
                        .is_some()
                })
                .expect("a delegatable built-in")
                .id,
            scope: WorkspaceScope::Temporary,
            context: Vec::new(),
            grants: Default::default(),
            executing_device: device(7),
            origin: Default::default(),
        },
        1_700_000_000_000,
    );
    header.status = status;
    header
}

/// **The newest step rides along with the status**, read from the document
/// the worker already fetched — the newest *step*, past any usage fact.
#[test]
fn the_newest_step_and_the_elapsed_time_come_from_the_same_read() {
    use koru_delegate_core::{StepKind, TimelineEntry, TurnOriginator};
    use koru_delegate_document::RunDocument;

    let header = a_header(RunStatus::running());
    let run = RunDocument::create(&header).unwrap();
    let started = 1_700_000_001_000;
    run.append_step(&TimelineEntry::new(
        StepKind::Started,
        Default::default(),
        started,
    ))
    .unwrap();
    run.append_step(&TimelineEntry::new(
        StepKind::TurnStarted {
            originator: TurnOriginator::Orchestrator,
        },
        Default::default(),
        started + 10,
    ))
    .unwrap();
    let tools = StepKind::ToolsCompleted {
        calls: 3,
        failed: 0,
        failures: Default::default(),
    };
    run.append_step(&TimelineEntry::new(tools, Default::default(), started + 20))
        .unwrap();

    let state = RunPillState::of(&run, &run.header().unwrap(), started + 65_000);
    assert_eq!(state.latest_step, Some(tools));
    assert_eq!(state.run, header.run);
    assert_eq!(state.device, Some(device(7)));
    // A moving run's elapsed time is folded up to the moment it was read.
    assert!(state.elapsed_ms.is_some_and(|ms| ms >= 60_000), "{state:?}");

    // A run with no history yet has no step and no time.
    let fresh = RunDocument::create(&header).unwrap();
    let state = RunPillState::of(&fresh, &header, started);
    assert_eq!(state.latest_step, None);
    assert_eq!(state.elapsed_ms, None);
}

/// A failed run keeps its cause for whoever says why.
#[test]
fn a_failed_run_keeps_its_cause() {
    use koru_delegate_core::{FailureStage, RunFailureReason};
    use koru_delegate_document::RunDocument;

    let mut header = a_header(RunStatus::Failed {
        stage: FailureStage::ProviderStart,
    });
    header.failure_reason = Some(RunFailureReason::AuthenticationRequired);
    let run = RunDocument::create(&header).unwrap();
    let state = RunPillState::of(&run, &run.header().unwrap(), 0);
    assert_eq!(
        state.failure_reason,
        Some(RunFailureReason::AuthenticationRequired)
    );
}

/// **A nudge brings the next read forward** and is spent by it; with
/// nothing on screen it still asks nothing.
#[test]
fn a_nudge_makes_the_next_read_due_now() {
    let mut pills = RunPills {
        next_due: Some(std::time::Instant::now() + std::time::Duration::from_secs(60)),
        ..RunPills::default()
    };
    let later = pills.deadline(true).unwrap();
    assert!(later > std::time::Instant::now() + std::time::Duration::from_secs(30));
    pills.nudge();
    assert!(pills.deadline(true).unwrap() <= std::time::Instant::now());
    // Nothing unsettled on screen: no clock, nudged or not.
    assert!(pills.deadline(false).is_none());
    // And with no client nothing goes out, so the nudge stays owed.
    let wanted: HashMap<String, koru_store_core::ids::DocumentId> =
        [("a".to_owned(), koru_store_core::ids::DocumentId::new())]
            .into_iter()
            .collect();
    assert!(!pills.refresh(
        std::time::Instant::now(),
        None,
        koru_store_core::ids::WorkspaceId::new(),
        &wanted,
        None,
    ));
    assert!(pills.nudged);
}
