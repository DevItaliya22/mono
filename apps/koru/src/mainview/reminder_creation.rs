// SPDX-License-Identifier: MPL-2.0

//! Visible reminder creation and exact-object management chrome.
//! Store-facing behavior stays in `reminders::Controller`; this module owns
//! only transient fields, focus, shared rectangles, and cached rendering.

use chrono::{Datelike, NaiveDate, NaiveTime, Timelike};
use koru_core::{
    NodeId,
    time::{FloatingDateTime, TimeZoneName, ZoneRules},
};
use koru_draw::{geometry::Rect, icons, scene, text::TextEngine, theme};
use koru_editor::{
    reminders::{
        Creation, CreationState, Draft, Failure, ItemState, Management, ManagementIntent,
        OptimisticCreation, OptimisticState, Request, RequestIdentity, RequestToken, Tone,
    },
    utils::{
        when::{ReminderDraft, ResolutionContext},
        zone::IanaZoneRules,
    },
};
use koru_i18n::{Key, Lang, t, time::LocaleFormat};
use koru_ui::{ActionButton, ActionStyle, InputStyle, PopoverAlign, PopoverShell, TextInput};
use vello::{Scene, kurbo::Affine, peniko::Fill};
use winit::{
    keyboard::{Key as KeyboardKey, ModifiersState, NamedKey},
    window::CursorIcon,
};

const QUICK_W: f32 = 410.0;
const QUICK_H: f32 = 286.0;
const EXPANDED_H: f32 = 362.0;
const MANAGEMENT_W: f32 = 470.0;
const PAD: f32 = 14.0;
const FIELD_H: f32 = 36.0;
const ROW_H: f32 = 36.0;
const PILL_W: f32 = 330.0;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Creation,
    Management,
}
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Preset {
    LaterToday,
    Tomorrow,
    Custom,
}
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum RowAction {
    Edit,
    Pause,
    Delete,
}
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum FailedAction {
    Retry,
    Edit,
    Dismiss,
}
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum DraftSource {
    Natural,
    Structured,
}

#[derive(Clone, Debug)]
struct Editing {
    id: String,
    draft: Draft,
    paused: bool,
}
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Hit {
    Panel,
    Field,
    Date,
    Time,
    More,
    EndOfDaySettings,
    Set,
    Cancel,
    Preset(Preset),
    Add,
    Row(usize, RowAction),
    History,
    Recreate(usize),
    Clear,
    ConfirmClear,
    CancelClear,
    Failed(RequestToken, FailedAction),
    RetryCurrent,
    DiscardCurrent,
}

#[derive(Copy, Clone, Debug)]
struct FocusTarget {
    hit: Hit,
    rect: Rect,
    enabled: bool,
}

#[derive(Clone, Debug, Default)]
struct Geometry {
    panel: Option<Rect>,
    field: Option<Rect>,
    date: Option<Rect>,
    time: Option<Rect>,
    more: Option<Rect>,
    end_of_day_settings: Option<Rect>,
    set: Option<Rect>,
    cancel: Option<Rect>,
    presets: Vec<(Preset, Rect)>,
    add: Option<Rect>,
    rows: Vec<(Rect, [Rect; 3])>,
    history: Option<Rect>,
    recreate: Vec<Rect>,
    clear: Option<Rect>,
    clear_answers: Option<[Rect; 2]>,
    error_answers: Option<[Rect; 2]>,
    failed: Vec<(RequestToken, Rect, [Rect; 3])>,
    targets: Vec<FocusTarget>,
    ordered: Vec<Hit>,
    preset_order: Vec<Hit>,
    body_clip: Option<Rect>,
    creation_relative: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct ActionWidths {
    creation: [f32; 3],
    end_of_day_settings: f32,
    add: f32,
    row: [f32; 4],
    error: [f32; 2],
    recreate: f32,
    clear: f32,
    clear_answers: [f32; 2],
    failed: [f32; 3],
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct CreationGeometryState {
    field_editable: bool,
    actions_ready: bool,
    structured_editable: bool,
    can_set: bool,
    load_failed: bool,
    needs_end_of_day: bool,
    relative: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct GeometryKey {
    viewport: (f32, f32),
    mode: Option<Mode>,
    anchor: Rect,
    expanded: bool,
    presets: Vec<Preset>,
    management_scroll: f32,
    action_widths: ActionWidths,
    creation: CreationGeometryState,
    management: Option<Management>,
    optimistic: Vec<OptimisticCreation>,
}

impl Default for ActionWidths {
    fn default() -> Self {
        Self {
            creation: [126.0, 112.0, 76.0],
            end_of_day_settings: 220.0,
            add: 112.0,
            row: [48.0; 4],
            error: [118.0, 92.0],
            recreate: 112.0,
            clear: 112.0,
            clear_answers: [118.0, 76.0],
            failed: [46.0, 42.0, 46.0],
        }
    }
}

impl Geometry {
    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        for (token, pill, actions) in &self.failed {
            for (i, rect) in actions.iter().enumerate() {
                if rect.contains(x, y) {
                    return Some(Hit::Failed(
                        *token,
                        [
                            FailedAction::Retry,
                            FailedAction::Edit,
                            FailedAction::Dismiss,
                        ][i],
                    ));
                }
            }
            if pill.contains(x, y) {
                return Some(Hit::Panel);
            }
        }
        if let Some(target) = self.targets.iter().find(|target| {
            target.enabled
                && target.rect.contains(x, y)
                && self
                    .body_clip
                    .is_none_or(|clip| !is_body_hit(target.hit) || clip.contains(x, y))
        }) {
            return Some(target.hit);
        }
        self.panel
            .filter(|panel| panel.contains(x, y))
            .map(|_| Hit::Panel)
    }

    fn rebuild_order(&mut self) {
        self.ordered.clear();
        self.preset_order.clear();
        for target in &self.targets {
            if target.enabled
                && self
                    .body_clip
                    .is_none_or(|clip| !is_body_hit(target.hit) || clip.intersects(target.rect))
            {
                self.ordered.push(target.hit);
                if matches!(target.hit, Hit::Preset(_)) {
                    self.preset_order.push(target.hit);
                }
            }
        }
    }

    fn ordered(&self) -> &[Hit] {
        &self.ordered
    }

    fn enabled(&self, hit: Hit) -> bool {
        self.targets
            .iter()
            .any(|target| target.hit == hit && target.enabled)
    }
}

/// Reserve one id for one opening of the reminder popover.
///
/// **Monotonic and never reused.** A screen reader addresses a control by
/// reading the tree, so it can hold an id from a popover that has since closed
/// — and §5 requires that request to be *refused* rather than land on whatever
/// opened next. Neither the anchored node nor the request token can do that
/// job: the node is the same when a person reopens the same item's reminder,
/// and a management opening has no request token at all. A number that only
/// moves forward can, and it is the mechanism `ContextMenu::instance` already
/// uses for the same reason.
///
/// Nothing but the accessibility projection reads it, and nothing persists it.
fn mint_surface_instance() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn is_body_hit(hit: Hit) -> bool {
    matches!(
        hit,
        Hit::Row(..)
            | Hit::History
            | Hit::Recreate(_)
            | Hit::Clear
            | Hit::ConfirmClear
            | Hit::CancelClear
            | Hit::RetryCurrent
            | Hit::DiscardCurrent
    )
}

#[derive(Clone, Debug)]
pub(super) enum Action {
    Cancel,
    Confirm(Draft),
    Phrase(String),
    Schedule(ReminderDraft),
    Add(NodeId),
    Recreate(Draft),
    Management(ManagementIntent),
    ToggleHistory,
    Retry(RequestToken),
    EditFailed(RequestToken),
    Dismiss(RequestToken),
    RetryCurrent,
    DiscardCurrent,
    OpenEndOfDaySettings,
}
#[derive(Clone, Debug, Default)]
pub(super) struct Outcome {
    pub handled: bool,
    pub action: Option<Action>,
}

pub(super) struct Surface {
    mode: Option<Mode>,
    /// This opening, for the accessibility projection. See
    /// [`mint_surface_instance`] and [`Surface::instance`].
    instance: u64,
    owner: Option<RequestIdentity>,
    node: Option<NodeId>,
    anchor: Rect,
    input: Option<TextInput>,
    date: Option<TextInput>,
    time: Option<TextInput>,
    expanded: bool,
    focus: Option<Hit>,
    hover: Option<Hit>,
    editing: Option<Editing>,
    edit_creation: Option<Creation>,
    source: DraftSource,
    context: Option<ResolutionContext>,
    default_tone: Tone,
    locale: LocaleFormat,
    cached_creation: Option<Creation>,
    wall: chrono::NaiveDateTime,
    presets: Vec<Preset>,
    management_scroll: f32,
    structured_schedule_cache: Option<ReminderDraft>,
    structured_dirty: bool,
    pending_schedule: Option<ReminderDraft>,
    labels: Vec<(Draft, String)>,
    authored_zone_label: Option<String>,
    action_widths: ActionWidths,
    measured_lang: Option<Lang>,
    creation_message: String,
    creation_error: bool,
    geometry: Option<Box<Geometry>>,
    geometry_key: Option<GeometryKey>,
}

impl Default for Surface {
    fn default() -> Self {
        Self {
            mode: None,
            instance: 0,
            owner: None,
            node: None,
            anchor: Rect::new(0.0, 44.0, 1.0, 1.0),
            input: None,
            date: None,
            time: None,
            expanded: false,
            focus: None,
            hover: None,
            editing: None,
            edit_creation: None,
            source: DraftSource::Natural,
            context: None,
            default_tone: Tone::System,
            locale: LocaleFormat::default(),
            cached_creation: None,
            wall: NaiveDate::from_ymd_opt(1970, 1, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
            presets: vec![Preset::LaterToday, Preset::Tomorrow, Preset::Custom],
            management_scroll: 0.0,
            structured_schedule_cache: None,
            structured_dirty: false,
            pending_schedule: None,
            labels: Vec::new(),
            authored_zone_label: None,
            action_widths: ActionWidths::default(),
            measured_lang: None,
            creation_message: String::new(),
            creation_error: false,
            geometry: Some(Box::default()),
            geometry_key: None,
        }
    }
}

impl Surface {
    pub(super) fn sync_creation(
        &mut self,
        engine: &mut TextEngine,
        creation: Option<&Creation>,
        anchor: Option<Rect>,
    ) -> bool {
        let Some(creation) = creation else {
            if self.mode == Some(Mode::Creation) && self.editing.is_none() {
                self.close();
                return true;
            }
            if self.mode == Some(Mode::Creation) {
                let draft = self
                    .edit_creation
                    .as_ref()
                    .and_then(Creation::editable_draft)
                    .cloned();
                self.refresh_structured_fields(engine, draft.as_ref());
            }
            return false;
        };
        self.cached_creation = Some(creation.clone());
        self.refresh_structured_fields(engine, creation.editable_draft());
        if self.owner == Some(creation.request.identity) && self.mode == Some(Mode::Creation) {
            if let Some(anchor) = anchor {
                self.anchor = anchor;
            }
            if matches!(
                creation.state,
                CreationState::Ambiguity
                    | CreationState::EndOfDay
                    | CreationState::Structured { .. }
            ) {
                self.expanded = true;
                self.ensure_structured(engine, creation.editable_draft());
                self.refresh_structured_fields(engine, creation.editable_draft());
            }
            return false;
        }
        self.mode = Some(Mode::Creation);
        self.instance = mint_surface_instance();
        self.owner = Some(creation.request.identity);
        self.node = Some(creation.request.identity.node);
        if let Some(anchor) = anchor {
            self.anchor = anchor;
        }
        self.input = Some(TextInput::with_text(
            engine,
            theme::role::body(),
            &creation.request.phrase,
        ));
        self.date = None;
        self.time = None;
        self.expanded = false;
        self.focus = Some(Hit::Field);
        self.hover = None;
        self.editing = None;
        self.edit_creation = None;
        self.source = DraftSource::Natural;
        self.structured_schedule_cache = None;
        self.structured_dirty = false;
        self.pending_schedule = None;
        self.labels.clear();
        self.authored_zone_label = None;
        if matches!(
            creation.state,
            CreationState::Ambiguity | CreationState::EndOfDay | CreationState::Structured { .. }
        ) {
            self.expanded = true;
            self.ensure_structured(engine, creation.editable_draft());
            self.refresh_structured_fields(engine, creation.editable_draft());
        }
        true
    }

    #[cfg(test)]
    pub(super) fn open_management(&mut self, node: NodeId, anchor: Rect) {
        self.close();
        self.mode = Some(Mode::Management);
        self.instance = mint_surface_instance();
        self.node = Some(node);
        self.anchor = anchor;
        self.focus = None;
        self.management_scroll = 0.0;
    }

    pub(super) fn open_existing_edit(
        &mut self,
        engine: &mut TextEngine,
        node: NodeId,
        anchor: Rect,
        id: String,
        draft: Draft,
        paused: bool,
    ) {
        self.close();
        self.node = Some(node);
        self.anchor = anchor;
        self.mode = Some(Mode::Creation);
        self.instance = mint_surface_instance();
        self.expanded = true;
        self.editing = Some(Editing {
            id,
            draft: draft.clone(),
            paused,
        });
        self.input = Some(TextInput::new(engine, theme::role::body()));
        let request = Request {
            identity: RequestIdentity {
                token: RequestToken::new(),
                node,
            },
            phrase: String::new(),
            due_anchor: self.context.as_ref().and_then(|c| c.due_anchor.clone()),
        };
        self.edit_creation = Some(if let Some(context) = self.context.as_ref() {
            Creation::reviewing(request, draft.clone(), context, self.default_tone)
        } else {
            Creation {
                request,
                state: CreationState::Confirmation(draft.clone()),
                focus: koru_editor::reminders::CreationFocus::Editor,
                default_tone: Some(self.default_tone),
                tone_override: draft.tone_override,
            }
        });
        self.source = DraftSource::Natural;
        self.focus = Some(Hit::Field);
        self.ensure_structured(engine, Some(&draft));
    }
    pub(super) fn reset_profile(&mut self) {
        self.close();
        self.node = None;
    }

    pub(super) fn close_context(&mut self) {
        self.close();
        self.node = None;
        self.context = None;
    }
    fn close(&mut self) {
        self.mode = None;
        self.owner = None;
        self.input = None;
        self.date = None;
        self.time = None;
        self.expanded = false;
        self.focus = None;
        self.hover = None;
        self.editing = None;
        self.edit_creation = None;
        self.cached_creation = None;
        self.source = DraftSource::Natural;
        self.authored_zone_label = None;
        // Geometry is a snapshot of `mode`. Retaining it after closing lets a
        // redraw paint the old panel or hand it to a newly selected mode before
        // maintenance republishes the matching snapshot.
        self.geometry = Some(Box::default());
        self.geometry_key = None;
    }
    pub(super) fn is_open(&self) -> bool {
        self.mode.is_some()
    }

    pub(super) fn contains(&self, x: f32, y: f32) -> bool {
        self.geometry()
            .panel
            .is_some_and(|panel| panel.contains(x, y))
    }

    pub(super) fn is_creation(&self) -> bool {
        self.mode == Some(Mode::Creation)
    }

    pub(super) fn is_management(&self) -> bool {
        self.mode == Some(Mode::Management)
    }

    pub(super) fn dictation_anchor(&self) -> Rect {
        self.anchor
    }

    pub(super) fn node(&self) -> Option<NodeId> {
        self.node
    }

    #[cfg(test)]
    pub(super) fn management_scroll_for_test(&self) -> f32 {
        self.management_scroll
    }

    /// Called only from MainView maintenance. Drawing, geometry, and input use
    /// this cached clock/zone rather than consulting the operating system.
    pub(super) fn update_context(
        &mut self,
        context: ResolutionContext,
        locale: LocaleFormat,
        default_tone: Tone,
    ) {
        let semantic_context_changed = self.context.as_ref().is_none_or(|old| {
            old.language != context.language
                || old.date_order != context.date_order
                || old.current_zone != context.current_zone
                || old.selection != context.selection
                || old.end_of_day != context.end_of_day
                || old.due_anchor != context.due_anchor
        }) || self.default_tone != default_tone;
        self.wall = IanaZoneRules::from_name(&context.current_zone)
            .map(|rules| rules.wall_of(context.now_ms))
            .unwrap_or_else(|_| {
                chrono::DateTime::from_timestamp_millis(context.now_ms)
                    .map(|instant| instant.naive_utc())
                    .unwrap_or(self.wall)
            });
        self.presets = presets_at(self.wall.time());
        self.context = Some(context);
        self.locale = locale;
        self.default_tone = default_tone;
        if semantic_context_changed
            && let (Some(editing), Some(edit), Some(context)) = (
                self.editing.as_ref(),
                self.edit_creation.as_mut(),
                self.context.as_ref(),
            )
        {
            let phrase = self
                .input
                .as_ref()
                .map(TextInput::value)
                .unwrap_or_default();
            if phrase.trim().is_empty() {
                *edit = Creation::reviewing(
                    edit.request.clone(),
                    editing.draft.clone(),
                    context,
                    default_tone,
                );
            } else {
                let tone = editing.draft.tone_override;
                edit.request.phrase = phrase;
                edit.resolve_preserving(context, default_tone, tone);
            }
        }
    }

    /// Build every schedule-derived value and measured action width during
    /// MainView maintenance. Draw, hit testing, and key handling consume this
    /// projection and never consult timezone rules or format dates themselves.
    pub(super) fn maintain_projection(
        &mut self,
        engine: &mut TextEngine,
        viewport: (f32, f32),
        lang: Lang,
        creation: Option<&Creation>,
        management: Option<&Management>,
        optimistic: &[OptimisticCreation],
    ) -> bool {
        if self.structured_dirty {
            self.structured_dirty = false;
            self.structured_schedule_cache = self.compute_structured_schedule();
            if let Some(schedule) = self.structured_schedule_cache.clone() {
                if let Some(editing) = &mut self.editing {
                    editing.draft.schedule = schedule.clone();
                }
                if let Some(edit) = &mut self.edit_creation {
                    edit.replace_draft(schedule.clone());
                }
                if self.editing.is_none() {
                    self.pending_schedule = Some(schedule);
                }
            }
        }
        let before_labels = self.labels.clone();
        let before_authored_zone = self.authored_zone_label.clone();
        self.labels.clear();
        let mut drafts = Vec::new();
        let active_creation = self.active_creation(creation).cloned();
        self.authored_zone_label = active_creation
            .as_ref()
            .and_then(Creation::editable_draft)
            .and_then(|draft| match &draft.schedule {
                ReminderDraft::Zoned(value) => Some(value.zone().to_string()),
                ReminderDraft::Floating(_) | ReminderDraft::Relative { .. } => None,
            });
        if let Some(creation) = active_creation.as_ref() {
            match &creation.state {
                CreationState::Confirmation(draft)
                | CreationState::Saving(draft)
                | CreationState::Failed { draft, .. } => drafts.push(draft),
                CreationState::Passed {
                    requested,
                    proposal,
                } => {
                    drafts.push(requested);
                    drafts.push(proposal);
                }
                _ => {}
            }
        }
        if let Some(management) = management {
            drafts.extend(management.active.iter().map(|item| &item.draft));
            drafts.extend(management.history.iter().map(|item| &item.draft));
        }
        drafts.extend(optimistic.iter().map(|item| &item.draft));
        for draft in drafts {
            if !self.labels.iter().any(|(cached, _)| cached == draft) {
                self.labels.push((
                    draft.clone(),
                    format_draft_label_during_maintenance(
                        lang,
                        &self.locale,
                        &self
                            .context
                            .as_ref()
                            .map_or_else(TimeZoneName::utc, |context| context.current_zone.clone()),
                        draft,
                    ),
                ));
            }
        }

        self.measure_actions(engine, lang);
        let next_message = active_creation
            .as_ref()
            .map(|creation| self.creation_message(lang, creation))
            .unwrap_or_else(|| t(lang, Key::ReminderLoading).to_owned());
        let next_error = matches!(
            active_creation.as_ref().map(|creation| &creation.state),
            Some(CreationState::LoadFailed { .. } | CreationState::Failed { .. })
        );
        let message_changed =
            self.creation_message != next_message || self.creation_error != next_error;
        self.creation_message = next_message;
        self.creation_error = next_error;
        let creation_geometry = self.creation_geometry_state(creation);
        let geometry_changed =
            self.publish_geometry(viewport, creation_geometry, management, optimistic);
        before_labels != self.labels
            || before_authored_zone != self.authored_zone_label
            || message_changed
            || geometry_changed
    }

    /// One semantic creation is active. Existing-item edits are surface-owned;
    /// slash/preset creation is controller-owned; the cache only bridges a
    /// frame where the controller reference is unavailable.
    fn active_creation<'a>(&'a self, controller: Option<&'a Creation>) -> Option<&'a Creation> {
        self.edit_creation
            .as_ref()
            .or(controller)
            .or(self.cached_creation.as_ref())
    }

    fn selected_draft_ref<'a>(&'a self, creation: Option<&'a Creation>) -> Option<&'a Draft> {
        if self.source == DraftSource::Structured {
            return self.structured_schedule_cache.as_ref().map(|_| {
                self.edit_creation
                    .as_ref()
                    .and_then(Creation::editable_draft)
                    .or_else(|| creation.and_then(Creation::editable_draft))
                    .or_else(|| {
                        self.cached_creation
                            .as_ref()
                            .and_then(Creation::editable_draft)
                    })
            })?;
        }
        self.edit_creation
            .as_ref()
            .and_then(Creation::confirmable_draft)
            .or_else(|| creation.and_then(Creation::confirmable_draft))
            .or_else(|| {
                self.cached_creation
                    .as_ref()
                    .and_then(Creation::confirmable_draft)
            })
    }

    fn selected_schedule_ref<'a>(
        &'a self,
        creation: Option<&'a Creation>,
    ) -> Option<&'a ReminderDraft> {
        if self.source == DraftSource::Structured {
            return self.structured_schedule_cache.as_ref();
        }
        self.selected_draft_ref(creation)
            .map(|draft| &draft.schedule)
    }

    fn creation_geometry_state(&self, creation: Option<&Creation>) -> CreationGeometryState {
        let state = self
            .active_creation(creation)
            .map(|creation| &creation.state);
        let field_editable = !matches!(
            state,
            Some(CreationState::LoadFailed { .. } | CreationState::Saving(_))
        );
        let actions_ready = field_editable && !matches!(state, Some(CreationState::Loading));
        let schedule = self.selected_schedule_ref(creation);
        let relative = matches!(schedule, Some(ReminderDraft::Relative { .. }));
        CreationGeometryState {
            field_editable,
            actions_ready,
            structured_editable: actions_ready && !relative,
            can_set: schedule.is_some(),
            load_failed: matches!(state, Some(CreationState::LoadFailed { .. })),
            needs_end_of_day: matches!(state, Some(CreationState::EndOfDay)),
            relative,
        }
    }

    fn publish_geometry(
        &mut self,
        viewport: (f32, f32),
        creation: CreationGeometryState,
        management: Option<&Management>,
        optimistic: &[OptimisticCreation],
    ) -> bool {
        let unchanged = self.geometry_key.as_ref().is_some_and(|key| {
            key.viewport == viewport
                && key.mode == self.mode
                && key.anchor == self.anchor
                && key.expanded == self.expanded
                && key.presets == self.presets
                && key.management_scroll == self.management_scroll
                && key.action_widths == self.action_widths
                && key.creation == creation
                && key.management.as_ref() == management
                && key.optimistic == optimistic
        });
        if unchanged {
            return false;
        }
        self.geometry = Some(Box::new(
            self.build_geometry(viewport, management, optimistic, creation),
        ));
        self.geometry_key = Some(GeometryKey {
            viewport,
            mode: self.mode,
            anchor: self.anchor,
            expanded: self.expanded,
            presets: self.presets.clone(),
            management_scroll: self.management_scroll,
            action_widths: self.action_widths.clone(),
            creation,
            management: management.cloned(),
            optimistic: optimistic.to_vec(),
        });
        // **A pill that is gone does not keep the keyboard.** All three of its
        // buttons end the pill they sit on — Try again resubmits, Edit reopens
        // the popover, Dismiss throws the draft away — so the focus `F6` put
        // there has to be let go the moment the rectangle stops being drawn,
        // or `accessibility_focus` would name a control no longer in the tree
        // and the ring would be painted on nothing. Let go *here*, where the
        // geometry that decides it was just rebuilt, rather than in each of
        // the three places a pill can end.
        if let Some(Hit::Failed(token, _)) = self.focus
            && !self
                .geometry()
                .failed
                .iter()
                .any(|(standing, ..)| *standing == token)
        {
            self.focus = None;
        }
        true
    }

    fn geometry(&self) -> &Geometry {
        self.geometry.as_deref().unwrap()
    }

    fn cached_management(&self) -> Option<&Management> {
        self.geometry_key
            .as_ref()
            .and_then(|key| key.management.as_ref())
    }

    // ----------------------------------------------- what a screen reader asks
    //
    // **Nothing below computes anything.** Each one hands back state this
    // module already keeps for drawing, hit-testing or the Tab order, so the
    // semantic tree in `accessibility/reminders.rs` cannot describe a control
    // somewhere other than where a press lands on it, nor offer one the
    // keyboard cannot reach (ADR-338). The projection itself lives beside its
    // own plan's module rather than here, because this file is 3,500 lines and
    // §7.10 is explicit that it gets calls rather than another subsystem.

    /// This opening of this popover, numbered once and never reused.
    ///
    /// Same reason a context menu has one: close the popover while a client
    /// holds one of its controls, open it again over a different item, and a
    /// request queued against the first must not land in the second. See
    /// [`mint_surface_instance`].
    pub(super) fn instance(&self) -> u64 {
        self.instance
    }

    /// Which of the two surfaces is standing, if either.
    pub(super) fn mode(&self) -> Option<Mode> {
        self.mode
    }

    /// The panel a person sees, in this window's logical coordinates.
    pub(super) fn panel(&self) -> Option<Rect> {
        self.geometry().panel
    }

    /// Which control has the keyboard, read from the state the focus ring is
    /// drawn from — not a second idea of focus kept in step with it (§7.1).
    pub(super) fn focus(&self) -> Option<Hit> {
        self.focus
    }

    /// Every control this popover offers, with the rectangle drawing and
    /// hit-testing already share, and whether it can be used.
    ///
    /// A **disabled** target is included: a screen reader announcing *Set
    /// reminder, unavailable* is how a person learns their phrase has not
    /// resolved yet, and a control that vanishes is one they think they
    /// imagined. [`Surface::tab_order`] is the keyboard's own list and
    /// excludes them, so the position a reader hears counts what they can
    /// actually reach.
    pub(super) fn controls(&self) -> impl Iterator<Item = (Hit, Rect, bool)> + '_ {
        self.geometry()
            .targets
            .iter()
            .map(|target| (target.hit, target.rect, target.enabled))
    }

    /// The Tab order: the enabled, unclipped controls, in the order the key
    /// cycles them.
    pub(super) fn tab_order(&self) -> &[Hit] {
        self.geometry().ordered()
    }

    /// The recovery pills a failed save leaves standing, outside the panel.
    pub(super) fn failed_pills(&self) -> impl Iterator<Item = (RequestToken, Rect)> + '_ {
        self.geometry()
            .failed
            .iter()
            .map(|(token, pill, _)| (*token, *pill))
    }

    // ------------------------------------------- the recovery pill's keyboard
    //
    // A recovery pill is the one thing this module draws that stands with **no
    // popover open**: the save it belongs to failed after the popover closed,
    // it floats outside every panel, and no blocker names it — so at that
    // moment nothing in the window owns the keyboard and there is no Tab order
    // for it to be in. `F6` is what reaches it (`MainView::focus_chrome`), and
    // these five are the whole of what that needs from here.
    //
    // **There is no second focus field.** A pill that has the keyboard is
    // `Surface::focus` holding a `Hit::Failed`, which is the state `draw`
    // paints its ring from and the state `accessibility/reminders.rs` reads
    // (ADR-338, §7.1).

    /// Whether a standing recovery pill currently has the keyboard.
    pub(super) fn pill_focused(&self) -> bool {
        matches!(self.focus, Some(Hit::Failed(..)))
    }

    /// Whether there is a recovery pill to reach at all.
    ///
    /// What decides whether `F6` has a stop here this press — read from the
    /// same `geometry().failed` that draws them, so a stop can never exist in
    /// the rotation and not on the screen.
    pub(super) fn has_failed_pill(&self) -> bool {
        !self.geometry().failed.is_empty()
    }

    /// Put the keyboard on the first standing pill's **Try again**.
    ///
    /// The first, rather than the most recent: `failed` is in the order the
    /// pills are drawn, so this is the one at the top of the stack — and *Try
    /// again* rather than the pill itself, because the pill is a group and the
    /// three buttons are what a person can act on. Tab moves among them.
    pub(super) fn focus_first_failed_pill(&mut self) -> bool {
        let Some((token, ..)) = self.geometry().failed.first().copied() else {
            return false;
        };
        self.take_focus(Hit::Failed(token, FailedAction::Retry))
    }

    /// Give the keyboard back, leaving every other focus alone.
    ///
    /// `None` is what an unfocused surface holds, and what
    /// `MainView::accessibility_focus` reads as *the keyboard is somewhere
    /// else* — so Escape and `F6` onward both land on the content region rather
    /// than nowhere.
    pub(super) fn blur_pill(&mut self) -> bool {
        if !self.pill_focused() {
            return false;
        }
        self.focus = None;
        true
    }

    /// The keys a focused recovery pill answers, and **only** those.
    ///
    /// The same bargain the reminder bell strikes with `F6`: a chrome stop
    /// claims what moves it and what activates it, and lets everything else
    /// through to whatever would otherwise have had the key. A pill that
    /// swallowed typing would be a pill that stopped the document dead while a
    /// failure stood.
    ///
    /// Tab and the arrows cycle **across every standing pill's buttons**, not
    /// just this one's: three refusals at once are three pills, each recovering
    /// its own request, and a rotation that stopped at the first would strand
    /// the other two.
    fn failed_pill_key(
        &mut self,
        engine: &mut TextEngine,
        key: &KeyboardKey,
        mods: ModifiersState,
    ) -> Outcome {
        let hits: Vec<Hit> = self
            .geometry()
            .failed
            .iter()
            .flat_map(|(token, ..)| {
                [
                    FailedAction::Retry,
                    FailedAction::Edit,
                    FailedAction::Dismiss,
                ]
                .map(|action| Hit::Failed(*token, action))
            })
            .collect();
        if hits.is_empty() {
            // The pill went while the key was in flight. Let go rather than
            // hold a control that is not drawn.
            self.focus = None;
            return Outcome::default();
        }
        match key {
            KeyboardKey::Named(NamedKey::Escape) => {
                self.focus = None;
                Outcome {
                    handled: true,
                    action: None,
                }
            }
            KeyboardKey::Named(NamedKey::Tab) => {
                self.focus = Some(cycle_focus(&hits, self.focus, mods.shift_key()));
                Outcome {
                    handled: true,
                    action: None,
                }
            }
            KeyboardKey::Named(NamedKey::ArrowRight | NamedKey::ArrowDown) => {
                self.focus = Some(cycle_focus(&hits, self.focus, false));
                Outcome {
                    handled: true,
                    action: None,
                }
            }
            KeyboardKey::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp) => {
                self.focus = Some(cycle_focus(&hits, self.focus, true));
                Outcome {
                    handled: true,
                    action: None,
                }
            }
            key if is_activation_key(key) => match self.focus {
                Some(hit) => self.activate(engine, hit),
                None => Outcome::default(),
            },
            _ => Outcome::default(),
        }
    }

    /// The one sentence the panel draws about what is happening, and whether
    /// it is a failure.
    ///
    /// This is the whole of *preview and validation*: `Scheduled for …`, `When
    /// should Koru remind you?`, `Saving reminder…`, `{} has passed. Use {}
    /// instead?`, and every `ReminderError*`. Built during maintenance by
    /// [`Surface::creation_message`], localized there, and read here so the
    /// drawn line and the spoken one are one string.
    pub(super) fn message(&self) -> (&str, bool) {
        (&self.creation_message, self.creation_error)
    }

    /// Whether the structured date/time pair currently says something that
    /// cannot be read as a moment — the red `ReminderErrorInvalidData` line
    /// under the fields, from the same expression `draw_creation` evaluates.
    pub(super) fn structured_is_invalid(&self) -> bool {
        self.expanded
            && self.source == DraftSource::Structured
            && self.structured_schedule_cache.is_none()
    }

    /// What is in a field, **as it is on the screen**.
    ///
    /// `raw_text`, not `value`: the trimmed value is what the form acts on,
    /// and what is drawn — leading spaces, a dictation still in progress — is
    /// what a person's screen reader has to agree with. Same rule, and the
    /// same reason, as the first-run form's fields.
    pub(super) fn field_text(&self, hit: Hit) -> Option<&str> {
        match hit {
            Hit::Field => self.input.as_ref(),
            Hit::Date => self.date.as_ref(),
            Hit::Time => self.time.as_ref(),
            _ => None,
        }
        .map(TextInput::raw_text)
    }

    /// The management snapshot the panel is showing — the rows the Store
    /// answered with, as the last geometry was built from them.
    pub(super) fn management(&self) -> Option<&Management> {
        self.cached_management()
    }

    /// The stored state of the saved reminder a management row stands for.
    pub(super) fn row_state(&self, index: usize) -> Option<&ItemState> {
        self.cached_management()
            .and_then(|state| state.active.get(index))
            .map(|item| &item.state)
    }

    /// **The word drawn on a control, in one place.**
    ///
    /// Every [`draw_action`] call site and every preset row reads this, and so
    /// does the semantic tree, so a screen reader cannot name a control
    /// something other than what a sighted person sees on it. One of them
    /// changes with state — the pause row becomes *Resume* — and that is
    /// exactly the drift a second table would grow.
    pub(super) fn hit_label(&self, lang: Lang, hit: Hit) -> String {
        match hit {
            Hit::Panel => t(
                lang,
                match self.mode {
                    Some(Mode::Management) => Key::ReminderTitle,
                    _ => Key::Reminder,
                },
            )
            .to_owned(),
            Hit::Field => t(lang, Key::ReminderWhenPlaceholder).to_owned(),
            Hit::Date => t(lang, Key::ReminderDate).to_owned(),
            Hit::Time => t(lang, Key::ReminderTime).to_owned(),
            Hit::More => t(lang, Key::ReminderMoreOptions).to_owned(),
            Hit::EndOfDaySettings => t(lang, Key::ReminderSetEndOfDayFirst).to_owned(),
            Hit::Set => t(lang, Key::ReminderSet).to_owned(),
            Hit::Cancel | Hit::CancelClear => t(lang, Key::ReminderCancel).to_owned(),
            Hit::Preset(Preset::LaterToday) => t(lang, Key::ReminderLaterToday).to_owned(),
            Hit::Preset(Preset::Tomorrow) => t(lang, Key::ReminderTomorrow).to_owned(),
            Hit::Preset(Preset::Custom) => t(lang, Key::ReminderCustom).to_owned(),
            Hit::Add => t(lang, Key::ReminderAdd).to_owned(),
            Hit::Row(index, action) => {
                let paused = self.row_state(index) == Some(&ItemState::Paused);
                t(
                    lang,
                    match action {
                        RowAction::Edit => Key::ReminderEdit,
                        RowAction::Pause if paused => Key::ReminderResume,
                        RowAction::Pause => Key::ReminderPause,
                        RowAction::Delete => Key::ReminderDelete,
                    },
                )
                .to_owned()
            }
            Hit::History => t(lang, Key::ReminderHistory).to_owned(),
            Hit::Recreate(_) => t(lang, Key::ReminderRecreate).to_owned(),
            Hit::Clear | Hit::ConfirmClear => t(lang, Key::ReminderClearHistory).to_owned(),
            Hit::Failed(_, FailedAction::Retry) | Hit::RetryCurrent => {
                t(lang, Key::ReminderRetry).to_owned()
            }
            Hit::Failed(_, FailedAction::Edit) => t(lang, Key::ReminderEdit).to_owned(),
            Hit::Failed(_, FailedAction::Dismiss) | Hit::DiscardCurrent => {
                t(lang, Key::ReminderDismiss).to_owned()
            }
        }
    }

    /// The schedule a management row or a history entry shows, already
    /// localized in the display zone by maintenance. `None` for every other
    /// control, and for a row whose management snapshot has gone.
    pub(super) fn hit_schedule(&self, lang: Lang, hit: Hit) -> Option<String> {
        let state = self.cached_management()?;
        match hit {
            Hit::Row(index, _) => state
                .active
                .get(index)
                .map(|item| self.draft_label(lang, &item.draft).to_owned()),
            Hit::Recreate(index) => state
                .history
                .get(index)
                .map(|item| self.draft_label(lang, &item.draft).to_owned()),
            _ => None,
        }
    }

    /// Do what a client asked, through the same call the keyboard or the
    /// pointer reaches.
    ///
    /// **The gates are the ones the input path already applies**, because an
    /// advertised action set is a hint rather than a filter
    /// (`guide/contributing/deps/ACCESSKIT.md`) and the refusal therefore has to live in the
    /// owner. For a control in the panel that is `key`'s pair — enabled, and in
    /// the Tab order — so an assistive technology cannot reach one a person
    /// cannot. For a **recovery pill** it is `Geometry::hit`'s: the pill has to
    /// be one of the ones standing. Those two are different because the pill
    /// itself is: it floats outside the panel, it is there whether or not a
    /// popover is open, and the pointer reaches it either way while the Tab
    /// order never has.
    pub(super) fn invoke(&mut self, engine: &mut TextEngine, hit: Hit) -> Option<Outcome> {
        self.take_focus(hit).then(|| self.activate(engine, hit))
    }

    /// Put the keyboard on a control, which is the whole of what a focus
    /// request means here: [`Surface::focus`] is the state the ring is painted
    /// from and the state `key` acts on, so moving it is moving the keyboard.
    pub(super) fn take_focus(&mut self, hit: Hit) -> bool {
        let reachable = match hit {
            Hit::Failed(token, _) => self
                .geometry()
                .failed
                .iter()
                .any(|(standing, ..)| *standing == token),
            _ => self.geometry().enabled(hit) && self.geometry().ordered().contains(&hit),
        };
        if reachable {
            self.focus = Some(hit);
        }
        reachable
    }

    pub(super) fn take_pending_schedule(&mut self) -> Option<ReminderDraft> {
        self.pending_schedule.take()
    }

    fn measure_actions(&mut self, engine: &mut TextEngine, lang: Lang) {
        if self.measured_lang == Some(lang) {
            return;
        }
        self.measured_lang = Some(lang);
        let width = |engine: &mut TextEngine, key| {
            ActionButton::new(t(lang, key), ActionStyle::Ghost, None).width(engine)
        };
        self.action_widths.creation = [
            width(engine, Key::ReminderMoreOptions),
            width(engine, Key::ReminderSet),
            width(engine, Key::ReminderCancel),
        ];
        self.action_widths.end_of_day_settings = engine
            .simple_layout(t(lang, Key::ReminderSetEndOfDayFirst), theme::role::row())
            .width();
        self.action_widths.add = width(engine, Key::ReminderAdd);
        self.action_widths.row = [
            width(engine, Key::ReminderEdit),
            width(engine, Key::ReminderPause),
            width(engine, Key::ReminderResume),
            width(engine, Key::ReminderDelete),
        ];
        self.action_widths.error = [
            width(engine, Key::ReminderRetry),
            width(engine, Key::ReminderDismiss),
        ];
        self.action_widths.recreate = width(engine, Key::ReminderRecreate);
        self.action_widths.clear = width(engine, Key::ReminderClearHistory);
        self.action_widths.clear_answers = [
            width(engine, Key::ReminderClearHistory),
            width(engine, Key::ReminderCancel),
        ];
        self.action_widths.failed = [
            width(engine, Key::ReminderRetry),
            width(engine, Key::ReminderEdit),
            width(engine, Key::ReminderDismiss),
        ];
    }

    pub(super) fn dictate(
        &mut self,
        engine: &mut TextEngine,
        action: &koru_dictation::DictationAction,
    ) -> Option<String> {
        let input = self.input.as_mut()?;
        let mut compose = koru_ui::dictation::ComposeInto::Input(input, engine);
        let handled = match action {
            koru_dictation::DictationAction::Begin { .. } => compose.begin(),
            koru_dictation::DictationAction::Update { text, .. } => {
                compose.update(text.as_str());
                true
            }
            koru_dictation::DictationAction::Commit { text, .. } => {
                compose.commit(text.as_str());
                true
            }
            koru_dictation::DictationAction::Cancel { .. } => {
                compose.cancel();
                true
            }
        };
        handled.then(|| self.input.as_ref().unwrap().value())
    }

    /// Resolve dictated or typed text against this surface's existing-item
    /// edit. New reminder creation remains controller-owned.
    pub(super) fn resolve_edit_phrase(&mut self, phrase: String) -> Option<String> {
        let Some(edit) = &mut self.edit_creation else {
            return Some(phrase);
        };
        let Some(context) = &self.context else {
            return None;
        };
        if edit.request.phrase == phrase || matches!(edit.state, CreationState::Saving(_)) {
            return None;
        }
        let tone = edit
            .editable_draft()
            .and_then(|draft| draft.tone_override)
            .or(edit.tone_override);
        self.source = DraftSource::Natural;
        edit.request.phrase = phrase;
        edit.resolve_preserving(context, self.default_tone, tone);
        None
    }

    pub(super) fn cursor(&self, x: f32, y: f32) -> Option<CursorIcon> {
        self.geometry().hit(x, y).map(|hit| {
            if matches!(hit, Hit::Field | Hit::Date | Hit::Time) {
                CursorIcon::Text
            } else {
                CursorIcon::Default
            }
        })
    }

    pub(super) fn pointer_move(&mut self, engine: &mut TextEngine, x: f32, y: f32) -> bool {
        let field = self.geometry().field;
        let date = self.geometry().date;
        let time = self.geometry().time;
        for (input, rect) in [
            (&mut self.input, field),
            (&mut self.date, date),
            (&mut self.time, time),
        ] {
            if let (Some(input), Some(rect)) = (input, rect)
                && input.drag(engine, rect, x)
            {
                return true;
            }
        }
        let next = self.geometry().hit(x, y);
        let changed = self.hover != next;
        self.hover = next;
        changed
    }
    pub(super) fn mouse_up(&mut self) -> bool {
        let mut changed = false;
        for input in [&mut self.input, &mut self.date, &mut self.time]
            .into_iter()
            .flatten()
        {
            changed |= input.is_dragging();
            input.release();
        }
        changed
    }

    pub(super) fn mouse_down(&mut self, engine: &mut TextEngine, x: f32, y: f32) -> Outcome {
        let hit = self.geometry().hit(x, y);
        let field = self.geometry().field;
        let date = self.geometry().date;
        let time = self.geometry().time;
        match hit {
            Some(Hit::Field) => {
                self.focus = Some(Hit::Field);
                if let (Some(input), Some(rect)) = (&mut self.input, field) {
                    input.click(engine, rect, x, y);
                }
                Outcome {
                    handled: true,
                    action: None,
                }
            }
            Some(Hit::Date) => {
                self.focus = Some(Hit::Date);
                if let (Some(input), Some(rect)) = (&mut self.date, date) {
                    input.click(engine, rect, x, y);
                }
                Outcome {
                    handled: true,
                    action: None,
                }
            }
            Some(Hit::Time) => {
                self.focus = Some(Hit::Time);
                if let (Some(input), Some(rect)) = (&mut self.time, time) {
                    input.click(engine, rect, x, y);
                }
                Outcome {
                    handled: true,
                    action: None,
                }
            }
            Some(hit) => self.activate(engine, hit),
            None if self.mode.is_some() => {
                let action = (self.mode == Some(Mode::Creation)).then_some(Action::Cancel);
                self.close();
                Outcome {
                    handled: true,
                    action,
                }
            }
            // Nothing under the pointer and no panel to close — which is the
            // case where a recovery pill is the only thing this surface is
            // drawing. **A press elsewhere gives the keyboard back**, the same
            // clearing `Shell::mouse_down` does for the bell, so a ring is
            // never left standing on a control the person has just pressed
            // past. Still *unhandled*: the press belongs to whatever is under
            // it.
            None => {
                self.blur_pill();
                Outcome::default()
            }
        }
    }

    fn activate(&mut self, engine: &mut TextEngine, hit: Hit) -> Outcome {
        self.focus = Some(hit);
        let action = match hit {
            Hit::More => {
                self.expanded = true;
                let draft = self
                    .editing
                    .as_ref()
                    .map(|editing| editing.draft.clone())
                    .or_else(|| {
                        self.cached_creation
                            .as_ref()
                            .and_then(Creation::editable_draft)
                            .cloned()
                    })
                    .or_else(|| {
                        self.cached_management()
                            .and_then(|m| m.active.first())
                            .map(|i| i.draft.clone())
                    });
                self.ensure_structured(engine, draft.as_ref());
                None
            }
            Hit::EndOfDaySettings => {
                self.close();
                Some(Action::OpenEndOfDaySettings)
            }
            Hit::Set => {
                let draft = self.selected_draft(None);
                match (self.editing.as_ref(), draft) {
                    (Some(editing), Some(draft)) => {
                        let id = editing.id.clone();
                        let paused = editing.paused;
                        self.finish_edit();
                        Some(Action::Management(ManagementIntent::Edit {
                            id,
                            draft,
                            paused,
                        }))
                    }
                    (None, Some(draft)) => Some(Action::Confirm(draft)),
                    _ => None,
                }
            }
            Hit::Cancel => {
                self.close();
                Some(Action::Cancel)
            }
            Hit::Preset(Preset::Custom) => {
                self.expanded = true;
                let draft = self
                    .cached_creation
                    .as_ref()
                    .and_then(Creation::editable_draft)
                    .cloned();
                self.ensure_structured(engine, draft.as_ref());
                None
            }
            Hit::Preset(preset) => preset_draft_at(preset, self.wall).map(Action::Confirm),
            Hit::Add => self.node.map(Action::Add),
            Hit::Row(index, RowAction::Edit) => {
                let item = self
                    .cached_management()
                    .and_then(|m| m.active.get(index))
                    .map(|item| {
                        (
                            item.id.clone(),
                            item.draft.clone(),
                            item.state == ItemState::Paused,
                        )
                    });
                if let Some((id, draft, paused)) = item {
                    let node = self.node.unwrap();
                    self.open_existing_edit(engine, node, self.anchor, id, draft.clone(), paused);
                }
                None
            }
            Hit::Row(index, RowAction::Pause) => self
                .cached_management()
                .and_then(|m| m.active.get(index))
                .map(|item| {
                    Action::Management(if item.state == ItemState::Paused {
                        ManagementIntent::Resume {
                            id: item.id.clone(),
                            revision: item.revision.clone(),
                        }
                    } else {
                        ManagementIntent::Pause {
                            id: item.id.clone(),
                            revision: item.revision.clone(),
                        }
                    })
                }),
            Hit::Row(index, RowAction::Delete) => self
                .cached_management()
                .and_then(|m| m.active.get(index))
                .map(|item| {
                    Action::Management(ManagementIntent::Delete {
                        id: item.id.clone(),
                    })
                }),
            Hit::History => Some(Action::ToggleHistory),
            Hit::Recreate(index) => self
                .cached_management()
                .and_then(|m| m.history.get(index))
                .map(|source| Action::Recreate(source.draft.clone())),
            Hit::Clear => Some(Action::Management(ManagementIntent::AskClearHistory)),
            Hit::ConfirmClear => Some(Action::Management(ManagementIntent::ConfirmClearHistory)),
            Hit::CancelClear => Some(Action::Management(ManagementIntent::CancelClearHistory)),
            Hit::Failed(token, FailedAction::Retry) => Some(Action::Retry(token)),
            Hit::Failed(token, FailedAction::Edit) => Some(Action::EditFailed(token)),
            Hit::Failed(token, FailedAction::Dismiss) => Some(Action::Dismiss(token)),
            Hit::RetryCurrent => Some(Action::RetryCurrent),
            Hit::DiscardCurrent => Some(Action::DiscardCurrent),
            Hit::Panel | Hit::Field | Hit::Date | Hit::Time => None,
        };
        Outcome {
            handled: true,
            action,
        }
    }

    pub(super) fn key(
        &mut self,
        engine: &mut TextEngine,
        key: &KeyboardKey,
        text: Option<&str>,
        mods: ModifiersState,
    ) -> Outcome {
        // **A recovery pill outlives the popover whose save failed**, so it is
        // asked before the `mode` gate below rather than after it: with no
        // panel standing there is no mode at all, and `Surface::focus` is the
        // only thing holding the keyboard. See `failed_pill_key`.
        if self.mode.is_none() {
            if self.pill_focused() {
                return self.failed_pill_key(engine, key, mods);
            }
            return Outcome::default();
        }
        if matches!(key, KeyboardKey::Named(NamedKey::Escape)) {
            let action = (self.mode == Some(Mode::Creation)).then_some(Action::Cancel);
            self.close();
            return Outcome {
                handled: true,
                action,
            };
        }
        if self.mode == Some(Mode::Management) {
            if matches!(
                key,
                KeyboardKey::Named(NamedKey::Tab | NamedKey::ArrowDown | NamedKey::ArrowUp)
            ) {
                let next = {
                    let hits = self.geometry().ordered();
                    (!hits.is_empty()).then(|| {
                        let backwards = mods.shift_key()
                            || matches!(key, KeyboardKey::Named(NamedKey::ArrowUp));
                        cycle_focus(hits, self.focus, backwards)
                    })
                };
                self.focus = next;
                return Outcome {
                    handled: true,
                    action: None,
                };
            }
            if is_activation_key(key)
                && let Some(hit) = self.focus
                && self.geometry().enabled(hit)
                && self.geometry().ordered().contains(&hit)
            {
                return self.activate(engine, hit);
            }
            return Outcome {
                handled: true,
                action: None,
            };
        }
        if matches!(key, KeyboardKey::Named(NamedKey::Tab)) {
            let next = {
                let hits = self.geometry().ordered();
                (!hits.is_empty()).then(|| cycle_focus(hits, self.focus, mods.shift_key()))
            };
            self.focus = next;
            return Outcome {
                handled: true,
                action: None,
            };
        }
        if matches!(
            key,
            KeyboardKey::Named(NamedKey::ArrowUp | NamedKey::ArrowDown)
        ) && !self.expanded
        {
            let next = {
                let presets = self.geometry().preset_order.as_slice();
                (!presets.is_empty()).then(|| {
                    cycle_focus(
                        presets,
                        self.focus,
                        matches!(key, KeyboardKey::Named(NamedKey::ArrowUp)),
                    )
                })
            };
            self.focus = next;
            return Outcome {
                handled: true,
                action: None,
            };
        }
        let enter = matches!(key, KeyboardKey::Named(NamedKey::Enter));
        let control_space = is_activation_key(key)
            && !enter
            && !matches!(self.focus, Some(Hit::Field | Hit::Date | Hit::Time));
        if enter || control_space {
            let hit = match self.focus {
                Some(Hit::Field) if enter && self.geometry().enabled(Hit::Set) => Some(Hit::Set),
                Some(hit)
                    if self.geometry().enabled(hit) && self.geometry().ordered().contains(&hit) =>
                {
                    Some(hit)
                }
                _ => None,
            };
            return hit
                .map(|hit| self.activate(engine, hit))
                .unwrap_or(Outcome {
                    handled: true,
                    action: None,
                });
        }
        match self.focus {
            Some(Hit::Date) if self.expanded => {
                if let Some(input) = &mut self.date {
                    input.key(engine, key, text, mods);
                }
            }
            Some(Hit::Time) if self.expanded => {
                if let Some(input) = &mut self.time {
                    input.key(engine, key, text, mods);
                }
            }
            Some(Hit::Field) => {
                if let Some(input) = &mut self.input {
                    input.key(engine, key, text, mods);
                }
            }
            _ => {
                return Outcome {
                    handled: true,
                    action: None,
                };
            }
        }
        if self.expanded && matches!(self.focus, Some(Hit::Date | Hit::Time)) {
            self.source = DraftSource::Structured;
            self.structured_schedule_cache = None;
            self.structured_dirty = true;
            return Outcome {
                handled: true,
                action: None,
            };
        }
        let phrase = self
            .input
            .as_ref()
            .map(TextInput::value)
            .unwrap_or_default();
        let Some(phrase) = self.resolve_edit_phrase(phrase) else {
            return Outcome {
                handled: true,
                action: None,
            };
        };
        self.source = DraftSource::Natural;
        Outcome {
            handled: true,
            action: Some(Action::Phrase(phrase)),
        }
    }

    fn ensure_structured(&mut self, engine: &mut TextEngine, draft: Option<&Draft>) {
        if self.date.is_some() {
            return;
        }
        // Allocation is input-safe; maintenance fills localized values from
        // the cached context on the next pass.
        let _ = draft;
        let date = String::new();
        let time = String::new();
        self.date = Some(TextInput::with_text(engine, theme::role::body(), &date));
        self.time = Some(TextInput::with_text(engine, theme::role::body(), &time));
    }

    fn refresh_structured_fields(&mut self, engine: &mut TextEngine, draft: Option<&Draft>) {
        if !self.expanded || self.source != DraftSource::Natural {
            return;
        }
        if self.context.is_none() {
            return;
        }
        let Some(wall) = draft.and_then(draft_wall_in_schedule_zone) else {
            return;
        };
        let (date, time) = structured_field_values(wall, &self.locale);
        if self.date.as_ref().map(TextInput::value).as_deref() != Some(date.as_str()) {
            self.date = Some(TextInput::with_text(engine, theme::role::body(), &date));
        }
        if self.time.as_ref().map(TextInput::value).as_deref() != Some(time.as_str()) {
            self.time = Some(TextInput::with_text(engine, theme::role::body(), &time));
        }
    }

    fn finish_edit(&mut self) {
        // An exact-pill edit has no grouped object surface to return to. The
        // old transition changed only `mode`, leaving creation geometry in
        // place until maintenance; a redraw in that gap asked management to
        // draw with no body clip and panicked. Closing is both the intended UX
        // and an atomic state: no surface means no geometry may be consumed.
        self.close();
    }

    fn selected_draft(&self, creation: Option<&Creation>) -> Option<Draft> {
        if self.source == DraftSource::Structured {
            let schedule = self.structured_schedule()?;
            let tone = self
                .edit_creation
                .as_ref()
                .and_then(Creation::editable_draft)
                .or_else(|| creation.and_then(Creation::editable_draft))
                .and_then(|draft| draft.tone_override);
            return Some(Draft::preserving(schedule, tone));
        }
        self.edit_creation
            .as_ref()
            .and_then(Creation::confirmable_draft)
            .or_else(|| creation.and_then(Creation::confirmable_draft))
            .or_else(|| {
                self.cached_creation
                    .as_ref()
                    .and_then(Creation::confirmable_draft)
            })
            .cloned()
    }

    fn structured_schedule(&self) -> Option<ReminderDraft> {
        self.structured_schedule_cache.clone()
    }

    fn compute_structured_schedule(&self) -> Option<ReminderDraft> {
        self.context.as_ref()?;
        let wall = parse_structured_fields(
            &self.date.as_ref()?.value(),
            &self.time.as_ref()?.value(),
            &self.locale,
        )?;
        let original = self
            .edit_creation
            .as_ref()
            .and_then(Creation::editable_draft)
            .or_else(|| {
                self.cached_creation
                    .as_ref()
                    .and_then(Creation::editable_draft)
            });
        match original.map(|draft| &draft.schedule) {
            Some(ReminderDraft::Zoned(value)) => {
                let rules = IanaZoneRules::from_name(value.zone()).ok()?;
                Some(ReminderDraft::Zoned(koru_core::time::ZonedDateTime::new(
                    rules.instant_of(wall),
                    value.zone().clone(),
                )))
            }
            // Phase one has no approved relation/offset field layout. Preserve
            // the semantic schedule exactly; Q-UI-14 records the deferred UI.
            Some(ReminderDraft::Relative { .. }) => original.map(|draft| draft.schedule.clone()),
            Some(ReminderDraft::Floating(_)) | None => {
                Some(ReminderDraft::Floating(FloatingDateTime::new(wall)))
            }
        }
    }

    pub(super) fn scroll_by(
        &mut self,
        viewport: (f32, f32),
        management: Option<&Management>,
        delta: f32,
    ) -> bool {
        if self.mode.is_none() {
            return false;
        }
        if self.mode != Some(Mode::Management) {
            return true;
        }
        let Some(state) = management else {
            return true;
        };
        let panel_height = 520.0_f32.min((viewport.1 - 16.0).max(160.0));
        let visible = (panel_height - 62.0).max(1.0);
        let content = management_content_height(state);
        let max = (content - visible).max(0.0);
        // MainView's wheel adapter has already converted line deltas to pixels.
        self.management_scroll = (self.management_scroll - delta).clamp(0.0, max);
        true
    }

    fn build_geometry(
        &self,
        viewport: (f32, f32),
        management: Option<&Management>,
        optimistic: &[OptimisticCreation],
        creation: CreationGeometryState,
    ) -> Geometry {
        let mut g = Geometry::default();
        if let Some(mode) = self.mode {
            let (width, height) = match mode {
                Mode::Creation => (QUICK_W, if self.expanded { EXPANDED_H } else { QUICK_H }),
                Mode::Management => (MANAGEMENT_W, 520.0_f32.min((viewport.1 - 16.0).max(160.0))),
            };
            let panel = PopoverShell::anchored(
                self.anchor,
                Rect::new(0.0, 0.0, viewport.0, viewport.1),
                width.min((viewport.0 - 16.0).max(1.0)),
                height,
                PopoverAlign::Start,
            )
            .rect;
            g.panel = Some(panel);
            match mode {
                Mode::Creation => {
                    g.field = Some(Rect::new(
                        panel.x + PAD,
                        panel.y + 40.0,
                        panel.w - PAD * 2.0,
                        FIELD_H,
                    ));
                    if self.expanded {
                        g.date = Some(Rect::new(panel.x + PAD, panel.y + 108.0, 218.0, FIELD_H));
                        g.time = Some(Rect::new(
                            panel.x + 240.0,
                            panel.y + 108.0,
                            panel.w - 254.0,
                            FIELD_H,
                        ));
                    } else {
                        let mut y = panel.y + 88.0;
                        for preset in self.presets.iter().copied() {
                            g.presets.push((
                                preset,
                                Rect::new(panel.x + PAD, y, panel.w - PAD * 2.0, ROW_H),
                            ));
                            y += ROW_H + 4.0;
                        }
                    }
                    let bottom = panel.y + panel.h - PAD - ActionButton::H;
                    if creation.needs_end_of_day {
                        g.end_of_day_settings = Some(Rect::new(
                            panel.x + PAD,
                            panel.y + panel.h - 68.0,
                            self.action_widths
                                .end_of_day_settings
                                .min(panel.w - PAD * 2.0),
                            22.0,
                        ));
                    }
                    let [more_w, set_w, cancel_w] = self.action_widths.creation;
                    g.more = Some(Rect::new(panel.x + PAD, bottom, more_w, ActionButton::H));
                    g.set = Some(Rect::new(
                        panel.x + panel.w - PAD - set_w,
                        bottom,
                        set_w,
                        ActionButton::H,
                    ));
                    g.cancel = Some(Rect::new(
                        panel.x + panel.w - PAD - set_w - ActionButton::GAP - cancel_w,
                        bottom,
                        cancel_w,
                        ActionButton::H,
                    ));
                }
                Mode::Management => {
                    let add_w = self.action_widths.add;
                    g.add = Some(Rect::new(
                        panel.x + panel.w - PAD - add_w,
                        panel.y + PAD,
                        add_w,
                        ActionButton::H,
                    ));
                    let body_clip = Rect::new(
                        panel.x + 1.0,
                        panel.y + 54.0,
                        panel.w - 2.0,
                        (panel.h - 62.0).max(1.0),
                    );
                    g.body_clip = Some(body_clip);
                    let mut y = body_clip.y - self.management_scroll;
                    if let Some(state) = management {
                        if state.error.is_some() {
                            let [retry_w, discard_w] = self.action_widths.error;
                            g.error_answers = Some([
                                Rect::new(panel.x + PAD, y + 28.0, retry_w, ActionButton::H),
                                Rect::new(
                                    panel.x + PAD + retry_w + ActionButton::GAP,
                                    y + 28.0,
                                    discard_w,
                                    ActionButton::H,
                                ),
                            ]);
                            y += 76.0;
                        }
                        for item in &state.active {
                            let row = Rect::new(panel.x + PAD, y, panel.w - PAD * 2.0, 54.0);
                            let edit_w = self.action_widths.row[0];
                            let pause_w = if item.state == ItemState::Paused {
                                self.action_widths.row[2]
                            } else {
                                self.action_widths.row[1]
                            };
                            let delete_w = self.action_widths.row[3];
                            let delete_x = row.x + row.w - delete_w;
                            let pause_x = delete_x - ActionButton::GAP - pause_w;
                            let actions = [
                                Rect::new(
                                    pause_x - ActionButton::GAP - edit_w,
                                    row.y + 14.0,
                                    edit_w,
                                    ActionButton::H,
                                ),
                                Rect::new(pause_x, row.y + 14.0, pause_w, ActionButton::H),
                                Rect::new(delete_x, row.y + 14.0, delete_w, ActionButton::H),
                            ];
                            g.rows.push((row, actions));
                            y += 58.0;
                        }
                        if !state.history.is_empty() {
                            g.history =
                                Some(Rect::new(panel.x + PAD, y, panel.w - PAD * 2.0, ROW_H));
                            y += ROW_H;
                            if state.history_expanded {
                                for _ in &state.history {
                                    let recreate_w = self.action_widths.recreate;
                                    g.recreate.push(Rect::new(
                                        panel.x + panel.w - PAD - recreate_w,
                                        y + 4.0,
                                        recreate_w,
                                        ActionButton::H,
                                    ));
                                    y += 40.0;
                                }
                                if state.clear_history_confirmation {
                                    let [confirm_w, cancel_w] = self.action_widths.clear_answers;
                                    g.clear_answers = Some([
                                        Rect::new(
                                            panel.x + PAD,
                                            y + 3.0,
                                            confirm_w,
                                            ActionButton::H,
                                        ),
                                        Rect::new(
                                            panel.x + PAD + confirm_w + ActionButton::GAP,
                                            y + 3.0,
                                            cancel_w,
                                            ActionButton::H,
                                        ),
                                    ]);
                                } else {
                                    g.clear = Some(Rect::new(
                                        panel.x + PAD,
                                        y + 3.0,
                                        self.action_widths.clear,
                                        ActionButton::H,
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            match mode {
                Mode::Creation => {
                    g.creation_relative = creation.relative;
                    for (hit, rect, enabled) in [
                        (Hit::Field, g.field, creation.field_editable),
                        (Hit::Date, g.date, creation.structured_editable),
                        (Hit::Time, g.time, creation.structured_editable),
                        (Hit::More, g.more, creation.actions_ready && !self.expanded),
                        (Hit::EndOfDaySettings, g.end_of_day_settings, true),
                        (Hit::Set, g.set, creation.can_set),
                        (Hit::Cancel, g.cancel, true),
                    ] {
                        if let Some(rect) = rect {
                            g.targets.push(FocusTarget { hit, rect, enabled });
                        }
                    }
                    if creation.actions_ready {
                        for (preset, rect) in &g.presets {
                            g.targets.push(FocusTarget {
                                hit: Hit::Preset(*preset),
                                rect: *rect,
                                enabled: true,
                            });
                        }
                    }
                    if creation.load_failed {
                        if let Some(rect) = g.more {
                            g.targets.push(FocusTarget {
                                hit: Hit::RetryCurrent,
                                rect,
                                enabled: true,
                            });
                        }
                    }
                }
                Mode::Management => {
                    let mutations =
                        management.is_some_and(|state| !state.loading && state.error.is_none());
                    if let Some(rect) = g.add {
                        g.targets.push(FocusTarget {
                            hit: Hit::Add,
                            rect,
                            enabled: mutations,
                        });
                    }
                    for (index, (_, actions)) in g.rows.iter().enumerate() {
                        for (action, rect) in [RowAction::Edit, RowAction::Pause, RowAction::Delete]
                            .into_iter()
                            .zip(actions.iter().copied())
                        {
                            g.targets.push(FocusTarget {
                                hit: Hit::Row(index, action),
                                rect,
                                enabled: mutations,
                            });
                        }
                    }
                    if let Some(rect) = g.history {
                        g.targets.push(FocusTarget {
                            hit: Hit::History,
                            rect,
                            enabled: true,
                        });
                    }
                    for (index, rect) in g.recreate.iter().copied().enumerate() {
                        g.targets.push(FocusTarget {
                            hit: Hit::Recreate(index),
                            rect,
                            enabled: mutations,
                        });
                    }
                    if let Some(rect) = g.clear {
                        g.targets.push(FocusTarget {
                            hit: Hit::Clear,
                            rect,
                            enabled: mutations,
                        });
                    }
                    if let Some([confirm, cancel]) = g.clear_answers {
                        g.targets.push(FocusTarget {
                            hit: Hit::ConfirmClear,
                            rect: confirm,
                            enabled: mutations,
                        });
                        g.targets.push(FocusTarget {
                            hit: Hit::CancelClear,
                            rect: cancel,
                            enabled: true,
                        });
                    }
                    if let Some([retry, discard]) = g.error_answers {
                        g.targets.push(FocusTarget {
                            hit: Hit::RetryCurrent,
                            rect: retry,
                            enabled: true,
                        });
                        g.targets.push(FocusTarget {
                            hit: Hit::DiscardCurrent,
                            rect: discard,
                            enabled: true,
                        });
                    }
                }
            }
        }
        let mut y = 50.0;
        // The inline reminder chip is sufficient feedback for successful and
        // still-saving submissions. A second floating pill below the caption
        // flashed during every normal create and offered no action. Failures
        // remain here because Retry/Edit/Dismiss are real recovery controls.
        for item in optimistic
            .iter()
            .filter(|item| matches!(item.state, OptimisticState::Failed { .. }))
        {
            let actions_w = self.action_widths.failed.iter().sum::<f32>() + ActionButton::GAP * 2.0;
            let pill_w = PILL_W
                .max(actions_w + 112.0)
                .min((viewport.0 - 20.0).max(1.0));
            let pill = Rect::new((viewport.0 - pill_w - 12.0).max(8.0), y, pill_w, 40.0);
            let [retry_w, edit_w, dismiss_w] = self.action_widths.failed;
            let dismiss_x = pill.x + pill.w - 6.0 - dismiss_w;
            let edit_x = dismiss_x - ActionButton::GAP - edit_w;
            let actions = [
                Rect::new(
                    edit_x - ActionButton::GAP - retry_w,
                    pill.y + 7.0,
                    retry_w,
                    ActionButton::H,
                ),
                Rect::new(edit_x, pill.y + 7.0, edit_w, ActionButton::H),
                Rect::new(dismiss_x, pill.y + 7.0, dismiss_w, ActionButton::H),
            ];
            g.failed.push((item.request.identity.token, pill, actions));
            y += 46.0;
        }
        g.rebuild_order();
        g
    }

    pub(super) fn draw(
        &mut self,
        canvas: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        lang: Lang,
    ) {
        // Moving the Box borrows mutable text inputs and immutable geometry
        // together without rebuilding either. Restoring it keeps Vec pointers.
        let g = self.geometry.take().unwrap();
        let geometry_key = self.geometry_key.take();
        let management = geometry_key
            .as_ref()
            .and_then(|key| key.management.as_ref());
        if let Some(panel) = g.panel {
            PopoverShell {
                rect: panel,
                radius: 10.0,
            }
            .draw_with_content(
                canvas,
                root,
                koru_ui::popover::KoruPopoverInspection {
                    occurrence: Some(self.instance),
                    state: koru_ui::popover::KoruPopoverState::Open,
                    placement: geometry_key.as_ref().map_or(
                        koru_ui::popover::KoruPopoverPlacement::CallerRect,
                        |key| koru_ui::popover::KoruPopoverPlacement::Anchored {
                            anchor: key.anchor,
                            align: PopoverAlign::Start,
                        },
                    ),
                    ..Default::default()
                },
                |canvas, root| {
                    koru_draw::inspect::current(|r| {
                        r.property(
                            "host association",
                            "reminder creation/management",
                            "Surface::instance; same per-opening ID used by native projection",
                        );
                        r.property("mode", format!("{:?}", self.mode), "Surface::mode");
                    });
                    let _content = koru_draw::inspect::scope(
                        "Reminder editor content",
                        "unregistered",
                        root,
                        scene::rect(panel.x, panel.y, panel.w, panel.h),
                        |_| {},
                    );
                    match self.mode {
                        Some(Mode::Creation) => self.draw_creation(canvas, root, engine, lang, &g),
                        Some(Mode::Management) => {
                            self.draw_management(canvas, root, engine, lang, management, &g)
                        }
                        None => {}
                    }
                },
            );
        }
        for (token, pill, actions) in &g.failed {
            scene::floating_panel(
                canvas,
                root,
                pill.x,
                pill.y,
                pill.w,
                pill.h,
                &scene::Panel::popover(20.0),
            );
            draw_text(
                canvas,
                root,
                engine,
                t(lang, Key::ReminderErrorRejected),
                pill.x + 10.0,
                pill.y + 12.0,
                theme::p().close_hover,
            );
            for (i, key) in [Key::ReminderRetry, Key::ReminderEdit, Key::ReminderDismiss]
                .into_iter()
                .enumerate()
            {
                let hit = Hit::Failed(
                    *token,
                    [
                        FailedAction::Retry,
                        FailedAction::Edit,
                        FailedAction::Dismiss,
                    ][i],
                );
                ActionButton::new(t(lang, key), ActionStyle::Ghost, None).draw(
                    canvas,
                    root,
                    engine,
                    actions[i],
                    self.hover == Some(hit),
                );
                // **The ring `F6` puts here**, in the same stroke `draw_action`
                // uses inside the panel — these three are not in
                // `Geometry::targets`, so that helper cannot draw them, but a
                // focus a person cannot see is not a focus (§7.1). No
                // `enabled` test: a standing pill's three buttons are always
                // available, which is the whole reason it is standing.
                if self.focus == Some(hit) {
                    scene::stroke(
                        canvas,
                        root,
                        theme::p().focus_ring,
                        theme::BORDER_W,
                        &scene::rrect(
                            actions[i].x,
                            actions[i].y,
                            actions[i].w,
                            actions[i].h,
                            ActionButton::RADIUS,
                        ),
                    );
                }
            }
        }
        self.geometry = Some(g);
        self.geometry_key = geometry_key;
    }

    fn draw_creation(
        &mut self,
        canvas: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        lang: Lang,
        g: &Geometry,
    ) {
        let panel = g.panel.unwrap();
        draw_text(
            canvas,
            root,
            engine,
            t(lang, Key::Reminder),
            panel.x + PAD,
            panel.y + 10.0,
            theme::p().text,
        );
        if let (Some(input), Some(rect)) = (&mut self.input, g.field) {
            input.draw(
                canvas,
                root,
                engine,
                rect,
                InputStyle::inline(),
                Some(t(lang, Key::ReminderWhenPlaceholder)),
                self.focus == Some(Hit::Field),
            );
        }
        if self.expanded {
            let invalid =
                self.source == DraftSource::Structured && self.structured_schedule_cache.is_none();
            let relative = g.creation_relative;
            let field_style = if invalid {
                InputStyle::inline().invalid()
            } else if relative {
                InputStyle {
                    border: Some(theme::p().panel_border),
                    ..InputStyle::inline()
                }
            } else {
                InputStyle::inline()
            };
            if let (Some(input), Some(rect)) = (&mut self.date, g.date) {
                draw_text(
                    canvas,
                    root,
                    engine,
                    t(lang, Key::ReminderDate),
                    rect.x,
                    rect.y - 16.0,
                    theme::p().text_dim,
                );
                input.draw(
                    canvas,
                    root,
                    engine,
                    rect,
                    field_style,
                    None,
                    self.focus == Some(Hit::Date),
                );
            }
            if let (Some(input), Some(rect)) = (&mut self.time, g.time) {
                draw_text(
                    canvas,
                    root,
                    engine,
                    t(lang, Key::ReminderTime),
                    rect.x,
                    rect.y - 16.0,
                    theme::p().text_dim,
                );
                input.draw(
                    canvas,
                    root,
                    engine,
                    rect,
                    field_style,
                    None,
                    self.focus == Some(Hit::Time),
                );
            }
            if let Some(zone) = &self.authored_zone_label {
                draw_text(
                    canvas,
                    root,
                    engine,
                    zone,
                    panel.x + PAD,
                    panel.y + 154.0,
                    theme::p().text_dim,
                );
            }
            if invalid {
                draw_text(
                    canvas,
                    root,
                    engine,
                    t(lang, Key::ReminderErrorInvalidData),
                    panel.x + PAD,
                    panel.y + 176.0,
                    theme::p().close_hover,
                );
            }
        } else {
            for (preset, rect) in &g.presets {
                if self.focus == Some(Hit::Preset(*preset))
                    || self.hover == Some(Hit::Preset(*preset))
                {
                    scene::fill(
                        canvas,
                        root,
                        theme::p().hover_tint,
                        &scene::rrect(rect.x, rect.y, rect.w, rect.h, 6.0),
                    );
                }
                let label = self.hit_label(lang, Hit::Preset(*preset));
                draw_text(
                    canvas,
                    root,
                    engine,
                    &label,
                    rect.x + 10.0,
                    rect.y + 10.0,
                    theme::p().text,
                );
                if self.focus == Some(Hit::Preset(*preset)) {
                    scene::stroke(
                        canvas,
                        root,
                        theme::p().focus_ring,
                        theme::BORDER_W,
                        &scene::rrect(rect.x, rect.y, rect.w, rect.h, 6.0),
                    );
                }
            }
        }
        let color = if self.creation_error {
            theme::p().close_hover
        } else {
            theme::p().accent
        };
        let message = engine.simple_layout_wrapped(
            &self.creation_message,
            theme::role::row(),
            (panel.w - PAD * 2.0).max(1.0),
        );
        let actions_top = [g.more, g.cancel, g.set]
            .into_iter()
            .flatten()
            .map(|rect| rect.y)
            .fold(panel.y + panel.h, f32::min);
        let message_y = creation_message_y(panel, actions_top, message.height());
        scene::draw_layout(
            canvas,
            root,
            &message,
            panel.x + PAD,
            message_y,
            Some(color),
        );
        if let Some(rect) = g.end_of_day_settings
            && matches!(
                (self.hover, self.focus),
                (Some(Hit::EndOfDaySettings), _) | (_, Some(Hit::EndOfDaySettings))
            )
        {
            scene::fill(
                canvas,
                root,
                theme::p().accent,
                &scene::rect(rect.x, rect.y + rect.h - 2.0, rect.w, 1.0),
            );
        }
        if let Some(rect) = g.more {
            let hit = if g.enabled(Hit::RetryCurrent) {
                Hit::RetryCurrent
            } else {
                Hit::More
            };
            let label = self.hit_label(lang, hit);
            draw_action(
                self,
                canvas,
                root,
                engine,
                g,
                hit,
                rect,
                &label,
                ActionStyle::Ghost,
            );
        }
        if let Some(rect) = g.cancel {
            draw_action(
                self,
                canvas,
                root,
                engine,
                g,
                Hit::Cancel,
                rect,
                &self.hit_label(lang, Hit::Cancel),
                ActionStyle::Ghost,
            );
        }
        if let Some(rect) = g.set {
            draw_action(
                self,
                canvas,
                root,
                engine,
                g,
                Hit::Set,
                rect,
                &self.hit_label(lang, Hit::Set),
                ActionStyle::Primary,
            );
        }
    }

    fn draw_management(
        &mut self,
        canvas: &mut Scene,
        root: Affine,
        engine: &mut TextEngine,
        lang: Lang,
        management: Option<&Management>,
        g: &Geometry,
    ) {
        let panel = g.panel.unwrap();
        draw_text(
            canvas,
            root,
            engine,
            t(lang, Key::ReminderTitle),
            panel.x + PAD,
            panel.y + 10.0,
            theme::p().text,
        );
        if let Some(rect) = g.add {
            draw_action(
                self,
                canvas,
                root,
                engine,
                g,
                Hit::Add,
                rect,
                &self.hit_label(lang, Hit::Add),
                ActionStyle::Primary,
            );
        }
        let Some(state) = management else {
            return;
        };
        let clip = g.body_clip.expect("management geometry has a body clip");
        let _inspection_clip =
            koru_draw::inspect::clip(root, scene::rect(clip.x, clip.y, clip.w, clip.h));
        canvas.push_clip_layer(
            Fill::NonZero,
            root,
            &scene::rrect(clip.x, clip.y, clip.w, clip.h, 0.0),
        );
        if state.loading {
            draw_text(
                canvas,
                root,
                engine,
                t(lang, Key::ReminderLoading),
                panel.x + PAD,
                panel.y + 58.0,
                theme::p().text_dim,
            );
        }
        if let (Some(reason), Some([retry, discard])) = (state.error, g.error_answers) {
            draw_text(
                canvas,
                root,
                engine,
                t(lang, failure_key(reason)),
                panel.x + PAD,
                retry.y - 22.0,
                theme::p().close_hover,
            );
            draw_action(
                self,
                canvas,
                root,
                engine,
                g,
                Hit::RetryCurrent,
                retry,
                &self.hit_label(lang, Hit::RetryCurrent),
                ActionStyle::Ghost,
            );
            draw_action(
                self,
                canvas,
                root,
                engine,
                g,
                Hit::DiscardCurrent,
                discard,
                &self.hit_label(lang, Hit::DiscardCurrent),
                ActionStyle::Ghost,
            );
        }
        for (i, item) in state.active.iter().enumerate() {
            let (row, actions) = g.rows[i];
            if self
                .hover
                .is_some_and(|hit| matches!(hit, Hit::Row(n, _) if n == i))
            {
                scene::fill(
                    canvas,
                    root,
                    theme::p().hover_tint,
                    &scene::rrect(row.x, row.y, row.w, row.h, 7.0),
                );
            }
            draw_text(
                canvas,
                root,
                engine,
                self.draft_label(lang, &item.draft),
                row.x + 8.0,
                row.y + 8.0,
                theme::p().text,
            );
            draw_text(
                canvas,
                root,
                engine,
                t(lang, state_key(&item.state)),
                row.x + 8.0,
                row.y + 29.0,
                theme::p().text_dim,
            );
            for (a, action) in [RowAction::Edit, RowAction::Pause, RowAction::Delete]
                .into_iter()
                .enumerate()
            {
                let style = if action == RowAction::Delete {
                    ActionStyle::Destructive
                } else {
                    ActionStyle::Ghost
                };
                draw_action(
                    self,
                    canvas,
                    root,
                    engine,
                    g,
                    Hit::Row(i, action),
                    actions[a],
                    &self.hit_label(lang, Hit::Row(i, action)),
                    style,
                );
            }
        }
        if let Some(rect) = g.history {
            let icon = if state.history_expanded {
                icons::CARET_DOWN
            } else {
                icons::CARET_RIGHT
            };
            ActionButton::new(
                &self.hit_label(lang, Hit::History),
                ActionStyle::Ghost,
                Some(icon),
            )
            .draw(canvas, root, engine, rect, self.hover == Some(Hit::History));
        }
        if state.history_expanded {
            for (i, item) in state.history.iter().enumerate() {
                let rect = g.recreate[i];
                draw_text(
                    canvas,
                    root,
                    engine,
                    self.draft_label(lang, &item.draft),
                    panel.x + PAD + 8.0,
                    rect.y + 9.0,
                    theme::p().text_dim,
                );
                draw_action(
                    self,
                    canvas,
                    root,
                    engine,
                    g,
                    Hit::Recreate(i),
                    rect,
                    &self.hit_label(lang, Hit::Recreate(i)),
                    ActionStyle::Ghost,
                );
            }
            if let Some(rect) = g.clear {
                draw_action(
                    self,
                    canvas,
                    root,
                    engine,
                    g,
                    Hit::Clear,
                    rect,
                    &self.hit_label(lang, Hit::Clear),
                    ActionStyle::Destructive,
                );
            }
            if let Some([confirm, cancel]) = g.clear_answers {
                draw_text(
                    canvas,
                    root,
                    engine,
                    t(lang, Key::ReminderClearHistoryConfirm),
                    panel.x + PAD,
                    confirm.y - 18.0,
                    theme::p().close_hover,
                );
                ActionButton::new(
                    &self.hit_label(lang, Hit::ConfirmClear),
                    ActionStyle::Destructive,
                    None,
                )
                .draw(
                    canvas,
                    root,
                    engine,
                    confirm,
                    self.hover == Some(Hit::ConfirmClear),
                );
                ActionButton::new(
                    &self.hit_label(lang, Hit::CancelClear),
                    ActionStyle::Ghost,
                    None,
                )
                .draw(
                    canvas,
                    root,
                    engine,
                    cancel,
                    self.hover == Some(Hit::CancelClear),
                );
            }
        }
        canvas.pop_layer();
    }

    fn draft_label<'a>(&'a self, lang: Lang, draft: &Draft) -> &'a str {
        self.labels
            .iter()
            .find(|(cached, _)| cached == draft)
            .map(|(_, label)| label.as_str())
            .unwrap_or_else(|| t(lang, Key::ReminderStateWaiting))
    }

    fn creation_draft_label(&self, lang: Lang, draft: &Draft) -> String {
        let label = self.draft_label(lang, draft);
        match (&draft.schedule, self.context.as_ref()) {
            (ReminderDraft::Zoned(value), Some(context))
                if value.zone() != &context.current_zone =>
            {
                format!("{label} \u{00b7} {}", value.zone())
            }
            _ => label.to_owned(),
        }
    }

    fn creation_message(&self, lang: Lang, creation: &Creation) -> String {
        match &creation.state {
            CreationState::Empty => t(lang, Key::ReminderWhenPlaceholder).to_owned(),
            CreationState::Confirmation(draft) => t(lang, Key::ReminderScheduledFor)
                .replace("{}", &self.creation_draft_label(lang, draft)),
            CreationState::Passed {
                requested,
                proposal,
            } => t(lang, Key::ReminderPastProposal)
                .replacen("{}", &self.creation_draft_label(lang, requested), 1)
                .replacen("{}", &self.creation_draft_label(lang, proposal), 1),
            CreationState::Loading => t(lang, Key::ReminderLoading).to_owned(),
            CreationState::LoadFailed { reason } | CreationState::Failed { reason, .. } => {
                t(lang, failure_key(*reason)).to_owned()
            }
            CreationState::Ambiguity => t(lang, Key::ReminderAmbiguousTimeZone).to_owned(),
            CreationState::EndOfDay => t(lang, Key::ReminderSetEndOfDayFirst).to_owned(),
            CreationState::Structured { .. } => t(lang, Key::ReminderStructuredFallback).to_owned(),
            CreationState::Saving(_) => t(lang, Key::ReminderSaving).to_owned(),
        }
    }
}

fn cycle_focus(hits: &[Hit], current: Option<Hit>, backwards: bool) -> Hit {
    let at = current
        .and_then(|current| hits.iter().position(|hit| *hit == current))
        .unwrap_or(if backwards { 0 } else { hits.len() - 1 });
    let next = if backwards {
        (at + hits.len() - 1) % hits.len()
    } else {
        (at + 1) % hits.len()
    };
    hits[next]
}

fn is_activation_key(key: &KeyboardKey) -> bool {
    matches!(key, KeyboardKey::Named(NamedKey::Enter))
        || matches!(key, KeyboardKey::Character(value) if value.as_str() == " ")
}

#[allow(clippy::too_many_arguments)]
fn draw_action(
    surface: &Surface,
    canvas: &mut Scene,
    root: Affine,
    engine: &mut TextEngine,
    geometry: &Geometry,
    hit: Hit,
    rect: Rect,
    label: &str,
    style: ActionStyle,
) {
    let button = ActionButton::new(label, style, None);
    if geometry.enabled(hit) {
        button.draw(canvas, root, engine, rect, surface.hover == Some(hit));
    } else {
        button.draw_disabled(canvas, root, engine, rect);
    }
    if surface.focus == Some(hit) && geometry.enabled(hit) {
        scene::stroke(
            canvas,
            root,
            theme::p().focus_ring,
            theme::BORDER_W,
            &scene::rrect(rect.x, rect.y, rect.w, rect.h, ActionButton::RADIUS),
        );
    }
}

fn draw_text(
    canvas: &mut Scene,
    root: Affine,
    engine: &mut TextEngine,
    text: &str,
    x: f32,
    y: f32,
    color: vello::peniko::Color,
) {
    let layout = engine.simple_layout(text, theme::role::row());
    scene::draw_layout_first_line(canvas, root, &layout, x, y, Some(color));
}

fn creation_message_y(panel: Rect, actions_top: f32, message_height: f32) -> f32 {
    // Bottom-align status copy above the actions so a wrapped translation grows
    // into the deliberately open middle of the editor, never through buttons.
    // The panel edge remains a hard fallback for unusually small viewports.
    (actions_top - 8.0 - message_height).max(panel.y + PAD)
}

fn presets_at(time: NaiveTime) -> Vec<Preset> {
    let mut out = Vec::with_capacity(3);
    if time < NaiveTime::from_hms_opt(18, 0, 0).unwrap() {
        out.push(Preset::LaterToday);
    }
    out.push(Preset::Tomorrow);
    out.push(Preset::Custom);
    out
}
fn management_content_height(state: &Management) -> f32 {
    let mut height = state.active.len() as f32 * 58.0;
    if state.error.is_some() {
        height += 76.0;
    }
    if !state.history.is_empty() {
        height += ROW_H;
        if state.history_expanded {
            height += state.history.len() as f32 * 40.0 + 40.0;
        }
    }
    height
}
fn preset_draft_at(preset: Preset, wall: chrono::NaiveDateTime) -> Option<Draft> {
    let value = match preset {
        Preset::LaterToday if wall.time() < NaiveTime::from_hms_opt(18, 0, 0)? => {
            wall.date().and_hms_opt(18, 0, 0)?
        }
        Preset::Tomorrow => wall.date().succ_opt()?.and_hms_opt(9, 0, 0)?,
        Preset::LaterToday | Preset::Custom => return None,
    };
    Some(Draft::new(ReminderDraft::Floating(FloatingDateTime::new(
        value,
    ))))
}
fn draft_wall_in_schedule_zone(draft: &Draft) -> Option<chrono::NaiveDateTime> {
    match &draft.schedule {
        ReminderDraft::Floating(value) => Some(value.wall()),
        ReminderDraft::Zoned(value) => IanaZoneRules::from_name(value.zone())
            .ok()
            .map(|rules| rules.wall_of(value.instant_ms())),
        ReminderDraft::Relative { preview, .. } => match preview {
            Some(koru_editor::utils::when::ReminderMoment::Floating(value)) => Some(value.wall()),
            Some(koru_editor::utils::when::ReminderMoment::Zoned(value)) => {
                IanaZoneRules::from_name(value.zone())
                    .ok()
                    .map(|rules| rules.wall_of(value.instant_ms()))
            }
            None => None,
        },
    }
}

fn draft_wall_in_display_zone(
    draft: &Draft,
    display_zone: &TimeZoneName,
) -> Option<chrono::NaiveDateTime> {
    let display_rules = IanaZoneRules::from_name(display_zone).ok();
    match &draft.schedule {
        ReminderDraft::Floating(value) => Some(value.wall()),
        ReminderDraft::Zoned(value) => display_rules.map(|rules| rules.wall_of(value.instant_ms())),
        ReminderDraft::Relative { preview, .. } => match preview {
            Some(koru_editor::utils::when::ReminderMoment::Floating(value)) => Some(value.wall()),
            Some(koru_editor::utils::when::ReminderMoment::Zoned(value)) => {
                display_rules.map(|rules| rules.wall_of(value.instant_ms()))
            }
            None => None,
        },
    }
}

fn date_time_parts(wall: chrono::NaiveDateTime) -> koru_i18n::time::DateTimeParts {
    koru_i18n::time::DateTimeParts {
        year: wall.year(),
        month: wall.month() as u8,
        day: wall.day() as u8,
        hour: wall.hour() as u8,
        minute: wall.minute() as u8,
        second: wall.second() as u8,
    }
}

/// Called from MainView/Surface maintenance only. Compact labels are read in
/// the current device zone; structured editing separately uses the authored
/// canonical zone through `draft_wall_in_schedule_zone`.
pub(super) fn format_draft_label_during_maintenance(
    lang: Lang,
    locale: &LocaleFormat,
    display_zone: &TimeZoneName,
    draft: &Draft,
) -> String {
    let Some(wall) = draft_wall_in_display_zone(draft, display_zone) else {
        return t(lang, Key::ReminderStateWaiting).to_owned();
    };
    let label = locale.date_time(date_time_parts(wall));
    if matches!(draft.schedule, ReminderDraft::Zoned(_)) {
        format!("{label} \u{00b7} {display_zone}")
    } else {
        label
    }
}

/// Format an absolute delivery instant in the device's current display zone.
/// Snooze replaces an occurrence's authored schedule only for delivery, so
/// its pill must name this instant rather than repeat the original schedule.
pub(super) fn format_instant_label_during_maintenance(
    locale: &LocaleFormat,
    display_rules: &koru_editor::utils::zone::IanaZoneRules,
    instant_ms: i64,
) -> String {
    locale.date_time(date_time_parts(display_rules.wall_of(instant_ms)))
}

fn structured_field_values(wall: chrono::NaiveDateTime, locale: &LocaleFormat) -> (String, String) {
    let parts = date_time_parts(wall);
    (locale.date_input(parts), locale.time_input(parts))
}

fn parse_structured_fields(
    date: &str,
    time: &str,
    locale: &LocaleFormat,
) -> Option<chrono::NaiveDateTime> {
    let (year, month, day) = locale.parse_date_input(date)?;
    let (hour, minute) = locale.parse_time_input(time)?;
    Some(
        NaiveDate::from_ymd_opt(year, month, day)?
            .and_time(NaiveTime::from_hms_opt(hour, minute, 0)?),
    )
}
fn state_key(state: &ItemState) -> Key {
    match state {
        ItemState::Active => Key::ReminderStateActive,
        ItemState::Paused => Key::ReminderStatePaused,
        ItemState::WaitingForDue => Key::ReminderStateWaiting,
        ItemState::Snoozed { .. } => Key::ReminderStateSnoozed,
    }
}
fn failure_key(failure: Failure) -> Key {
    match failure {
        Failure::Unavailable => Key::ReminderErrorUnavailable,
        Failure::InvalidData => Key::ReminderErrorInvalidData,
        Failure::Rejected => Key::ReminderErrorRejected,
    }
}

#[cfg(test)]
mod inspection_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use koru_editor::utils::when::DueRelation;

    const VIEWPORT: (f32, f32) = (900.0, 600.0);

    fn locale() -> LocaleFormat {
        LocaleFormat::for_system(Some("en-US"), Some(koru_i18n::time::HourCycle::H12))
    }

    fn update_context(surface: &mut Surface) {
        surface.update_context(context(), locale(), Tone::System);
    }

    fn floating(hour: u32) -> Draft {
        Draft::new(ReminderDraft::Floating(FloatingDateTime::new(
            NaiveDate::from_ymd_opt(2026, 9, 3)
                .unwrap()
                .and_hms_opt(hour, 0, 0)
                .unwrap(),
        )))
    }

    fn context() -> ResolutionContext {
        ResolutionContext {
            now_ms: NaiveDate::from_ymd_opt(2026, 9, 2)
                .unwrap()
                .and_hms_opt(17, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp_millis(),
            language: koru_editor::utils::when::Language::English,
            date_order: koru_editor::utils::when::DateOrder::MonthFirst,
            current_zone: koru_core::time::TimeZoneName::utc(),
            selection: None,
            end_of_day: None,
            due_anchor: None,
            system_week_start: koru_editor::utils::when::WeekStart::Monday,
            week_start_override: None,
        }
    }

    fn creation(draft: Draft) -> Creation {
        let tone_override = draft.tone_override;
        Creation {
            request: Request {
                identity: RequestIdentity {
                    token: RequestToken::new(),
                    node: NodeId::new(),
                },
                phrase: "tomorrow at nine".to_owned(),
                due_anchor: None,
            },
            state: CreationState::Confirmation(draft),
            focus: koru_editor::reminders::CreationFocus::Editor,
            default_tone: Some(Tone::System),
            tone_override,
        }
    }

    fn item(index: usize) -> koru_editor::reminders::Item {
        koru_editor::reminders::Item {
            id: format!("id-{index}"),
            revision: format!("revision-{index}"),
            occurrence: None,
            draft: floating(9),
            state: ItemState::Active,
        }
    }

    fn project(surface: &mut Surface, engine: &mut TextEngine) {
        let creation = surface.cached_creation.clone();
        surface.maintain_projection(engine, VIEWPORT, Lang::En, creation.as_ref(), None, &[]);
    }

    fn project_with(
        surface: &mut Surface,
        engine: &mut TextEngine,
        creation: Option<&Creation>,
        management: Option<&Management>,
        optimistic: &[OptimisticCreation],
    ) {
        surface.maintain_projection(engine, VIEWPORT, Lang::En, creation, management, optimistic);
    }
    #[test]
    fn six_pm_boundary_hides_later_today() {
        let five = NaiveTime::from_hms_opt(17, 59, 0).unwrap();
        let six = NaiveTime::from_hms_opt(18, 0, 0).unwrap();
        assert!(presets_at(five).contains(&Preset::LaterToday));
        assert!(!presets_at(six).contains(&Preset::LaterToday));
        let date = NaiveDate::from_ymd_opt(2026, 9, 2).unwrap();
        assert!(preset_draft_at(Preset::LaterToday, date.and_time(five)).is_some());
        assert!(preset_draft_at(Preset::LaterToday, date.and_time(six)).is_none());
    }
    #[test]
    fn management_mutations_are_disabled_while_loading_and_enabled_when_ready() {
        let mut surface = Surface::default();
        surface.mode = Some(Mode::Management);
        let mut management = Management::loading(String::new());
        let mut engine = TextEngine::new();
        project_with(&mut surface, &mut engine, None, Some(&management), &[]);
        let g = surface.geometry();
        let add = g.add.unwrap();
        assert_eq!(g.hit(add.centre().0, add.centre().1), Some(Hit::Panel));
        assert!(!g.enabled(Hit::Add));

        management.replace(Vec::new(), Vec::new());
        project_with(&mut surface, &mut engine, None, Some(&management), &[]);
        let g = surface.geometry();
        assert_eq!(g.hit(add.centre().0, add.centre().1), Some(Hit::Add));
        assert!(g.enabled(Hit::Add));
    }

    #[test]
    fn failed_management_exposes_only_recovery_and_recovers() {
        let mut surface = Surface::default();
        surface.mode = Some(Mode::Management);
        let mut management = Management::loading("target".to_owned());
        management.loading = false;
        management.error = Some(Failure::Unavailable);
        let mut engine = TextEngine::new();
        project_with(&mut surface, &mut engine, None, Some(&management), &[]);
        let failed = surface.geometry();
        assert!(failed.enabled(Hit::RetryCurrent));
        assert!(failed.enabled(Hit::DiscardCurrent));
        assert!(!failed.enabled(Hit::Add));

        management.replace(vec![item(0)], Vec::new());
        project_with(&mut surface, &mut engine, None, Some(&management), &[]);
        let recovered = surface.geometry();
        assert!(recovered.enabled(Hit::Add));
        assert!(!recovered.enabled(Hit::RetryCurrent));
    }

    #[test]
    fn object_history_recreate_returns_typed_draft_not_a_management_mutation() {
        let mut surface = Surface::default();
        surface.mode = Some(Mode::Management);
        surface.node = Some(NodeId::new());
        let source = Draft::preserving(floating(9).schedule, Some(Tone::Soft));
        let mut management = Management::loading("target".to_owned());
        management.replace(
            Vec::new(),
            vec![koru_editor::reminders::HistoryItem {
                reminder_id: "old".to_owned(),
                occurrence: "occurrence".to_owned(),
                draft: source.clone(),
            }],
        );
        let mut engine = TextEngine::new();
        project_with(&mut surface, &mut engine, None, Some(&management), &[]);
        let outcome = surface.activate(&mut engine, Hit::Recreate(0));
        assert!(matches!(
            outcome.action,
            Some(Action::Recreate(draft)) if draft == source
        ));
    }

    #[test]
    fn existing_edit_uses_current_due_anchor_and_late_profile_settings() {
        let due = koru_core::time::Temporal::Date(koru_core::time::DateOnly::new(
            NaiveDate::from_ymd_opt(2026, 9, 5).unwrap(),
        ));
        let source = Draft::preserving(
            ReminderDraft::Relative {
                relation: DueRelation::At,
                offset: None,
                anchor: None,
                preview: None,
            },
            Some(Tone::Soft),
        );
        let mut initial = context();
        initial.due_anchor = Some(due.clone());
        let mut surface = Surface::default();
        let mut engine = TextEngine::new();
        surface.update_context(initial.clone(), locale(), Tone::Calm);
        surface.open_existing_edit(
            &mut engine,
            NodeId::new(),
            Rect::new(20.0, 20.0, 1.0, 1.0),
            "existing".to_owned(),
            source,
            false,
        );
        let draft = surface
            .edit_creation
            .as_ref()
            .and_then(Creation::editable_draft)
            .unwrap();
        assert_eq!(draft.tone_override, Some(Tone::Soft));
        assert!(matches!(
            &draft.schedule,
            ReminderDraft::Relative {
                anchor: Some(anchor),
                preview: Some(_),
                ..
            } if anchor == &due
        ));
        assert_eq!(
            surface.edit_creation.as_ref().unwrap().default_tone,
            Some(Tone::Calm)
        );

        surface.input = Some(TextInput::with_text(
            &mut engine,
            theme::role::body(),
            "end of day",
        ));
        let mut loaded = initial;
        loaded.end_of_day = Some(NaiveTime::from_hms_opt(17, 45, 0).unwrap());
        surface.update_context(loaded, locale(), Tone::Urgent);
        let creation = surface.edit_creation.as_ref().unwrap();
        assert_eq!(creation.default_tone, Some(Tone::Urgent));
        let draft = creation.confirmable_draft().unwrap();
        assert_eq!(draft.tone_override, Some(Tone::Soft));
        assert!(matches!(
            &draft.schedule,
            ReminderDraft::Floating(value) if value.wall().time() == NaiveTime::from_hms_opt(17, 45, 0).unwrap()
        ));
    }

    #[test]
    fn management_enter_refuses_disabled_and_clipped_targets() {
        let mut surface = Surface::default();
        surface.mode = Some(Mode::Management);
        surface.focus = Some(Hit::Add);
        let mut engine = TextEngine::new();
        let loading = Management::loading("target".to_owned());
        surface.maintain_projection(
            &mut engine,
            (900.0, 320.0),
            Lang::En,
            None,
            Some(&loading),
            &[],
        );
        let disabled = surface.key(
            &mut engine,
            &KeyboardKey::Named(NamedKey::Enter),
            None,
            ModifiersState::empty(),
        );
        assert!(disabled.action.is_none());

        let mut ready = Management::loading("target".to_owned());
        ready.replace((0..24).map(item).collect(), Vec::new());
        surface.maintain_projection(
            &mut engine,
            (900.0, 320.0),
            Lang::En,
            None,
            Some(&ready),
            &[],
        );
        surface.focus = Some(Hit::Row(23, RowAction::Edit));
        let clipped = surface.key(
            &mut engine,
            &KeyboardKey::Named(NamedKey::Enter),
            None,
            ModifiersState::empty(),
        );
        assert!(clipped.action.is_none());
    }

    #[test]
    fn invalid_structured_value_has_no_set_target() {
        let mut surface = Surface::default();
        surface.mode = Some(Mode::Creation);
        surface.expanded = true;
        surface.cached_creation = Some(creation(floating(9)));
        surface.source = DraftSource::Structured;
        let mut engine = TextEngine::new();
        surface.date = Some(TextInput::with_text(
            &mut engine,
            theme::role::body(),
            "2026-09-",
        ));
        surface.time = Some(TextInput::with_text(
            &mut engine,
            theme::role::body(),
            "09:00",
        ));
        project(&mut surface, &mut engine);
        let g = surface.geometry();
        assert!(!g.enabled(Hit::Set));
        assert!(!g.ordered().contains(&Hit::Set));
    }

    #[test]
    fn arrow_focus_uses_draw_targets_and_cancel_enter_never_submits() {
        let mut surface = Surface::default();
        surface.mode = Some(Mode::Creation);
        surface.cached_creation = Some(creation(floating(9)));
        surface.presets = presets_at(NaiveTime::from_hms_opt(17, 0, 0).unwrap());
        surface.focus = Some(Hit::Field);
        let mut engine = TextEngine::new();
        let state = surface.cached_creation.clone();
        project_with(&mut surface, &mut engine, state.as_ref(), None, &[]);

        let moved = surface.key(
            &mut engine,
            &KeyboardKey::Named(NamedKey::ArrowDown),
            None,
            ModifiersState::empty(),
        );
        assert!(moved.handled);
        assert_eq!(surface.focus, Some(Hit::Preset(Preset::LaterToday)));

        surface.focus = Some(Hit::Cancel);
        let cancelled = surface.key(
            &mut engine,
            &KeyboardKey::Named(NamedKey::Enter),
            None,
            ModifiersState::empty(),
        );
        assert!(matches!(cancelled.action, Some(Action::Cancel)));
        assert!(!matches!(cancelled.action, Some(Action::Confirm(_))));
    }

    #[test]
    fn only_fields_accept_text_and_space_activates_focused_controls() {
        let mut surface = Surface::default();
        surface.mode = Some(Mode::Creation);
        surface.cached_creation = Some(creation(floating(9)));
        surface.presets = presets_at(NaiveTime::from_hms_opt(17, 0, 0).unwrap());
        let mut engine = TextEngine::new();
        surface.input = Some(TextInput::with_text(
            &mut engine,
            theme::role::body(),
            "tomorrow",
        ));
        surface.focus = Some(Hit::Preset(Preset::Tomorrow));
        let state = surface.cached_creation.clone();
        project_with(&mut surface, &mut engine, state.as_ref(), None, &[]);

        let printable = surface.key(
            &mut engine,
            &KeyboardKey::Character("x".into()),
            Some("x"),
            ModifiersState::empty(),
        );
        assert!(printable.handled);
        assert!(printable.action.is_none());
        assert_eq!(surface.input.as_ref().unwrap().value(), "tomorrow");

        let activated = surface.key(
            &mut engine,
            &KeyboardKey::Character(" ".into()),
            Some(" "),
            ModifiersState::empty(),
        );
        assert!(matches!(activated.action, Some(Action::Confirm(_))));
        assert_eq!(surface.input.as_ref().unwrap().value(), "tomorrow");
    }

    #[test]
    fn empty_request_stays_quick_and_enter_in_field_confirms_when_ready() {
        let mut surface = Surface::default();
        let mut engine = TextEngine::new();
        let mut state = creation(floating(9));
        state.request.phrase.clear();
        state.state = CreationState::Empty;
        surface.sync_creation(
            &mut engine,
            Some(&state),
            Some(Rect::new(20.0, 20.0, 1.0, 1.0)),
        );
        assert!(!surface.expanded);

        state.state = CreationState::Confirmation(floating(9));
        surface.sync_creation(&mut engine, Some(&state), None);
        project_with(&mut surface, &mut engine, Some(&state), None, &[]);
        surface.focus = Some(Hit::Field);
        let outcome = surface.key(
            &mut engine,
            &KeyboardKey::Named(NamedKey::Enter),
            None,
            ModifiersState::empty(),
        );
        assert!(matches!(outcome.action, Some(Action::Confirm(_))));
    }

    #[test]
    fn structured_zoned_edit_keeps_the_canonical_zone() {
        let mut surface = Surface::default();
        update_context(&mut surface);
        surface.mode = Some(Mode::Creation);
        surface.expanded = true;
        let mut engine = TextEngine::new();
        let zone = koru_core::time::TimeZoneName::parse("America/New_York").unwrap();
        let rules = IanaZoneRules::from_name(&zone).unwrap();
        let original = koru_core::time::ZonedDateTime::new(
            rules.instant_of(
                NaiveDate::from_ymd_opt(2026, 9, 3)
                    .unwrap()
                    .and_hms_opt(9, 0, 0)
                    .unwrap(),
            ),
            zone.clone(),
        );
        surface.cached_creation = Some(creation(Draft::new(ReminderDraft::Zoned(original))));
        let cached = surface.cached_creation.clone();
        surface.refresh_structured_fields(
            &mut engine,
            cached.as_ref().and_then(Creation::editable_draft),
        );
        // The injected device zone is UTC, but a zoned reminder is displayed
        // and edited in its stored New York wall time.
        assert_eq!(surface.date.as_ref().unwrap().value(), "09/03/2026");
        assert_eq!(surface.time.as_ref().unwrap().value(), "9:00 AM");
        surface.date = Some(TextInput::with_text(
            &mut engine,
            theme::role::body(),
            "09/04/2026",
        ));
        surface.time = Some(TextInput::with_text(
            &mut engine,
            theme::role::body(),
            "11:30 AM",
        ));
        surface.source = DraftSource::Structured;
        surface.structured_dirty = true;
        project(&mut surface, &mut engine);

        let Some(ReminderDraft::Zoned(edited)) = surface.structured_schedule() else {
            panic!("zoned edit changed schedule kind");
        };
        assert_eq!(edited.zone(), &zone);
        assert_eq!(
            edited.instant_ms(),
            rules.instant_of(
                NaiveDate::from_ymd_opt(2026, 9, 4)
                    .unwrap()
                    .and_hms_opt(11, 30, 0)
                    .unwrap()
            )
        );
    }

    #[test]
    fn compact_zoned_and_relative_labels_use_device_zone_but_edit_uses_authored_zone() {
        let display_zone = TimeZoneName::utc();
        let authored_zone = TimeZoneName::parse("America/New_York").unwrap();
        let rules = IanaZoneRules::from_name(&authored_zone).unwrap();
        let authored_wall = NaiveDate::from_ymd_opt(2026, 9, 3)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap();
        let moment = koru_core::time::ZonedDateTime::new(
            rules.instant_of(authored_wall),
            authored_zone.clone(),
        );
        let draft = Draft::new(ReminderDraft::Zoned(moment.clone()));
        let locale = locale();

        assert_eq!(draft_wall_in_schedule_zone(&draft), Some(authored_wall));
        assert_eq!(
            structured_field_values(draft_wall_in_schedule_zone(&draft).unwrap(), &locale),
            ("09/03/2026".to_owned(), "9:00 AM".to_owned())
        );
        let compact =
            format_draft_label_during_maintenance(Lang::En, &locale, &display_zone, &draft);
        assert_eq!(compact, "09/03/2026 1:00 PM \u{00b7} UTC");

        let mut surface = Surface::default();
        surface.update_context(context(), locale.clone(), Tone::System);
        surface.mode = Some(Mode::Creation);
        surface.expanded = true;
        surface.cached_creation = Some(creation(draft.clone()));
        project(&mut surface, &mut TextEngine::new());
        assert_eq!(
            surface.authored_zone_label.as_deref(),
            Some("America/New_York")
        );
        let active = surface.active_creation(None).unwrap();
        assert_eq!(
            surface.creation_draft_label(Lang::En, active.editable_draft().unwrap()),
            "09/03/2026 1:00 PM \u{00b7} UTC \u{00b7} America/New_York"
        );
        assert!(
            surface
                .creation_message(Lang::En, active)
                .contains("1:00 PM \u{00b7} UTC \u{00b7} America/New_York")
        );

        let relative = Draft::new(ReminderDraft::Relative {
            relation: koru_editor::utils::when::DueRelation::At,
            offset: None,
            anchor: None,
            preview: Some(koru_editor::utils::when::ReminderMoment::Zoned(moment)),
        });
        assert_eq!(
            format_draft_label_during_maintenance(Lang::En, &locale, &display_zone, &relative),
            "09/03/2026 1:00 PM"
        );
    }

    #[test]
    fn existing_reminder_natural_language_edit_updates_one_preserving_draft() {
        let mut surface = Surface::default();
        update_context(&mut surface);
        surface.mode = Some(Mode::Creation);
        surface.focus = Some(Hit::Field);
        let original = Draft::preserving(floating(9).schedule, Some(Tone::Soft));
        surface.editing = Some(Editing {
            id: "existing".to_owned(),
            draft: original.clone(),
            paused: false,
        });
        surface.edit_creation = Some(creation(original));
        let mut engine = TextEngine::new();
        surface.input = Some(TextInput::with_text(
            &mut engine,
            theme::role::body(),
            "tomorrow at 10am",
        ));
        let state = surface.cached_creation.clone();
        project_with(&mut surface, &mut engine, state.as_ref(), None, &[]);
        let outcome = surface.key(
            &mut engine,
            &KeyboardKey::Named(NamedKey::F9),
            None,
            ModifiersState::empty(),
        );
        assert!(outcome.handled);
        assert!(outcome.action.is_none());
        let selected = surface.selected_draft(None).expect("resolved edit");
        assert_eq!(selected.tone_override, Some(Tone::Soft));
        assert_eq!(draft_wall_in_schedule_zone(&selected).unwrap().hour(), 10);
    }

    #[test]
    fn existing_reminder_tone_survives_unresolved_intermediate_typing() {
        let mut surface = Surface::default();
        update_context(&mut surface);
        let original = Draft::preserving(floating(9).schedule, Some(Tone::Soft));
        let mut engine = TextEngine::new();
        surface.open_existing_edit(
            &mut engine,
            NodeId::new(),
            Rect::new(20.0, 20.0, 1.0, 1.0),
            "existing".to_owned(),
            original,
            false,
        );
        project_with(&mut surface, &mut engine, None, None, &[]);

        let mut saw_unresolved = false;
        for character in "tomorrow at 10am".chars() {
            let text = character.to_string();
            let outcome = surface.key(
                &mut engine,
                &KeyboardKey::Character(text.clone().into()),
                Some(&text),
                ModifiersState::empty(),
            );
            assert!(outcome.handled);
            assert!(outcome.action.is_none());
            let creation = surface.edit_creation.as_ref().expect("edit creation");
            saw_unresolved |= matches!(
                creation.state,
                CreationState::Ambiguity
                    | CreationState::EndOfDay
                    | CreationState::Structured { .. }
            );
            assert_eq!(creation.tone_override, Some(Tone::Soft));
        }

        assert!(
            saw_unresolved,
            "typing should cross an unresolved parse state"
        );
        let selected = surface.selected_draft(None).expect("resolved edit");
        assert_eq!(selected.tone_override, Some(Tone::Soft));
        assert_eq!(draft_wall_in_schedule_zone(&selected).unwrap().hour(), 10);
    }

    #[test]
    fn missing_end_of_day_message_opens_its_settings_destination() {
        let mut surface = Surface::default();
        update_context(&mut surface);
        let mut engine = TextEngine::new();
        let mut state = creation(floating(9));
        state.state = CreationState::EndOfDay;
        surface.sync_creation(
            &mut engine,
            Some(&state),
            Some(Rect::new(20.0, 20.0, 1.0, 1.0)),
        );
        project_with(&mut surface, &mut engine, Some(&state), None, &[]);
        let rect = surface.geometry().end_of_day_settings.unwrap();

        let outcome = surface.mouse_down(&mut engine, rect.centre().0, rect.centre().1);

        assert!(outcome.handled);
        assert!(matches!(outcome.action, Some(Action::OpenEndOfDaySettings)));
        assert!(!surface.is_open());
    }

    #[test]
    fn enter_after_natural_language_edit_closes_without_management_geometry() {
        let mut surface = Surface::default();
        update_context(&mut surface);
        let mut engine = TextEngine::new();
        surface.open_existing_edit(
            &mut engine,
            NodeId::new(),
            Rect::new(20.0, 20.0, 1.0, 1.0),
            "existing".to_owned(),
            floating(9),
            false,
        );
        project_with(&mut surface, &mut engine, None, None, &[]);
        for character in "In 5 min".chars() {
            let text = character.to_string();
            let outcome = surface.key(
                &mut engine,
                &KeyboardKey::Character(text.clone().into()),
                Some(&text),
                ModifiersState::empty(),
            );
            assert!(outcome.handled);
            assert!(outcome.action.is_none());
        }
        project_with(&mut surface, &mut engine, None, None, &[]);
        assert!(surface.geometry().enabled(Hit::Set));

        let outcome = surface.key(
            &mut engine,
            &KeyboardKey::Named(NamedKey::Enter),
            None,
            ModifiersState::empty(),
        );
        assert!(matches!(
            outcome.action,
            Some(Action::Management(ManagementIntent::Edit { ref id, .. })) if id == "existing"
        ));
        assert!(!surface.is_open());
        assert!(surface.geometry().panel.is_none());

        // The application may redraw immediately after input, before another
        // maintenance pass. That frame must be a no-op, never a stale-mode
        // geometry assertion.
        let mut scene = Scene::new();
        surface.draw(&mut scene, Affine::IDENTITY, &mut engine, Lang::En);
    }

    #[test]
    fn row_edit_is_the_active_creation_for_projection_geometry_and_summary() {
        let mut surface = Surface::default();
        update_context(&mut surface);
        surface.mode = Some(Mode::Management);
        surface.node = Some(NodeId::new());
        let mut management = Management::loading("target".to_owned());
        management.replace(vec![item(0)], Vec::new());
        let mut engine = TextEngine::new();

        project_with(&mut surface, &mut engine, None, Some(&management), &[]);
        surface.activate(&mut engine, Hit::Row(0, RowAction::Edit));
        surface.sync_creation(&mut engine, None, None);
        surface.maintain_projection(
            &mut engine,
            VIEWPORT,
            Lang::En,
            None,
            Some(&management),
            &[],
        );
        let active = surface
            .active_creation(None)
            .expect("surface edit creation");
        assert!(matches!(active.state, CreationState::Confirmation(_)));
        assert!(
            surface
                .creation_message(Lang::En, active)
                .contains("9:00 AM")
        );
        assert!(surface.geometry().enabled(Hit::Set));

        surface.date = Some(TextInput::with_text(
            &mut engine,
            theme::role::body(),
            "09/04/2026",
        ));
        surface.time = Some(TextInput::with_text(
            &mut engine,
            theme::role::body(),
            "11:30 AM",
        ));
        surface.source = DraftSource::Structured;
        surface.structured_dirty = true;
        surface.maintain_projection(
            &mut engine,
            VIEWPORT,
            Lang::En,
            None,
            Some(&management),
            &[],
        );
        let active = surface
            .active_creation(None)
            .expect("evolving edit creation");
        assert!(
            surface
                .creation_message(Lang::En, active)
                .contains("11:30 AM")
        );
    }

    #[test]
    fn relative_edit_keeps_its_relation_and_has_no_date_time_targets() {
        let mut surface = Surface::default();
        update_context(&mut surface);
        surface.mode = Some(Mode::Creation);
        surface.expanded = true;
        surface.source = DraftSource::Structured;
        let mut engine = TextEngine::new();
        surface.date = Some(TextInput::with_text(
            &mut engine,
            theme::role::body(),
            "09/04/2026",
        ));
        surface.time = Some(TextInput::with_text(
            &mut engine,
            theme::role::body(),
            "11:30 AM",
        ));
        let relative = ReminderDraft::Relative {
            relation: koru_editor::utils::when::DueRelation::At,
            offset: None,
            anchor: None,
            preview: Some(koru_editor::utils::when::ReminderMoment::Floating(
                FloatingDateTime::new(
                    NaiveDate::from_ymd_opt(2026, 9, 3)
                        .unwrap()
                        .and_hms_opt(9, 0, 0)
                        .unwrap(),
                ),
            )),
        };
        surface.cached_creation = Some(creation(Draft::new(relative.clone())));
        surface.structured_dirty = true;
        project(&mut surface, &mut engine);

        assert_eq!(surface.structured_schedule(), Some(relative));
        let geometry = surface.geometry();
        assert!(!geometry.enabled(Hit::Date));
        assert!(!geometry.enabled(Hit::Time));
    }

    #[test]
    fn management_rows_are_clipped_and_scroll_without_off_panel_hits() {
        let mut surface = Surface::default();
        surface.mode = Some(Mode::Management);
        let mut management = Management::loading("target".to_owned());
        management.replace((0..24).map(item).collect(), Vec::new());
        let viewport = (900.0, 320.0);
        let mut engine = TextEngine::new();
        surface.maintain_projection(
            &mut engine,
            viewport,
            Lang::En,
            None,
            Some(&management),
            &[],
        );
        let before = surface.geometry();
        let (last, actions) = before.rows.last().copied().unwrap();
        let before_y = before.rows[5].0.y;
        assert!(
            !before
                .body_clip
                .unwrap()
                .contains(last.centre().0, last.centre().1)
        );
        assert_ne!(
            before.hit(actions[0].centre().0, actions[0].centre().1),
            Some(Hit::Row(23, RowAction::Edit))
        );
        assert!(surface.scroll_by(viewport, Some(&management), -8.0));
        assert!(surface.management_scroll > 0.0);
        surface.maintain_projection(
            &mut engine,
            viewport,
            Lang::En,
            None,
            Some(&management),
            &[],
        );
        let after = surface.geometry();
        assert!(after.rows[5].0.y < before_y);
    }

    #[test]
    fn creation_messages_interpolate_values_in_every_language() {
        let mut surface = Surface::default();
        update_context(&mut surface);
        let requested = floating(8);
        let proposal = floating(9);
        let mut state = creation(proposal.clone());
        state.state = CreationState::Passed {
            requested: requested.clone(),
            proposal: proposal.clone(),
        };
        for lang in Lang::ALL {
            surface.maintain_projection(
                &mut TextEngine::new(),
                VIEWPORT,
                lang,
                Some(&state),
                None,
                &[],
            );
            let message = surface.creation_message(lang, &state);
            assert!(!message.contains("{}"), "{lang:?}: {message}");
            assert!(message.contains(&surface.draft_label(lang, &requested)));
            assert!(message.contains(&surface.draft_label(lang, &proposal)));
        }
    }

    #[test]
    fn localized_creation_messages_wrap_inside_the_panel_above_the_actions() {
        let mut surface = Surface::default();
        update_context(&mut surface);
        let requested = floating(8);
        let proposal = floating(9);
        let mut state = creation(proposal.clone());
        state.state = CreationState::Passed {
            requested,
            proposal,
        };
        surface.mode = Some(Mode::Creation);
        surface.expanded = true;

        for lang in Lang::ALL {
            let mut engine = TextEngine::new();
            surface.maintain_projection(&mut engine, VIEWPORT, lang, Some(&state), None, &[]);
            let geometry = surface.geometry();
            let panel = geometry.panel.unwrap();
            let actions_top = [geometry.more, geometry.cancel, geometry.set]
                .into_iter()
                .flatten()
                .map(|rect| rect.y)
                .fold(panel.y + panel.h, f32::min);
            let layout = engine.simple_layout_wrapped(
                &surface.creation_message,
                theme::role::row(),
                panel.w - PAD * 2.0,
            );
            let y = creation_message_y(panel, actions_top, layout.height());

            assert!(
                layout.width() <= panel.w - PAD * 2.0 + 0.01,
                "{} escaped the creation panel",
                lang.tag()
            );
            assert!(
                y + layout.height() <= actions_top - 8.0 + 0.01,
                "{} overlapped the creation actions",
                lang.tag()
            );
        }
    }

    #[test]
    fn structured_fields_follow_localized_numeric_date_and_clock_policy() {
        let wall = NaiveDate::from_ymd_opt(2026, 9, 4)
            .unwrap()
            .and_hms_opt(21, 7, 0)
            .unwrap();
        let us = LocaleFormat::for_system(Some("en-US"), None);
        assert_eq!(
            structured_field_values(wall, &us),
            ("09/04/2026".to_owned(), "9:07 PM".to_owned())
        );
        assert_eq!(
            parse_structured_fields("09/04/2026", "9:07 PM", &us),
            Some(wall)
        );

        let de = LocaleFormat::for_system(Some("de-DE"), None);
        assert_eq!(
            structured_field_values(wall, &de),
            ("04.09.2026".to_owned(), "21:07".to_owned())
        );
        assert_eq!(
            parse_structured_fields("04.09.2026", "21:07", &de),
            Some(wall)
        );

        let ja = LocaleFormat::for_system(Some("ja-JP"), None);
        assert_eq!(
            structured_field_values(wall, &ja),
            ("2026/09/04".to_owned(), "21:07".to_owned())
        );
        assert_eq!(
            parse_structured_fields("2026/09/04", "21:07", &ja),
            Some(wall)
        );
    }

    #[test]
    fn draw_pointer_and_key_preserve_maintenance_cache_backing_storage() {
        let mut surface = Surface::default();
        surface.mode = Some(Mode::Management);
        let mut management = Management::loading(String::new());
        management.replace(vec![item(0)], Vec::new());
        let optimistic = [OptimisticCreation {
            request: Request {
                identity: RequestIdentity {
                    token: RequestToken::new(),
                    node: NodeId::new(),
                },
                phrase: "tomorrow".to_owned(),
                due_anchor: None,
            },
            target: "target".to_owned(),
            reminder_id: "reminder".to_owned(),
            draft: floating(9),
            state: OptimisticState::Failed {
                reason: Failure::Rejected,
            },
        }];
        let mut engine = TextEngine::new();
        project_with(
            &mut surface,
            &mut engine,
            None,
            Some(&management),
            &optimistic,
        );
        let geometry = surface.geometry() as *const Geometry;
        let rows = surface.geometry().rows.as_ptr();
        let targets = surface.geometry().targets.as_ptr();
        let ordered = surface.geometry().ordered.as_ptr();
        let failed = surface.geometry().failed.as_ptr();
        let action = surface.geometry().rows[0].1[0];

        assert!(!surface.maintain_projection(
            &mut engine,
            VIEWPORT,
            Lang::En,
            None,
            Some(&management),
            &optimistic,
        ));
        surface.pointer_move(&mut engine, action.centre().0, action.centre().1);
        surface.key(
            &mut engine,
            &KeyboardKey::Named(NamedKey::ArrowDown),
            None,
            ModifiersState::empty(),
        );
        let mut scene = Scene::new();
        surface.draw(&mut scene, Affine::IDENTITY, &mut engine, Lang::En);

        assert_eq!(surface.geometry() as *const Geometry, geometry);
        assert_eq!(surface.geometry().rows.as_ptr(), rows);
        assert_eq!(surface.geometry().targets.as_ptr(), targets);
        assert_eq!(surface.geometry().ordered.as_ptr(), ordered);
        assert_eq!(surface.geometry().failed.as_ptr(), failed);
    }

    #[test]
    fn normal_submission_states_do_not_publish_caption_status_pills() {
        let request = || Request {
            identity: RequestIdentity {
                token: RequestToken::new(),
                node: NodeId::new(),
            },
            phrase: "tomorrow".to_owned(),
            due_anchor: None,
        };
        let optimistic = [
            OptimisticCreation {
                request: request(),
                target: "target".to_owned(),
                reminder_id: "pending".to_owned(),
                draft: floating(9),
                state: OptimisticState::Pending,
            },
            OptimisticCreation {
                request: request(),
                target: "target".to_owned(),
                reminder_id: "succeeded".to_owned(),
                draft: floating(10),
                state: OptimisticState::Succeeded,
            },
        ];
        let mut surface = Surface::default();
        let mut engine = TextEngine::new();

        project_with(&mut surface, &mut engine, None, None, &optimistic);

        assert!(surface.geometry().failed.is_empty());
        assert!(surface.geometry().panel.is_none());
        assert_eq!(surface.geometry().hit(20.0, 55.0), None);
    }

    #[test]
    fn localized_failed_actions_have_measured_non_overlapping_geometry() {
        let request = Request {
            identity: RequestIdentity {
                token: RequestToken::new(),
                node: NodeId::new(),
            },
            phrase: "tomorrow".to_owned(),
            due_anchor: None,
        };
        let optimistic = OptimisticCreation {
            request,
            target: "target".to_owned(),
            reminder_id: "reminder".to_owned(),
            draft: floating(9),
            state: OptimisticState::Failed {
                reason: Failure::Rejected,
            },
        };
        let mut surface = Surface::default();
        let mut engine = TextEngine::new();
        for lang in Lang::ALL {
            surface.maintain_projection(
                &mut engine,
                VIEWPORT,
                lang,
                None,
                None,
                std::slice::from_ref(&optimistic),
            );
            let geometry = surface.geometry();
            let (_, pill, [retry, edit, dismiss]) = geometry.failed[0];
            assert!(
                retry.x >= pill.x && dismiss.x + dismiss.w <= pill.x + pill.w,
                "{lang:?}"
            );
            assert!(retry.x + retry.w + ActionButton::GAP <= edit.x, "{lang:?}");
            assert!(edit.x + edit.w + ActionButton::GAP <= dismiss.x, "{lang:?}");
        }
    }

    // ------------------------------------------- the recovery pill's keyboard

    /// One refused submission, standing as a pill with no popover open — which
    /// is the only state this pill is ever seen in.
    fn refused() -> OptimisticCreation {
        OptimisticCreation {
            request: Request {
                identity: RequestIdentity {
                    token: RequestToken::new(),
                    node: NodeId::new(),
                },
                phrase: "tomorrow".to_owned(),
                due_anchor: None,
            },
            target: "target".to_owned(),
            reminder_id: "reminder".to_owned(),
            draft: floating(9),
            state: OptimisticState::Failed {
                reason: Failure::Rejected,
            },
        }
    }

    /// **`F6`'s stop here is a real focus**, and Tab walks the three buttons.
    ///
    /// The pill stands with `mode` at `None` — there is no panel — so this is
    /// also the proof that `key` answers *before* the mode gate rather than
    /// falling out of it (Q-R1).
    #[test]
    fn a_standing_pill_takes_the_keyboard_and_tab_walks_its_three_buttons() {
        let mut surface = Surface::default();
        let mut engine = TextEngine::new();
        let optimistic = [refused()];
        project_with(&mut surface, &mut engine, None, None, &optimistic);
        assert!(surface.mode().is_none(), "no popover stands");
        assert!(surface.has_failed_pill());
        let token = surface.geometry().failed[0].0;

        assert!(surface.focus_first_failed_pill());
        assert!(surface.pill_focused());
        assert_eq!(
            surface.focus(),
            Some(Hit::Failed(token, FailedAction::Retry))
        );

        let tab = |surface: &mut Surface, engine: &mut TextEngine, shift: bool| {
            let mods = if shift {
                ModifiersState::SHIFT
            } else {
                ModifiersState::empty()
            };
            surface.key(engine, &KeyboardKey::Named(NamedKey::Tab), None, mods)
        };
        assert!(tab(&mut surface, &mut engine, false).handled);
        assert_eq!(
            surface.focus(),
            Some(Hit::Failed(token, FailedAction::Edit))
        );
        assert!(tab(&mut surface, &mut engine, false).handled);
        assert_eq!(
            surface.focus(),
            Some(Hit::Failed(token, FailedAction::Dismiss))
        );
        // …and round, so there is no end of the list to fall off.
        tab(&mut surface, &mut engine, false);
        assert_eq!(
            surface.focus(),
            Some(Hit::Failed(token, FailedAction::Retry))
        );
        tab(&mut surface, &mut engine, true);
        assert_eq!(
            surface.focus(),
            Some(Hit::Failed(token, FailedAction::Dismiss)),
            "Shift+Tab goes back"
        );
    }

    /// **Three refusals are one rotation**, because each pill recovers its own
    /// request: a Tab order that stopped at the first pill's three buttons
    /// would leave the other two reachable by pointer only, which is the exact
    /// hole this work closed.
    #[test]
    fn tab_reaches_every_standing_pill_not_only_the_first() {
        let mut surface = Surface::default();
        let mut engine = TextEngine::new();
        let optimistic = [refused(), refused(), refused()];
        project_with(&mut surface, &mut engine, None, None, &optimistic);
        let tokens: Vec<RequestToken> = surface
            .geometry()
            .failed
            .iter()
            .map(|(token, ..)| *token)
            .collect();
        assert_eq!(tokens.len(), 3);

        surface.focus_first_failed_pill();
        let mut seen = vec![surface.focus().expect("focused")];
        for _ in 0..8 {
            surface.key(
                &mut engine,
                &KeyboardKey::Named(NamedKey::Tab),
                None,
                ModifiersState::empty(),
            );
            seen.push(surface.focus().expect("still focused"));
        }
        for token in tokens {
            for action in [
                FailedAction::Retry,
                FailedAction::Edit,
                FailedAction::Dismiss,
            ] {
                assert!(
                    seen.contains(&Hit::Failed(token, action)),
                    "{action:?} on one of the three pills was never reached"
                );
            }
        }
    }

    /// **Enter performs the same action a press performs**, and Escape gives
    /// the keyboard back rather than leaving it on a control nobody can see.
    #[test]
    fn enter_recovers_the_focused_pill_and_escape_lets_go() {
        let mut surface = Surface::default();
        let mut engine = TextEngine::new();
        let optimistic = [refused()];
        project_with(&mut surface, &mut engine, None, None, &optimistic);
        let token = surface.geometry().failed[0].0;

        surface.focus_first_failed_pill();
        let outcome = surface.key(
            &mut engine,
            &KeyboardKey::Named(NamedKey::Enter),
            None,
            ModifiersState::empty(),
        );
        assert!(outcome.handled);
        assert!(
            matches!(outcome.action, Some(Action::Retry(asked)) if asked == token),
            "Enter on Try again asks to retry *this* request: {:?}",
            outcome.action
        );

        // Escape, from a fresh pill.
        project_with(&mut surface, &mut engine, None, None, &optimistic);
        surface.focus_first_failed_pill();
        let outcome = surface.key(
            &mut engine,
            &KeyboardKey::Named(NamedKey::Escape),
            None,
            ModifiersState::empty(),
        );
        assert!(outcome.handled);
        assert!(outcome.action.is_none(), "Escape recovers nothing");
        assert!(!surface.pill_focused(), "and the keyboard went back");
    }

    /// **Typing is not swallowed.** A chrome stop claims what moves it and what
    /// activates it and nothing else — the bargain the bell already strikes
    /// with `F6`. A pill that ate keys would stop the document dead for as long
    /// as a failure stood.
    #[test]
    fn a_focused_pill_lets_every_other_key_through() {
        let mut surface = Surface::default();
        let mut engine = TextEngine::new();
        project_with(&mut surface, &mut engine, None, None, &[refused()]);
        surface.focus_first_failed_pill();

        for key in [
            KeyboardKey::Character("a".into()),
            KeyboardKey::Named(NamedKey::Backspace),
            KeyboardKey::Named(NamedKey::Home),
        ] {
            let outcome = surface.key(&mut engine, &key, Some("a"), ModifiersState::empty());
            assert!(!outcome.handled, "{key:?} was swallowed by the pill");
            assert!(surface.pill_focused(), "{key:?} moved the focus");
        }
    }

    /// **A pill that has gone does not keep the keyboard.** All three of its
    /// buttons end the pill, so the focus has to be let go where the geometry
    /// says the rectangle stopped being drawn — otherwise the published focus
    /// names a control that is not in the tree.
    #[test]
    fn a_pill_that_stops_standing_gives_the_keyboard_back() {
        let mut surface = Surface::default();
        let mut engine = TextEngine::new();
        project_with(&mut surface, &mut engine, None, None, &[refused()]);
        surface.focus_first_failed_pill();
        assert!(surface.pill_focused());

        // The controller resolved the failure: nothing optimistic is left.
        project_with(&mut surface, &mut engine, None, None, &[]);
        assert!(!surface.has_failed_pill());
        assert!(
            !surface.pill_focused(),
            "the focus went with the rectangle it named"
        );
        assert_eq!(
            surface.focus(),
            None,
            "and landed nowhere, not somewhere odd"
        );
    }

    /// With no pill standing there is nothing to focus, so `F6`'s stop here
    /// simply is not one — the rotation skips it rather than stopping dead.
    #[test]
    fn there_is_no_stop_when_no_save_has_failed() {
        let mut surface = Surface::default();
        let mut engine = TextEngine::new();
        project_with(&mut surface, &mut engine, None, None, &[]);
        assert!(!surface.has_failed_pill());
        assert!(!surface.focus_first_failed_pill());
        assert!(!surface.pill_focused());
    }

    /// **A press past the pill gives the keyboard back.** The bell does this
    /// from `Shell::mouse_down`, and without it a ring would be left standing
    /// on a control the person had visibly moved on from — and Enter would then
    /// recover a reminder instead of splitting a line.
    #[test]
    fn pressing_past_a_focused_pill_releases_it_without_claiming_the_press() {
        let mut surface = Surface::default();
        let mut engine = TextEngine::new();
        project_with(&mut surface, &mut engine, None, None, &[refused()]);
        surface.focus_first_failed_pill();
        assert!(surface.pill_focused());

        // Somewhere the pill is not: the geometry's only rectangles are its.
        let (_, pill, _) = surface.geometry().failed[0];
        let outcome = surface.mouse_down(&mut engine, pill.x - 40.0, pill.y + pill.h + 40.0);
        assert!(
            !outcome.handled,
            "the press belongs to whatever is under it"
        );
        assert!(outcome.action.is_none());
        assert!(!surface.pill_focused(), "and the ring went with it");
        assert!(
            surface.has_failed_pill(),
            "pressing past a pill does not dismiss it"
        );
    }
}
