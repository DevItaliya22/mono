// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_account_runtime::account::directory::{AccountDirectory, MemoryAccountDirectory};
use koru_account_runtime::identity::AccountOwner;
use std::collections::HashSet;
use std::time::Instant;

const REQUESTER: u64 = 7;
const DEADLINE: Duration = Duration::from_secs(10);

fn owner(n: u8) -> AccountOwner {
    AccountOwner {
        account: format!("10000000-0000-4000-8000-{n:012}").parse().unwrap(),
        auth_identity: format!("20000000-0000-4000-8000-{n:012}").parse().unwrap(),
    }
}
fn slot_entry() -> CredentialEntry {
    CredentialContext::new(
        "test",
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/mobile-switching-fixture"),
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
fn account_of(credential: &SessionCredential) -> u8 {
    credential
        .session_id
        .strip_prefix("session-")
        .unwrap()
        .parse()
        .unwrap()
}

/// Each fork owns its account continuation; the bridge and counters are ports.
struct Service {
    bridge: Arc<PlatformBridge>,
    rotations: Arc<AtomicUsize>,
}
impl AccountService for Service {
    fn fork(&self) -> Option<Box<dyn AccountService>> {
        Some(Box::new(Self {
            bridge: self.bridge.clone(),
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
        cancel: &Cancel,
        _: &mut dyn FnMut(ServiceProgress),
    ) -> Result<ServiceOutcome, ServiceError> {
        let account = account_of(restored.expect("an admitted held account restores its own slot"));
        let mut state = AccountState {
            generation,
            capability: true,
            stage: AccountStage::SignedIn,
            profile_count: usize::from(account) * 11,
            ..Default::default()
        };
        let (credential, result) = match command {
            AccountCommand::StartGoogle { .. } => {
                let mut delivery = NativeDelivery(self.bridge.clone());
                let token = NativeSignInSession::wait(
                    &mut delivery,
                    &NativeSignInRequest {
                        generation,
                        request_id: generation,
                        nonce: "injected-nonce".into(),
                        expires_at_millis: self.bridge.clock.now_millis().unwrap() + 60_000,
                        reauthentication: false,
                    },
                    cancel,
                )?;
                assert_eq!(token.0, "injected-google-token");
                (
                    CredentialChange::Replace(credential(
                        account,
                        self.rotations.fetch_add(1, Ordering::SeqCst) + 1,
                    )),
                    AccountOperationResult::SignedIn,
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
            AccountCommand::ListProfileChoices => (
                CredentialChange::Keep,
                AccountOperationResult::ProfileChoices {
                    display_name: format!("Account {account}"),
                    choices: vec![],
                },
            ),
            _ => panic!("unexpected injected mobile command"),
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
    account: MobileAccount,
    directory: Arc<MemoryAccountDirectory>,
    entries: HashMap<String, Vec<u8>>,
    answered_vault: HashSet<u64>,
    results: HashMap<u64, wire::MobileAccountResult>,
    cursor: u64,
}
impl Harness {
    fn new() -> Self {
        let directory = Arc::new(MemoryAccountDirectory::new());
        let entry = slot_entry();
        directory.retire_legacy(&entry.namespace()).unwrap();
        let mut entries = HashMap::new();
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
            entries.insert(
                entry
                    .for_account(&owner(account).account.to_string())
                    .unwrap()
                    .account()
                    .to_owned(),
                credential(account, 0)
                    .encode()
                    .unwrap()
                    .expose()
                    .as_bytes()
                    .to_vec(),
            );
        }
        let bridge = PlatformBridge::new("injected-web.apps.googleusercontent.com".into()).unwrap();
        let service = Service {
            bridge: bridge.clone(),
            rotations: Arc::new(AtomicUsize::new(0)),
        };
        let account = MobileAccount::start_inner(service, bridge, entry, Some(directory.clone()));
        let mut h = Self {
            account,
            directory,
            entries,
            answered_vault: HashSet::new(),
            results: HashMap::new(),
            cursor: 0,
        };
        h.platform(wire::account_platform_request::Response::Foreground(true));
        let deadline = Instant::now() + DEADLINE;
        loop {
            let reply = h.poll(true);
            if reply
                .status
                .unwrap()
                .held_accounts
                .iter()
                .filter(|a| a.stage == "signed_in")
                .count()
                == 2
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "both seeded slots must restore through mobile vault prompts"
            );
            std::thread::yield_now();
        }
        // A real command/result fences Finished and ownership publication.
        let generation = h.command(wire::AccountAction::ListProfiles, None);
        h.result(generation, true);
        h
    }
    fn platform(&self, response: wire::account_platform_request::Response) {
        self.account
            .platform(&mut wire::AccountPlatformRequest {
                version: 1,
                requester: REQUESTER,
                response: Some(response),
                ..Default::default()
            })
            .unwrap();
    }
    fn poll(&mut self, answer_vault: bool) -> wire::AccountPollReply {
        let reply = self.account.poll(REQUESTER, self.cursor, true);
        assert!(
            reply.error_code.is_none(),
            "fixture consumes every result without cursor gaps"
        );
        for result in &reply.results {
            self.cursor = self.cursor.max(result.sequence);
            self.results.insert(result.generation, result.clone());
        }
        if answer_vault && let Some(prompt) = &reply.vault {
            self.answer_vault(prompt);
        }
        reply
    }
    fn answer_vault(&mut self, prompt: &wire::AccountVaultPrompt) {
        if !self.answered_vault.insert(prompt.request_id) {
            return;
        }
        let credential = match wire::AccountVaultAction::try_from(prompt.action).unwrap() {
            wire::AccountVaultAction::Load => self.entries.get(&prompt.entry).cloned(),
            wire::AccountVaultAction::Store => {
                self.entries
                    .insert(prompt.entry.clone(), prompt.credential.clone().unwrap());
                None
            }
            wire::AccountVaultAction::Clear => {
                self.entries.remove(&prompt.entry);
                None
            }
            _ => panic!("unexpected vault action"),
        };
        self.platform(wire::account_platform_request::Response::Vault(
            wire::AccountVaultReply {
                request_id: prompt.request_id,
                success: true,
                credential,
            },
        ));
    }
    fn command(&self, action: wire::AccountAction, account: Option<u8>) -> u64 {
        self.account
            .command(&wire::AccountCommandRequest {
                version: 1,
                requester: REQUESTER,
                expected_owner_scope: Some(self.account.control.state().active_scope),
                action: action as i32,
                account_id: account
                    .map(|a| owner(a).account.to_string())
                    .unwrap_or_default(),
                locale: if action == wire::AccountAction::StartGoogle {
                    "en".into()
                } else {
                    String::new()
                },
                ..Default::default()
            })
            .unwrap()
    }
    fn result(&mut self, generation: u64, answer_vault: bool) -> wire::MobileAccountResult {
        let deadline = Instant::now() + DEADLINE;
        loop {
            if let Some(result) = self.results.remove(&generation) {
                return result;
            }
            self.poll(answer_vault);
            assert!(
                Instant::now() < deadline,
                "mobile command must complete before held effect release"
            );
            std::thread::yield_now();
        }
    }
    fn switch_b(&mut self) -> u64 {
        let generation = self.command(wire::AccountAction::SwitchAccount, Some(2));
        let result = self.result(generation, false);
        assert_eq!(result.kind, "account_selected");
        let status = self.poll(false).status.unwrap();
        assert_eq!(status.active_account_id, Some(owner(2).account.to_string()));
        assert_eq!(status.profile_count, 22);
        assert_eq!(result.owner_scope, status.active_scope);
        status.active_scope
    }
    fn bytes(&self, account: u8) -> Option<&Vec<u8>> {
        self.entries.get(
            slot_entry()
                .for_account(&owner(account).account.to_string())
                .unwrap()
                .account(),
        )
    }
    fn held_store(&mut self) -> wire::AccountVaultPrompt {
        let deadline = Instant::now() + DEADLINE;
        loop {
            if let Some(prompt) = self.poll(false).vault {
                if prompt.action == wire::AccountVaultAction::Store as i32
                    && !self.answered_vault.contains(&prompt.request_id)
                {
                    return prompt;
                }
                self.answer_vault(&prompt);
            }
            assert!(
                Instant::now() < deadline,
                "A must reach its held Store prompt"
            );
            std::thread::yield_now();
        }
    }
}

#[test]
fn mobile_switches_stay_responsive_and_retain_a_prompt_owner_through_result_ring_churn() {
    for hold_sdk in [true, false] {
        let mut h = Harness::new();
        let a_scope = h.account.control.state().active_scope;
        let b_bytes = h.bytes(2).unwrap().clone();
        let generation = h.command(
            if hold_sdk {
                wire::AccountAction::StartGoogle
            } else {
                wire::AccountAction::Retry
            },
            None,
        );
        let native = if hold_sdk {
            let deadline = Instant::now() + DEADLINE;
            loop {
                if let Some(prompt) = h.poll(true).sign_in {
                    break Some(prompt);
                }
                assert!(
                    Instant::now() < deadline,
                    "injected SDK prompt must be published"
                );
                std::thread::yield_now();
            }
        } else {
            None
        };
        let store = if hold_sdk { None } else { Some(h.held_store()) };
        for _ in 0..(RESULT_CAPACITY + 8) {
            h.switch_b();
            let status = h.poll(false).status.unwrap();
            assert!(
                status
                    .held_accounts
                    .iter()
                    .any(|a| a.scope_id == a_scope && a.requester_active == Some(true))
            );
            assert_eq!(
                h.account.results.lock().unwrap().owners.get(&generation),
                Some(&REQUESTER)
            );
        }
        assert_eq!(
            h.account.poll(REQUESTER, 0, false).error_code.as_deref(),
            Some("account_result_gap"),
            "ring really churned while living ownership survived"
        );
        if let Some(prompt) = native {
            assert_eq!(prompt.generation, generation);
            h.platform(wire::account_platform_request::Response::SignIn(
                wire::NativeSignInReply {
                    generation,
                    request_id: prompt.request_id,
                    cancelled: false,
                    id_token: Some(b"injected-google-token".to_vec()),
                },
            ));
        }
        if let Some(prompt) = store {
            h.answer_vault(&prompt);
        }
        let completed = h.result(generation, true);
        assert_eq!(completed.owner_scope, a_scope);
        assert_eq!(completed.kind, "signed_in");
        assert_eq!(
            h.poll(true).status.unwrap().active_account_id,
            Some(owner(2).account.to_string())
        );
        assert_eq!(h.bytes(2).unwrap(), &b_bytes);
        let back = h.command(wire::AccountAction::SwitchAccount, Some(1));
        assert_eq!(h.result(back, true).owner_scope, a_scope);
        assert_eq!(h.poll(true).status.unwrap().profile_count, 11);
    }
}

#[test]
fn mobile_logout_before_a_vault_ack_removes_only_a_and_late_ack_does_not_resurrect_it() {
    let mut h = Harness::new();
    let a_scope = h.account.control.state().active_scope;
    let b_bytes = h.bytes(2).unwrap().clone();
    h.command(wire::AccountAction::Retry, None);
    let store = h.held_store();
    let logout = h.command(wire::AccountAction::Logout, None);
    h.switch_b(); // Admission/visible projection precede release of A's Store ACK.
    h.answer_vault(&store);
    let result = h.result(logout, true);
    assert_eq!(result.owner_scope, a_scope);
    assert_eq!(result.kind, "signed_out");
    let fence = h.command(wire::AccountAction::ListProfiles, None);
    h.result(fence, true);
    let status = h.poll(true).status.unwrap();
    assert_eq!(status.active_account_id, Some(owner(2).account.to_string()));
    assert!(
        !status
            .held_accounts
            .iter()
            .any(|a| a.account_id == owner(1).account.to_string())
    );
    assert!(h.bytes(1).is_none());
    assert_eq!(h.bytes(2).unwrap(), &b_bytes);
    let directory = h.directory.snapshot(&slot_entry().namespace()).unwrap();
    assert_eq!(directory.active, Some(owner(2).account));
    assert_eq!(directory.records.len(), 1);
    assert_eq!(directory.records[0].owner, owner(2));
}
