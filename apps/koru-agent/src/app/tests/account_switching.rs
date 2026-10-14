// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_account_runtime::account::directory::{AccountDirectory, MemoryAccountDirectory};
use koru_account_runtime::account::{
    AccountControl, AccountService, Cancel, CredentialChange, ServiceError, ServiceOutcome,
    ServiceProgress,
};
use koru_account_runtime::identity::AccountOwner;
use koru_agent_wire::{AccountCommand, AccountOperationResult, AccountStage, AccountState};
use koru_oauth_credentials::{
    CredentialContext, CredentialEntry, CredentialStore, Error as VaultError, SecretString,
    SessionCredential, SessionCredentialStore,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};

const WAIT: Duration = Duration::from_secs(10);
const A_WINDOW: ConnectionId = ConnectionId(7);
const B_WINDOW: ConnectionId = ConnectionId(8);

fn owner(n: u8) -> AccountOwner {
    AccountOwner {
        account: format!("10000000-0000-4000-8000-{n:012}").parse().unwrap(),
        auth_identity: format!("20000000-0000-4000-8000-{n:012}").parse().unwrap(),
    }
}
fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/desktop-switching-fixture")
}
fn entry() -> CredentialEntry {
    CredentialContext::new(
        "test",
        &root(),
        koru_device::DeviceId::parse("30000000-0000-4000-8000-000000000003").unwrap(),
    )
    .unwrap()
    .entry("https://account.invalid")
    .unwrap()
}
fn credential(account: u8, revision: usize) -> SessionCredential {
    SessionCredential::new(
        format!("session-{account}"),
        format!("refresh-{account}-{revision}"),
    )
    .unwrap()
}
fn account_of(value: &SessionCredential) -> u8 {
    value
        .session_id
        .strip_prefix("session-")
        .unwrap()
        .parse()
        .unwrap()
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Boundary {
    Service,
    Vault,
}
#[derive(Default)]
struct GateState {
    armed: Option<Boundary>,
    entered: bool,
    released: bool,
}
#[derive(Default)]
struct Gate {
    state: Mutex<GateState>,
    changed: Condvar,
}
impl Gate {
    fn arm(&self, boundary: Boundary) {
        *self.state.lock().unwrap() = GateState {
            armed: Some(boundary),
            entered: false,
            released: false,
        };
    }
    fn cross(&self, boundary: Boundary, account: u8) {
        let mut state = self.state.lock().unwrap();
        if account != 1 || state.armed != Some(boundary) {
            return;
        }
        state.armed = None;
        state.entered = true;
        self.changed.notify_all();
        while !state.released {
            state = self.changed.wait(state).unwrap();
        }
    }
    fn entered(&self) {
        let (state, _) = self
            .changed
            .wait_timeout_while(self.state.lock().unwrap(), WAIT, |state| !state.entered)
            .unwrap();
        assert!(state.entered, "A must reach its controlled effect boundary");
    }
    fn release(&self) {
        self.state.lock().unwrap().released = true;
        self.changed.notify_all();
    }
}
struct Vault {
    memory: SessionCredentialStore,
    gate: Arc<Gate>,
}
impl CredentialStore for Vault {
    fn load(&self, entry: &CredentialEntry) -> Result<Option<SecretString>, VaultError> {
        self.memory.load(entry)
    }
    fn store(&self, entry: &CredentialEntry, value: SecretString) -> Result<(), VaultError> {
        self.gate.cross(
            Boundary::Vault,
            account_of(&SessionCredential::decode(&value)?),
        );
        self.memory.store(entry, value)
    }
    fn clear(&self, entry: &CredentialEntry) -> Result<(), VaultError> {
        self.memory.clear(entry)
    }
}
struct FakeService {
    gate: Arc<Gate>,
    rotations: Arc<AtomicUsize>,
}
impl AccountService for FakeService {
    fn fork(&self) -> Option<Box<dyn AccountService>> {
        Some(Box::new(Self {
            gate: self.gate.clone(),
            rotations: self.rotations.clone(),
        }))
    }
    fn inspect_credential(
        &mut self,
        value: &SessionCredential,
    ) -> Result<AccountOwner, ServiceError> {
        Ok(owner(account_of(value)))
    }
    fn execute(
        &mut self,
        generation: u64,
        command: AccountCommand,
        restored: Option<&SessionCredential>,
        _: &Cancel,
        _: &mut dyn FnMut(ServiceProgress),
    ) -> Result<ServiceOutcome, ServiceError> {
        let account = account_of(restored.expect("held service uses its own restored credential"));
        let mut state = AccountState {
            generation,
            stage: AccountStage::SignedIn,
            capability: true,
            profile_count: usize::from(account) * 11,
            ..Default::default()
        };
        let (credential, result) = match command {
            AccountCommand::ListProfiles => {
                self.gate.cross(Boundary::Service, account);
                (
                    CredentialChange::Keep,
                    AccountOperationResult::Profiles {
                        profiles: vec![koru_agent_wire::AccountProfileRow {
                            koru_identity_id: owner(account).account.to_string(),
                            profile_id: format!("profile-{account}"),
                            name: None,
                            locale: None,
                            auth_identities: vec![],
                        }],
                    },
                )
            }
            AccountCommand::Retry => (
                CredentialChange::Replace(credential(
                    account,
                    self.rotations.fetch_add(1, Ordering::SeqCst) + 1,
                )),
                AccountOperationResult::SignedIn,
            ),
            AccountCommand::Logout => {
                state.stage = AccountStage::SignedOut;
                (CredentialChange::Remove, AccountOperationResult::SignedOut)
            }
            // The Agent's own Cloud sync reconcile. The real handler needs a
            // host port; here only the routing is under test.
            AccountCommand::InspectCloudSync { request_id } => (
                CredentialChange::Keep,
                AccountOperationResult::CloudSync {
                    request_id,
                    profiles: Vec::new(),
                },
            ),
            _ => panic!("unexpected desktop fixture command"),
        };
        Ok(ServiceOutcome {
            state,
            credential,
            result,
            follow_up: None,
        })
    }
}

struct Harness {
    agent: Agent,
    events: mpsc::Receiver<AgentEvent>,
    outbound: Vec<(ConnectionId, mpsc::Receiver<FromAgent>)>,
    results: HashMap<(u64, u64), (u64, AccountOperationResult)>,
    states: HashMap<u64, AccountState>,
    gate: Arc<Gate>,
    vault: Arc<Vault>,
    request: u64,
}
impl Harness {
    fn new() -> Self {
        let gate = Arc::new(Gate::default());
        let vault = Arc::new(Vault {
            memory: SessionCredentialStore::default(),
            gate: gate.clone(),
        });
        let directory = Arc::new(MemoryAccountDirectory::new());
        let entry = entry();
        directory.retire_legacy(&entry.namespace()).unwrap();
        for account in [1, 2] {
            let record = directory
                .pending(&entry.namespace(), owner(account))
                .unwrap();
            directory
                .ready(
                    &entry.namespace(),
                    &record,
                    record.owner,
                    account == 1,
                    false,
                )
                .unwrap();
            vault
                .store(
                    &entry
                        .for_account(&owner(account).account.to_string())
                        .unwrap(),
                    credential(account, 0).encode().unwrap(),
                )
                .unwrap();
        }
        let (tx, events) = mpsc::channel();
        let control = AccountControl::start_with_directory(
            FakeService {
                gate: gate.clone(),
                rotations: Arc::new(AtomicUsize::new(0)),
            },
            vault.clone(),
            entry,
            Some(directory),
            Arc::new(koru_account_runtime::ports::SystemClock),
            false,
            move |event| {
                let _ = tx.send(crate::account::account_event(event));
            },
        );
        let mut agent = Agent::new(
            Paths::for_root(root()),
            "injected-device".into(),
            "injected-instance".into(),
        );
        agent.set_account_control(control);
        let mut outbound = Vec::new();
        for id in [A_WINDOW, B_WINDOW] {
            let (connection, queue) = Connection::for_test(id);
            agent.connections.push(connection);
            outbound.push((id, queue));
        }
        agent.account.as_ref().unwrap().set_foreground(true);
        let mut h = Self {
            agent,
            events,
            outbound,
            results: HashMap::new(),
            states: HashMap::new(),
            gate,
            vault,
            request: 0,
        };
        let deadline = Instant::now() + WAIT;
        while h
            .agent
            .account
            .as_ref()
            .unwrap()
            .state()
            .held_accounts
            .iter()
            .filter(|account| account.stage == AccountStage::SignedIn)
            .count()
            != 2
        {
            h.pump(deadline);
        }
        let (generation, _) = h.command(A_WINDOW, AccountCommand::ListProfiles);
        h.result(A_WINDOW, generation);
        h
    }
    fn drain_pushes(&mut self) {
        for (connection, queue) in &self.outbound {
            while let Ok(message) = queue.try_recv() {
                match message {
                    FromAgent::Push {
                        body:
                            Push::AccountResult {
                                owner_scope,
                                generation,
                                result,
                            },
                    } => {
                        self.results
                            .insert((connection.0, generation), (owner_scope, result));
                    }
                    FromAgent::Push {
                        body: Push::Account { state },
                    } => {
                        self.states.insert(connection.0, state);
                    }
                    FromAgent::Push {
                        body: Push::AccountChanged,
                    } => {}
                    other => panic!("unexpected desktop outbound frame: {other:?}"),
                }
            }
        }
    }
    fn pump(&mut self, deadline: Instant) {
        let event = self
            .events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("desktop event before deadline");
        self.agent.wake(event);
        self.drain_pushes();
    }
    fn response(&mut self, connection: ConnectionId, command: AccountCommand) -> Response {
        self.drain_pushes();
        self.request += 1;
        let scope = self.agent.account.as_ref().unwrap().state().active_scope;
        self.agent.said(
            connection,
            self.request,
            Request::Account {
                command,
                expected_owner_scope: Some(scope),
            },
            Duration::ZERO,
        );
        let queue = &self
            .outbound
            .iter()
            .find(|(id, _)| *id == connection)
            .unwrap()
            .1;
        match queue.recv_timeout(WAIT).unwrap() {
            FromAgent::Response { id, body } => {
                assert_eq!(id, self.request);
                body
            }
            other => panic!("expected Agent request response, got {other:?}"),
        }
    }
    fn command(&mut self, connection: ConnectionId, command: AccountCommand) -> (u64, u64) {
        match self.response(connection, command) {
            Response::AccountQueued {
                generation,
                owner_scope,
            } => (generation, owner_scope),
            other => panic!("account request was not admitted: {other:?}"),
        }
    }
    fn result(
        &mut self,
        connection: ConnectionId,
        generation: u64,
    ) -> (u64, AccountOperationResult) {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(result) = self.results.remove(&(connection.0, generation)) {
                return result;
            }
            self.pump(deadline);
        }
    }
    fn switch_b(&mut self) -> u64 {
        let (generation, scope) = self.command(
            B_WINDOW,
            AccountCommand::SwitchAccount {
                account_id: owner(2).account.to_string(),
            },
        );
        let (result_scope, result) = self.result(B_WINDOW, generation);
        assert_eq!(scope, result_scope);
        assert!(
            matches!(result, AccountOperationResult::AccountSelected { scope_id, account_id: Some(id) } if scope_id == scope && id == owner(2).account.to_string())
        );
        let Response::Account { state } = self.response(B_WINDOW, AccountCommand::Status) else {
            panic!("status response");
        };
        assert_eq!(state.active_scope, scope);
        assert_eq!(state.active_account_id, Some(owner(2).account.to_string()));
        assert_eq!(state.profile_count, 22);
        for connection in [A_WINDOW, B_WINDOW] {
            let push = self
                .states
                .get(&connection.0)
                .expect("visible projection broadcast before terminal result");
            assert_eq!(push.active_scope, scope);
            assert_eq!(push.profile_count, 22);
        }
        scope
    }
    fn bytes(&self, account: u8) -> Option<SecretString> {
        self.vault
            .load(
                &entry()
                    .for_account(&owner(account).account.to_string())
                    .unwrap(),
            )
            .unwrap()
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.gate.release();
    }
}

#[test]
fn desktop_request_event_and_push_keep_late_a_owned_while_b_switch_is_immediate() {
    for boundary in [Boundary::Service, Boundary::Vault] {
        let mut h = Harness::new();
        let b_bytes = h.bytes(2).unwrap();
        h.gate.arm(boundary);
        let (generation, a_scope) = h.command(
            A_WINDOW,
            if boundary == Boundary::Service {
                AccountCommand::ListProfiles
            } else {
                AccountCommand::Retry
            },
        );
        h.gate.entered();
        let b_scope = h.switch_b();
        assert_ne!(a_scope, b_scope);
        h.gate.release();
        let (scope, result) = h.result(A_WINDOW, generation);
        assert_eq!(scope, a_scope);
        if boundary == Boundary::Service {
            assert!(
                matches!(result, AccountOperationResult::Profiles { profiles } if profiles[0].koru_identity_id == owner(1).account.to_string())
            );
        } else {
            assert!(matches!(result, AccountOperationResult::SignedIn));
        }
        assert!(
            !h.results.contains_key(&(B_WINDOW.0, generation)),
            "private completion is not broadcast to B's connection"
        );
        assert_eq!(h.bytes(2).unwrap().expose(), b_bytes.expose());
        let Response::Account { state } = h.response(B_WINDOW, AccountCommand::Status) else {
            panic!("status response");
        };
        assert_eq!(state.active_scope, b_scope);
        assert_eq!(state.profile_count, 22);
        let (back, _) = h.command(
            A_WINDOW,
            AccountCommand::SwitchAccount {
                account_id: owner(1).account.to_string(),
            },
        );
        assert_eq!(h.result(A_WINDOW, back).0, a_scope);
        let Response::Account { state } = h.response(A_WINDOW, AccountCommand::Status) else {
            panic!("status response");
        };
        assert_eq!(state.profile_count, 11);
    }
}

#[test]
fn desktop_logout_before_vault_ack_retires_a_and_preserves_b_slot_and_projection() {
    let mut h = Harness::new();
    let b_bytes = h.bytes(2).unwrap();
    h.gate.arm(Boundary::Vault);
    h.command(A_WINDOW, AccountCommand::Retry);
    h.gate.entered();
    let (logout, a_scope) = h.command(A_WINDOW, AccountCommand::Logout);
    let b_scope = h.switch_b();
    h.gate.release();
    let (scope, result) = h.result(A_WINDOW, logout);
    assert_eq!(scope, a_scope);
    assert!(matches!(result, AccountOperationResult::SignedOut));
    assert!(h.bytes(1).is_none());
    assert_eq!(h.bytes(2).unwrap().expose(), b_bytes.expose());
    let Response::Account { state } = h.response(B_WINDOW, AccountCommand::Status) else {
        panic!("status response");
    };
    assert_eq!(state.active_scope, b_scope);
    assert_eq!(state.active_account_id, Some(owner(2).account.to_string()));
    assert!(
        !state
            .held_accounts
            .iter()
            .any(|account| account.account_id == owner(1).account.to_string())
    );
    assert!(!h.results.contains_key(&(B_WINDOW.0, logout)));
}

#[test]
fn the_cloud_sync_reconcile_is_the_agents_own_and_never_queues_behind_itself() {
    let mut h = Harness::new();

    // Signed in and idle: the heartbeat's nudge is admitted. Its answer is
    // addressed to requester 0 — the worker's own — so no window is told
    // about a question it never asked.
    assert!(h.agent.reconcile_cloud_sync());
    // A second nudge while the first is still waiting for admission is
    // refused. The pass runs on a timer; letting it queue behind itself would
    // spend the slot a person's next command needs, and skipping one costs
    // thirty seconds.
    assert!(!h.agent.reconcile_cloud_sync());

    let deadline = Instant::now() + WAIT;
    while h.agent.account.as_ref().unwrap().queued_commands() > 0 {
        h.pump(deadline);
    }
    h.drain_pushes();
    assert!(
        h.results.is_empty(),
        "an unsolicited reconcile must not surface as a window's result"
    );
    assert!(h.agent.reconcile_cloud_sync());
}

#[test]
fn an_agent_without_the_account_capability_simply_does_not_reconcile() {
    assert!(!agent().reconcile_cloud_sync());
}
