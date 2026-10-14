// SPDX-License-Identifier: MPL-2.0

//! A gone session cleans up (Q-E6 P2): the catalog's confirmed end closes a
//! session tab and releases the window's controller, while a person closing a
//! tab stays detach-only — and the managed reveal of the composer band whose
//! default is hidden (Q-E6 P1).

use std::collections::VecDeque;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use koru_agents_ui::PaneStatus;
use koru_session_backend::{BackendCommand, BackendEvent};
use koru_session_protocol::{
    AttentionState, CapabilitySet, ControllerState, ProcessState, SessionId, SessionMode,
    SessionSummary,
};
use koru_workspace_host::{Settings, TabRef, View};

use super::SessionBackend;
use crate::mainview::MainView;
use crate::mainview::harness::*;
use crate::mainview::tab::TabView;
use crate::session_tab::SessionView;

/// A backend over a plain channel: every command the container sends is
/// observable on the returned receiver, and nothing dials a host.
fn with_backend(v: &mut MainView) -> Receiver<BackendCommand> {
    let (sender, commands) = koru_session_backend::channel();
    v.session_backend = Some(SessionBackend {
        sender,
        events: Arc::new(Mutex::new(VecDeque::new())),
    });
    commands
}

fn drained(commands: &Receiver<BackendCommand>) -> Vec<BackendCommand> {
    let mut all = Vec::new();
    while let Ok(command) = commands.try_recv() {
        all.push(command);
    }
    all
}

/// One catalog row, shaped as the session host reports a raw shell.
fn summary(id: SessionId, process: ProcessState) -> SessionSummary {
    SessionSummary {
        id,
        parent_session_id: None,
        profile: koru_session_profiles::built_in_profiles()
            .into_iter()
            .next()
            .expect("a built-in profile")
            .id,
        display_name: "Shell".into(),
        working_directory: None,
        mode: SessionMode::RawPty,
        process,
        controller: ControllerState::Detached,
        managed: None,
        active_turn_id: None,
        attention: AttentionState::default(),
        pending_attachment_count: 0,
        capabilities: CapabilitySet::default(),
        project_id: None,
        handover: Default::default(),
        model: None,
        origin: Default::default(),
        created_at_ms: 1,
    }
}

/// The first session tab's id and view.
fn session_tab(v: &mut MainView) -> (u64, &mut SessionView) {
    v.tabs_mut()
        .iter_mut()
        .find_map(|tab| match &mut tab.view {
            TabView::Session(view) => Some((tab.id, view)),
            _ => None,
        })
        .expect("a session tab")
}

fn session_tab_count(v: &MainView) -> usize {
    v.tabs()
        .iter()
        .filter(|tab| matches!(tab.view, TabView::Session(_)))
        .count()
}

/// Open a session tab and bind it to `id`, as a completed launch leaves one.
fn open_bound(v: &mut MainView, id: SessionId) -> u64 {
    assert!(
        v.open_session_tab_for_profile(0, None),
        "a session tab opened"
    );
    let (tab_id, view) = session_tab(v);
    view.session = Some(id);
    view.surface.status = PaneStatus::Running;
    tab_id
}

/// **A gone session has no tab.** The catalog is the confirmed end — a row
/// that says the process exited and a row the host has already reaped close
/// the tab the same way, releasing the lease this window held. Nothing
/// terminates: the process ended itself, and the host reaps its own catalog.
#[test]
fn a_session_whose_process_ended_closes_its_tab_and_releases_the_controller() {
    let id = SessionId::mint();
    let verdicts = [
        Vec::new(),
        vec![summary(id, ProcessState::Exited { code: Some(0) })],
    ];
    for sessions in verdicts {
        let (mut v, _workspace) = container();
        let commands = with_backend(&mut v);
        let tab_id = open_bound(&mut v, id);
        v.session_attaching = None;
        v.session_attached = Some(tab_id);
        let _ = drained(&commands); // the launch, which this test is not about
        assert!(v.handle_session_event(BackendEvent::Sessions(sessions)));
        assert_eq!(session_tab_count(&v), 0, "the tab closed with its process");
        assert_eq!(v.session_attached, None, "no controller record lingers");
        let after = drained(&commands);
        assert!(
            after
                .iter()
                .any(|command| matches!(command, BackendCommand::Detach)),
            "the dead session's lease was released"
        );
        assert!(
            !after
                .iter()
                .any(|command| matches!(command, BackendCommand::Terminate)),
            "nothing terminates a process that already ended"
        );
    }
}

/// The three catalog states a tab's session can be in, told apart: running
/// (kept, status reconciled), and the host away entirely (kept, connecting) —
/// only the process's own end ever closes a tab.
#[test]
fn a_running_or_unreachable_session_keeps_its_tab() {
    let id = SessionId::mint();
    let (mut v, _workspace) = container();
    let _commands = with_backend(&mut v);
    open_bound(&mut v, id);

    // Alive but detached: the controller state is not a process state.
    v.handle_session_event(BackendEvent::Sessions(vec![summary(
        id,
        ProcessState::Running,
    )]));
    assert_eq!(session_tab_count(&v), 1);
    {
        let (_, view) = session_tab(&mut v);
        assert_eq!(view.surface.status, PaneStatus::Detached);
    }

    // The host away is not a catalog: no tab may be closed on silence.
    v.handle_session_event(BackendEvent::Disconnected);
    assert_eq!(session_tab_count(&v), 1, "unreachable is never loss");
    let (_, view) = session_tab(&mut v);
    assert_eq!(view.surface.status, PaneStatus::AgentUnavailable);
}

/// A person closing a session tab is the other direction, and must stay
/// **detach-only**: the session outlives its tab, so the settle pass releases
/// the controller and never terminates.
#[test]
fn closing_a_session_tab_detaches_and_never_terminates() {
    let id = SessionId::mint();
    let (mut v, _workspace) = container();
    let commands = with_backend(&mut v);
    let tab_id = open_bound(&mut v, id);
    v.session_attaching = None;
    v.session_attached = Some(tab_id);
    let _ = drained(&commands);

    let index = v
        .tabs()
        .iter()
        .position(|tab| tab.id == tab_id)
        .expect("the session tab's index");
    assert!(v.close_requested(index), "the ✕ closed the tab");
    v.maintain_sessions();

    let after = drained(&commands);
    assert!(
        after
            .iter()
            .any(|command| matches!(command, BackendCommand::Detach)),
        "closing a tab detaches"
    );
    assert!(
        !after
            .iter()
            .any(|command| matches!(command, BackendCommand::Terminate)),
        "and never terminates: the session stays alive without its tab"
    );
}

/// The composer band is hidden on a fresh session tab (Q-E6 P1), and a
/// managed attach reveals it — a managed agent's input *is* the composer.
#[test]
fn a_managed_attach_reveals_the_hidden_composer() {
    let (mut v, _workspace) = container();
    let _commands = with_backend(&mut v);
    assert!(v.open_session_tab_for_profile(0, None));
    {
        let (_, view) = session_tab(&mut v);
        assert!(!view.surface.composer_visible, "hidden by default");
    }
    let id = SessionId::mint();
    let provider: koru_session_protocol::ProviderKind = "codex".parse().expect("provider token");
    assert!(v.handle_session_event(BackendEvent::Attached {
        session_id: id,
        mode: SessionMode::Managed { provider },
        epoch: 1,
    }));
    let (_, view) = session_tab(&mut v);
    assert!(view.surface.session_is_managed);
    assert!(
        view.surface.composer_visible,
        "the managed attach brought the composer up"
    );
}

/// Restore's half of P2: every saved tab comes back connecting (startup
/// silence is never loss), and the first catalog keeps the living session's
/// tab while a confirmed-dead one simply has no tab — no resume prompt.
#[test]
fn restore_reattaches_the_living_and_a_confirmed_dead_session_has_no_tab() {
    let ws = koru_core::schema::WorkspaceDoc::create(koru_core::WorkspaceId::new(), "t")
        .expect("create");
    let workspace = koru_workspace::Workspace::new(ws);
    let profile = koru_session_profiles::built_in_profiles()
        .into_iter()
        .next()
        .expect("a built-in profile")
        .id;
    let alive = SessionId::mint();
    let dead = SessionId::mint();
    let mut settings = Settings::default();
    settings.tabs = vec![
        (TabRef::Welcome, 0.0),
        (
            TabRef::Session {
                session: alive,
                profile: profile.clone(),
            },
            0.0,
        ),
        (
            TabRef::Session {
                session: dead,
                profile,
            },
            0.0,
        ),
    ];

    let mut back = MainView::new();
    // The backend exists before restore, as it does in the live app once a
    // session has ever been used, so the rebuilt tabs bind to it.
    let commands = with_backend(&mut back);
    back.restore(&workspace, &settings);
    back.set_viewport(1200.0, 700.0);

    // Before any catalog: both tabs are back, connecting — never lost.
    assert_eq!(session_tab_count(&back), 2);
    for tab in back.tabs().iter() {
        if let TabView::Session(view) = &tab.view {
            assert_eq!(
                view.surface.status,
                PaneStatus::Starting,
                "connecting until the first catalog"
            );
        }
    }

    // The handshake's first catalog: one survived, one did not.
    assert!(
        back.handle_session_event(BackendEvent::Connected(vec![summary(
            alive,
            ProcessState::Running,
        )]))
    );
    assert_eq!(session_tab_count(&back), 1, "the dead session has no tab");
    let (_, view) = session_tab(&mut back);
    assert_eq!(view.session, Some(alive), "the living one kept its tab");
    let _ = drained(&commands);
}

/// The handshake said restart: every session this window knew died with the
/// old host, so their tabs close. A tab still waiting on a launch holds no
/// session id and is not loss — its answer arrives as its own event.
#[test]
fn a_host_restart_closes_the_tabs_whose_sessions_died_with_it() {
    let id = SessionId::mint();
    let (mut v, _workspace) = container();
    let commands = with_backend(&mut v);
    open_bound(&mut v, id);
    assert!(
        v.open_session_tab_for_profile(0, None),
        "a second, still launching"
    );
    let _ = drained(&commands);

    assert!(v.handle_session_event(BackendEvent::SessionsLost {
        sessions: Vec::new()
    }));
    assert_eq!(session_tab_count(&v), 1, "only the launching tab stays");
    let (_, view) = session_tab(&mut v);
    assert_eq!(view.session, None, "and it never held a session to lose");
}

/// A deliberate terminate, once the backend confirms it, is a gone session
/// like any other: the tab closes rather than lingering as a placeholder.
#[test]
fn a_confirmed_terminate_closes_the_tab() {
    let id = SessionId::mint();
    let (mut v, _workspace) = container();
    let _commands = with_backend(&mut v);
    open_bound(&mut v, id);
    assert!(v.handle_session_event(BackendEvent::Terminated { session_id: id }));
    assert_eq!(session_tab_count(&v), 0);
}

/// **The tray's Agents ask lands here** (Q-E8 P9). `shows_session` is the
/// routing fact the shell asks before choosing a pane; `open_session_tab` on
/// a session already shown focuses that tab rather than opening a second
/// claim on it; and New opens the `+` chooser — which agent to start stays
/// the person's choice.
#[test]
fn the_agents_ask_focuses_an_existing_tab_and_new_opens_the_chooser() {
    let id = SessionId::mint();
    let (mut v, _workspace) = container();
    let _commands = with_backend(&mut v);
    assert!(!View::shows_session(&v, id), "nothing shown yet");

    open_bound(&mut v, id);
    assert!(View::shows_session(&v, id));
    assert!(View::open_session_tab(&mut v, id), "focus is a change");
    assert_eq!(
        session_tab_count(&v),
        1,
        "focused, never duplicated: one window holds one controller"
    );

    assert!(View::open_new_session_chooser(&mut v), "the + menu opened");
}

/// A session this window does not show opens as a tab bound to that id — an
/// attach through the settle pass, never a launch.
#[test]
fn the_agents_ask_opens_a_tab_for_an_unshown_session() {
    let id = SessionId::mint();
    let (mut v, _workspace) = container();
    let _commands = with_backend(&mut v);
    assert!(View::open_session_tab(&mut v, id));
    assert_eq!(session_tab_count(&v), 1);
    let (_, view) = session_tab(&mut v);
    assert_eq!(view.session, Some(id), "bound by id, not launched");
}

#[test]
fn cold_managed_menu_uses_readiness_without_creating_a_session() {
    let (mut view, _workspace) = container();
    let commands = with_backend(&mut view);
    view.ensure_provider_readiness();
    assert!(view.provider_readiness.is_none());
    assert!(view.handle_session_event(BackendEvent::Providers(vec![])));
    assert_eq!(view.provider_readiness, Some(vec![]));
    assert!(!view.handle_session_event(BackendEvent::Providers(vec![])));
    let row = koru_session_protocol::ProviderReadiness {
        kind: "fixture".parse().unwrap(),
        availability: koru_session_protocol::ExecutableAvailability::SignedOut {
            code: koru_session_protocol::ErrorCode::ProviderUnavailable,
        },
        capabilities: Default::default(),
        failure: None,
    };
    assert!(view.handle_session_event(BackendEvent::Providers(vec![row.clone()])));
    assert_eq!(view.provider_readiness, Some(vec![row.clone()]));
    assert!(!view.handle_session_event(BackendEvent::Providers(vec![row])));
    assert!(drained(&commands).is_empty());
    assert!(view.panes.iter_groups().all(|(_, group)| {
        group
            .tabs()
            .iter()
            .all(|tab| !matches!(tab.view, TabView::Session(_)))
    }));
}

#[test]
fn open_delegate_picker_accepts_readiness_updates_and_retries_without_launching() {
    use crate::mainview::delegate::{DelegationOrigin, Hit, Outcome};
    use koru_session_protocol::{
        ExecutableAvailability as Availability, ProviderReadiness, capability,
    };
    let (mut view, _workspace) = container();
    let commands = with_backend(&mut view);
    let kind = koru_session_profiles::MANAGED_PROVIDER_REGISTRATIONS[0]
        .kind
        .parse()
        .unwrap();
    let row = ProviderReadiness {
        kind,
        availability: Availability::Missing,
        capabilities: Default::default(),
        failure: None,
    };
    assert!(view.begin_delegation(DelegationOrigin::PlusMenu));
    view.delegation
        .as_mut()
        .unwrap()
        .objective
        .set_text(&mut view.engine, "Keep this draft");
    assert!(view.handle_session_event(BackendEvent::Providers(vec![row.clone()])));
    let modal = view.delegation.as_mut().unwrap();
    assert!(!modal.world.agents.is_empty());
    assert!(modal.world.agents.iter().all(|row| !row.choosable()));
    let agent_index = modal
        .world
        .agents
        .iter()
        .position(|agent| agent.provider == row.kind)
        .unwrap();
    modal.activate(Hit::Agent(agent_index));
    assert_eq!(
        modal.activate(Hit::AgentRefresh(agent_index)),
        Outcome::RefreshProvider(row.kind.clone())
    );
    // Dispatch through the same nonblocking backend command used by the modal.
    view.refresh_managed_provider(row.kind.clone());
    assert!(
        matches!(commands.try_recv().unwrap(), BackendCommand::RefreshProvider { kind } if kind == row.kind)
    );
    assert!(drained(&commands).is_empty());
    assert!(view.handle_session_event(BackendEvent::Providers(vec![row.clone()])));
    assert!(
        matches!(
            view.delegation.as_ref().unwrap().world.agents[agent_index]
                .readiness
                .availability,
            Availability::Missing
        ),
        "an identical retry result settles local Checking"
    );
    let ready = ProviderReadiness {
        availability: Availability::Available { version: None },
        capabilities: CapabilitySet::new([capability::MANAGED_DELEGATION.parse().unwrap()]),
        ..row
    };
    assert!(view.handle_session_event(BackendEvent::Providers(vec![ready])));
    let modal = view.delegation.as_ref().unwrap();
    assert_eq!(modal.objective.value(), "Keep this draft");
    assert!(modal.ready());
    assert!(view.handle_session_event(BackendEvent::Providers(vec![])));
    let modal = view.delegation.as_ref().unwrap();
    assert!(modal.world.agents.iter().all(|row| matches!(
        row.readiness.availability,
        Availability::Incompatible { .. }
    )));
    assert!(!modal.ready());
    assert_eq!(session_tab_count(&view), 0);
}
