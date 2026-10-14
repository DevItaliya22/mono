// SPDX-License-Identifier: MPL-2.0

//! Native Socials surface. Store work is queued to a dedicated worker.
/// Public only for `examples/store-smoke.rs`, which drives the same worker
/// against a real Store; `AddChannel` exists for that exercise.
#[doc(hidden)]
pub mod backend;
mod navigation;
mod paint;
pub use navigation::NavigationOutcome;
pub type SharedSurface = std::rc::Rc<std::cell::RefCell<Surface>>;
#[cfg(feature = "no-reader")]
mod preview;
#[cfg(feature = "no-reader")]
pub use preview::fixture as preview_fixture;

use chrono::{Datelike, Local, TimeZone, Timelike};
use koru_draw::{geometry::Rect, text::TextEngine, theme};
use koru_i18n::{
    socials::{Key as Label, t},
    time::{DateTimeParts, LocaleFormat},
};
use koru_socials::{Channel, DeliveryState, Destination, Issue, Network, Post, Status};
use koru_store_core::WorkspaceId;
use koru_ui::{TextArea, TextInput};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use uuid::Uuid;
use vello::{Scene, kurbo::Affine};
use winit::{
    event::MouseScrollDelta,
    keyboard::{Key, ModifiersState, NamedKey},
    window::CursorIcon,
};

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Scheduled,
    Drafts,
    History,
    Accounts,
}
#[derive(Clone, Copy, PartialEq)]
enum Field {
    Title,
    Body,
    Variant,
    Date,
    Time,
    Subreddit,
    RedditTitle,
    Flair,
    ClientId,
    OrganizationId,
}
#[derive(Clone, PartialEq)]
enum Action {
    Agenda,
    Page(Page),
    New,
    Refresh,
    Week(i64),
    Today,
    Open(Uuid),
    Slot(i64),
    Toggle(Uuid),
    Select(Uuid),
    Reset,
    Save(bool),
    Close,
    Delete,
    ConfirmDelete,
    Field(Field),
    Variants,
    Original,
    Edit,
    Resume,
    ConnectProvider,
    Provider(Network),
    CancelConnection,
    ChannelQueue,
    ChannelPosts,
    OpenSchedule,
    Duplicate,
    Retry,
    CancelDelivery,
    OpenPublished(String),
}

pub struct Surface {
    engine: TextEngine,
    backend: backend::Backend,
    workspace: Option<WorkspaceId>,
    viewport: Rect,
    posts: Vec<Post>,
    channels: Vec<Channel>,
    page: Page,
    agenda: bool,
    week: i64,
    scroll: f32,
    working: Option<Post>,
    base: Option<Post>,
    selected: Option<Uuid>,
    title: TextInput,
    body: TextArea,
    variant: TextArea,
    preview: TextArea,
    date: TextInput,
    time: TextInput,
    subreddit: TextInput,
    reddit_title: TextInput,
    flair: TextInput,
    client_id: TextInput,
    organization_id: TextInput,
    account_network: Network,
    focus: Option<Field>,
    keyboard_target: Option<Action>,
    hits: Vec<(Rect, Action)>,
    pointer: (f32, f32),
    pending: usize,
    submitted: Option<Post>,
    form_scroll: f32,
    variant_scroll: f32,
    form_max: f32,
    variant_max: f32,
    variant_x: f32,
    preview_dirty: bool,
    preview_count: usize,
    preview_parts: usize,
    form_bar: Option<koru_ui::scrollbar::ScrollbarGeom>,
    variant_bar: Option<koru_ui::scrollbar::ScrollbarGeom>,
    scroll_drag: Option<(bool, koru_ui::scrollbar::ScrollbarGeom, f32)>,
    message: Option<Label>,
    filter: Option<Uuid>,
    confirm_delete: bool,
    show_variants: bool,
    drag_post: Option<(Uuid, (f32, f32), bool)>,
    initial_date: String,
    initial_time: String,
    preview_rect: Option<Rect>,
    navigation: navigation::Navigation,
    channel_page: Option<Uuid>,
    composer_visible: bool,
    browser_opener: fn(&str) -> std::io::Result<()>,
    connecting: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    connected: std::collections::HashSet<Uuid>,
    device: Option<Uuid>,
}

impl Surface {
    pub fn new(root: Option<PathBuf>) -> Self {
        let mut engine = TextEngine::new();
        let title = TextInput::new(&mut engine, theme::role::ui());
        let body = TextArea::new(&mut engine, theme::role::ui());
        let variant = TextArea::new(&mut engine, theme::role::ui());
        let preview = TextArea::new(&mut engine, theme::role::ui());
        let date = TextInput::new(&mut engine, theme::role::ui());
        let time = TextInput::new(&mut engine, theme::role::ui());
        let subreddit = TextInput::new(&mut engine, theme::role::ui());
        let reddit_title = TextInput::new(&mut engine, theme::role::ui());
        let flair = TextInput::new(&mut engine, theme::role::ui());
        let organization_id = TextInput::new(&mut engine, theme::role::ui());
        let client_id = TextInput::new(&mut engine, theme::role::ui());
        Self {
            engine,
            backend: backend::Backend::new(root),
            workspace: None,
            viewport: Rect::new(0., 0., 900., 700.),
            posts: vec![],
            channels: vec![],
            page: Page::Scheduled,
            agenda: false,
            week: 0,
            scroll: 432.,
            working: None,
            base: None,
            selected: None,
            title,
            body,
            variant,
            preview,
            date,
            time,
            subreddit,
            reddit_title,
            flair,
            client_id,
            organization_id,
            account_network: Network::X,
            focus: None,
            keyboard_target: None,
            hits: vec![],
            pointer: (0., 0.),
            pending: 0,
            submitted: None,
            form_scroll: 0.,
            variant_scroll: 0.,
            form_max: 0.,
            variant_max: 0.,
            variant_x: f32::MAX,
            preview_dirty: true,
            preview_count: 0,
            preview_parts: 0,
            form_bar: None,
            variant_bar: None,
            scroll_drag: None,
            message: None,
            filter: None,
            confirm_delete: false,
            show_variants: false,
            drag_post: None,
            initial_date: String::new(),
            initial_time: String::new(),
            preview_rect: None,
            navigation: navigation::Navigation::default(),
            channel_page: None,
            composer_visible: false,
            browser_opener: |_| Err(std::io::Error::other("browser unavailable")),
            connecting: None,
            connected: Default::default(),
            device: None,
        }
    }
    pub fn set_browser_opener(&mut self, opener: fn(&str) -> std::io::Result<()>) {
        self.browser_opener = opener;
    }
    pub fn set_context_workspace(&mut self, workspace: Option<WorkspaceId>) {
        if self.workspace == workspace {
            return;
        }
        if self.workspace.is_some() && self.request_close() {
            return;
        }
        self.workspace = workspace;
        self.posts.clear();
        self.channels.clear();
        self.connected.clear();
        self.channel_page = None;
        self.working = None;
        self.composer_visible = false;
        self.base = None;
        self.filter = None;
        self.load();
    }
    fn send(&mut self, request: backend::Request) {
        if self.backend.send.send(request).is_ok() {
            self.pending += 1;
        } else {
            self.message = Some(Label::StorageUnavailable);
        }
    }
    fn load(&mut self) {
        if let Some(ws) = self.workspace {
            self.send(backend::Request::Load(ws));
        } else {
            self.message = Some(Label::NoWorkspace);
        }
    }
    pub fn maintain(&mut self, _now: Instant) -> bool {
        let mut changed = false;
        while let Ok(reply) = self.backend.receive.try_recv() {
            if !matches!(
                &reply,
                backend::Reply::PostChanged(..)
                    | backend::Reply::ListingChanged(..)
                    | backend::Reply::WatchFailed
            ) {
                self.pending = self.pending.saturating_sub(1);
            }
            changed = true;
            match reply {
                backend::Reply::Loaded(ws, listing)
                | backend::Reply::ListingChanged(ws, listing)
                    if Some(ws) == self.workspace =>
                {
                    self.device = listing.device;
                    self.posts = listing.posts;
                    self.channels = listing.channels;
                    self.connected = listing.connected;
                    self.message = listing.incomplete.then_some(Label::StorageUnavailable);
                }
                backend::Reply::PostChanged(ws, post) if Some(ws) == self.workspace => {
                    if self.working.as_ref().is_some_and(|p| p.id == post.id)
                        && self.working == self.base
                    {
                        self.open(post.clone(), Some(post.clone()));
                    }
                    self.posts.retain(|p| p.id != post.id);
                    self.posts.push(post);
                }
                backend::Reply::WatchFailed => self.message = Some(Label::StorageUnavailable),
                backend::Reply::Imported(ws, post) if Some(ws) == self.workspace => {
                    self.open(post, None)
                }
                backend::Reply::Saved(ws, post) if Some(ws) == self.workspace => {
                    self.posts.retain(|p| p.id != post.id);
                    self.posts.push(post);
                    self.working = None;
                    self.composer_visible = false;
                    self.base = None;
                    self.focus = None;
                    self.message = Some(Label::Saved);
                    self.confirm_delete = false;
                }
                backend::Reply::ChannelSaved(ws, channel) if Some(ws) == self.workspace => {
                    self.channels.retain(|c| c.id != channel.id);
                    self.channels.push(channel);
                    self.message = Some(Label::AccountSaved);
                }
                backend::Reply::ChannelConnected(ws, channel) if Some(ws) == self.workspace => {
                    self.connecting = None;
                    self.connected.insert(channel.id);
                    self.channel_page = Some(channel.id);
                    self.filter = Some(channel.id);
                    self.navigation.expand(channel.id);
                    self.channels.retain(|c| c.id != channel.id);
                    self.channels.push(channel);
                    self.page = Page::Scheduled;
                    self.scroll = 0.;
                    self.message = Some(Label::Connected);
                }
                backend::Reply::Failed(failure) => {
                    self.connecting = None;
                    self.message = Some(match failure {
                        backend::Failure::Changed => Label::ChangedElsewhere,
                        backend::Failure::Invalid | backend::Failure::Unavailable => {
                            Label::SaveFailed
                        }
                        backend::Failure::OAuth(error) => oauth_label(error),
                    })
                }
                _ => {}
            }
        }
        if changed {
            self.navigation.refresh_counts(&self.posts);
        }
        changed
    }
    pub fn maintenance_deadline(&self) -> Option<Instant> {
        (self.pending > 0 || self.workspace.is_some())
            .then(|| Instant::now() + Duration::from_millis(100))
    }
    fn connect_provider(&mut self) {
        if self.pending > 0 {
            return;
        }
        if self.client_id.value().trim().is_empty() {
            self.message = Some(Label::OAuthClientRequired);
            self.focus = Some(Field::ClientId);
            return;
        }
        if self.account_network == Network::LinkedInPage {
            let organization = self.organization_id.value();
            let organization = organization.trim();
            if organization.is_empty()
                || organization.len() > 20
                || organization.starts_with('0')
                || !organization.bytes().all(|b| b.is_ascii_digit())
            {
                self.message = Some(Label::OrganizationRequired);
                self.focus = Some(Field::OrganizationId);
                return;
            }
        }
        let Some(workspace) = self.workspace else {
            self.message = Some(Label::NoWorkspace);
            return;
        };
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.connecting = Some(cancelled.clone());
        self.message = Some(Label::OAuthBrowserHint);
        self.send(backend::Request::ConnectProvider {
            workspace,
            client_id: self.client_id.value(),
            network: self.account_network,
            organization: self.organization_id.value().trim().to_owned(),
            cancelled,
            open_browser: self.browser_opener,
            success_html: label(Label::OAuthSuccess).into(),
            error_html: label(Label::OAuthFailed).into(),
        });
    }
    pub fn set_viewport(&mut self, rect: Rect) {
        self.viewport = rect;
    }
    pub fn invalidate(&mut self) {}
    pub fn is_dragging(&self) -> bool {
        self.drag_post.is_some() || self.scroll_drag.is_some()
    }
    pub fn import_document(
        &mut self,
        workspace: WorkspaceId,
        document: koru_store_core::DocumentId,
        title: String,
    ) -> bool {
        if self.request_close() {
            return false;
        }
        self.set_context_workspace(Some(workspace));
        self.page = Page::Drafts;
        self.send(backend::Request::Import {
            workspace,
            document,
            title,
        });
        true
    }
    /// A host close is vetoed while edits need an explicit save or discard.
    pub fn request_close(&mut self) -> bool {
        if self.pending > 0 {
            self.message = Some(Label::Saving);
            return true;
        }
        self.sync_fields();
        if self.working.as_ref().is_some_and(|post| {
            self.base.as_ref() != Some(post)
                || self.date.raw_text() != self.initial_date
                || self.time.raw_text() != self.initial_time
        }) {
            // Navigation can hide the retained editor. A veto must bring it
            // back, otherwise New/Open appear inert and the user cannot see
            // the work that must be saved or explicitly discarded.
            self.composer_visible = true;
            if self.page == Page::Accounts {
                self.page = Page::Drafts;
            }
            self.focus = Some(Field::Body);
            self.message = Some(Label::Unsaved);
            return true;
        }
        false
    }
    pub fn save_draft(&mut self) -> bool {
        if self.working.is_none() {
            return false;
        }
        self.save(self.working.as_ref().is_some_and(|p| p.scheduled), false);
        true
    }
    pub fn cursor_icon(&self) -> CursorIcon {
        match self
            .hits
            .iter()
            .rev()
            .find(|(r, _)| r.contains(self.pointer.0, self.pointer.1))
        {
            Some((_, Action::Field(_))) => CursorIcon::Text,
            Some(_) => CursorIcon::Default,
            None => CursorIcon::Default,
        }
    }
    fn open(&mut self, post: Post, base: Option<Post>) {
        self.title.set_text(&mut self.engine, &post.title);
        self.body.set_text(&mut self.engine, &post.body);
        let timestamp = post
            .scheduled_at_ms
            .unwrap_or_else(|| Local::now().timestamp_millis() + 3_600_000);
        let dt = Local
            .timestamp_millis_opt(timestamp)
            .single()
            .unwrap_or_else(Local::now);
        let format = LocaleFormat::for_system(None, None);
        self.date
            .set_text(&mut self.engine, &format.date_input(parts(dt)));
        self.time
            .set_text(&mut self.engine, &format.time_input(parts(dt)));
        self.initial_date = self.date.raw_text().into();
        self.initial_time = self.time.raw_text().into();
        self.working = Some(post);
        self.composer_visible = true;
        self.submitted = None;
        self.form_scroll = 0.;
        self.variant_scroll = 0.;
        self.base = base;
        self.selected = None;
        self.show_variants = false;
        self.focus = Some(Field::Body);
        self.message = None;
        self.confirm_delete = false;
    }
    fn sync_fields(&mut self) {
        self.preview_dirty = true;
        let Some(post) = &mut self.working else {
            return;
        };
        if !post.editable() {
            return;
        }
        post.title = self.title.value();
        post.body = self.body.raw_text().into();
        if let Some(d) = post
            .destinations
            .iter_mut()
            .find(|d| Some(d.channel.id) == self.selected)
        {
            if d.override_body.is_some() {
                d.override_body = Some(self.variant.raw_text().into());
            }
            d.subreddit = self.subreddit.value();
            d.reddit_title = self.reddit_title.value();
            d.flair_id = (!self.flair.value().is_empty()).then(|| self.flair.value());
        }
    }
    fn select(&mut self, id: Uuid) {
        self.preview_dirty = true;
        self.sync_fields();
        self.selected = Some(id);
        if let Some(d) = self
            .working
            .as_ref()
            .and_then(|p| p.destinations.iter().find(|d| d.channel.id == id))
        {
            let body = d.body(&self.working.as_ref().unwrap().body).to_string();
            self.variant.set_text(&mut self.engine, &body);
            self.subreddit.set_text(&mut self.engine, &d.subreddit);
            self.reddit_title
                .set_text(&mut self.engine, &d.reddit_title);
            self.flair
                .set_text(&mut self.engine, d.flair_id.as_deref().unwrap_or(""));
        }
    }
    fn save(&mut self, scheduled: bool, deleted: bool) {
        if self.pending > 0 {
            return;
        }
        if scheduled
            && self
                .base
                .as_ref()
                .is_some_and(|p| p.execution_device.is_none())
        {
            self.message = Some(Label::LegacySchedule);
            return;
        }
        self.sync_fields();
        let Some(ws) = self.workspace else {
            self.message = Some(Label::NoWorkspace);
            return;
        };
        let Some(mut post) = self.working.clone() else {
            return;
        };
        if !post.editable() {
            self.message = Some(Label::ReadOnly);
            return;
        }
        post.scheduled = scheduled;
        post.deleted = deleted;
        post.updated_at_ms = Local::now().timestamp_millis();
        let format = LocaleFormat::for_system(None, None);
        post.scheduled_at_ms = format
            .parse_date_input(&self.date.value())
            .and_then(|(y, m, d)| {
                let (h, min) = format.parse_time_input(&self.time.value())?;
                Local
                    .with_ymd_and_hms(y, m, d, h, min, 0)
                    .single()
                    .map(|dt| dt.timestamp_millis())
            });
        // Credentials and the durable execution owner are assigned by the publisher,
        // never inferred from merely opening the composer.
        if scheduled && !deleted {
            for destination in &mut post.destinations {
                if destination.channel.remote_actor.is_none() {
                    destination.delivery.state = DeliveryState::NeedsConnection;
                }
            }
        }
        let mut identical_retry = false;
        if let Some(previous) = &self.submitted {
            let mut retry = post.clone();
            retry.updated_at_ms = previous.updated_at_ms;
            if &retry == previous {
                post = retry;
                identical_retry = true;
            }
        }
        if scheduled && !deleted && !identical_retry {
            if let Some((_, issue)) =
                koru_socials::validate(&post, Local::now().timestamp_millis()).first()
            {
                self.message = Some(issue_label(*issue));
                return;
            }
        }
        self.submitted = Some(post.clone());
        self.send(backend::Request::Save {
            workspace: ws,
            post,
            base: self.base.clone(),
        });
        self.message = Some(Label::Saving);
    }
    fn act(&mut self, action: Action) {
        match action {
            Action::Duplicate => {
                self.sync_fields();
                if let Some(mut post) = self.working.clone() {
                    post.id = Uuid::new_v4();
                    post.created_at_ms = Local::now().timestamp_millis();
                    post.updated_at_ms = post.created_at_ms;
                    post.execution_device = None;
                    post.scheduled = false;
                    post.scheduled_at_ms = None;
                    post.cancel_requested = false;
                    post.retry_request = None;
                    post.deleted = false;
                    for d in &mut post.destinations {
                        d.delivery = Default::default();
                    }
                    self.open(post, None);
                }
            }
            Action::Retry | Action::CancelDelivery => {
                let Some(ws) = self.workspace else { return };
                let Some(base) = self.base.clone() else {
                    return;
                };
                if base.execution_device.is_none() {
                    self.message = Some(Label::LegacySchedule);
                    return;
                }
                let mut post = base.clone();
                if action == Action::Retry {
                    if post.cancel_requested
                        || !post.destinations.iter().any(|d| {
                            matches!(
                                d.delivery.state,
                                DeliveryState::Failed | DeliveryState::NeedsConnection
                            )
                        })
                    {
                        return;
                    }
                    post.retry_request = Some(Uuid::new_v4());
                } else {
                    post.cancel_requested = true;
                }
                post.updated_at_ms = Local::now().timestamp_millis();
                self.send(backend::Request::Save {
                    workspace: ws,
                    post,
                    base: Some(base),
                });
                self.message = Some(if action == Action::Retry {
                    Label::RetryQueued
                } else {
                    Label::CancelRequested
                });
            }
            Action::OpenPublished(url) => {
                let _ = (self.browser_opener)(&url);
            }
            Action::Resume => self.composer_visible = true,
            Action::ChannelQueue => {
                self.page = Page::Scheduled;
                self.scroll = 0.;
            }
            Action::ChannelPosts => {
                self.page = Page::History;
                self.scroll = 0.;
            }
            Action::OpenSchedule => {
                self.channel_page = None;
                self.page = Page::Scheduled;
                self.agenda = false;
                self.scroll = 432.;
            }
            Action::Agenda => {
                self.agenda = !self.agenda;
                self.scroll = if self.agenda { 0. } else { 432. };
            }
            Action::Page(page) => {
                self.sync_fields();
                self.page = page;
                self.scroll = if page == Page::Scheduled { 432. } else { 0. };
                self.focus = None;
                self.composer_visible = false;
            }
            Action::New => {
                if !self.request_close() {
                    let mut post = Post::draft(Local::now().timestamp_millis());
                    if let Some(channel) = self.channels.iter().find(|c| Some(c.id) == self.filter)
                    {
                        post.destinations.push(Destination::new(channel.clone()));
                    }
                    self.open(post, None)
                }
            }
            Action::Refresh => {
                if self.pending == 0 {
                    self.load();
                }
            }
            Action::Week(n) => self.week += n,
            Action::Today => self.week = 0,
            Action::Open(id) => {
                if self.request_close() {
                    return;
                }
                if let Some(post) = self.posts.iter().find(|p| p.id == id).cloned() {
                    self.open(post.clone(), Some(post));
                }
            }
            Action::Slot(at) => {
                let mut p = Post::draft(Local::now().timestamp_millis());
                p.scheduled_at_ms = Some(at);
                self.open(p, None);
            }
            Action::Close => {
                self.working = None;
                self.composer_visible = false;
                self.base = None;
                self.focus = None;
                self.confirm_delete = false;
            }
            Action::Save(scheduled) => self.save(scheduled, false),
            Action::Delete => self.confirm_delete = true,
            Action::ConfirmDelete => self.save(false, true),
            Action::Provider(network) => {
                if self.pending > 0 {
                    return;
                }
                self.account_network = network;
                self.client_id.set_text(&mut self.engine, "");
                self.organization_id.set_text(&mut self.engine, "");
                self.focus = Some(Field::ClientId);
                self.message = None;
                self.scroll = 0.;
            }
            Action::ConnectProvider => self.connect_provider(),
            Action::CancelConnection => {
                if let Some(cancelled) = &self.connecting {
                    cancelled.store(true, std::sync::atomic::Ordering::Release);
                }
            }
            Action::Toggle(id) => {
                self.sync_fields();
                if let Some(p) = &mut self.working {
                    if p.editable() {
                        if p.destinations.iter().any(|d| d.channel.id == id) {
                            p.destinations.retain(|d| d.channel.id != id);
                            if self.selected == Some(id) {
                                self.selected = None;
                            }
                        } else if let Some(c) = self.channels.iter().find(|c| c.id == id) {
                            p.destinations.push(Destination::new(c.clone()));
                        }
                    }
                }
            }
            Action::Select(id) => self.select(id),
            Action::Reset => {
                if let Some(p) = &mut self.working {
                    if p.editable() {
                        if let Some(d) = p
                            .destinations
                            .iter_mut()
                            .find(|d| Some(d.channel.id) == self.selected)
                        {
                            d.override_body = None;
                        }
                    }
                }
                if let Some(id) = self.selected {
                    self.select(id);
                }
            }
            Action::Field(field) => self.focus = Some(field),
            Action::Variants => {
                self.show_variants = true;
                self.focus = None;
            }
            Action::Original => {
                self.show_variants = false;
                self.focus = None;
            }
            Action::Edit => {
                self.sync_fields();
                if let Some(p) = &mut self.working {
                    if p.editable() {
                        if let Some(d) = p
                            .destinations
                            .iter_mut()
                            .find(|d| Some(d.channel.id) == self.selected)
                        {
                            d.override_body = Some(d.body(&p.body).to_string());
                        }
                    }
                }
                if let Some(id) = self.selected {
                    self.select(id);
                    self.focus = Some(Field::Variant);
                }
            }
        }
    }
    pub fn key(&mut self, key: &Key, text: Option<&str>, mods: ModifiersState) -> bool {
        if self.connecting.is_some() && *key == Key::Named(NamedKey::Escape) {
            self.act(Action::CancelConnection);
            return true;
        }
        if self.pending > 0 {
            return false;
        }
        if *key == Key::Named(NamedKey::Escape) {
            self.focus = None;
            self.keyboard_target = None;
            return true;
        }
        if *key == Key::Named(NamedKey::Tab) {
            let targets: Vec<_> = self
                .hits
                .iter()
                .filter(|(_, a)| !matches!(a, Action::Slot(_)))
                .map(|(_, a)| a.clone())
                .collect();
            if !targets.is_empty() {
                let current = self
                    .keyboard_target
                    .clone()
                    .or_else(|| self.focus.map(Action::Field));
                let index = current.and_then(|target| targets.iter().position(|x| *x == target));
                let next = match index {
                    None => 0,
                    Some(i) if mods.shift_key() => (i + targets.len() - 1) % targets.len(),
                    Some(i) => (i + 1) % targets.len(),
                };
                self.focus = if let Action::Field(field) = targets[next] {
                    Some(field)
                } else {
                    None
                };
                self.keyboard_target = Some(targets[next].clone());
                return true;
            }
        }
        if matches!(key, Key::Named(NamedKey::Enter | NamedKey::Space)) && self.focus.is_none() {
            if let Some(action) = self.keyboard_target.take() {
                if self.hits.iter().any(|(_, a)| *a == action) {
                    self.act(action);
                    return true;
                }
            }
        }
        if self.page != Page::Accounts && self.working.as_ref().is_some_and(|p| !p.editable()) {
            return false;
        }
        let Some(field) = self.focus else {
            return false;
        };
        let previous_variant = self.variant.raw_text().to_string();
        match field {
            Field::Body => {
                self.body.key(&mut self.engine, key, text, mods);
            }
            Field::Variant => {
                self.variant.key(&mut self.engine, key, text, mods);
            }
            _ => {
                let (input, engine) = match field {
                    Field::Title => (&mut self.title, &mut self.engine),
                    Field::Date => (&mut self.date, &mut self.engine),
                    Field::Time => (&mut self.time, &mut self.engine),
                    Field::Subreddit => (&mut self.subreddit, &mut self.engine),
                    Field::RedditTitle => (&mut self.reddit_title, &mut self.engine),
                    Field::Flair => (&mut self.flair, &mut self.engine),
                    Field::ClientId => (&mut self.client_id, &mut self.engine),
                    Field::OrganizationId => (&mut self.organization_id, &mut self.engine),
                    _ => unreachable!(),
                };
                input.key(engine, key, text, mods);
            }
        }
        if field == Field::Variant && previous_variant != self.variant.raw_text() {
            if let Some(p) = &mut self.working {
                if let Some(d) = p
                    .destinations
                    .iter_mut()
                    .find(|d| Some(d.channel.id) == self.selected)
                {
                    d.override_body = Some(self.variant.raw_text().into());
                }
            }
        }
        self.sync_fields();
        true
    }
    pub fn mouse_down(&mut self, x: f32, y: f32) -> bool {
        self.pointer = (x, y);
        self.keyboard_target = None;
        if self.pending > 0 {
            if self
                .hits
                .iter()
                .any(|(rect, action)| *action == Action::CancelConnection && rect.contains(x, y))
            {
                self.act(Action::CancelConnection);
                return true;
            }
            return false;
        }
        for (variant, bar) in [(false, self.form_bar), (true, self.variant_bar)] {
            if let Some(bar) = bar {
                if bar.in_gutter(x, y) {
                    let grab = if bar.on_thumb(x, y) {
                        y - bar.thumb_top
                    } else {
                        bar.thumb_h / 2.
                    };
                    self.scroll_drag = Some((variant, bar, grab));
                    return self.mouse_move(x, y);
                }
            }
        }
        let hit = self
            .hits
            .iter()
            .rev()
            .find(|(r, _)| r.contains(x, y))
            .cloned();
        let Some((rect, action)) = hit else {
            self.focus = None;
            return false;
        };
        if self.pending > 0 {
            return false;
        }
        if let Action::Open(id) = action {
            if self.page == Page::Scheduled && !self.agenda && self.working.is_none() {
                self.drag_post = Some((id, (x, y), false));
                return true;
            }
        }
        if let Action::Field(field) = action {
            self.focus = Some(field);
            match field {
                Field::Body => {
                    self.body.click(&mut self.engine, rect, x, y);
                }
                Field::Variant => {
                    self.variant.click(&mut self.engine, rect, x, y);
                }
                _ => {
                    let input = match field {
                        Field::Title => &mut self.title,
                        Field::Date => &mut self.date,
                        Field::Time => &mut self.time,
                        Field::Subreddit => &mut self.subreddit,
                        Field::RedditTitle => &mut self.reddit_title,
                        Field::Flair => &mut self.flair,
                        Field::ClientId => &mut self.client_id,
                        Field::OrganizationId => &mut self.organization_id,
                        _ => unreachable!(),
                    };
                    input.click(&mut self.engine, rect, x, y);
                }
            }
        } else {
            self.act(action);
        }
        true
    }
    pub fn mouse_move(&mut self, x: f32, y: f32) -> bool {
        self.pointer = (x, y);
        if let Some((variant, bar, grab)) = self.scroll_drag {
            let scroll = bar.scroll_for_pointer(y, grab);
            if variant {
                self.variant_scroll = scroll;
            } else {
                self.form_scroll = scroll;
            }
            return true;
        }
        if let Some((_, origin, moved)) = &mut self.drag_post {
            *moved |= (origin.0 - x).abs() + (origin.1 - y).abs() > 5.;
            return true;
        }
        if let Some(field) = self.focus {
            if let Some((rect, _)) = self
                .hits
                .iter()
                .find(|(_, a)| matches!(a,Action::Field(f) if *f==field))
            {
                match field {
                    Field::Body => {
                        self.body.drag(&mut self.engine, *rect, x, y);
                    }
                    Field::Variant => {
                        self.variant.drag(&mut self.engine, *rect, x, y);
                    }
                    _ => {
                        let input = match field {
                            Field::Title => &mut self.title,
                            Field::Date => &mut self.date,
                            Field::Time => &mut self.time,
                            Field::Subreddit => &mut self.subreddit,
                            Field::RedditTitle => &mut self.reddit_title,
                            Field::Flair => &mut self.flair,
                            Field::ClientId => &mut self.client_id,
                            Field::OrganizationId => &mut self.organization_id,
                            _ => unreachable!(),
                        };
                        input.drag(&mut self.engine, *rect, x);
                    }
                }
            }
        }
        true
    }
    pub fn mouse_up(&mut self) -> bool {
        if self.scroll_drag.take().is_some() {
            return true;
        }
        if let Some((id, _, moved)) = self.drag_post.take() {
            if !moved {
                self.act(Action::Open(id));
            } else if let Some(at) = self.drop_time() {
                if let Some(mut post) = self.posts.iter().find(|p| p.id == id).cloned() {
                    if post.editable() {
                        let base = post.clone();
                        post.scheduled_at_ms = Some(at);
                        self.open(post, Some(base));
                        self.save(true, false);
                    }
                }
            }
            return true;
        }
        self.body.release();
        self.variant.release();
        self.title.release();
        self.date.release();
        self.time.release();
        self.subreddit.release();
        self.reddit_title.release();
        self.flair.release();
        self.client_id.release();
        self.organization_id.release();
        true
    }
    pub fn mouse_double_click(&mut self, x: f32, y: f32) -> bool {
        self.mouse_down(x, y)
    }
    pub fn scroll_by(&mut self, delta: MouseScrollDelta) {
        let dy = match delta {
            MouseScrollDelta::LineDelta(_, y) => y * 36.,
            MouseScrollDelta::PixelDelta(p) => p.y as f32,
        };
        if self
            .preview_rect
            .is_some_and(|rect| rect.contains(self.pointer.0, self.pointer.1))
        {
            self.preview.scroll_by(dy);
            return;
        }
        let hovered = self.hits.iter().rev().find_map(|(rect, action)| {
            if rect.contains(self.pointer.0, self.pointer.1) {
                if let Action::Field(field) = action {
                    return Some(*field);
                }
            }
            None
        });
        match hovered {
            Some(Field::Body) => {
                self.body.scroll_by(dy);
            }
            Some(Field::Variant) => {
                self.variant.scroll_by(dy);
            }
            _ if self.working.is_some() && self.composer_visible && self.page != Page::Accounts => {
                if self.pointer.0 >= self.variant_x || self.show_variants {
                    self.variant_scroll = (self.variant_scroll - dy).clamp(0., self.variant_max);
                } else {
                    self.form_scroll = (self.form_scroll - dy).clamp(0., self.form_max);
                }
            }
            _ => self.scroll = (self.scroll - dy).max(0.),
        }
    }
    pub fn draw(&mut self, scene: &mut Scene, root: Affine) {
        self.paint(scene, root);
    }
    fn drop_time(&self) -> Option<i64> {
        self.hits
            .iter()
            .filter_map(|(rect, action)| {
                if let Action::Slot(at) = action {
                    if self.pointer.0 >= rect.x && self.pointer.0 < rect.x + rect.w {
                        return Some(((self.pointer.1 - rect.y).abs(), *at));
                    }
                }
                None
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .filter(|(distance, _)| *distance < 40.)
            .map(|(_, at)| at)
    }
}
fn label(key: Label) -> &'static str {
    t(koru_i18n::current(), key)
}
fn oauth_label(error: koru_socials_x::Error) -> Label {
    use koru_socials_x::Error;
    match error {
        Error::InvalidClientId => Label::OAuthClientRequired,
        Error::Cancelled => Label::OAuthCancelled,
        Error::TimedOut => Label::OAuthTimedOut,
        Error::CallbackUnavailable => Label::OAuthCallbackBusy,
        Error::VaultUnavailable => Label::OAuthVaultFailed,
        Error::Denied => Label::OAuthDenied,
        Error::AuthorizationRejected
        | Error::IdentityRejected
        | Error::MissingScopes
        | Error::InvalidResponse => Label::OAuthApiFailed,
        _ => Label::OAuthFailed,
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        if let Some(cancelled) = &self.connecting {
            cancelled.store(true, std::sync::atomic::Ordering::Release);
        }
    }
}
fn parts(dt: chrono::DateTime<Local>) -> DateTimeParts {
    DateTimeParts {
        year: dt.year(),
        month: dt.month() as u8,
        day: dt.day() as u8,
        hour: dt.hour() as u8,
        minute: dt.minute() as u8,
        second: dt.second() as u8,
    }
}
fn status_label(status: Status) -> Label {
    match status {
        Status::Draft => Label::Draft,
        Status::Scheduled => Label::Scheduled,
        Status::Queued => Label::Queued,
        Status::Publishing => Label::Publishing,
        Status::Published => Label::Published,
        Status::Partial => Label::Partial,
        Status::Failed => Label::Failed,
        Status::Unconfirmed => Label::Unconfirmed,
        Status::NeedsConnection => Label::NeedsConnection,
        Status::Cancelled => Label::Cancelled,
        Status::CancelRequested => Label::CancelRequested,
        Status::Waiting => Label::Waiting,
        Status::NeedsDevice => Label::NeedsDevice,
        Status::Deleted => Label::Delete,
    }
}
fn issue_label(issue: Issue) -> Label {
    match issue {
        Issue::NoDestination => Label::NoDestination,
        Issue::EmptyBody => Label::EmptyBody,
        Issue::MissingSubreddit => Label::MissingSubreddit,
        Issue::MissingRedditTitle => Label::MissingRedditTitle,
        Issue::MissingFlair => Label::MissingFlair,
        Issue::TooLong => Label::TooLong,
        Issue::MissingTime => Label::MissingTime,
        Issue::PastTime => Label::PastTime,
        Issue::AlreadyPublishing => Label::ReadOnly,
    }
}

#[cfg(test)]
mod tests;
