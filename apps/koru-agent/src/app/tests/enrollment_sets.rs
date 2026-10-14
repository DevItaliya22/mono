// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_account_runtime::account::AccountControl;
use koru_account_runtime::enrollment::test_support::{Boundary, Effect, Fixture, owner, profile};
use koru_agent_wire::{AccountCommand, AccountOperationResult, AccountStage, AccountState};
use std::sync::{Arc, mpsc};

const WINDOW: ConnectionId = ConnectionId(71);
const WAIT: Duration = Duration::from_secs(10);

struct Harness {
    agent: Agent,
    fixture: Fixture,
    events: mpsc::Receiver<AgentEvent>,
    outbound: mpsc::Receiver<FromAgent>,
    results: HashMap<u64, (u64, AccountOperationResult)>,
    result_counts: HashMap<u64, usize>,
    request: u64,
    expected_visible: Option<(u64, u8)>,
    b_complete: bool,
}

impl Harness {
    fn new() -> Self {
        Self::with_boundary(Some(Boundary::Enroll))
    }

    fn with_boundary(boundary: Option<Boundary>) -> Self {
        let fixture = Fixture::new();
        if let Some(boundary) = boundary {
            fixture.arm(boundary, 1);
        }
        let (tx, events) = mpsc::channel();
        let mut control = AccountControl::start_with_directory(
            fixture.service(),
            fixture.vault.clone(),
            fixture.entry.clone(),
            Some(fixture.directory.clone()),
            Arc::new(koru_account_runtime::ports::SystemClock),
            false,
            move |event| {
                let _ = tx.send(crate::account::account_event(event));
            },
        );
        control.set_enrollment_control(fixture.enrollment.clone());
        let mut agent = agent();
        agent.set_account_control(control);
        let (connection, outbound) = Connection::for_test(WINDOW);
        agent.connections.push(connection);
        agent.account.as_ref().unwrap().set_foreground(true);
        Self {
            agent,
            fixture,
            events,
            outbound,
            results: HashMap::new(),
            result_counts: HashMap::new(),
            request: 0,
            expected_visible: None,
            b_complete: false,
        }
    }

    fn check_visible(&self, state: &AccountState) {
        if self.b_complete {
            assert_eq!(state.enrollments.len(), 1);
            assert_eq!(
                state.enrollments[0].state,
                koru_agent_wire::EnrollmentState::Complete
            );
        }
        if let Some((scope, account)) = self.expected_visible {
            assert_eq!(state.active_scope, scope);
            assert_eq!(
                state.active_account_id,
                Some(owner(account).account.to_string())
            );
            assert!(
                state
                    .enrollments
                    .iter()
                    .all(|row| row.profile_id == profile(account).to_string()),
                "late A enrollment must never appear in B's projection"
            );
        }
    }

    fn drain(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            self.agent.wake(event);
        }
        while let Ok(frame) = self.outbound.try_recv() {
            match frame {
                FromAgent::Push {
                    body:
                        Push::AccountResult {
                            owner_scope,
                            generation,
                            result,
                        },
                } => {
                    *self.result_counts.entry(generation).or_default() += 1;
                    self.results.insert(generation, (owner_scope, result));
                }
                FromAgent::Push {
                    body: Push::Account { state },
                } => self.check_visible(&state),
                FromAgent::Push {
                    body: Push::AccountChanged,
                } => {}
                other => panic!("unexpected adapter frame: {other:?}"),
            }
        }
    }

    fn response(&mut self, command: AccountCommand) -> Response {
        self.drain();
        self.request += 1;
        self.agent.said(
            WINDOW,
            self.request,
            Request::Account {
                command,
                expected_owner_scope: Some(
                    self.agent.account.as_ref().unwrap().state().active_scope,
                ),
            },
            Duration::ZERO,
        );
        match self.outbound.recv_timeout(WAIT).unwrap() {
            FromAgent::Response { id, body } => {
                assert_eq!(id, self.request);
                body
            }
            other => panic!("expected response, got {other:?}"),
        }
    }

    fn status(&mut self) -> AccountState {
        let Response::Account { state } = self.response(AccountCommand::Status) else {
            panic!("status response")
        };
        self.check_visible(&state);
        state
    }

    fn command(&mut self, command: AccountCommand) -> (u64, u64) {
        let Response::AccountQueued {
            generation,
            owner_scope,
        } = self.response(command)
        else {
            panic!("command admission")
        };
        (generation, owner_scope)
    }

    fn until(&mut self, mut ready: impl FnMut(&Self, &AccountState) -> bool) -> AccountState {
        let deadline = Instant::now() + WAIT;
        loop {
            let state = self.status();
            if ready(self, &state) {
                return state;
            }
            assert!(Instant::now() < deadline, "adapter state did not settle");
            std::thread::yield_now();
        }
    }

    fn result(&mut self, generation: u64) -> (u64, AccountOperationResult) {
        self.until(|h, _| h.results.contains_key(&generation));
        self.results.remove(&generation).unwrap()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.fixture.release();
    }
}

#[test]
fn held_a_selection_load_returns_one_original_scoped_result_after_b_switch() {
    let mut h = Harness::with_boundary(None);
    h.until(|h, state| {
        state
            .held_accounts
            .iter()
            .filter(|row| row.stage == AccountStage::SignedIn)
            .count()
            == 2
            && state.enrollments.len() == 1
            && state.enrollments[0].state == koru_agent_wire::EnrollmentState::Complete
            && h.fixture.targets(1).is_empty()
    });
    // Fence A's startup service work before arming the selection's target load.
    let (fence, _) = h.command(AccountCommand::ListProfiles);
    h.result(fence);
    let original_bytes = h.fixture.seeded_entries();
    h.fixture.arm(Boundary::Load, 1);
    let (selected, a_scope) = h.command(AccountCommand::SelectProfiles {
        profile_ids: vec![profile(1).to_string(), profile(3).to_string()],
    });
    h.until(|h, _| h.fixture.entered());
    assert!(!h.results.contains_key(&selected));

    let (switched, b_scope) = h.command(AccountCommand::SwitchAccount {
        account_id: owner(2).account.to_string(),
    });
    let (result_scope, result) = h.result(switched);
    assert_eq!(result_scope, b_scope);
    assert!(
        matches!(result, AccountOperationResult::AccountSelected { scope_id, account_id: Some(id) }
        if scope_id == b_scope && id == owner(2).account.to_string())
    );
    assert_ne!(a_scope, b_scope);
    h.expected_visible = Some((b_scope, 2));
    h.until(|h, state| {
        state.enrollments.len() == 1
            && state.enrollments[0].state == koru_agent_wire::EnrollmentState::Complete
            && h.fixture.targets(2).is_empty()
    });
    h.b_complete = true;
    assert_eq!(
        h.result_counts.get(&selected),
        None,
        "held A has no premature result"
    );
    let b_targets = h.fixture.targets(2);

    h.fixture.release();
    let (result_scope, result) = h.result(selected);
    assert_eq!(result_scope, a_scope);
    let AccountOperationResult::ProfilesSelected { profiles } = result else {
        panic!("original A selection must return its committed rows");
    };
    assert_eq!(
        profiles
            .iter()
            .map(|row| row.profile_id.clone())
            .collect::<Vec<_>>(),
        vec![profile(1).to_string(), profile(3).to_string()]
    );
    // A later actual B command fences delivery; the earlier A generation must
    // not receive a second completion or overwrite B's visible rows.
    let (fence, fence_scope) = h.command(AccountCommand::ListProfiles);
    assert_eq!(fence_scope, b_scope);
    h.result(fence);
    h.drain();
    assert_eq!(h.result_counts.get(&selected), Some(&1));
    assert_eq!(h.fixture.targets(2), b_targets);
    assert_eq!(h.fixture.seeded_entries(), original_bytes);
}

#[test]
fn real_agent_adapter_restores_singleton_and_keeps_late_a_enrollment_out_of_b() {
    let mut h = Harness::new();
    let original_bytes = h.fixture.seeded_entries();
    let initial = h.until(|h, state| {
        h.fixture.entered()
            && state
                .held_accounts
                .iter()
                .filter(|row| row.stage == AccountStage::SignedIn)
                .count()
                == 2
            && state.enrollments.len() == 1
    });
    assert_eq!(
        initial.active_account_id,
        Some(owner(1).account.to_string())
    );
    assert_eq!(initial.enrollments[0].profile_id, profile(1).to_string());
    assert_eq!(h.fixture.targets(1), [profile(1)].into());

    let (selected, a_scope) = h.command(AccountCommand::SelectProfiles {
        profile_ids: vec![profile(1).to_string(), profile(3).to_string()],
    });
    let (result_scope, result) = h.result(selected);
    assert_eq!(result_scope, a_scope);
    let AccountOperationResult::ProfilesSelected { profiles } = result else {
        panic!("whole selected result")
    };
    assert_eq!(
        profiles
            .iter()
            .map(|row| row.profile_id.clone())
            .collect::<Vec<_>>(),
        vec![profile(1).to_string(), profile(3).to_string()]
    );
    let unfinished_a = h.fixture.targets(1);
    assert!(unfinished_a.contains(&profile(1)));
    assert!(unfinished_a.is_subset(&[profile(1), profile(3)].into()));

    let (switched, b_scope) = h.command(AccountCommand::SwitchAccount {
        account_id: owner(2).account.to_string(),
    });
    let (result_scope, result) = h.result(switched);
    assert_eq!(result_scope, b_scope);
    assert!(
        matches!(result, AccountOperationResult::AccountSelected { scope_id, account_id: Some(id) }
        if scope_id == b_scope && id == owner(2).account.to_string())
    );
    assert_ne!(a_scope, b_scope);
    // Selection must complete before the blocked effect is released.
    h.expected_visible = Some((b_scope, 2));
    h.until(|h, state| {
        state.enrollments.len() == 1
            && state.enrollments[0].state == koru_agent_wire::EnrollmentState::Complete
            && h.fixture.targets(2).is_empty()
    });
    h.b_complete = true;
    assert!(h.fixture.targets(2).is_empty());
    for _ in 0..32 {
        h.status();
    }

    h.fixture.release();
    h.until(|h, _| {
        h.fixture.trace().iter().any(|effect| match effect {
            Effect::Commit {
                account: 1,
                profile: id,
                revision,
            } if *id == profile(1) => h
                .fixture
                .revisions()
                .iter()
                .any(|published| published >= revision),
            _ => false,
        })
    });
    // Every read and pushed status above and below is checked, including the
    // observations while A's committed revision reaches the runtime.
    for _ in 0..32 {
        h.status();
    }
    assert_eq!(h.fixture.seeded_entries(), original_bytes);
}
