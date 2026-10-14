// SPDX-License-Identifier: MPL-2.0

use super::*;

use koru_settings::{DensityChoice, ThemeChoice};

fn test_link(
    authoritative: GlobalSettings,
) -> (GlobalLink, Receiver<Command<GlobalSettings, GlobalPatch>>) {
    let (_send_update, updates) = std::sync::mpsc::channel();
    let (commands, take_command) = std::sync::mpsc::channel();
    let link = GlobalLink {
        updates: Some(updates),
        commands: Some(commands),
        authoritative,
        notify: Arc::new(Mutex::new(None)),
        confirmed_generation: Arc::new(AtomicU64::new(0)),
    };
    (link, take_command)
}

fn take_request(
    commands: &Receiver<Command<GlobalSettings, GlobalPatch>>,
) -> ConfirmedRequest<GlobalSettings> {
    match commands.recv().unwrap() {
        Command::Patch {
            confirmed: Some(request),
            ..
        } => request,
        _ => unreachable!(),
    }
}

/// **What a window shows before it has heard anything**, and what it goes on
/// showing if the Gateway never answers: the registry's defaults, immediately,
/// with no thread and no wait. That is S9, and it is the whole contract a
/// caller on the event thread depends on.
#[test]
fn a_disabled_link_answers_the_defaults_and_never_blocks() {
    let mut link = GlobalLink::disabled();
    assert!(!link.is_connected());
    assert_eq!(link.authoritative(), &GlobalSettings::default());
    assert!(link.poll().is_none());
    assert!(link.poll().is_none(), "and keeps not blocking");
}

#[test]
fn confirmed_truth_can_be_installed_before_the_broadcast_arrives() {
    let mut link = GlobalLink::disabled();
    let mut confirmed = GlobalSettings::default();
    confirmed.global_dictation_enabled = Some(true);
    link.install_confirmed(confirmed.clone());
    assert_eq!(link.authoritative(), &confirmed);
}

/// A patch is this window's answer the moment it is made — before, and
/// regardless of, the authority's reply. Without that the same patch goes out
/// on every tick until the round trip completes.
#[test]
fn a_patch_is_taken_locally_the_moment_it_is_sent() {
    let mut link = GlobalLink::disabled();
    link.save(
        GlobalPatch {
            theme: Some(ThemeChoice::Dark),
            ..GlobalPatch::default()
        },
        |patch, section| patch.clone().apply(section),
    );
    assert_eq!(link.authoritative().theme, ThemeChoice::Dark);
    assert_eq!(
        link.authoritative().density,
        DensityChoice::default(),
        "a field the patch did not name did not move"
    );
}

#[test]
fn confirmed_write_waits_for_the_authoritys_exact_echo() {
    let before = GlobalSettings::default();
    let (mut link, commands) = test_link(before.clone());
    let patch = GlobalPatch {
        start_on_login: Some(true),
        ..GlobalPatch::default()
    };
    let mut receipt = link.save_confirmed(patch);
    assert_eq!(link.authoritative(), &before);
    assert!(receipt.poll().is_none());

    let mut request = take_request(&commands);
    request.phase.store(CONFIRMED_IN_FLIGHT, Ordering::Release);
    let mut echoed = before;
    echoed.start_on_login = true;
    assert!(request.settle(ConfirmedOutcome::Accepted(echoed.clone())));
    assert_eq!(receipt.poll(), Some(ConfirmedOutcome::Accepted(echoed)));
    assert!(receipt.poll().is_none());
}

#[test]
fn confirmed_write_without_a_worker_is_explicitly_disconnected() {
    let mut link = GlobalLink::disabled();
    let mut receipt = link.save_confirmed(GlobalPatch::default());
    assert_eq!(receipt.poll(), Some(ConfirmedOutcome::Disconnected));
}

#[test]
fn a_new_intent_makes_an_older_queued_receipt_stale_and_cancels_its_send() {
    let (mut link, commands) = test_link(GlobalSettings::default());
    let mut old = link.save_confirmed(GlobalPatch::default());
    let _new = link.save_confirmed(GlobalPatch::default());
    assert_eq!(old.poll(), Some(ConfirmedOutcome::Stale));
    let request = take_request(&commands);
    assert_eq!(request.phase.load(Ordering::Acquire), CONFIRMED_CANCELLED);
}

#[test]
fn an_in_flight_old_write_reports_stale_but_is_not_cancelled() {
    let (mut link, commands) = test_link(GlobalSettings::default());
    let mut old = link.save_confirmed(GlobalPatch::default());
    let request = take_request(&commands);
    request.phase.store(CONFIRMED_IN_FLIGHT, Ordering::Release);
    let _new = link.save_confirmed(GlobalPatch::default());
    assert_eq!(old.poll(), Some(ConfirmedOutcome::Stale));
    assert_eq!(request.phase.load(Ordering::Acquire), CONFIRMED_IN_FLIGHT);
}

#[test]
fn only_a_queued_write_can_time_out() {
    let (mut link, commands) = test_link(GlobalSettings::default());
    let mut queued = link.save_confirmed(GlobalPatch::default());
    let request = take_request(&commands);
    let now = Instant::now();
    queued.expires_at = now;
    assert_eq!(queued.poll_at(now), Some(ConfirmedOutcome::TimedOut));
    assert_eq!(request.phase.load(Ordering::Acquire), CONFIRMED_CANCELLED);

    let mut in_flight = link.save_confirmed(GlobalPatch::default());
    let request = take_request(&commands);
    request.phase.store(CONFIRMED_IN_FLIGHT, Ordering::Release);
    in_flight.expires_at = now;
    assert!(in_flight.poll_at(now).is_none());
    assert!(in_flight.deadline().is_none());
}

#[test]
fn worker_preflight_skips_a_receipt_cancelled_before_send() {
    let (mut link, commands) = test_link(GlobalSettings::default());
    let mut receipt = link.save_confirmed(GlobalPatch::default());
    let request = take_request(&commands);
    let now = Instant::now();
    receipt.expires_at = now;
    assert_eq!(receipt.poll_at(now), Some(ConfirmedOutcome::TimedOut));
    assert_eq!(begin_confirmed(&request, now), ConfirmedStart::Skip);
}

#[test]
fn worker_preflight_is_the_only_mutation_authority() {
    let (send, _result) = std::sync::mpsc::channel();
    let generation = Arc::new(AtomicU64::new(1));
    let phase = Arc::new(AtomicU8::new(CONFIRMED_QUEUED));
    let now = Instant::now();
    let request: ConfirmedRequest<GlobalSettings> = ConfirmedRequest {
        result: Some(send),
        generation: 1,
        newest_generation: Arc::clone(&generation),
        phase: Arc::clone(&phase),
        expires_at: now + CONFIRMED_WRITE_BUDGET,
        notify: Arc::new(Mutex::new(None)),
    };
    assert_eq!(begin_confirmed(&request, now), ConfirmedStart::Send);
    assert_eq!(phase.load(Ordering::Acquire), CONFIRMED_IN_FLIGHT);
    generation.store(2, Ordering::Release);
    assert_eq!(
        phase.load(Ordering::Acquire),
        CONFIRMED_IN_FLIGHT,
        "a newer receipt cannot revoke a route call that already owns the mutation"
    );
    assert_eq!(
        begin_confirmed(&request, now),
        ConfirmedStart::Skip,
        "a second claimant must not duplicate the route call"
    );
}

#[test]
fn worker_preflight_settles_stale_and_expired_queued_work_without_sending() {
    let now = Instant::now();
    for (generation, newest, expires_at, expected) in [
        (
            1,
            2,
            now + CONFIRMED_WRITE_BUDGET,
            ConfirmedStart::FinishStale,
        ),
        (1, 1, now, ConfirmedStart::FinishTimedOut),
    ] {
        let (send, _result) = std::sync::mpsc::channel();
        let phase = Arc::new(AtomicU8::new(CONFIRMED_QUEUED));
        let request: ConfirmedRequest<GlobalSettings> = ConfirmedRequest {
            result: Some(send),
            generation,
            newest_generation: Arc::new(AtomicU64::new(newest)),
            phase: Arc::clone(&phase),
            expires_at,
            notify: Arc::new(Mutex::new(None)),
        };
        assert_eq!(begin_confirmed(&request, now), expected);
        assert_eq!(phase.load(Ordering::Acquire), CONFIRMED_CANCELLED);
    }
}

#[test]
fn a_disconnected_attempt_can_be_retried_on_a_fresh_link() {
    let mut first = GlobalLink::disabled();
    let mut failed = first.save_confirmed(GlobalPatch::default());
    assert_eq!(failed.poll(), Some(ConfirmedOutcome::Disconnected));

    let (mut restarted, commands) = test_link(GlobalSettings::default());
    let mut retried = restarted.save_confirmed(GlobalPatch::default());
    let mut request = take_request(&commands);
    assert!(request.settle(ConfirmedOutcome::Accepted(GlobalSettings::default())));
    assert!(matches!(
        retried.poll(),
        Some(ConfirmedOutcome::Accepted(_))
    ));
}

#[test]
fn success_requires_the_authority_to_echo_every_named_field() {
    let patch = GlobalPatch {
        start_on_login: Some(true),
        ..Default::default()
    };
    let mut echoed = GlobalSettings::default();
    echoed.start_on_login = true;
    assert!(matches!(
        successful_outcome(echoed, &patch, global_echoes),
        ConfirmedOutcome::Accepted(_)
    ));
    assert!(matches!(
        successful_outcome(GlobalSettings::default(), &patch, global_echoes),
        ConfirmedOutcome::ContractMismatch { .. }
    ));
}

#[test]
fn explicit_refusal_reconciles_with_a_read_but_unknown_transport_does_not() {
    let section = GlobalSettings::default();
    let refused = ConfirmedFailure::Refused {
        status: 409,
        code: "settings_write_failed".into(),
    };
    assert!(matches!(
        failed_outcome(refused.clone(), Some(section.clone())),
        ConfirmedOutcome::Reconciled {
            failure: ConfirmedFailure::Refused { .. },
            ..
        }
    ));
    assert!(matches!(
        failed_outcome(refused, None::<GlobalSettings>),
        ConfirmedOutcome::Refused { .. }
    ));
    assert_eq!(
        failed_outcome(ConfirmedFailure::Disconnected, Some(section)),
        ConfirmedOutcome::Disconnected,
        "a best-effort reread is not a causal barrier for an in-flight timed-out PATCH"
    );
    assert_eq!(
        failed_outcome(ConfirmedFailure::Disconnected, None::<GlobalSettings>),
        ConfirmedOutcome::Disconnected
    );
}

#[test]
fn settings_errors_keep_refusal_separate_from_unknown_outcome() {
    let refusal = super::super::SettingsError::Api(super::super::SettingsApiFailure {
        status: 409,
        code: "settings_write_failed".into(),
        message: String::new(),
    });
    assert!(matches!(
        confirmed_failure(&refusal),
        ConfirmedFailure::Refused { .. }
    ));
    let transport = super::super::SettingsError::Transport(koru_store_core::errors::Error::bare(
        koru_store_core::errors::Code::StorageFailed,
    ));
    assert_eq!(
        confirmed_failure(&transport),
        ConfirmedFailure::Disconnected
    );
    assert_eq!(
        confirmed_failure(&super::super::SettingsError::Local(String::new())),
        ConfirmedFailure::Disconnected
    );
}

#[test]
fn every_terminal_result_is_queued_before_the_worker_wakes_the_surface() {
    let (send, receive) = std::sync::mpsc::channel();
    let observed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = Arc::clone(&observed);
    let notify: Arc<Mutex<Option<Wake>>> = Arc::new(Mutex::new(Some(Box::new(move || {
        assert!(receive.try_recv().is_ok(), "result must precede wake");
        seen.fetch_add(1, Ordering::AcqRel);
    }))));
    let request = ConfirmedRequest {
        result: Some(send),
        generation: 1,
        newest_generation: Arc::new(AtomicU64::new(1)),
        phase: Arc::new(AtomicU8::new(CONFIRMED_IN_FLIGHT)),
        expires_at: Instant::now(),
        notify: Arc::clone(&notify),
    };
    finish_confirmed(
        Some(request),
        ConfirmedOutcome::<GlobalSettings>::Disconnected,
        true,
    );
    assert_eq!(
        observed.load(Ordering::Acquire),
        1,
        "normal settlement disarms Drop and wakes exactly once"
    );
}

#[test]
fn worker_loss_after_accepting_the_command_is_disconnected_before_echo() {
    let (mut link, commands) = test_link(GlobalSettings::default());
    let mut receipt = link.save_confirmed(GlobalPatch::default());
    drop(take_request(&commands));
    assert_eq!(receipt.poll(), Some(ConfirmedOutcome::Disconnected));
}

#[test]
fn worker_loss_queues_disconnected_before_waking_an_in_flight_receipt() {
    let (send, receive) = std::sync::mpsc::channel();
    let observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = Arc::clone(&observed);
    let notify: Arc<Mutex<Option<Wake>>> = Arc::new(Mutex::new(Some(Box::new(move || {
        seen.store(
            matches!(receive.try_recv(), Ok(ConfirmedOutcome::Disconnected)),
            Ordering::Release,
        );
    }))));
    let request: ConfirmedRequest<GlobalSettings> = ConfirmedRequest {
        result: Some(send),
        generation: 1,
        newest_generation: Arc::new(AtomicU64::new(1)),
        phase: Arc::new(AtomicU8::new(CONFIRMED_IN_FLIGHT)),
        expires_at: Instant::now(),
        notify,
    };
    drop(request);
    assert!(observed.load(Ordering::Acquire));
}

#[test]
fn failed_enable_and_disable_never_become_optimistic_authority() {
    let (mut off, commands) = test_link(GlobalSettings::default());
    let mut enabling = off.save_confirmed(GlobalPatch {
        start_on_login: Some(true),
        ..Default::default()
    });
    assert!(take_request(&commands).settle(ConfirmedOutcome::Refused {
        status: 409,
        code: String::new(),
    }));
    assert!(matches!(
        enabling.poll(),
        Some(ConfirmedOutcome::Refused { .. })
    ));
    assert!(!off.authoritative().start_on_login);

    let on = GlobalSettings {
        start_on_login: true,
        ..Default::default()
    };
    let (mut on, commands) = test_link(on);
    let mut disabling = on.save_confirmed(GlobalPatch {
        start_on_login: Some(false),
        ..Default::default()
    });
    assert!(take_request(&commands).settle(ConfirmedOutcome::Disconnected));
    assert_eq!(disabling.poll(), Some(ConfirmedOutcome::Disconnected));
    assert!(on.authoritative().start_on_login);
}

#[test]
fn a_correlated_patch_receipt_names_the_exact_exchange() {
    let (commands, take_command) = std::sync::mpsc::channel();
    let mut link = GlobalLink {
        updates: None,
        commands: Some(commands),
        authoritative: GlobalSettings::default(),
        notify: Arc::new(Mutex::new(None)),
        confirmed_generation: Arc::new(AtomicU64::new(0)),
    };
    let receipt = link.save_with_reply(
        GlobalPatch {
            theme: Some(ThemeChoice::Dark),
            ..GlobalPatch::default()
        },
        |patch, section| patch.clone().apply(section),
    );

    let Command::Patch {
        patch,
        confirmed: None,
        reply: Some(reply),
    } = take_command.recv().unwrap()
    else {
        panic!("the command carries its own reply channel");
    };
    assert_eq!(patch.theme, Some(ThemeChoice::Dark));
    let authoritative = link.authoritative().clone();
    reply
        .send(PatchReply::Applied(authoritative.clone()))
        .unwrap();
    assert_eq!(receipt.recv().unwrap(), PatchReply::Applied(authoritative));
}

#[test]
fn a_correlated_patch_refuses_immediately_when_the_link_is_unavailable() {
    let mut link = GlobalLink::disabled();
    let before = link.authoritative().clone();
    let receipt = link.save_with_reply(
        GlobalPatch {
            theme: Some(ThemeChoice::Dark),
            ..GlobalPatch::default()
        },
        |patch, section| patch.clone().apply(section),
    );

    assert_eq!(link.authoritative().theme, ThemeChoice::Dark);
    assert_eq!(
        receipt.recv().unwrap(),
        PatchReply::Refused {
            authoritative: Some(before),
            message: "the settings link is unavailable".into(),
        }
    );
}

/// The Terminal section takes the same shape — one worker, one contract. A
/// window that could only link to Global would have meant a second copy of all
/// of this for the terminal.
#[test]
fn the_terminal_section_links_the_same_way() {
    let mut link = TerminalLink::disabled();
    assert_eq!(link.authoritative(), &TerminalSettings::default());
    link.save(
        TerminalPatch {
            composer_default: Some(false),
            ..TerminalPatch::default()
        },
        |patch, section| patch.clone().apply(section),
    );
    assert!(!link.authoritative().composer_default);
}

/// The link names every scope it uses, because naming any scope *filters*
/// rather than adds — a link that asked only for `settings.write` could not
/// read the section it was about to patch, nor hear that somebody else had.
#[test]
fn the_link_names_all_three_scopes_it_needs() {
    assert_eq!(
        LINK_SCOPES,
        [
            koru_settings::SETTINGS_READ_SCOPE,
            koru_settings::SETTINGS_WRITE_SCOPE,
            crate::EVENTS_READ_SCOPE,
        ]
    );
}

/// The connect budget is deliberately longer than the section's own wait, and
/// the two answer different questions: one is "draw now", the other is "the
/// Agent may still be starting the Gateway". Pinned so a later edit that
/// collapsed them into one number has to say why.
#[test]
fn the_connect_budget_outlasts_the_sections_own_wait() {
    let section_wait = std::time::Duration::from_millis(koru_settings::STARTUP_WAIT_MILLIS);
    assert!(CONNECT_BUDGET > section_wait);
    assert!(POLL < section_wait);
}

/// **A healthy link has no clock.** It blocks on its channel: an idle window
/// costs one parked thread and no round trips, which is the rule the whole
/// no-poller design rests on.
#[test]
fn a_link_in_touch_blocks_rather_than_ticking() {
    assert_eq!(Touch::In.wait(Instant::now()), None);
}

/// **Losing touch arms a burst, and the burst ends.**
///
/// A Gateway that went away is worth asking after; a Gateway that is never
/// coming back must not leave a thread waking forever. After the budget the
/// link goes quiet again and the change subscription — whose own thread
/// reconnects for as long as it lives — is what carries the next real change.
#[test]
fn a_link_that_lost_touch_retries_for_a_bounded_while_and_then_goes_quiet() {
    let lost_at = Instant::now();
    let touch = Touch::Lost(lost_at);
    assert_eq!(touch.wait(lost_at), Some(HEAL_RETRY));
    assert_eq!(touch.wait(lost_at + HEAL_BUDGET / 2), Some(HEAL_RETRY));
    assert_eq!(
        touch.wait(lost_at + HEAL_BUDGET),
        None,
        "the burst is over, and a thread that keeps waking is a poller"
    );
    assert!(
        HEAL_RETRY < HEAL_BUDGET,
        "a budget shorter than its own retry would ask exactly once"
    );
}

/// **The budget runs from when touch was lost, not from the latest attempt.**
/// Restamping it on every failed retry would push the end out forever, which is
/// how a bounded burst quietly becomes the clock it was written not to be.
#[test]
fn a_failed_retry_does_not_extend_the_burst() {
    let lost_at = Instant::now();
    let mut touch = Touch::In;
    touch.lose(lost_at);
    assert_eq!(touch, Touch::Lost(lost_at));
    touch.lose(lost_at + HEAL_RETRY);
    assert_eq!(touch, Touch::Lost(lost_at), "still the first failure");
}

/// A read that answers is what puts a link back in touch, and the same read is
/// what the caller applies — so the reconnect and the change fact take exactly
/// one path into the window, and a healed link goes back to blocking.
#[test]
fn coming_back_into_touch_stops_the_burst() {
    let mut touch = Touch::In;
    touch.lose(Instant::now());
    assert!(touch.wait(Instant::now()).is_some());
    touch = Touch::In;
    assert_eq!(touch.wait(Instant::now()), None);
}
