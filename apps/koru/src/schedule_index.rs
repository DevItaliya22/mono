// SPDX-License-Identifier: MPL-2.0

//! The workspace's scheduled delegations, kept current off the window thread.
//!
//! The API already owns schedule discovery and change detection. This index is
//! only the app adapter: one typed list, one content-free watch, and localized
//! rows for the Agents navigator. It never polls, opens the Store, or performs
//! one request per firing.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use koru_api_client::{Client, ScheduleSubscription};
use koru_core::time::{TimeZoneName, ZoneRules};
use koru_delegate_api::wire::{
    Failure, ListSchedulesResponse, PauseScheduleRequest, PauseScheduleResponse,
    RenameScheduleRequest, RenameScheduleResponse, ResumeScheduleRequest, ResumeScheduleResponse,
    RetireScheduleRequest, RetireScheduleResponse, ScheduleChange, ScheduleNextFiring,
    ScheduleView,
};
use koru_delegate_core::{DeviceId, RunId, ScheduleId, WorkspaceId};
use koru_delegate_schedule::{
    RecurrenceRule, ScheduleContextFailureStage, ScheduleDefinition, ScheduleFiringOutcome,
    ScheduleName, SchedulePauseReason, ScheduleState,
};
use koru_i18n::time::LocaleFormat;
use koru_i18n::{Key, Lang, fill, number, t};
use koru_recurrence::Occurrence;
use koru_runs_ui::RunRow;
use koru_time_zone::SystemZone;

use crate::schedule_words::{ScheduleProjection, schedule_words};

/// One actual run made by a schedule. Skips never enter this type: the wire
/// deliberately exposes no RunId for them, and the in-memory join accepts only
/// ids present in the workspace's existing run index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScheduleRunRow {
    pub(crate) run: RunId,
    pub(crate) label: String,
}

/// One schedule as the navigator consumes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScheduleRow {
    pub(crate) schedule: ScheduleId,
    pub(crate) name: String,
    /// Semantic state retained for icon/action presentation. The localized
    /// `state` below is words for a person, not something a renderer parses.
    pub(crate) paused: bool,
    pub(crate) rule: String,
    pub(crate) next: String,
    pub(crate) owner: String,
    pub(crate) state: String,
    pub(crate) runs: Vec<ScheduleRunRow>,
    pub(crate) created_at_ms: i64,
}

/// Stable mutation facts copied from one canonical schedule projection. Menu
/// and keyboard actions carry this identity, never a row position that a watch
/// refresh can retarget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScheduleSubject {
    pub(crate) schedule: ScheduleId,
    pub(crate) workspace: WorkspaceId,
    pub(crate) document: koru_delegate_core::DocumentId,
    pub(crate) owner: DeviceId,
    pub(crate) name: ScheduleName,
    pub(crate) definition: ScheduleDefinition,
    pub(crate) state: ScheduleState,
}

pub(crate) struct ScheduleRenameResult {
    pub(crate) schedule: ScheduleId,
    pub(crate) result: Result<RenameScheduleResponse, Failure>,
}

pub(crate) struct ScheduleLifecycleResult {
    pub(crate) result: Result<ScheduleView, Failure>,
}

pub(crate) struct ScheduleRetireResult {
    pub(crate) result: Result<ScheduleView, Failure>,
}

/// The list-level truth shown directly below the Schedules heading.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CatalogState {
    Loading,
    #[default]
    Ready,
    Incomplete,
    Error,
}

enum Ask {
    Source {
        token: u64,
        source: Box<dyn ScheduleSource>,
        workspace: WorkspaceId,
    },
    Clear {
        token: u64,
    },
    Waker(Waker),
    Invalidated {
        token: u64,
        pending: Arc<AtomicBool>,
    },
    Refresh {
        token: u64,
    },
    Rename {
        token: u64,
        request: RenameScheduleRequest,
    },
    Pause {
        token: u64,
        request: PauseScheduleRequest,
    },
    Resume {
        token: u64,
        request: ResumeScheduleRequest,
    },
    Retire {
        token: u64,
        request: RetireScheduleRequest,
    },
    Stop,
}

type Waker = Arc<dyn Fn() + Send + Sync + 'static>;
type WatchHandler = Box<dyn FnMut(Result<ScheduleChange, Failure>) + Send + 'static>;

trait ScheduleWatch: Send {}

impl ScheduleWatch for ScheduleSubscription {}

trait ScheduleSource: Send {
    fn list(&self, workspace: WorkspaceId) -> Result<ListSchedulesResponse, Failure>;

    fn watch(
        &self,
        workspace: WorkspaceId,
        handler: WatchHandler,
    ) -> Result<Box<dyn ScheduleWatch>, Failure>;

    fn rename(&self, request: &RenameScheduleRequest) -> Result<RenameScheduleResponse, Failure> {
        let _ = request;
        Err(Failure::new(
            "unsupported_schedule_command",
            "This schedule-management command is unavailable.",
            false,
        ))
    }

    fn pause(&self, request: &PauseScheduleRequest) -> Result<PauseScheduleResponse, Failure> {
        let _ = request;
        Err(Failure::new(
            "unsupported_schedule_command",
            "This schedule-management command is unavailable.",
            false,
        ))
    }

    fn resume(&self, request: &ResumeScheduleRequest) -> Result<ResumeScheduleResponse, Failure> {
        let _ = request;
        Err(Failure::new(
            "unsupported_schedule_command",
            "This schedule-management command is unavailable.",
            false,
        ))
    }

    fn retire(&self, request: &RetireScheduleRequest) -> Result<RetireScheduleResponse, Failure> {
        let _ = request;
        Err(Failure::new(
            "unsupported_schedule_command",
            "This schedule-management command is unavailable.",
            false,
        ))
    }
}

struct ApiScheduleSource(Client);

impl ScheduleSource for ApiScheduleSource {
    fn list(&self, workspace: WorkspaceId) -> Result<ListSchedulesResponse, Failure> {
        self.0.schedules().list(workspace)
    }

    fn watch(
        &self,
        workspace: WorkspaceId,
        handler: WatchHandler,
    ) -> Result<Box<dyn ScheduleWatch>, Failure> {
        self.0
            .schedules()
            .watch(workspace, None, handler)
            .map(|watch| Box::new(watch) as Box<dyn ScheduleWatch>)
    }

    fn rename(&self, request: &RenameScheduleRequest) -> Result<RenameScheduleResponse, Failure> {
        self.0.schedules().rename(request)
    }

    fn pause(&self, request: &PauseScheduleRequest) -> Result<PauseScheduleResponse, Failure> {
        self.0.schedules().pause(request)
    }

    fn resume(&self, request: &ResumeScheduleRequest) -> Result<ResumeScheduleResponse, Failure> {
        self.0.schedules().resume(request)
    }

    fn retire(&self, request: &RetireScheduleRequest) -> Result<RetireScheduleResponse, Failure> {
        self.0.schedules().retire(request)
    }
}

enum Answer {
    Listing {
        token: u64,
        listing: ListSchedulesResponse,
    },
    Error {
        token: u64,
    },
    Renamed {
        token: u64,
        expected_name: ScheduleName,
        answer: ScheduleRenameResult,
    },
    Lifecycle {
        token: u64,
        answer: ScheduleLifecycleResult,
    },
    Retired {
        token: u64,
        answer: ScheduleRetireResult,
    },
}

/// A workspace-scoped, watch-driven schedule listing.
pub(crate) struct ScheduleIndex {
    asks: Option<Sender<Ask>>,
    answers: Receiver<Answer>,
    source: Option<WorkspaceId>,
    connected: bool,
    token: u64,
    listing: ListSchedulesResponse,
    state: CatalogState,
    rows: Vec<ScheduleRow>,
    lang: Lang,
    presentation_ready: bool,
    locale: LocaleFormat,
    zone: SystemZone,
    this_device: Option<DeviceId>,
    device_names: HashMap<DeviceId, String>,
    runs: Vec<RunRow>,
    runs_refresh_needed: bool,
    rename_results: Vec<ScheduleRenameResult>,
    lifecycle_results: Vec<ScheduleLifecycleResult>,
    retire_results: Vec<ScheduleRetireResult>,
}

impl Default for ScheduleIndex {
    fn default() -> Self {
        Self::inert()
    }
}

impl ScheduleIndex {
    /// A test/headless index with no worker and therefore no external I/O.
    pub(crate) fn inert() -> Self {
        let (_, answers) = channel();
        Self::base(None, answers)
    }

    pub(crate) fn open() -> Self {
        let (asks, inbox) = channel();
        let (outbox, answers) = channel();
        let relay = asks.clone();
        let spawned = std::thread::Builder::new()
            .name("koru-schedule-index".into())
            .stack_size(256 * 1024)
            .spawn(move || worker(inbox, outbox, relay));
        match spawned {
            Ok(_) => Self::base(Some(asks), answers),
            Err(_) => Self::inert(),
        }
    }

    fn base(asks: Option<Sender<Ask>>, answers: Receiver<Answer>) -> Self {
        Self {
            asks,
            answers,
            source: None,
            connected: false,
            token: 0,
            listing: empty_listing(),
            state: CatalogState::Ready,
            rows: Vec::new(),
            lang: Lang::En,
            presentation_ready: false,
            locale: LocaleFormat::default(),
            zone: SystemZone::utc(),
            this_device: None,
            device_names: HashMap::new(),
            runs: Vec::new(),
            runs_refresh_needed: false,
            rename_results: Vec::new(),
            lifecycle_results: Vec::new(),
            retire_results: Vec::new(),
        }
    }

    /// Bind the typed gateway client to one workspace. Repeating an identical
    /// binding is free; a profile/workspace change invalidates old answers by
    /// token before the background subscription is replaced.
    pub(crate) fn set_source(
        &mut self,
        client: Option<Client>,
        workspace: Option<WorkspaceId>,
    ) -> bool {
        let connected = client.is_some() && workspace.is_some();
        if self.source == workspace && self.connected == connected {
            return false;
        }
        self.source = workspace;
        self.connected = connected;
        self.token = self.token.saturating_add(1);
        self.listing = empty_listing();
        self.rows.clear();
        self.runs_refresh_needed = false;
        self.rename_results.clear();
        self.lifecycle_results.clear();
        self.retire_results.clear();
        self.state = if workspace.is_some() {
            CatalogState::Loading
        } else {
            CatalogState::Ready
        };
        if let Some(asks) = &self.asks {
            match (client, workspace) {
                (Some(client), Some(workspace)) => {
                    let _ = asks.send(Ask::Source {
                        token: self.token,
                        source: Box::new(ApiScheduleSource(client)),
                        workspace,
                    });
                }
                _ => {
                    let _ = asks.send(Ask::Clear { token: self.token });
                }
            }
        }
        true
    }

    pub(crate) fn set_waker(&mut self, wake: Waker) {
        if let Some(asks) = &self.asks {
            let _ = asks.send(Ask::Waker(wake));
        }
    }

    /// Update display-only inputs. The run list is joined once here; there is
    /// no per-schedule or per-firing request.
    pub(crate) fn set_presentation(
        &mut self,
        lang: Lang,
        current_zone: &TimeZoneName,
        this_device: Option<DeviceId>,
        device_names: &HashMap<DeviceId, String>,
        runs: Option<&[RunRow]>,
    ) -> bool {
        let runs_changed = runs.is_some_and(|runs| self.runs != runs);
        let zone_changed = self.zone.zone() != current_zone;
        let context_changed = !self.presentation_ready
            || self.lang != lang
            || zone_changed
            || self.this_device != this_device
            || self.device_names != *device_names;
        if !context_changed && !runs_changed {
            return false;
        }
        if context_changed {
            self.lang = lang;
            let system_tag = koru_window::system_locale::system_locale();
            let tag = system_tag
                .as_deref()
                .filter(|tag| Lang::matching(tag) == Some(lang))
                .unwrap_or_else(|| lang.tag());
            self.locale = LocaleFormat::for_system(
                Some(tag),
                koru_workspace_host::environment::preferred_hour_cycle(),
            );
            self.zone = SystemZone::from_name(current_zone)
                .expect("the maintenance environment contains validated IANA rules");
            self.this_device = this_device;
            self.device_names.clone_from(device_names);
        }
        if let Some(runs) = runs {
            self.runs = runs.to_vec();
        }
        self.presentation_ready = true;
        self.rebuild();
        true
    }

    pub(crate) fn needs_presentation(&self) -> bool {
        !self.presentation_ready
    }

    /// Re-read the canonical list after a revision/name conflict. This is an
    /// explicit person-action recovery, not polling.
    pub(crate) fn refresh(&self) {
        if let Some(asks) = &self.asks {
            let _ = asks.send(Ask::Refresh { token: self.token });
        }
    }

    pub(crate) fn poll(&mut self) -> bool {
        let mut changed = false;
        loop {
            match self.answers.try_recv() {
                Ok(Answer::Listing { token, listing }) if token == self.token => {
                    self.state = if listing.incomplete {
                        CatalogState::Incomplete
                    } else {
                        CatalogState::Ready
                    };
                    self.replace_listing(listing);
                    changed = true;
                }
                Ok(Answer::Error { token }) if token == self.token => {
                    self.state = CatalogState::Error;
                    changed = true;
                }
                Ok(Answer::Renamed {
                    token,
                    expected_name,
                    answer,
                }) if token == self.token => {
                    match &answer.result {
                        Ok(response) => self.replace_schedule(response.schedule.clone()),
                        Err(_) => self.set_name(answer.schedule, expected_name),
                    }
                    self.rename_results.push(answer);
                    changed = true;
                }
                Ok(Answer::Lifecycle { token, answer }) if token == self.token => {
                    if let Ok(schedule) = &answer.result {
                        self.replace_schedule(schedule.clone());
                    }
                    self.lifecycle_results.push(answer);
                    changed = true;
                }
                Ok(Answer::Retired { token, answer }) if token == self.token => {
                    if let Ok(schedule) = &answer.result {
                        self.replace_schedule(schedule.clone());
                    }
                    self.retire_results.push(answer);
                    changed = true;
                }
                Ok(_) => {}
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        if changed {
            self.rebuild();
        }
        changed
    }

    pub(crate) fn rows(&self) -> &[ScheduleRow] {
        &self.rows
    }

    pub(crate) fn state(&self) -> CatalogState {
        self.state
    }

    pub(crate) fn subject(&self, schedule: ScheduleId) -> Option<ScheduleSubject> {
        let view = self
            .listing
            .schedules
            .iter()
            .find(|view| view.header.schedule == schedule)?;
        let owner = match view.state {
            ScheduleState::Active { owner }
            | ScheduleState::Paused { owner, .. }
            | ScheduleState::HandoffPending { owner, .. } => owner,
            ScheduleState::Retired => return None,
        };
        Some(ScheduleSubject {
            schedule,
            workspace: view.workspace,
            document: view.document,
            owner,
            name: view.name.clone().unwrap_or_else(|| {
                ScheduleName::from_objective(&view.header.definition.template.objective)
            }),
            definition: view.header.definition.clone(),
            state: view.state.clone(),
        })
    }

    /// Optimistically update the row and queue the durable owner command. The
    /// worker answer either replaces it with the canonical projection or
    /// restores the observed name and leaves the inline editor to explain.
    pub(crate) fn rename(&mut self, request: RenameScheduleRequest) -> bool {
        let Some(asks) = &self.asks else {
            return false;
        };
        if asks
            .send(Ask::Rename {
                token: self.token,
                request: request.clone(),
            })
            .is_err()
        {
            return false;
        }
        self.set_name(request.schedule, request.name);
        self.rebuild();
        true
    }

    pub(crate) fn take_rename_result(&mut self) -> Option<ScheduleRenameResult> {
        if self.rename_results.is_empty() {
            None
        } else {
            Some(self.rename_results.remove(0))
        }
    }

    pub(crate) fn pause(&self, request: PauseScheduleRequest) -> bool {
        self.asks.as_ref().is_some_and(|asks| {
            asks.send(Ask::Pause {
                token: self.token,
                request,
            })
            .is_ok()
        })
    }

    pub(crate) fn resume(&self, request: ResumeScheduleRequest) -> bool {
        self.asks.as_ref().is_some_and(|asks| {
            asks.send(Ask::Resume {
                token: self.token,
                request,
            })
            .is_ok()
        })
    }

    pub(crate) fn take_lifecycle_result(&mut self) -> Option<ScheduleLifecycleResult> {
        if self.lifecycle_results.is_empty() {
            None
        } else {
            Some(self.lifecycle_results.remove(0))
        }
    }

    pub(crate) fn retire(&self, request: RetireScheduleRequest) -> bool {
        self.asks.as_ref().is_some_and(|asks| {
            asks.send(Ask::Retire {
                token: self.token,
                request,
            })
            .is_ok()
        })
    }

    pub(crate) fn take_retire_result(&mut self) -> Option<ScheduleRetireResult> {
        if self.retire_results.is_empty() {
            None
        } else {
            Some(self.retire_results.remove(0))
        }
    }

    /// Whether the latest schedule listing names a fired run the shared run
    /// index has not seen yet. Drained once per listing: a recovered identical
    /// listing can retry a failed read, while an idle window never polls.
    pub(crate) fn take_runs_refresh(&mut self) -> bool {
        std::mem::take(&mut self.runs_refresh_needed)
    }

    /// Fixture seam: deterministic UI/index tests exercise the same rebuild
    /// without a gateway, Store, disk, or watcher.
    #[cfg(test)]
    pub(crate) fn take_listing(&mut self, listing: ListSchedulesResponse) {
        self.state = if listing.incomplete {
            CatalogState::Incomplete
        } else {
            CatalogState::Ready
        };
        self.replace_listing(listing);
        self.rebuild();
    }

    #[cfg(test)]
    pub(crate) fn take_error(&mut self) {
        self.state = CatalogState::Error;
        self.rebuild();
    }

    fn rebuild(&mut self) {
        let runs: HashMap<RunId, &RunRow> = self.runs.iter().map(|run| (run.run, run)).collect();
        let mut rows = Vec::new();
        for schedule in &self.listing.schedules {
            if matches!(schedule.state, ScheduleState::Retired) {
                continue;
            }
            let Some((owner, state)) = state_words(
                &schedule.state,
                self.locale.lang(),
                self.this_device,
                &self.device_names,
            ) else {
                continue;
            };
            let Ok(projection) = projection(schedule, self.this_device, &self.zone) else {
                continue;
            };
            let Ok(words) = schedule_words(&projection, &self.locale) else {
                continue;
            };
            let next = words
                .next
                .unwrap_or_else(|| t(self.locale.lang(), Key::ScheduleNavigatorNoNext).to_owned());
            let mut fired = schedule
                .firings
                .iter()
                .filter(|firing| matches!(firing.outcome, ScheduleFiringOutcome::Fired))
                .filter_map(|firing| firing.run.map(|run| (firing.occurrence_ms, run)))
                .filter_map(|(at, run)| runs.get(&run).map(|row| (at, *row)))
                .collect::<Vec<_>>();
            fired.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.run.cmp(&b.1.run)));
            let fired = fired
                .into_iter()
                .map(|(_, run)| ScheduleRunRow {
                    run: run.run,
                    label: run.name.clone(),
                })
                .collect();
            rows.push(ScheduleRow {
                schedule: schedule.header.schedule,
                name: schedule
                    .name
                    .clone()
                    .unwrap_or_else(|| {
                        koru_delegate_schedule::ScheduleName::from_objective(
                            &schedule.header.definition.template.objective,
                        )
                    })
                    .as_str()
                    .to_owned(),
                paused: matches!(schedule.state, ScheduleState::Paused { .. }),
                rule: words.rule,
                next,
                owner,
                state,
                runs: fired,
                created_at_ms: schedule.header.created_at_ms,
            });
        }
        rows.sort_by(|a, b| {
            b.created_at_ms
                .cmp(&a.created_at_ms)
                .then_with(|| a.schedule.cmp(&b.schedule))
        });
        self.rows = rows;
    }

    fn replace_listing(&mut self, listing: ListSchedulesResponse) {
        let next = fired_runs(&listing);
        if next
            .iter()
            .any(|run| !self.runs.iter().any(|known| known.run == *run))
        {
            self.runs_refresh_needed = true;
        }
        self.listing = listing;
    }

    fn replace_schedule(&mut self, schedule: ScheduleView) {
        match self
            .listing
            .schedules
            .iter_mut()
            .find(|known| known.header.schedule == schedule.header.schedule)
        {
            Some(known) => *known = schedule,
            None => self.listing.schedules.push(schedule),
        }
    }

    fn set_name(&mut self, schedule: ScheduleId, name: ScheduleName) {
        if let Some(view) = self
            .listing
            .schedules
            .iter_mut()
            .find(|view| view.header.schedule == schedule)
        {
            view.name = Some(name);
        }
    }
}

impl Drop for ScheduleIndex {
    fn drop(&mut self) {
        if let Some(asks) = &self.asks {
            let _ = asks.send(Ask::Stop);
        }
    }
}

fn empty_listing() -> ListSchedulesResponse {
    ListSchedulesResponse {
        schedules: Vec::new(),
        incomplete: false,
    }
}

fn fired_runs(listing: &ListSchedulesResponse) -> BTreeSet<RunId> {
    listing
        .schedules
        .iter()
        .flat_map(|schedule| &schedule.firings)
        .filter(|firing| matches!(firing.outcome, ScheduleFiringOutcome::Fired))
        .filter_map(|firing| firing.run)
        .collect()
}

fn projection<'a>(
    schedule: &'a ScheduleView,
    this_device: Option<DeviceId>,
    zone: &'a SystemZone,
) -> Result<ScheduleProjection<'a>, ()> {
    let owner = match schedule.state {
        ScheduleState::Active { owner }
        | ScheduleState::Paused { owner, .. }
        | ScheduleState::HandoffPending { owner, .. } => Some(owner),
        ScheduleState::Retired => None,
    };
    let occurrence = match schedule.next_firing {
        Some(ScheduleNextFiring::Exact { at_ms }) => {
            Some(Occurrence::from_instant_ms(at_ms).map_err(|_| ())?)
        }
        _ => None,
    };
    match &schedule.header.definition.recurrence {
        RecurrenceRule::Floating { .. }
            if matches!(schedule.next_firing, Some(ScheduleNextFiring::OwnerLocal))
                || owner.is_some_and(|owner| Some(owner) != this_device) =>
        {
            ScheduleProjection::owner_local(&schedule.header.definition.recurrence).map_err(|_| ())
        }
        RecurrenceRule::Floating { .. } => ScheduleProjection::local_floating(
            &schedule.header.definition.recurrence,
            zone,
            occurrence,
        )
        .map_err(|_| ()),
        RecurrenceRule::Zoned { .. } => {
            ScheduleProjection::zoned(&schedule.header.definition.recurrence, occurrence)
                .map_err(|_| ())
        }
    }
}

fn state_words(
    state: &ScheduleState,
    lang: Lang,
    this_device: Option<DeviceId>,
    device_names: &HashMap<DeviceId, String>,
) -> Option<(String, String)> {
    let (owner, state_key, reason) = match state {
        ScheduleState::Active { owner } => (*owner, Key::ScheduleNavigatorActive, None),
        ScheduleState::Paused { owner, reason } => {
            (*owner, Key::ScheduleNavigatorPaused, Some(*reason))
        }
        ScheduleState::HandoffPending { owner, .. } => {
            (*owner, Key::ScheduleNavigatorHandoffPending, None)
        }
        ScheduleState::Retired => return None,
    };
    let owner_name = if Some(owner) == this_device {
        match device_names.get(&owner) {
            Some(name) => fill(t(lang, Key::ScheduleNavigatorOwnerHere), &[name]),
            None => fill(
                t(lang, Key::ScheduleNavigatorOwner),
                &[t(lang, Key::ScheduleNavigatorThisDevice)],
            ),
        }
    } else {
        let name = device_names
            .get(&owner)
            .map(String::as_str)
            .unwrap_or_else(|| t(lang, Key::ScheduleNavigatorOtherDevice));
        fill(t(lang, Key::ScheduleNavigatorOwner), &[name])
    };
    let state = match reason {
        None => t(lang, state_key).to_owned(),
        Some(SchedulePauseReason::Manual) => t(lang, Key::ScheduleNavigatorPausedManual).to_owned(),
        Some(SchedulePauseReason::ProviderNotDelegatable) => {
            t(lang, Key::ScheduleNavigatorPausedProvider).to_owned()
        }
        Some(SchedulePauseReason::ScopeUnavailable) => {
            t(lang, Key::ScheduleNavigatorPausedScope).to_owned()
        }
        Some(SchedulePauseReason::ContextUnavailable { item, stage }) => {
            let stage = match stage {
                ScheduleContextFailureStage::Resolve => Key::ScheduleNavigatorContextResolve,
                ScheduleContextFailureStage::Commit => Key::ScheduleNavigatorContextCommit,
                ScheduleContextFailureStage::Retention => Key::ScheduleNavigatorContextRetention,
                ScheduleContextFailureStage::Materialize => {
                    Key::ScheduleNavigatorContextMaterialize
                }
            };
            fill(
                t(lang, Key::ScheduleNavigatorPausedContext),
                &[&number::decimal(lang, u64::from(item) + 1), t(lang, stage)],
            )
        }
    };
    Some((owner_name, state))
}

fn queue_refresh(
    asks: &Sender<Ask>,
    token: u64,
    _result: Result<ScheduleChange, Failure>,
    pending: &Arc<AtomicBool>,
) {
    // Every watch callback means "list now". In particular an error followed
    // by a healthy reconnect is one coalesced refresh opportunity, not an Err
    // payload that can hide the later Ok. The list answer alone decides what
    // the navigator can truthfully show.
    if !pending.swap(true, Ordering::AcqRel) {
        let _ = asks.send(Ask::Invalidated {
            token,
            pending: Arc::clone(pending),
        });
    }
}

fn worker(inbox: Receiver<Ask>, outbox: Sender<Answer>, relay: Sender<Ask>) {
    let mut active_token = 0;
    let mut source: Option<(Box<dyn ScheduleSource>, WorkspaceId)> = None;
    let mut subscription: Option<Box<dyn ScheduleWatch>> = None;
    let mut wake: Option<Waker> = None;
    while let Ok(ask) = inbox.recv() {
        match ask {
            Ask::Source {
                token,
                source: next_source,
                workspace,
            } => {
                subscription.take();
                active_token = token;
                let pending = Arc::new(AtomicBool::new(false));
                let callback_sender = relay.clone();
                let callback_pending = Arc::clone(&pending);
                match next_source.watch(
                    workspace,
                    Box::new(move |result| {
                        queue_refresh(&callback_sender, token, result, &callback_pending);
                    }),
                ) {
                    Ok(watch) => subscription = Some(watch),
                    Err(_) => {
                        let _ = outbox.send(Answer::Error { token });
                        if let Some(wake) = &wake {
                            wake();
                        }
                    }
                }
                source = Some((next_source, workspace));
            }
            Ask::Clear { token } => {
                subscription.take();
                active_token = token;
                source = None;
            }
            Ask::Waker(next) => wake = Some(next),
            Ask::Invalidated { token, pending } => {
                pending.store(false, Ordering::Release);
                if token != active_token {
                    continue;
                }
                if let Some((source, workspace)) = &source {
                    match source.list(*workspace) {
                        Ok(listing) => {
                            let _ = outbox.send(Answer::Listing { token, listing });
                        }
                        Err(_) => {
                            let _ = outbox.send(Answer::Error { token });
                        }
                    }
                    if let Some(wake) = &wake {
                        wake();
                    }
                }
            }
            Ask::Refresh { token } => {
                if token != active_token {
                    continue;
                }
                if let Some((source, workspace)) = &source {
                    match source.list(*workspace) {
                        Ok(listing) => {
                            let _ = outbox.send(Answer::Listing { token, listing });
                        }
                        Err(_) => {
                            let _ = outbox.send(Answer::Error { token });
                        }
                    }
                    if let Some(wake) = &wake {
                        wake();
                    }
                }
            }
            Ask::Rename { token, request } => {
                if token != active_token {
                    continue;
                }
                if let Some((source, _)) = &source {
                    let expected_name = request.expected_name.clone();
                    let schedule = request.schedule;
                    let result = source.rename(&request);
                    let failed = result.is_err();
                    let _ = outbox.send(Answer::Renamed {
                        token,
                        expected_name,
                        answer: ScheduleRenameResult { schedule, result },
                    });
                    if failed {
                        match source.list(request.workspace) {
                            Ok(listing) => {
                                let _ = outbox.send(Answer::Listing { token, listing });
                            }
                            Err(_) => {
                                let _ = outbox.send(Answer::Error { token });
                            }
                        }
                    }
                    if let Some(wake) = &wake {
                        wake();
                    }
                }
            }
            Ask::Pause { token, request } => {
                if token != active_token {
                    continue;
                }
                if let Some((source, _)) = &source {
                    let result = source.pause(&request).map(|response| response.schedule);
                    let failed = result.is_err();
                    let _ = outbox.send(Answer::Lifecycle {
                        token,
                        answer: ScheduleLifecycleResult { result },
                    });
                    if failed {
                        let _ = relay.send(Ask::Refresh { token });
                    }
                    if let Some(wake) = &wake {
                        wake();
                    }
                }
            }
            Ask::Resume { token, request } => {
                if token != active_token {
                    continue;
                }
                if let Some((source, _)) = &source {
                    let result = source.resume(&request).map(|response| response.schedule);
                    let failed = result.is_err();
                    let _ = outbox.send(Answer::Lifecycle {
                        token,
                        answer: ScheduleLifecycleResult { result },
                    });
                    if failed {
                        let _ = relay.send(Ask::Refresh { token });
                    }
                    if let Some(wake) = &wake {
                        wake();
                    }
                }
            }
            Ask::Retire { token, request } => {
                if token != active_token {
                    continue;
                }
                if let Some((source, _)) = &source {
                    let result = source.retire(&request).map(|response| response.schedule);
                    let failed = result.is_err();
                    let _ = outbox.send(Answer::Retired {
                        token,
                        answer: ScheduleRetireResult { result },
                    });
                    if failed {
                        let _ = relay.send(Ask::Refresh { token });
                    }
                    if let Some(wake) = &wake {
                        wake();
                    }
                }
            }
            Ask::Stop => break,
        }
    }
    drop(subscription);
}

#[cfg(test)]
mod tests;
