// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_account_runtime::enrollment::test_support::{Boundary, Effect, Fixture, owner, profile};
use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

const REQUESTER: u64 = 71;
const WAIT: Duration = Duration::from_secs(10);

struct Harness {
    account: MobileAccount,
    fixture: Fixture,
    entries: BTreeMap<String, Vec<u8>>,
    answered: HashSet<u64>,
    results: HashMap<u64, wire::MobileAccountResult>,
    cursor: u64,
    expected_visible: Option<u64>,
    b_complete: bool,
}

impl Harness {
    fn new() -> Self {
        let fixture = Fixture::new();
        fixture.arm(Boundary::Enroll, 1);
        let bridge = PlatformBridge::new("injected-web.apps.googleusercontent.com".into()).unwrap();
        let mut account = MobileAccount::start_inner(
            fixture.service(),
            bridge,
            fixture.entry.clone(),
            Some(fixture.directory.clone()),
        );
        account
            .control
            .set_enrollment_control(fixture.enrollment.clone());
        account.enrollment = Some(fixture.enrollment.clone());
        let h = Self {
            account,
            entries: fixture.seeded_entries(),
            fixture,
            answered: HashSet::new(),
            results: HashMap::new(),
            cursor: 0,
            expected_visible: None,
            b_complete: false,
        };
        h.platform(wire::account_platform_request::Response::Foreground(true));
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

    fn poll(&mut self) -> wire::MobileAccountStatus {
        let reply = self.account.poll(REQUESTER, self.cursor, true);
        assert!(reply.error_code.is_none());
        assert!(
            reply.sign_in.is_none(),
            "restoration never starts a provider visit"
        );
        for result in reply.results {
            self.cursor = self.cursor.max(result.sequence);
            self.results.insert(result.generation, result);
        }
        if let Some(prompt) = reply.vault {
            if self.answered.insert(prompt.request_id) {
                let credential = match wire::AccountVaultAction::try_from(prompt.action).unwrap() {
                    wire::AccountVaultAction::Load => self.entries.get(&prompt.entry).cloned(),
                    wire::AccountVaultAction::Store => {
                        self.entries
                            .insert(prompt.entry, prompt.credential.unwrap());
                        None
                    }
                    wire::AccountVaultAction::Clear => {
                        self.entries.remove(&prompt.entry);
                        None
                    }
                    _ => panic!("unexpected vault operation"),
                };
                self.platform(wire::account_platform_request::Response::Vault(
                    wire::AccountVaultReply {
                        request_id: prompt.request_id,
                        success: true,
                        credential,
                    },
                ));
            }
        }
        let status = reply.status.unwrap();
        if self.b_complete {
            assert_eq!(status.enrollments.len(), 1);
            assert_eq!(status.enrollments[0].stage, "complete");
        }
        if let Some(scope) = self.expected_visible {
            assert_eq!(status.active_scope, scope);
            assert_eq!(status.active_account_id, Some(owner(2).account.to_string()));
            assert!(
                status
                    .enrollments
                    .iter()
                    .all(|row| row.profile_id == profile(2).to_string()),
                "late A work cannot enter any B poll"
            );
        }
        status
    }

    fn command(
        &self,
        action: wire::AccountAction,
        account: Option<u8>,
        profiles: &[u8],
    ) -> (u64, u64) {
        let admission = self
            .account
            .command_owned(&wire::AccountCommandRequest {
                version: 1,
                requester: REQUESTER,
                expected_owner_scope: Some(self.account.control.state().active_scope),
                action: action as i32,
                account_id: account
                    .map(|account| owner(account).account.to_string())
                    .unwrap_or_default(),
                profile_ids: profiles.iter().map(|id| profile(*id).to_string()).collect(),
                ..Default::default()
            })
            .unwrap();
        (admission.generation, admission.owner_scope)
    }

    fn until(
        &mut self,
        mut ready: impl FnMut(&Self, &wire::MobileAccountStatus) -> bool,
    ) -> wire::MobileAccountStatus {
        let deadline = Instant::now() + WAIT;
        loop {
            let status = self.poll();
            if ready(self, &status) {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "mobile enrollment adapter did not settle"
            );
            std::thread::yield_now();
        }
    }

    fn result(&mut self, generation: u64) -> wire::MobileAccountResult {
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
fn mobile_command_poll_restores_singleton_and_keeps_every_b_poll_owned_after_late_a() {
    let mut h = Harness::new();
    let original_bytes = h.entries.clone();
    let initial = h.until(|h, state| {
        h.fixture.entered()
            && state
                .held_accounts
                .iter()
                .filter(|row| row.stage == "signed_in")
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

    let (selected, a_scope) = h.command(wire::AccountAction::SelectProfiles, None, &[1, 3]);
    let result = h.result(selected);
    assert_eq!(result.owner_scope, a_scope);
    assert_eq!(result.kind, "profiles_selected");
    assert_eq!(
        result
            .enrollments
            .iter()
            .map(|row| row.profile_id.clone())
            .collect::<Vec<_>>(),
        vec![profile(1).to_string(), profile(3).to_string()]
    );
    let unfinished_a = h.fixture.targets(1);
    assert!(unfinished_a.contains(&profile(1)));
    assert!(unfinished_a.is_subset(&[profile(1), profile(3)].into()));

    let (switched, b_scope) = h.command(wire::AccountAction::SwitchAccount, Some(2), &[]);
    let result = h.result(switched);
    assert_eq!(result.kind, "account_selected");
    assert_eq!(result.owner_scope, b_scope);
    assert_eq!(result.selected_scope, Some(b_scope));
    assert_eq!(
        result.selected_account_id,
        Some(owner(2).account.to_string())
    );
    assert_ne!(a_scope, b_scope);
    // The enrollment ceremony remains held while selection and polling finish.
    h.expected_visible = Some(b_scope);
    h.until(|h, state| {
        state.enrollments.len() == 1
            && state.enrollments[0].stage == "complete"
            && h.fixture.targets(2).is_empty()
    });
    h.b_complete = true;
    assert!(h.fixture.targets(2).is_empty());
    for _ in 0..32 {
        h.poll();
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
    for _ in 0..32 {
        h.poll();
    }
    assert_eq!(
        h.entries, original_bytes,
        "account selection and enrollment do not replace another slot's bytes"
    );
}
