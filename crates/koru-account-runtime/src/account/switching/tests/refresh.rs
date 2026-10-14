// SPDX-License-Identifier: MPL-2.0

use super::*;

struct AdvancingClock {
    base: Instant,
    probe: Arc<Probe>,
}
impl Clock for AdvancingClock {
    fn now_millis(&self) -> Option<i64> {
        Some(self.probe.now_millis.load(Ordering::Acquire))
    }
    fn monotonic(&self) -> Instant {
        self.base + Duration::from_millis(self.now_millis().unwrap() as u64)
    }
}
fn timed() -> Harness {
    let probe = Arc::new(Probe::default());
    probe.refresh_deadlines.store(true, Ordering::Release);
    probe.now_millis.store(1_000, Ordering::Release);
    let clock = Arc::new(AdvancingClock {
        base: Instant::now(),
        probe: probe.clone(),
    });
    Harness::with_environment(None, false, probe, clock)
}

#[test]
fn idle_selected_and_held_sessions_rotate_once_at_their_deadline_without_browser() {
    let mut h = timed();
    h.ready();
    assert_eq!(h.probe.rotations.load(Ordering::Acquire), 2);
    h.probe.now_millis.store(61_000, Ordering::Release);
    h.control.set_foreground(true); // Wake the existing timed wait after fake-time advancement.
    let deadline = Instant::now() + WAIT;
    while h.probe.rotations.load(Ordering::Acquire) != 4 {
        h.receive(deadline);
    }
    for account in [1, 2] {
        h.switch(account);
        let generation = h.command(AccountCommand::Status);
        h.result(generation);
        let saved = SessionCredential::decode(&h.bytes(account).unwrap()).unwrap();
        assert_ne!(saved.refresh_token, format!("refresh-{account}-{account}"));
    }
    assert_eq!(h.probe.rotations.load(Ordering::Acquire), 4);
    assert!(h.probe.acknowledged_records.lock().unwrap().len() >= 4);
}

#[test]
fn terminal_background_refresh_failure_does_not_spin_on_an_expired_deadline() {
    let mut h = timed();
    h.ready();
    h.probe.fail_refresh_once.store(true, Ordering::Release);
    h.probe.now_millis.store(61_000, Ordering::Release);
    h.control.set_foreground(true);
    let deadline = Instant::now() + WAIT;
    while !h.control.state().held_accounts.iter().any(|row| {
        row.account_id == owner(1).account.to_string()
            && row.failure_code.as_deref() == Some("refresh_fixture_refused")
    }) {
        h.receive(deadline);
    }
    // B remains healthy; FIFO B commands exercise further scheduling turns
    // without manually changing A's failed state or borrowing A's credential.
    h.switch(2);
    for _ in 0..3 {
        let generation = h.command(AccountCommand::Status);
        h.result(generation);
    }
    while h.probe.rotations.load(Ordering::Acquire) != 3 {
        h.receive(deadline);
    }
    assert_eq!(h.probe.rotations.load(Ordering::Acquire), 3);
    assert_eq!(
        SessionCredential::decode(&h.bytes(1).unwrap())
            .unwrap()
            .refresh_token,
        "refresh-1-1"
    );
}

fn account_row(h: &Harness, account: u8) -> koru_agent_wire::AccountHeldAccount {
    h.control
        .state()
        .held_accounts
        .into_iter()
        .find(|row| row.account_id == owner(account).account.to_string())
        .expect("held account row")
}

#[test]
fn rate_limited_hosted_command_answers_its_requester_and_keeps_the_account_signed_in() {
    let mut h = timed();
    h.ready();
    h.probe.fail_list_once.store(true, Ordering::Release);
    let generation = h.command(AccountCommand::ListProfiles);
    let (_, result) = h.result(generation);
    assert!(
        matches!(&result, AccountOperationResult::Failed { code } if code == "fixture_held_retry"),
        "{result:?}"
    );
    let deadline = Instant::now() + WAIT;
    while h.control.state().retry_at_millis.is_none() {
        h.receive(deadline);
    }
    let state = h.control.state();
    assert_eq!(state.stage, AccountStage::SignedIn);
    assert_eq!(state.failure_code, None);
    assert_eq!(state.retry_at_millis, Some(1_000 + 60_000));
    let row = account_row(&h, 1);
    assert_eq!(row.stage, AccountStage::SignedIn);
    assert_eq!(row.failure_code, None);
    assert!(
        !h.control.requester_generation_active(7, generation),
        "an answered requester no longer owns the background retry"
    );

    // The parked call still runs at its own time, in the background.
    h.probe.now_millis.store(61_000, Ordering::Release);
    h.control.set_foreground(true);
    while h.probe.list_calls.load(Ordering::Acquire) != 2 {
        h.receive(deadline);
    }
    assert_eq!(h.control.state().failure_code, None);
}

#[test]
fn retryable_refresh_failure_still_takes_the_account_to_failed() {
    let mut h = timed();
    h.ready();
    h.probe
        .refresh_unavailable_once
        .store(true, Ordering::Release);
    h.probe.now_millis.store(61_000, Ordering::Release);
    h.control.set_foreground(true);
    let deadline = Instant::now() + WAIT;
    while account_row(&h, 1).failure_code.as_deref() != Some("account_unavailable") {
        h.receive(deadline);
    }
    let state = h.control.state();
    assert_eq!(state.stage, AccountStage::Failed);
    assert_eq!(state.failure_code.as_deref(), Some("account_unavailable"));
    assert_eq!(state.retry_at_millis, Some(61_000 + 60_000));
}

#[test]
fn idle_refresh_runs_on_schedule_while_a_hosted_retry_waits() {
    let mut h = timed();
    h.ready();
    // Refresh is due at 61 000 (expiry minus a minute in this fixture).
    h.probe.now_millis.store(30_000, Ordering::Release);
    h.probe.fail_list_once.store(true, Ordering::Release);
    let generation = h.command(AccountCommand::ListProfiles);
    h.result(generation);
    let deadline = Instant::now() + WAIT;
    while h.control.state().retry_at_millis != Some(90_000) {
        h.receive(deadline);
    }
    let stored = |h: &Harness| {
        SessionCredential::decode(&h.bytes(1).unwrap())
            .unwrap()
            .refresh_token
            .clone()
    };
    let before = stored(&h);

    h.probe.now_millis.store(61_000, Ordering::Release);
    h.control.set_foreground(true);
    while stored(&h) == before {
        h.receive(deadline);
    }
    assert_eq!(
        h.probe.list_calls.load(Ordering::Acquire),
        1,
        "the refresh runs alone; the parked call keeps its own time"
    );
    assert_eq!(account_row(&h, 1).stage, AccountStage::SignedIn);

    h.probe.now_millis.store(90_000, Ordering::Release);
    h.control.set_foreground(true);
    while h.probe.list_calls.load(Ordering::Acquire) != 2 {
        h.receive(deadline);
    }
    assert_eq!(account_row(&h, 1).failure_code, None);
}
