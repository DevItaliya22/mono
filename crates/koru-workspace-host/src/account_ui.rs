// SPDX-License-Identifier: MPL-2.0

//! Pure projection of Agent account state into approved native surfaces.
//!
//! Views receive this state from the shell; they never branch on backend prose
//! and never perform browser, network, keyring, or Store work themselves.

use koru_agent_wire::{AccountStage, AccountState, EnrollmentState};
pub mod switcher;

/// Local UI correlation, deliberately independent of Agent generations and IPC IDs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandSubmissionId(u64);

impl CommandSubmissionId {
    pub fn next() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandFailure {
    pub submission: CommandSubmissionId,
    /// The request may have reached the worker. This is never a terminal result.
    pub uncertain: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandAdmission {
    pub submission: CommandSubmissionId,
    pub owner_scope: u64,
    pub generation: u64,
}

pub fn command_failure_surface() -> Surface {
    unavailable()
}
pub fn profile_connection_pending(lang: koru_i18n::Lang) -> Surface {
    Surface::new(
        SurfaceKind::Unavailable,
        koru_i18n::t(lang, koru_i18n::Key::AccountProfileSavedTitle),
        koru_i18n::t(lang, koru_i18n::Key::AccountProfileConnectionBody),
        vec![Action::Retry, Action::ContinueOffline],
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    ConfirmAccountCreation,
    CancelAccountCreation,
    SignInGoogle,
    SignInAnotherGoogle,
    ChooseProfile,
    Retry,
    RetryEnrollment,
    ContactSupport,
    ContinueOffline,
    Cancel,
    CancelEnrollment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceKind {
    AccountCreation,
    WaitingForDevice,
    WaitingForBrowser,
    SignedOut,
    Finishing,
    SignedIn,
    /// The build has no Koru Cloud (`AccountStage::NotConfigured`). Says so
    /// without alarm and offers nothing to retry: there is no server.
    NotConfigured,
    Unavailable,
    AccessRevoked,
    Reauthenticate,
    DeviceVerification,
    Persistence,
    Collision,
    ErasedAccount,
    OlderPeer,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Surface {
    /// Stable scenario identity. Labels and copy are presentation only and must
    /// never be used to choose an operation.
    pub kind: SurfaceKind,
    pub title: &'static str,
    pub body: String,
    pub status: Option<&'static str>,
    pub actions: Vec<Action>,
    pub automatic_retry: bool,
    pub terminal: bool,
}

impl Surface {
    fn new(
        kind: SurfaceKind,
        title: &'static str,
        body: impl Into<String>,
        actions: Vec<Action>,
    ) -> Self {
        Self {
            kind,
            title,
            body: body.into(),
            status: None,
            actions,
            automatic_retry: false,
            terminal: false,
        }
    }
}

/// Project stable state/failure codes into person-facing copy.
///
/// `profile` is local display metadata. Backend error prose is intentionally
/// absent from this API so it cannot leak into the surface by accident.
pub fn project(state: &AccountState, profile: Option<&str>) -> Surface {
    project_for_lang(state, profile, koru_i18n::Lang::En)
}

/// The codes that mean a stored credential, not the account, needs repair.
pub fn is_credential_repair_code(code: &str) -> bool {
    matches!(
        code,
        "account_credential_missing" | "account_credential_rejected" | "account_credential_invalid"
    )
}

/// The failure code the surface will act on, if any.
///
/// A credential-repair code beside a signed-in stage is the directory
/// remembering a credential that was rejected before the sign-in replaced it.
/// The surface never lets it displace a signed-in account, so a caller
/// deciding whether to show the surface at all must not count it either:
/// counting it produced a bare *Signed in* card with no actions in place of
/// a Profile's detail pane.
pub fn actionable_failure_code(state: &AccountState) -> Option<&str> {
    let code = state.failure_code.as_deref()?;
    (!(is_credential_repair_code(code) && state.stage == AccountStage::SignedIn)).then_some(code)
}

/// A Profile surface must never borrow another member's failure or deadline.
/// The unchanged state still carries every row for account-level inspection.
pub fn state_for_profile(state: &AccountState, profile_id: Option<&str>) -> AccountState {
    let mut projected = state.clone();
    if let Some(profile_id) = profile_id {
        if state
            .cloud_profiles
            .iter()
            .any(|row| row.profile_id == profile_id)
        {
            projected.enrollment = EnrollmentState::Idle;
            projected.enrollment_failure_code = None;
            projected.enrollment_retry_at_millis = None;
            return projected;
        }
        let row = state
            .enrollments
            .iter()
            .find(|row| row.profile_id == profile_id);
        projected.enrollment = row.map_or(EnrollmentState::Idle, |row| row.state);
        projected.enrollment_failure_code = row.and_then(|row| row.failure_code.clone());
        projected.enrollment_retry_at_millis = row.and_then(|row| row.retry_at_millis);
    }
    projected
}

/// The active scope's status can already be newer than a delayed selection ACK.
/// Parked scopes have no current status and retain their targeted result directly.
pub fn selection_result_rows(
    state: &AccountState,
    generation: u64,
    rows: Vec<koru_agent_wire::AccountProfileEnrollment>,
) -> Vec<koru_agent_wire::AccountProfileEnrollment> {
    if state.generation >= generation {
        state.enrollments.clone()
    } else {
        rows
    }
}

pub fn project_for_lang(
    state: &AccountState,
    profile: Option<&str>,
    lang: koru_i18n::Lang,
) -> Surface {
    use koru_i18n::{Key, t};
    if state.stage == AccountStage::NotConfigured {
        return not_configured(lang);
    }
    if state.failure_code.as_deref() == Some("account_repair_identity_mismatch") {
        let mut surface = Surface::new(
            SurfaceKind::Unavailable,
            t(lang, Key::AccountCreationExpiredTitle),
            t(lang, Key::AccountRepairIdentityMismatchBody),
            vec![Action::ContinueOffline],
        );
        surface.terminal = true;
        return surface;
    }
    if state.failure_code.as_deref() == Some("profile_lifecycle_owner_unproven") {
        let mut surface = Surface::new(
            SurfaceKind::Unavailable,
            t(lang, Key::AccountRemovalPausedTitle),
            t(lang, Key::AccountRemovalOwnerUnprovenBody),
            vec![Action::ContinueOffline],
        );
        surface.terminal = true;
        return surface;
    }
    let credential_repair = state
        .failure_code
        .as_deref()
        .is_some_and(is_credential_repair_code);
    if credential_repair
        && state.stage == AccountStage::Failed
        && state.capability
        && state.profile_count == 0
        && state.queue_depth == 0
    {
        return Surface::new(
            SurfaceKind::Reauthenticate,
            t(lang, Key::AccountCreationExpiredTitle),
            t(lang, Key::AccountCredentialRepairBody),
            vec![Action::SignInGoogle, Action::ContinueOffline],
        );
    }
    if state.stage == AccountStage::AwaitingAccountCreation {
        return Surface::new(
            SurfaceKind::AccountCreation,
            t(lang, Key::AccountCreationTitle),
            t(lang, Key::AccountCreationBody),
            vec![
                Action::ConfirmAccountCreation,
                Action::CancelAccountCreation,
                Action::ContinueOffline,
            ],
        );
    }
    if state.failure_code.as_deref() == Some("account_confirmation_expired") {
        return Surface::new(
            SurfaceKind::Reauthenticate,
            t(lang, Key::AccountCreationExpiredTitle),
            t(lang, Key::AccountCreationExpiredBody),
            vec![Action::SignInGoogle, Action::ContinueOffline],
        );
    }
    let profile = profile.unwrap_or("this Profile");
    let enrollment_code = state.enrollment_failure_code.as_deref();
    match state.enrollment {
        EnrollmentState::Waiting if enrollment_code == Some("older_peer") => {
            return failure("older_peer", profile, true);
        }
        EnrollmentState::Waiting
            if enrollment_code.is_none() || enrollment_code == Some("no_peer_online") =>
        {
            let mut surface = Surface::new(
                SurfaceKind::WaitingForDevice,
                "Waiting for another Koru device",
                format!(
                    "Open Koru on a device that already has {profile}. This device will connect automatically when one is available."
                ),
                vec![Action::ContinueOffline, Action::CancelEnrollment],
            );
            surface.status = Some("Looking for a trusted device…");
            surface.automatic_retry = true;
            return surface;
        }
        EnrollmentState::Waiting => {
            return enrollment_code
                .map(|code| failure(code, profile, true))
                .unwrap_or_else(unavailable);
        }
        EnrollmentState::Enrolling => {
            let mut surface = Surface::new(
                SurfaceKind::Finishing,
                "Connecting this device…",
                format!("Koru is securely connecting this device to {profile}."),
                vec![Action::ContinueOffline, Action::CancelEnrollment],
            );
            surface.status = Some("Connecting…");
            return surface;
        }
        EnrollmentState::Failed => {
            return enrollment_code
                .map(|code| failure(code, profile, true))
                .unwrap_or_else(unavailable);
        }
        EnrollmentState::Complete if enrollment_code == Some("enrollment_provisioning_pending") => {
            let mut surface = Surface::new(
                SurfaceKind::Finishing,
                "Device connected",
                "Your device connection is saved. Koru will finish updating account discovery automatically.",
                vec![Action::ContinueOffline],
            );
            surface.status = Some("Finishing account setup…");
            surface.automatic_retry = true;
            return surface;
        }
        EnrollmentState::Idle | EnrollmentState::Complete => {}
    }

    if state.stage == AccountStage::WaitingForBrowser {
        let mut surface = Surface::new(
            SurfaceKind::WaitingForBrowser,
            "Finish signing in in your browser",
            "Koru opened your system browser. Finish Google sign-in there, then return here.",
            vec![Action::Cancel],
        );
        surface.status = Some("Waiting for Google…");
        return surface;
    }

    if let Some(code) = actionable_failure_code(state) {
        return failure(code, profile, false);
    }

    match state.stage {
        AccountStage::NotConfigured => not_configured(lang),
        AccountStage::Unavailable | AccountStage::Failed => unavailable(),
        // The email journey draws its own code and password screens; this
        // surface describes the account, and there is not one yet.
        AccountStage::AwaitingEmailCode | AccountStage::AwaitingEmailPassword => Surface::new(
            SurfaceKind::Finishing,
            "Signing in…",
            "Finish signing in with your email address.",
            Vec::new(),
        ),
        AccountStage::SignedOut | AccountStage::Cancelled => Surface::new(
            SurfaceKind::SignedOut,
            "Not signed in",
            "Your local Profiles and content remain available offline.",
            vec![Action::SignInGoogle],
        ),
        AccountStage::Restoring | AccountStage::Exchanging | AccountStage::Refreshing => {
            let mut surface = Surface::new(
                SurfaceKind::Finishing,
                "Finishing…",
                "Koru is finishing the account operation safely.",
                Vec::new(),
            );
            surface.status = Some("Finishing…");
            surface
        }
        AccountStage::SignedIn => Surface::new(
            SurfaceKind::SignedIn,
            "Signed in",
            "Your Koru account helps connect devices. Profile content stays on your devices.",
            Vec::new(),
        ),
        AccountStage::WaitingForBrowser | AccountStage::AwaitingAccountCreation => {
            unreachable!("handled above")
        }
    }
}
/// A build with no Koru Cloud: what is off and that local work is not, and
/// no *Try again* — a retry would only ask again for a server that does not
/// exist. *Continue offline* is the one way on.
pub fn not_configured(lang: koru_i18n::Lang) -> Surface {
    use koru_i18n::{Key, t};
    Surface::new(
        SurfaceKind::NotConfigured,
        t(lang, Key::CloudNotConfigured),
        t(lang, Key::CloudNotConfiguredBody),
        vec![Action::ContinueOffline],
    )
}

fn unavailable() -> Surface {
    Surface::new(
        SurfaceKind::Unavailable,
        "Koru can't connect right now",
        "Your local Profiles and content are still available. You can try again without losing your place.",
        vec![Action::Retry, Action::ContinueOffline],
    )
}

fn failure(code: &str, profile: &str, enrollment: bool) -> Surface {
    match code {
        "profile_access_revoked" | "profile_not_authorized" => {
            let mut surface = Surface::new(
                SurfaceKind::AccessRevoked,
                "You no longer have access to this Profile",
                format!(
                    "The Google account signed in on this device is no longer authorized for {profile}. Profiles already stored on this device remain available."
                ),
                vec![
                    Action::SignInAnotherGoogle,
                    Action::ChooseProfile,
                    Action::ContinueOffline,
                ],
            );
            surface.terminal = true;
            surface
        }
        "session_revoked" | "authentication_required" | "callback_expired" => Surface::new(
            SurfaceKind::Reauthenticate,
            "Sign in again to continue",
            format!(
                "Koru needs a new sign-in to continue connecting {profile}. Your local Profile and content remain available."
            ),
            vec![
                Action::SignInGoogle,
                Action::ContinueOffline,
                if enrollment {
                    Action::CancelEnrollment
                } else {
                    Action::Cancel
                },
            ],
        ),
        "device_verification_failed" | "attestation_invalid" => Surface::new(
            SurfaceKind::DeviceVerification,
            "Koru couldn't verify this device",
            "No connection was made. Try again to create a new verification for this device.",
            vec![
                if enrollment {
                    Action::RetryEnrollment
                } else {
                    Action::Retry
                },
                Action::ContinueOffline,
                Action::CancelEnrollment,
            ],
        ),
        "connection_save_failed" | "trust_persistence_failed" => Surface::new(
            SurfaceKind::Persistence,
            "Koru couldn't finish connecting this device",
            "Koru couldn't save this connection safely. Your Profiles and content on this device are unchanged.",
            vec![
                if enrollment {
                    Action::RetryEnrollment
                } else {
                    Action::Retry
                },
                Action::ContinueOffline,
                Action::CancelEnrollment,
            ],
        ),
        "collision" | "collision_required" => Surface::new(
            SurfaceKind::Collision,
            "This email is connected to another Koru account",
            "Sign in with the Google Account already connected to this Koru account to prove it is yours. Koru will never merge accounts from an email match.",
            vec![
                Action::SignInGoogle,
                Action::SignInAnotherGoogle,
                Action::Cancel,
            ],
        ),
        "account_erased" | "identity_erased" => Surface::new(
            SurfaceKind::ErasedAccount,
            "This Koru account was deleted",
            "Koru no longer has this account, but your Profiles and content remain on this device. Reconnecting creates a new Koru account; it does not delete or upload your local content.",
            vec![Action::SignInGoogle, Action::ContinueOffline],
        ),
        "account_unavailable" => {
            let mut surface = unavailable();
            surface.status = Some("Trying again automatically…");
            surface.automatic_retry = true;
            surface
        }
        "account_retry_exhausted" => {
            let mut surface = Surface::new(
                SurfaceKind::Unavailable,
                "Koru still can't connect",
                "Koru has tried for 10 minutes. Please try again later or contact support. Your local Profiles and content remain available.",
                vec![
                    Action::Retry,
                    Action::ContactSupport,
                    Action::ContinueOffline,
                ],
            );
            surface.terminal = true;
            surface
        }
        "enrollment_unavailable" => {
            let mut surface = Surface::new(
                SurfaceKind::Unavailable,
                "Koru can't connect right now",
                "Koru can't reach the account service needed to find your other devices. Your local Profiles and content remain available.",
                vec![Action::ContinueOffline, Action::CancelEnrollment],
            );
            surface.status = Some("Trying again…");
            surface.automatic_retry = true;
            surface
        }
        "older_peer" | "enrollment_protocol_unsupported" => {
            let mut surface = Surface::new(
                SurfaceKind::OlderPeer,
                "Update Koru on your other device",
                "The online device can't approve this one until Koru is updated there. This device will retry automatically.",
                vec![Action::ContinueOffline, Action::CancelEnrollment],
            );
            surface.automatic_retry = true;
            surface
        }
        "enrollment_rate_limited" => {
            let mut surface = Surface::new(
                SurfaceKind::Unavailable,
                "Waiting to connect this Profile",
                "Koru is temporarily limiting connection requests. We'll retry automatically. Your local Profiles and content remain available.",
                vec![Action::ContinueOffline, Action::CancelEnrollment],
            );
            surface.automatic_retry = true;
            surface
        }
        _ => unavailable(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_without_koru_cloud_is_not_an_outage_and_offers_no_retry() {
        let state = AccountState::not_configured();
        for lang in koru_i18n::Lang::ALL {
            let surface = project_for_lang(&state, Some("Work"), lang);
            assert_eq!(surface.kind, SurfaceKind::NotConfigured);
            assert_eq!(
                surface.title,
                koru_i18n::t(lang, koru_i18n::Key::CloudNotConfigured)
            );
            assert_eq!(surface.actions, vec![Action::ContinueOffline]);
            assert!(!surface.automatic_retry);
            assert!(!surface.terminal);
            assert_eq!(surface.status, None);
        }
        // Not the can't-connect surface it used to fall through to.
        assert_ne!(project(&state, None).title, unavailable().title);
        assert!(unavailable().actions.contains(&Action::Retry));
    }

    #[test]
    fn profile_projection_never_borrows_a_siblings_failure_or_retry() {
        let a = "10000000-0000-4000-8000-000000000001";
        let b = "10000000-0000-4000-8000-000000000002";
        let state = AccountState {
            generation: 7,
            enrollment: EnrollmentState::Failed,
            enrollment_failure_code: Some("enrollment_unavailable".into()),
            enrollment_retry_at_millis: Some(99),
            enrollments: vec![
                koru_agent_wire::AccountProfileEnrollment {
                    profile_id: a.into(),
                    state: EnrollmentState::Failed,
                    failure_code: Some("no_peer".into()),
                    retry_at_millis: Some(77),
                },
                koru_agent_wire::AccountProfileEnrollment {
                    profile_id: b.into(),
                    state: EnrollmentState::Complete,
                    failure_code: None,
                    retry_at_millis: None,
                },
            ],
            ..Default::default()
        };
        let completed = state_for_profile(&state, Some(b));
        assert_eq!(completed.enrollment, EnrollmentState::Complete);
        assert_eq!(completed.enrollment_failure_code, None);
        assert_eq!(completed.enrollment_retry_at_millis, None);
        assert_eq!(completed.enrollments, state.enrollments);
        let absent = state_for_profile(&state, Some("10000000-0000-4000-8000-000000000003"));
        assert_eq!(absent.enrollment, EnrollmentState::Idle);
        assert_eq!(absent.enrollment_failure_code, None);
        let waiting = state_for_profile(&state, Some(a));
        assert_eq!(waiting.enrollment_retry_at_millis, Some(77));
        assert_eq!(waiting.enrollment_failure_code.as_deref(), Some("no_peer"));
        assert_eq!(selection_result_rows(&state, 7, vec![]), state.enrollments);
        assert!(selection_result_rows(&state, 8, vec![]).is_empty());
    }

    #[test]
    fn repair_identity_mismatch_keeps_offline_entry_without_retry_in_every_locale() {
        for lang in koru_i18n::Lang::ALL {
            let surface = project_for_lang(&failed("account_repair_identity_mismatch"), None, lang);
            assert_eq!(surface.actions, vec![Action::ContinueOffline]);
            assert!(surface.terminal);
            assert!(!surface.automatic_retry);
            assert_eq!(
                surface.body,
                koru_i18n::t(lang, koru_i18n::Key::AccountRepairIdentityMismatchBody)
            );
        }
    }

    #[test]
    fn unknown_removal_owner_has_no_retry_in_any_locale_and_does_not_change_session() {
        for lang in koru_i18n::Lang::ALL {
            let state = AccountState {
                stage: AccountStage::SignedIn,
                failure_code: Some("profile_lifecycle_owner_unproven".into()),
                ..Default::default()
            };
            let surface = project_for_lang(&state, None, lang);
            assert_eq!(surface.actions, vec![Action::ContinueOffline]);
            assert!(surface.terminal);
            assert_eq!(
                surface.body,
                koru_i18n::t(lang, koru_i18n::Key::AccountRemovalOwnerUnprovenBody)
            );
            assert_eq!(state.stage, AccountStage::SignedIn);
        }
    }

    fn failed(code: &str) -> AccountState {
        AccountState {
            stage: AccountStage::Failed,
            capability: true,
            failure_code: Some(code.into()),
            ..AccountState::default()
        }
    }

    #[test]
    fn revoked_access_is_terminal_and_never_retries() {
        let surface = project(&failed("profile_access_revoked"), Some("Research"));
        assert!(surface.terminal);
        assert!(!surface.automatic_retry);
        assert_eq!(surface.actions[0], Action::SignInAnotherGoogle);
        assert!(surface.body.contains("Research"));
    }

    #[test]
    fn throttled_enrollment_explains_waiting_without_offering_early_retry() {
        let surface = project(&failed("enrollment_rate_limited"), Some("Research"));
        assert!(surface.automatic_retry);
        assert!(!surface.terminal);
        assert!(!surface.actions.contains(&Action::Retry));
        assert!(surface.body.contains("limiting connection requests"));
    }

    #[test]
    fn waiting_is_automatic_and_has_no_manual_retry() {
        let state = AccountState {
            enrollment: EnrollmentState::Waiting,
            ..AccountState::default()
        };
        let surface = project(&state, Some("Personal"));
        assert!(surface.automatic_retry);
        assert!(!surface.actions.contains(&Action::Retry));
        assert_eq!(surface.status, Some("Looking for a trusted device…"));
    }

    #[test]
    fn browser_wait_can_cancel_but_not_duplicate_submission() {
        let state = AccountState {
            stage: AccountStage::WaitingForBrowser,
            capability: true,
            ..AccountState::default()
        };
        let surface = project(&state, None);
        assert_eq!(surface.actions, vec![Action::Cancel]);
        assert_eq!(surface.status, Some("Waiting for Google…"));
    }

    #[test]
    fn unknown_backend_prose_never_reaches_the_surface() {
        let surface = project(&failed("secret_backend_detail"), None);
        assert_eq!(surface.title, "Koru can't connect right now");
        assert!(!surface.body.contains("secret_backend_detail"));
    }

    #[test]
    fn conclusive_credential_repair_offers_localized_sign_in_without_replacing_a_signed_in_account()
    {
        for code in [
            "account_credential_missing",
            "account_credential_rejected",
            "account_credential_invalid",
        ] {
            let state = failed(code);
            let surface = project_for_lang(&state, None, koru_i18n::Lang::Es);
            assert_eq!(surface.kind, SurfaceKind::Reauthenticate);
            assert_eq!(
                surface.actions,
                vec![Action::SignInGoogle, Action::ContinueOffline]
            );
            assert_eq!(
                surface.body,
                koru_i18n::t(
                    koru_i18n::Lang::Es,
                    koru_i18n::Key::AccountCredentialRepairBody
                )
            );
            assert_eq!(
                project(
                    &AccountState {
                        stage: AccountStage::SignedIn,
                        profile_count: 1,
                        ..state.clone()
                    },
                    None
                )
                .kind,
                SurfaceKind::SignedIn
            );
            for protected in [
                AccountState {
                    queue_depth: 1,
                    ..state.clone()
                },
                AccountState {
                    profile_count: 1,
                    ..state.clone()
                },
                AccountState {
                    capability: false,
                    ..state
                },
            ] {
                assert!(
                    !project(&protected, None)
                        .actions
                        .contains(&Action::SignInGoogle)
                );
            }
        }
    }

    #[test]
    fn vault_outages_and_pending_credential_failures_keep_retry_without_new_sign_in() {
        for code in [
            "credential_unavailable",
            "credential_migration_pending",
            "credential_missing",
            "credential_rejected",
            "credential_invalid",
        ] {
            let surface = project(&failed(code), None);
            assert!(surface.actions.contains(&Action::Retry));
            assert!(!surface.actions.contains(&Action::SignInGoogle));
        }
    }

    #[test]
    fn collision_and_erased_account_have_stable_distinct_scenario_identity() {
        let collision = project(&failed("collision"), None);
        let erased = project(&failed("account_erased"), None);
        assert_eq!(collision.kind, SurfaceKind::Collision);
        assert_eq!(erased.kind, SurfaceKind::ErasedAccount);
    }
    #[test]
    fn enrollment_persistence_retry_is_not_hosted_account_retry() {
        let state = AccountState {
            enrollment: EnrollmentState::Failed,
            enrollment_failure_code: Some("trust_persistence_failed".into()),
            ..AccountState::default()
        };
        let surface = project(&state, Some("Research"));
        assert_eq!(surface.actions[0], Action::RetryEnrollment);
        assert!(!surface.actions.contains(&Action::Retry));
    }

    #[test]
    fn hosted_enrollment_outage_does_not_claim_no_peer_is_online() {
        let state = AccountState {
            enrollment: EnrollmentState::Waiting,
            enrollment_failure_code: Some("enrollment_unavailable".into()),
            ..AccountState::default()
        };
        let surface = project(&state, Some("Research"));
        assert_eq!(surface.kind, SurfaceKind::Unavailable);
        assert!(surface.automatic_retry);
        assert!(!surface.body.contains("Open Koru on a device"));
        assert!(surface.actions.contains(&Action::CancelEnrollment));
    }

    #[test]
    fn hosted_outage_stays_on_one_automatic_recovery_surface() {
        let surface = project(&failed("account_unavailable"), None);
        assert_eq!(surface.kind, SurfaceKind::Unavailable);
        assert!(surface.automatic_retry);
        assert_eq!(surface.status, Some("Trying again automatically…"));
        assert_eq!(surface.actions[0], Action::Retry);
    }

    #[test]
    fn exhausted_recovery_offers_retry_support_and_offline_work() {
        let surface = project(&failed("account_retry_exhausted"), None);
        assert!(surface.terminal);
        assert!(!surface.automatic_retry);
        assert_eq!(
            surface.actions,
            vec![
                Action::Retry,
                Action::ContactSupport,
                Action::ContinueOffline
            ]
        );
        assert!(surface.body.contains("10 minutes"));
    }
}
