// SPDX-License-Identifier: MPL-2.0

//! Account-scoped Koru Cloud state: the subscription, each Profile's sync
//! switch, and the D258 question about a Profile's Workspaces. No layout, no
//! credentials and no Stripe IDs; the one I/O is the device-local transport
//! write, on a worker thread.
//!
//! Two surfaces live here because they are one fact seen twice: the card says
//! whether this account has Cloud at all, and the switch on each Profile row
//! says whether this device syncs that Profile. The card's "syncing n of m
//! profiles" is counted from the very switches below it, which is why the
//! subscription read and the per-Profile read share a state struct rather than
//! sitting in two modules that would have to agree.
use std::sync::mpsc::{self, Receiver};

use super::*;
use crate::mainview::cloud_sync::Connection;
use koru_agent_wire::CloudSyncProfile;
use koru_i18n::{Key as TextKey, Lang};
use koru_workspace_host::account_ui::{CommandAdmission, CommandFailure, CommandSubmissionId};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Unknown,
    Loading,
    Subscribe,
    Manage,
    Unavailable,
}
pub(super) struct Pending {
    id: u64,
    submission: CommandSubmissionId,
    generation: Option<u64>,
}
/// An `InspectCloudSync` or a `SetProfileCloudSync`. **Both are answered by
/// `CloudSync` carrying the same `request_id`**, so one slot correlates both and
/// only one is ever in flight; `set` is what tells the result which of the two
/// it was finishing.
pub(super) struct SyncPending {
    id: u64,
    submission: CommandSubmissionId,
    generation: Option<u64>,
    set: Option<(String, bool)>,
}
/// What the Store said about a transport-mode write, off the winit thread.
#[derive(Debug)]
pub(super) enum TransportAnswer {
    Settled(Box<koru_store_wire::ProfileTransportModeOutcome>),
    Refused(String),
}

pub(super) struct State {
    pub(super) status: Status,
    due: bool,
    pending: Option<Pending>,
    /// Per-Profile Cloud sync as the Agent last reported it. `None` until the
    /// first answer arrives: **unknown is not the same as "nothing is on"**, and
    /// a card that counted `None` as zero would say "syncing 0 of 3" about a
    /// device that is syncing all three.
    pub(super) sync: Option<Vec<CloudSyncProfile>>,
    sync_due: bool,
    pub(super) sync_pending: Option<SyncPending>,
    /// A switch that has been pressed and not yet drained onto the wire. Held
    /// separately so the row can show the pending state on the very next frame
    /// rather than after the command lane gets a turn.
    pub(super) set_request: Option<(String, bool)>,
    /// **D258's prompt**, while it is up: *and its Workspaces?*
    ///
    /// Raised by turning a Profile's Cloud sync **on**, and by nothing else.
    /// Turning it off is D227's immediate stop and already has its behaviour;
    /// a second dialog there would be a second answer to a settled question.
    pub(super) move_prompt: Option<(ProfileId, Confirm)>,
    /// An answer given and not yet put on the wire. Held separately for the
    /// reason `set_request` beside it is: the frame that answered should not
    /// be the frame that blocks, and a test should be able to read what was
    /// asked for without a Store to answer it.
    pub(super) move_request: Option<(ProfileId, bool)>,
    /// A `SetTransportMode` group in flight. The Store's answer is the truth,
    /// so only one is ever outstanding.
    transport: Option<Receiver<TransportAnswer>>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            status: Status::Unknown,
            due: true,
            pending: None,
            sync: None,
            sync_due: true,
            sync_pending: None,
            set_request: None,
            move_prompt: None,
            move_request: None,
            transport: None,
        }
    }
}
impl State {
    pub fn invalidate(&mut self) {
        *self = Self::default();
    }
}

impl ProfilesController {
    /// What the account's subscription is, as far as this form knows.
    pub fn cloud_status(&self) -> Status {
        self.cloud.status
    }
    pub fn set_cloud_visible(&mut self, visible: bool) {
        if visible && !self.cloud_visible {
            self.refresh_cloud();
        }
        self.cloud_visible = visible;
    }
    pub fn refresh_cloud(&mut self) {
        if self.cloud.pending.is_none() {
            self.cloud.due = true;
            self.cloud.status = Status::Unknown;
        }
        if self.cloud.sync_pending.is_none() {
            self.cloud.sync_due = true;
        }
    }
    fn cloud_readable(&self) -> bool {
        self.cloud_visible
            && self.account.stage == AccountStage::SignedIn
            && self.account.active_account_id.is_some()
    }
    pub(super) fn take_cloud_request(&mut self) -> Option<AccountCommand> {
        if !self.cloud_readable() {
            return None;
        }
        if self.cloud.due && self.cloud.pending.is_none() {
            let id = next_request_id();
            self.cloud.due = false;
            self.cloud.status = Status::Loading;
            self.cloud.pending = Some(Pending {
                id,
                submission: CommandSubmissionId::next(),
                generation: None,
            });
            return Some(AccountCommand::InspectCloudSubscription { request_id: id });
        }
        if self.cloud.sync_pending.is_some() {
            return None;
        }
        // A write the person has already made is more urgent than a re-read, and
        // a re-read issued first would answer with the state they just changed.
        if let Some((profile_id, enabled)) = self.cloud.set_request.take() {
            let id = next_request_id();
            self.cloud.sync_pending = Some(SyncPending {
                id,
                submission: CommandSubmissionId::next(),
                generation: None,
                set: Some((profile_id.clone(), enabled)),
            });
            return Some(AccountCommand::SetProfileCloudSync {
                request_id: id,
                profile_id,
                enabled,
            });
        }
        // Nothing can be synced without a subscription, so the per-Profile read
        // waits for the subscription answer rather than racing it.
        if self.cloud.sync_due && self.cloud.status == Status::Manage {
            let id = next_request_id();
            self.cloud.sync_due = false;
            self.cloud.sync_pending = Some(SyncPending {
                id,
                submission: CommandSubmissionId::next(),
                generation: None,
                set: None,
            });
            return Some(AccountCommand::InspectCloudSync { request_id: id });
        }
        None
    }
    pub(super) fn cloud_submission(&self, command: &AccountCommand) -> Option<CommandSubmissionId> {
        match command {
            AccountCommand::InspectCloudSubscription { request_id } => self
                .cloud
                .pending
                .as_ref()
                .filter(|p| p.id == *request_id)
                .map(|p| p.submission),
            AccountCommand::InspectCloudSync { request_id }
            | AccountCommand::SetProfileCloudSync { request_id, .. } => self
                .cloud
                .sync_pending
                .as_ref()
                .filter(|p| p.id == *request_id)
                .map(|p| p.submission),
            _ => None,
        }
    }
    pub(super) fn cloud_admitted(&mut self, admission: CommandAdmission) -> bool {
        if admission.owner_scope != self.account.active_scope {
            return false;
        }
        if let Some(pending) = self
            .cloud
            .pending
            .as_mut()
            .filter(|p| p.submission == admission.submission)
        {
            pending.generation = Some(admission.generation);
            return true;
        }
        if let Some(pending) = self
            .cloud
            .sync_pending
            .as_mut()
            .filter(|p| p.submission == admission.submission)
        {
            pending.generation = Some(admission.generation);
            return true;
        }
        false
    }
    pub(super) fn cloud_failed(&mut self, failure: CommandFailure) -> bool {
        if self
            .cloud
            .pending
            .as_ref()
            .is_some_and(|p| p.submission == failure.submission)
        {
            self.cloud.pending = None;
            self.cloud.status = Status::Unavailable;
            return true;
        }
        let Some(pending) = self
            .cloud
            .sync_pending
            .take_if(|p| p.submission == failure.submission)
        else {
            return false;
        };
        // A write that never reached the Agent must not leave the switch stuck
        // mid-flight. The row goes back to the last state the Agent confirmed.
        if let Some((profile_id, _)) = pending.set {
            self.note_cloud_sync_error(&profile_id, "cloud_sync_unavailable");
        }
        self.cloud.sync_due = true;
        true
    }
    fn note_cloud_sync_error(&mut self, profile_id: &str, code: &str) {
        let rows = self.cloud.sync.get_or_insert_with(Vec::new);
        match rows.iter_mut().find(|row| row.profile_id == profile_id) {
            Some(row) => row.error = Some(code.to_owned()),
            None => rows.push(CloudSyncProfile {
                profile_id: profile_id.to_owned(),
                enabled: false,
                workspaces: 0,
                synced_workspaces: 0,
                error: Some(code.to_owned()),
            }),
        }
    }
    pub(super) fn cloud_result(
        &mut self,
        generation: u64,
        result: &AccountOperationResult,
    ) -> bool {
        match result {
            AccountOperationResult::CloudSubscription {
                request_id,
                current,
            } => {
                let Some(pending) = self.cloud.pending.as_ref() else {
                    return false;
                };
                if *request_id != pending.id
                    || !pending
                        .generation
                        .is_none_or(|expected| expected == generation)
                {
                    return false;
                }
                self.cloud.status = match current {
                    Some(true) => Status::Manage,
                    Some(false) => Status::Subscribe,
                    None => Status::Unavailable,
                };
                self.cloud.pending = None;
                // A confirmed subscription is what makes the per-Profile state
                // worth reading at all, so confirming one asks for it.
                if self.cloud.status == Status::Manage {
                    self.cloud.sync_due = true;
                } else {
                    self.cloud.sync = None;
                }
                true
            }
            AccountOperationResult::CloudSync {
                request_id,
                profiles,
            } => {
                let Some(pending) = self.cloud.sync_pending.as_ref() else {
                    return false;
                };
                if *request_id != pending.id
                    || !pending
                        .generation
                        .is_none_or(|expected| expected == generation)
                {
                    return false;
                }
                let was_write = pending.set.is_some();
                self.cloud.sync = Some(profiles.clone());
                self.cloud.sync_pending = None;
                // A write is answered with the state it produced; the re-read
                // after it is what catches a Workspace that was still uploading.
                self.cloud.sync_due = was_write;
                true
            }
            AccountOperationResult::Failed { code } => {
                let Some(pending) = self.cloud.sync_pending.as_ref() else {
                    return false;
                };
                // `Failed` carries no request id, so an *admitted* generation is
                // the only correlation there is. A write that has not been
                // admitted yet claims nothing: it would swallow a refusal that
                // belonged to some other flow entirely.
                if pending.generation != Some(generation) || pending.set.is_none() {
                    return false;
                }
                let set = self.cloud.sync_pending.take().and_then(|p| p.set);
                if let Some((profile_id, _)) = set {
                    let code = code.clone();
                    self.note_cloud_sync_error(&profile_id, &code);
                }
                self.cloud.sync_due = true;
                true
            }
            _ => false,
        }
    }

    // ---- the card ---------------------------------------------------------

    /// The Profiles this account owns on this device — the `m` in "syncing n of
    /// m profiles", and the only rows that may carry a switch.
    fn account_profile_ids(&self) -> Vec<String> {
        let Some(active) = self.account.active_account_id.as_deref() else {
            return Vec::new();
        };
        self.profiles
            .iter()
            .filter(|row| {
                matches!(
                    koru_store_core::profile_account::LocalProfileAttribution::from_binding(
                        row.account.as_deref().unwrap_or(""),
                    ),
                    koru_store_core::profile_account::LocalProfileAttribution::Account(binding)
                        if binding.identity_id() == active
                )
            })
            .map(|row| row.id.to_storage())
            .collect()
    }
    pub fn cloud_profile_counts(&self) -> Option<(usize, usize)> {
        let sync = self.cloud.sync.as_ref()?;
        let ids = self.account_profile_ids();
        let on = ids
            .iter()
            .filter(|id| sync.iter().any(|row| row.profile_id == **id && row.enabled))
            .count();
        Some((on, ids.len()))
    }
    pub fn cloud_message(&self) -> TextKey {
        // Not an outage: there is no Koru Cloud in this build to check.
        if self.account.stage == AccountStage::NotConfigured {
            return TextKey::CloudNotConfiguredBody;
        }
        if matches!(
            self.account.stage,
            AccountStage::Restoring
                | AccountStage::Refreshing
                | AccountStage::WaitingForBrowser
                | AccountStage::Exchanging
                | AccountStage::AwaitingAccountCreation
        ) {
            return TextKey::CloudLoading;
        }
        if self.account.stage != AccountStage::SignedIn && self.cloud.pending.is_none() {
            return TextKey::CloudSignIn;
        }
        match self.cloud.status {
            Status::Unknown | Status::Loading => TextKey::CloudLoading,
            Status::Unavailable => TextKey::CloudUnavailable,
            Status::Subscribe => TextKey::CloudSubscribeIntro,
            Status::Manage if self.cloud_profile_counts().is_some() => {
                TextKey::CloudSubscribedSyncing
            }
            Status::Manage => TextKey::CloudSubscribed,
        }
    }
    pub fn cloud_body_text(&self, lang: Lang) -> String {
        let key = self.cloud_message();
        match (key, self.cloud_profile_counts()) {
            (TextKey::CloudSubscribedSyncing, Some((on, total))) => koru_i18n::fill(
                koru_i18n::t(lang, key),
                &[&on.to_string(), &total.to_string()],
            ),
            _ => koru_i18n::t(lang, key).to_owned(),
        }
    }
    pub fn cloud_actionable(&self) -> bool {
        self.cloud.pending.is_none()
            && match self.account.stage {
                AccountStage::SignedIn => {
                    !matches!(self.cloud.status, Status::Loading | Status::Unknown)
                }
                AccountStage::SignedOut
                | AccountStage::Cancelled
                | AccountStage::Failed
                | AccountStage::Unavailable => true,
                _ => false,
            }
    }
    /// A current subscription is not something to press — the only thing left to
    /// do with it is go and change the billing, so it reads as a link.
    pub fn cloud_action_is_link(&self) -> bool {
        self.account.stage == AccountStage::SignedIn && self.cloud.status == Status::Manage
    }
    pub fn cloud_learn_more_visible(&self) -> bool {
        self.cloud_actionable()
            && self.account.stage == AccountStage::SignedIn
            && self.cloud.status == Status::Subscribe
    }
    pub fn cloud_action_label(&self) -> &'static str {
        let key = if self.account.stage != AccountStage::SignedIn {
            TextKey::CloudSignInAction
        } else {
            match self.cloud.status {
                Status::Manage => TextKey::CloudManageSubscription,
                Status::Subscribe => TextKey::CloudSubscribe,
                _ => TextKey::TryAgain,
            }
        };
        koru_i18n::t(crate::format::lang(), key)
    }
    pub(super) fn cloud_activate(&mut self) {
        if !self.cloud_actionable() {
            return;
        }
        if self.account.stage != AccountStage::SignedIn {
            self.perform(Intent::Session);
            return;
        }
        match self.cloud.status {
            Status::Manage | Status::Subscribe => {
                let page = if self.cloud.status == Status::Manage {
                    crate::website::Page::Dashboard
                } else {
                    crate::website::Page::Pricing
                };
                self.open_cloud_page(page);
            }
            _ => self.refresh_cloud(),
        }
    }
    pub(super) fn cloud_learn_more_activate(&mut self) {
        if self.cloud_learn_more_visible() {
            self.open_cloud_page(crate::website::Page::Pricing);
        }
    }
    fn open_cloud_page(&mut self, page: crate::website::Page) {
        self.cloud_uri = Some(crate::website::page_url(
            &crate::website::origin(),
            crate::format::lang(),
            page,
        ));
    }

    // ---- the per-Profile switch -------------------------------------------

    /// Whether this row carries a switch, and what to say when it does not work.
    ///
    /// A Profile with no account binding is not a Cloud thing at all, so it gets
    /// no switch rather than a dead one — an affordance that can never do
    /// anything is worse than no affordance.
    pub fn cloud_sync_availability(&self, profile: &ProfileSummary) -> SyncAvailability {
        use koru_store_core::profile_account::LocalProfileAttribution as Attribution;
        let Attribution::Account(binding) =
            Attribution::from_binding(profile.account.as_deref().unwrap_or(""))
        else {
            return SyncAvailability::Hidden;
        };
        if self.account.stage == AccountStage::NotConfigured {
            return SyncAvailability::Disabled(TextKey::CloudNotConfigured);
        }
        if self.account.stage != AccountStage::SignedIn {
            return SyncAvailability::Disabled(TextKey::CloudSyncSignInReason);
        }
        if self.cloud.status != Status::Manage {
            return SyncAvailability::Disabled(TextKey::CloudSyncSubscribeReason);
        }
        match self.account.active_account_id.as_deref() {
            Some(active) if binding.identity_id() == active => SyncAvailability::Enabled,
            _ => SyncAvailability::Disabled(TextKey::CloudSyncSubscribeReason),
        }
    }
    fn cloud_sync_row(&self, profile: ProfileId) -> Option<&CloudSyncProfile> {
        let id = profile.to_storage();
        self.cloud
            .sync
            .as_ref()?
            .iter()
            .find(|row| row.profile_id == id)
    }
    /// `Some(target)` while a `SetProfileCloudSync` for this Profile is in
    /// flight, whether or not it has reached the wire yet.
    pub fn cloud_sync_pending_for(&self, profile: ProfileId) -> Option<bool> {
        let id = profile.to_storage();
        self.cloud
            .set_request
            .as_ref()
            .or_else(|| {
                self.cloud
                    .sync_pending
                    .as_ref()
                    .and_then(|pending| pending.set.as_ref())
            })
            .filter(|(pending_id, _)| *pending_id == id)
            .map(|(_, enabled)| *enabled)
    }
    /// What the switch shows. A pending change shows its *target*, so the switch
    /// moves under the finger and the dimming is what says it is not settled.
    pub fn cloud_sync_on(&self, profile: ProfileId) -> bool {
        self.cloud_sync_pending_for(profile).unwrap_or_else(|| {
            self.cloud_sync_row(profile).map_or_else(
                || {
                    self.account
                        .cloud_profiles
                        .iter()
                        .any(|row| row.profile_id == profile.to_string())
                },
                |row| row.enabled,
            )
        })
    }
    /// **This Profile's Koru Cloud connection**, the one value its row
    /// subtitle and every workspace row under its sync progress read, so the
    /// two cannot disagree. A refusal the Agent gave this Profile's Cloud sync
    /// row counts as unavailable.
    pub fn cloud_connection(&self, profile: ProfileId) -> Connection {
        Connection::of(
            &self.account,
            &profile.to_string(),
            self.cloud_sync_row(profile)
                .is_some_and(|row| row.error.is_some()),
        )
    }
    /// The Cloud half of a Profile row's subtitle, appended after the account
    /// label. `None` when the row has nothing Cloud-related to say.
    pub fn cloud_sync_subtitle(&self, profile: &ProfileSummary) -> Option<String> {
        let lang = crate::format::lang();
        if self.cloud_sync_on(profile.id) && self.cloud_sync_pending_for(profile.id).is_none() {
            return Some(self.cloud_connection(profile.id).sentence(lang));
        }
        match self.cloud_sync_availability(profile) {
            SyncAvailability::Hidden => return None,
            SyncAvailability::Disabled(reason) => {
                return Some(koru_i18n::t(lang, reason).to_owned());
            }
            SyncAvailability::Enabled => {}
        }
        if let Some(target) = self.cloud_sync_pending_for(profile.id) {
            let key = if target {
                TextKey::CloudSyncTurningOn
            } else {
                TextKey::CloudSyncTurningOff
            };
            return Some(koru_i18n::t(lang, key).to_owned());
        }
        let row = self.cloud_sync_row(profile.id)?;
        // A code is shown verbatim: it is a stable identifier the support
        // conversation can search for, not a sentence to be translated into one.
        if let Some(code) = row.error.as_deref() {
            return Some(koru_i18n::fill(
                koru_i18n::t(lang, TextKey::CloudSyncError),
                &[code],
            ));
        }
        None
    }
    // ---- the Profile prompt (D258) ----------------------------------------

    /// Whether a transport-mode write is still out.
    ///
    /// The window comes back for it on its own clock while this is true — a
    /// person who pressed *Move everything* is watching for the answer.
    pub fn cloud_move_pending(&self) -> bool {
        self.cloud.transport.is_some() || self.cloud.move_request.is_some()
    }

    /// Raise the prompt for `profile`.
    fn ask_about_workspaces(&mut self, profile: ProfileId) {
        if self.cloud.move_prompt.is_some() || self.cloud_move_pending() {
            return;
        }
        let lang = crate::format::lang();
        let name = self
            .profiles
            .iter()
            .find(|row| row.id == profile)
            .map(|row| row.name.clone())
            .unwrap_or_default();
        let confirm = Confirm::asking(
            koru_i18n::t(lang, TextKey::CloudMoveTitle),
            koru_i18n::fill(koru_i18n::t(lang, TextKey::CloudMoveBody), &[&name]),
            koru_i18n::t(lang, TextKey::CloudMoveConfirm),
            // **Cancel is Cancel** (D259). This slot used to carry *Just this
            // Profile*, so the dialog had no dismissal at all and Escape —
            // which `Confirm::key` answers as `Cancel`, as every modal in this
            // app does — committed a Profile to Cloud. A question with three
            // answers gets three buttons; the third one is the one that
            // writes nothing.
            koru_i18n::t(lang, TextKey::CloudMoveNotNow),
        )
        // *Just this Profile* is the middle answer, where a question with
        // three answers puts the one that is neither the corner nor the
        // dismissal (`Confirm::with_alternative`).
        .with_alternative(koru_i18n::t(lang, TextKey::CloudMoveProfileOnly))
        // **Enter answers nothing until a button is chosen** (ADR-265, row
        // four). The corner answer starts sending every file every Workspace
        // under this Profile references, which on a Workspace of several
        // years is gigabytes; a modal appearing under somebody's hands must
        // not read the next keystroke as *yes, send all of that*.
        .declaring_no_default();
        self.cloud.move_prompt = Some((profile, confirm));
    }

    /// Settle the prompt. **Two answers write and one does not** (D259).
    ///
    /// *Move everything* and *Just this Profile* are both choices about the
    /// Workspaces, and the Profile is moving either way — the person said so
    /// when they turned the switch on. **Cancel is neither.** It used to be
    /// wired to *Just this Profile* and to commit, which made Escape — and
    /// the window-manager close that `Confirm::key` answers the same way — a
    /// durable write about somebody's content that they had asked to get rid
    /// of. Nothing is written, no mode changes, and the question comes back
    /// the next time the switch is turned on.
    pub(super) fn settle_move_prompt(&mut self, outcome: ConfirmOutcome) -> bool {
        let include = match outcome {
            ConfirmOutcome::Keep => return false,
            ConfirmOutcome::Confirm => true,
            ConfirmOutcome::Alternative => false,
            ConfirmOutcome::Cancel => {
                if self.cloud.move_prompt.take().is_none() {
                    return false;
                }
                // Said out loud, because a dialog that vanishes having done
                // nothing is indistinguishable from one that did something
                // quietly — and the way back is one sentence long.
                self.notice = Some(
                    koru_i18n::t(crate::format::lang(), TextKey::CloudMoveDismissed).to_owned(),
                );
                return true;
            }
        };
        let Some((profile, _)) = self.cloud.move_prompt.take() else {
            return false;
        };
        self.cloud.move_request = Some((profile, include));
        true
    }

    /// The local Store operation the prompt's answer is (D258).
    ///
    /// **Not a hosted route**: D230 settled that the mode is device-local, so
    /// nothing here tells another device anything. Off the winit thread, like
    /// every Store call this window makes.
    fn start_transport_write(&mut self, profile: ProfileId, include_workspaces: bool) {
        let lang = crate::format::lang();
        // **A window with no Store claims nothing.** The hosted toggle has
        // already done its half; what it cannot do is write a device-local
        // mode, and saying so with the same code a refusal would carry is the
        // honest answer rather than a dialog that looked like it worked.
        let Some(client) = self.client.clone() else {
            self.notice = Some(koru_i18n::fill(
                koru_i18n::t(lang, TextKey::CloudMoveFailed),
                &[koru_store_core::errors::Code::StoreUnreachable.as_str()],
            ));
            return;
        };
        // The window and the Store spell an id in two types. Parsing the
        // storage form is the one conversion between them, and a Profile
        // whose id will not parse is a Profile this write must not guess at.
        let Some(profile) = koru_store_core::ProfileId::parse(&profile.to_storage()) else {
            return;
        };
        let (send, receive) = mpsc::channel();
        if std::thread::Builder::new()
            .name("koru-transport-mode-ui".into())
            .spawn(move || {
                let answer = match client.set_profile_transport_mode(
                    profile,
                    koru_store_core::TransportMode::Cloud.as_storage(),
                    include_workspaces,
                ) {
                    Ok(outcome) => TransportAnswer::Settled(Box::new(outcome)),
                    // The code, not a sentence: it is a stable identifier a
                    // support conversation can search for.
                    Err(error) => TransportAnswer::Refused(error.code().as_str().to_owned()),
                };
                let _ = send.send(answer);
            })
            .is_err()
        {
            return;
        }
        self.cloud.transport = Some(receive);
    }

    /// Put an answered prompt on the wire, then drain whatever came back.
    ///
    /// Both in one call because they are one frame's worth of background
    /// work, and because a caller that had to remember to do the second would
    /// eventually not.
    pub(super) fn drain_transport_answer(&mut self) -> bool {
        if let Some((profile, include)) = self.cloud.move_request.take() {
            self.start_transport_write(profile, include);
            return true;
        }
        let Some(receiver) = &self.cloud.transport else {
            return false;
        };
        let Ok(answer) = receiver.try_recv() else {
            return false;
        };
        self.cloud.transport = None;
        let lang = crate::format::lang();
        self.notice = Some(match answer {
            TransportAnswer::Refused(code) => {
                koru_i18n::fill(koru_i18n::t(lang, TextKey::CloudMoveFailed), &[&code])
            }
            TransportAnswer::Settled(outcome) if outcome.workspaces_submitted == 0 => {
                koru_i18n::t(lang, TextKey::CloudMoveProfileOnlyDone).to_owned()
            }
            TransportAnswer::Settled(outcome) => {
                let moved = koru_i18n::fill(
                    koru_i18n::t(lang, TextKey::CloudMoveDone),
                    &[&outcome.workspaces_moved.to_string()],
                );
                // **A Workspace left alone is not a Workspace that failed.**
                // Saying so is the difference between a person believing
                // their own earlier choice was honoured and believing
                // something went wrong.
                match outcome.workspaces_left_alone {
                    0 => moved,
                    left => format!(
                        "{moved} · {}",
                        koru_i18n::fill(
                            koru_i18n::t(lang, TextKey::CloudMoveKept),
                            &[&left.to_string()],
                        )
                    ),
                }
            }
        });
        true
    }

    pub(super) fn cloud_sync_activate(&mut self, profile: ProfileId) {
        let Some(row) = self.profiles.iter().find(|row| row.id == profile) else {
            return;
        };
        if self.cloud_sync_availability(row) != SyncAvailability::Enabled {
            return;
        }
        // One write at a time. A second press while the first is unanswered is
        // dropped rather than queued: the Agent's answer is the truth, and two
        // writes in flight would make the second one race it.
        if self.cloud.set_request.is_some() || self.cloud.sync_pending.is_some() {
            return;
        }
        let enabled = !self.cloud_sync_on(profile);
        self.cloud.set_request = Some((profile.to_storage(), enabled));
        // **Turning it on is the Profile being spoken about** (D258), so this
        // is where the question about its Workspaces belongs. Turning it off
        // raises nothing: that is D227's immediate stop, it already has its
        // behaviour, and the row says plainly what it does while it happens.
        if enabled {
            self.ask_about_workspaces(profile);
        }
    }
}

/// Whether a Profile row carries a Cloud sync switch, and why it is inert.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SyncAvailability {
    Hidden,
    Disabled(TextKey),
    Enabled,
}

fn next_request_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod tests;
