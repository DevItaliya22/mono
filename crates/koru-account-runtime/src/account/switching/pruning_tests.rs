// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::identity::{AccountOwner, DirectoryRecord, DirectoryState};
use koru_oauth_credentials::{CredentialContext, SessionCredentialStore};

struct Service {
    safe: bool,
    dropped: Arc<AtomicUsize>,
}
impl Drop for Service {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }
}
impl AccountService for Service {
    fn execute(
        &mut self,
        _: u64,
        _: AccountCommand,
        _: Option<&SessionCredential>,
        _: &Cancel,
        _: &mut dyn FnMut(ServiceProgress),
    ) -> Result<ServiceOutcome, ServiceError> {
        panic!("pruning must not perform effects")
    }
    fn cancellation_safe(&self) -> bool {
        self.safe
    }
}
fn entry() -> CredentialEntry {
    CredentialContext::new(
        "nightly",
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/pruning-fixture"),
        koru_device::DeviceId::parse("30000000-0000-4000-8000-000000000001").unwrap(),
    )
    .unwrap()
    .entry("https://account.invalid")
    .unwrap()
}
fn record(account: u8) -> DirectoryRecord {
    DirectoryRecord {
        owner: AccountOwner {
            account: format!("10000000-0000-4000-8000-{account:012}")
                .parse()
                .unwrap(),
            auth_identity: format!("20000000-0000-4000-8000-{account:012}")
                .parse()
                .unwrap(),
        },
        state: DirectoryState::Ready,
        version: 1,
    }
}
fn terminal(scope: u64, safe: bool, dropped: &Arc<AtomicUsize>) -> Held {
    let state = Arc::new(Mutex::new(signed_out(0)));
    let persistence = directory::CredentialPersistence::new(
        Arc::new(SessionCredentialStore::default()),
        entry(),
        None,
    )
    .for_selection(None);
    let mut runtime = SessionRuntime::new(
        scope,
        Box::new(Service {
            safe,
            dropped: dropped.clone(),
        }),
        persistence,
        state,
    );
    runtime.initialized = true;
    let mut held = Held::new(runtime, None);
    held.removing = true;
    held
}
fn routes() -> Mutex<Routing> {
    Mutex::new(Routing {
        selected: 1,
        anonymous: None,
        next_scope: 2,
        signin_scopes: Default::default(),
        accounts: BTreeMap::from([(1, Some(record(1).owner.account))]),
        generations: Default::default(),
        queued_generations: Default::default(),
        retiring_scopes: Default::default(),
        retired_accounts: Default::default(),
    })
}
fn envelope(generation: u64) -> Envelope {
    Envelope {
        owner_scope: 2,
        requester: 7,
        generation,
        command: AccountCommand::Logout,
        cancel: Arc::new(AtomicBool::new(false)),
        counted: false,
        cancel_attempt: false,
    }
}

#[test]
fn repeated_logout_and_rejected_repair_cycles_release_services_and_routing() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut selected = terminal(1, true, &dropped);
    selected.record = Some(record(1));
    selected.removing = false;
    let mut held = BTreeMap::from([(1, selected)]);
    let routing = routes();
    let active: Active = Arc::new(Mutex::new(HashMap::new()));
    for scope in 2..202 {
        let mut completed = terminal(scope, true, &dropped);
        if scope % 2 == 0 {
            completed.removing = false;
            completed.state.lock().unwrap().stage = AccountStage::Failed;
            completed.state.lock().unwrap().failure_code =
                Some("account_repair_identity_mismatch".into());
        }
        held.insert(scope, completed);
        {
            let mut routes = routing.lock().unwrap();
            routes.next_scope = scope + 1;
            routes.accounts.insert(scope, None);
            routes.generations.insert(scope, scope);
            routes.retiring_scopes.insert(scope);
            routes.signin_scopes.insert(scope);
            routes
                .retired_accounts
                .insert(record(2).owner.account, scope);
        }
        prune_retired(&mut held, &routing, &active);
        assert_eq!(held.len(), 1);
        assert_eq!(dropped.load(Ordering::Relaxed), (scope - 1) as usize);
        let routes = routing.lock().unwrap();
        assert_eq!(routes.accounts.len(), 1);
        assert!(routes.generations.is_empty());
        assert!(routes.queued_generations.is_empty());
        assert!(routes.retiring_scopes.is_empty());
        assert!(routes.signin_scopes.is_empty());
        assert!(routes.retired_accounts.is_empty());
        assert_eq!(routes.next_scope, scope + 1);
    }
}

#[test]
fn selection_live_delivery_and_unresolved_effects_survive_pruning() {
    for case in 0..12 {
        let dropped = Arc::new(AtomicUsize::new(0));
        let mut h = terminal(2, case != 0, &dropped);
        let routing = routes();
        let active: Active = Arc::new(Mutex::new(HashMap::new()));
        let mut running = None;
        match case {
            0 => {} // Remote uncertainty is not terminal cleanup.
            1 => running = h.runtime.take(),
            2 => h.runtime.as_mut().unwrap().retry = Some((envelope(7), Instant::now(), 0)),
            3 => h.queue.push_back(envelope(7)),
            4 => {
                h.pending_result = Some(WorkerEvent::Result {
                    owner_scope: 2,
                    requester: 7,
                    generation: 7,
                    result: AccountOperationResult::SignedOut,
                })
            }
            5 => {
                active
                    .lock()
                    .unwrap()
                    .insert(7, (7, Arc::new(AtomicBool::new(false))));
                routing.lock().unwrap().generations.insert(7, 2);
            }
            6 => {
                h.runtime.as_mut().unwrap().credential =
                    Some(SessionCredential::new("session".into(), "secret".into()).unwrap())
            }
            7 => h.record = Some(record(2)),
            8 => routing.lock().unwrap().selected = 2,
            9 => routing.lock().unwrap().anonymous = Some(2),
            10 => h.state.lock().unwrap().stage = AccountStage::AwaitingAccountCreation,
            11 => {
                let mut routes = routing.lock().unwrap();
                routes.generations.insert(7, 2);
                routes.queued_generations.insert(7);
            }
            _ => unreachable!(),
        }
        let mut held = BTreeMap::from([(2, h)]);
        prune_retired(&mut held, &routing, &active);
        assert!(held.contains_key(&2), "retained case {case}");
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        drop(running);
    }
}

#[test]
fn retirement_fence_survives_an_unknown_old_scope_until_its_owner_is_known() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut held = BTreeMap::from([(1, terminal(1, false, &dropped))]);
    let routing = routes();
    routing
        .lock()
        .unwrap()
        .retired_accounts
        .insert(record(2).owner.account, 5);
    let active: Active = Arc::new(Mutex::new(HashMap::new()));
    prune_retired(&mut held, &routing, &active);
    assert_eq!(routing.lock().unwrap().retired_accounts.len(), 1);
    held.get_mut(&1).unwrap().record = Some(record(1));
    prune_retired(&mut held, &routing, &active);
    assert!(routing.lock().unwrap().retired_accounts.is_empty());
}
