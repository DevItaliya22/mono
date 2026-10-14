// SPDX-License-Identifier: MPL-2.0

//! The Socials projection inside the host's existing navigator column.
use super::*;
use koru_draw::{icons, scene};
use koru_ui::selectable_list::SelectableList;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationOutcome {
    Ignored,
    Changed,
    Open,
}

#[derive(Clone, Copy, PartialEq)]
enum Target {
    Scheduled,
    Drafts,
    Connect,
    Channel(Uuid),
    Expand(Uuid),
    Queue(Uuid),
    Posts(Uuid),
}

struct Row {
    title: String,
    icon: char,
    target: Option<Target>,
    count: Option<usize>,
    channel: Option<Channel>,
    indent: bool,
    active: bool,
}

pub(super) struct Navigation {
    list: SelectableList,
    expanded: HashSet<Uuid>,
    counts: HashMap<Uuid, (usize, usize)>,
    scheduled: usize,
    drafts: usize,
    rows: Vec<Row>,
    hits: Vec<(Rect, Target)>,
    area: Rect,
    pointer: (f32, f32),
    keyboard: bool,
}

impl Default for Navigation {
    fn default() -> Self {
        Self {
            list: SelectableList::new(32.),
            expanded: Default::default(),
            counts: Default::default(),
            scheduled: 0,
            drafts: 0,
            rows: vec![],
            hits: vec![],
            area: Rect::new(0., 0., 0., 0.),
            pointer: (-1., -1.),
            keyboard: false,
        }
    }
}
impl Navigation {
    pub(super) fn expand(&mut self, id: Uuid) {
        self.expanded.insert(id);
    }
    pub(super) fn refresh_counts(&mut self, posts: &[Post]) {
        self.counts.clear();
        self.scheduled = 0;
        self.drafts = 0;
        for post in posts.iter().filter(|p| !p.deleted) {
            let upcoming = post.scheduled
                && !matches!(
                    post.status(Local::now().timestamp_millis()),
                    Status::Published | Status::Cancelled
                );
            self.scheduled += usize::from(upcoming);
            self.drafts += usize::from(!post.scheduled);
            for destination in &post.destinations {
                let entry = self.counts.entry(destination.channel.id).or_default();
                entry.0 += usize::from(
                    upcoming
                        && !matches!(
                            destination.delivery.state,
                            DeliveryState::Published | DeliveryState::Cancelled
                        ),
                );
                entry.1 += usize::from(!destination.delivery.published.is_empty());
            }
        }
    }
}

impl Surface {
    pub fn navigation_draw(&mut self, s: &mut Scene, root: Affine, area: Rect) {
        let mut rows = Vec::new();
        for (title, icon, target, count, active) in [
            (
                Label::Scheduled,
                icons::DATE,
                Target::Scheduled,
                self.navigation.scheduled,
                self.page == Page::Scheduled && self.channel_page.is_none(),
            ),
            (
                Label::Drafts,
                icons::PENCIL,
                Target::Drafts,
                self.navigation.drafts,
                self.page == Page::Drafts,
            ),
        ] {
            rows.push(Row {
                title: label(title).into(),
                icon,
                target: Some(target),
                count: Some(count),
                channel: None,
                indent: false,
                active,
            });
        }
        rows.push(Row {
            title: label(Label::Channels).into(),
            icon: icons::PLUS,
            target: Some(Target::Connect),
            count: None,
            channel: None,
            indent: false,
            active: false,
        });
        for channel in &self.channels {
            let counts = self
                .navigation
                .counts
                .get(&channel.id)
                .copied()
                .unwrap_or_default();
            rows.push(Row {
                title: channel.name.clone(),
                icon: if self.navigation.expanded.contains(&channel.id) {
                    icons::CARET_DOWN
                } else {
                    icons::CARET_RIGHT
                },
                target: Some(Target::Channel(channel.id)),
                count: Some(counts.0),
                channel: Some(channel.clone()),
                indent: false,
                active: self.channel_page == Some(channel.id),
            });
            if self.navigation.expanded.contains(&channel.id) {
                for (title, icon, target, count, active) in [
                    (Label::Feed, icons::GLOBE, None, None, false),
                    (Label::Inbox, icons::TRAY, None, None, false),
                    (
                        Label::Queue,
                        icons::DATE,
                        Some(Target::Queue(channel.id)),
                        Some(counts.0),
                        self.channel_page == Some(channel.id) && self.page == Page::Scheduled,
                    ),
                    (
                        Label::Posts,
                        icons::ARROW_UP,
                        Some(Target::Posts(channel.id)),
                        Some(counts.1),
                        self.channel_page == Some(channel.id) && self.page == Page::History,
                    ),
                ] {
                    rows.push(Row {
                        title: label(title).into(),
                        icon,
                        target,
                        count,
                        channel: None,
                        indent: true,
                        active,
                    });
                }
            }
        }
        rows.push(Row {
            title: label(Label::ConnectAccount).into(),
            icon: icons::PLUS,
            target: Some(Target::Connect),
            count: None,
            channel: None,
            indent: false,
            active: self.page == Page::Accounts,
        });
        self.navigation.rows = rows;
        self.navigation.area = Rect::new(area.x, area.y + 4., area.w, (area.h - 38.).max(0.));
        let band = self.navigation.area;
        self.navigation
            .list
            .clamp_scroll(self.navigation.rows.len(), band.h);
        self.navigation.hits.clear();
        s.push_clip_layer(
            vello::peniko::Fill::NonZero,
            root,
            &scene::rect(band.x, band.y, band.w, band.h),
        );
        for index in 0..self.navigation.rows.len() {
            let row = &self.navigation.rows[index];
            let rect = Rect::new(
                band.x + 6.,
                self.navigation.list.row_y(band.y, index),
                (band.w - 12.).max(0.),
                30.,
            );
            if rect.y + rect.h <= band.y || rect.y >= band.y + band.h {
                continue;
            }
            let hovered = rect.contains(self.navigation.pointer.0, self.navigation.pointer.1);
            if row.active
                || (hovered && row.target.is_some())
                || (self.navigation.keyboard && self.navigation.list.selected == index)
            {
                scene::fill(
                    s,
                    root,
                    if row.active {
                        theme::p().chip_bg
                    } else {
                        theme::p().hover_tint
                    },
                    &scene::rrect(rect.x, rect.y, rect.w, rect.h, 5.),
                );
            }
            let dim = row.target.is_none() || index == 2;
            let color = if dim {
                theme::p().text_dim
            } else {
                theme::p().text
            };
            let mut x = rect.x + if row.indent { 34. } else { 12. };
            let icon_x = if index == 2 { rect.x + rect.w - 25. } else { x };
            let icon = self.engine.icon_layout(row.icon, 16.);
            scene::draw_layout(
                s,
                root,
                &icon,
                icon_x,
                rect.y + 7.,
                Some(theme::p().text_dim),
            );
            if index != 2 {
                x += 24.;
            }
            if let Some(channel) = &row.channel {
                network_badge(
                    s,
                    root,
                    &mut self.engine,
                    channel.network,
                    Rect::new(x, rect.y + 6., 18., 18.),
                );
                x += 26.;
                let top = rect.y.max(band.y);
                let bottom = (rect.y + rect.h).min(band.y + band.h);
                self.navigation.hits.push((
                    Rect::new(rect.x, top, 32., bottom - top),
                    Target::Expand(channel.id),
                ));
            }
            let title = self.engine.elided(
                &row.title,
                theme::role::row(),
                (rect.x + rect.w - x - 32.).max(0.),
            );
            let layout = self.engine.simple_layout(&title, theme::role::row());
            scene::draw_layout(s, root, &layout, x, rect.y + 6., Some(color));
            if let Some(count) = row.count.filter(|count| *count > 0) {
                let layout = self.engine.simple_layout(
                    &koru_i18n::number::decimal(koru_i18n::current(), count as u64),
                    theme::role::hint(),
                );
                scene::draw_layout(
                    s,
                    root,
                    &layout,
                    rect.x + rect.w - layout.width() - 10.,
                    rect.y + 8.,
                    Some(theme::p().text_dim),
                );
            }
            if let Some(target) = row.target {
                let inset = if row.channel.is_some() { 32. } else { 0. };
                let top = rect.y.max(band.y);
                let bottom = (rect.y + rect.h).min(band.y + band.h);
                self.navigation.hits.push((
                    Rect::new(rect.x + inset, top, rect.w - inset, bottom - top),
                    target,
                ));
            }
        }
        s.pop_layer();
        let footer = format!(
            "{} · {}",
            label(Label::Channels),
            koru_i18n::number::decimal(koru_i18n::current(), self.channels.len() as u64)
        );
        let footer = self
            .engine
            .elided(&footer, theme::role::meta(), area.w - 32.);
        let layout = self.engine.simple_layout(&footer, theme::role::meta());
        scene::draw_layout(
            s,
            root,
            &layout,
            area.x + 16.,
            area.y + area.h - 25.,
            Some(theme::p().text_dim),
        );
        if let Some(bar) = koru_ui::scrollbar::ScrollbarGeom::for_content(
            band,
            self.navigation.rows.len() as f32 * 32.,
            self.navigation.list.scroll,
        ) {
            koru_ui::scrollbar::draw(
                s,
                root,
                bar,
                bar.in_gutter(self.navigation.pointer.0, self.navigation.pointer.1),
            );
        }
    }
    pub fn navigation_mouse_down(&mut self, x: f32, y: f32) -> NavigationOutcome {
        self.navigation.pointer = (x, y);
        self.navigation.keyboard = false;
        let target = self
            .navigation
            .hits
            .iter()
            .rev()
            .find(|(r, _)| r.contains(x, y))
            .map(|(_, t)| *t);
        target.map_or(NavigationOutcome::Ignored, |t| self.navigate(t))
    }
    fn navigate(&mut self, target: Target) -> NavigationOutcome {
        if let Target::Expand(id) = target {
            if !self.navigation.expanded.insert(id) {
                self.navigation.expanded.remove(&id);
            }
            return NavigationOutcome::Changed;
        }
        self.sync_fields();
        self.composer_visible = false;
        self.focus = None;
        self.scroll = 0.;
        match target {
            Target::Scheduled => {
                self.channel_page = None;
                self.filter = None;
                self.page = Page::Scheduled;
                self.scroll = 432.;
            }
            Target::Drafts => {
                self.channel_page = None;
                self.filter = None;
                self.page = Page::Drafts;
            }
            Target::Connect => {
                self.page = Page::Accounts;
                self.channel_page = None;
            }
            Target::Channel(id) | Target::Queue(id) => {
                self.channel_page = Some(id);
                self.filter = Some(id);
                self.page = Page::Scheduled;
                self.navigation.expanded.insert(id);
            }
            Target::Posts(id) => {
                self.channel_page = Some(id);
                self.filter = Some(id);
                self.page = Page::History;
            }
            Target::Expand(_) => unreachable!(),
        }
        NavigationOutcome::Open
    }
    pub fn navigation_mouse_move(&mut self, x: f32, y: f32) -> bool {
        let changed = self.navigation.pointer != (x, y);
        self.navigation.pointer = (x, y);
        changed
    }
    pub fn navigation_scroll(&mut self, delta: &MouseScrollDelta) -> bool {
        let dy = match delta {
            MouseScrollDelta::LineDelta(_, y) => *y * 32.,
            MouseScrollDelta::PixelDelta(p) => p.y as f32,
        };
        let before = self.navigation.list.scroll;
        self.navigation
            .list
            .scroll_by(-dy, self.navigation.rows.len(), self.navigation.area.h);
        before != self.navigation.list.scroll
    }
    pub fn navigation_has_keyboard(&self) -> bool {
        self.navigation.keyboard
    }
    pub fn navigation_focus(&mut self) {
        self.navigation.keyboard = true;
    }
    pub fn navigation_blur(&mut self) {
        self.navigation.keyboard = false;
    }
    pub fn navigation_key(&mut self, key: &Key, _mods: ModifiersState) -> NavigationOutcome {
        if !self.navigation.keyboard {
            return NavigationOutcome::Ignored;
        }
        match key {
            Key::Named(
                NamedKey::ArrowDown | NamedKey::ArrowUp | NamedKey::Home | NamedKey::End,
            ) => {
                let len = self.navigation.rows.len();
                let delta = match key {
                    Key::Named(NamedKey::ArrowUp) => -1,
                    Key::Named(NamedKey::Home) => -(len as isize),
                    Key::Named(NamedKey::End) => len as isize,
                    _ => 1,
                };
                self.navigation
                    .list
                    .step(delta, len, self.navigation.area.h);
                NavigationOutcome::Changed
            }
            Key::Named(NamedKey::Enter | NamedKey::Space) => self
                .navigation
                .rows
                .get(self.navigation.list.selected)
                .and_then(|r| r.target)
                .map_or(NavigationOutcome::Changed, |t| self.navigate(t)),
            Key::Named(NamedKey::Escape) => {
                self.navigation.keyboard = false;
                NavigationOutcome::Changed
            }
            _ => NavigationOutcome::Ignored,
        }
    }
}

pub(super) fn network_badge(
    s: &mut Scene,
    r: Affine,
    engine: &mut TextEngine,
    network: Network,
    rect: Rect,
) {
    use vello::peniko::Color;
    let (text, color) = match network {
        Network::X => ("X", Color::from_rgb8(24, 24, 24)),
        Network::LinkedInPerson | Network::LinkedInPage => ("in", Color::from_rgb8(35, 103, 165)),
        Network::Reddit => ("r/", Color::from_rgb8(212, 83, 41)),
    };
    scene::fill(
        s,
        r,
        color,
        &scene::rrect(rect.x, rect.y, rect.w, rect.h, 4.),
    );
    let style = if rect.w > 30. {
        theme::role::subtitle()
    } else {
        theme::role::hint()
    };
    let layout = engine.simple_layout(text, style.with_weight(theme::FontWeight::SemiBold));
    scene::draw_layout(
        s,
        r,
        &layout,
        rect.x + (rect.w - layout.width()) / 2.,
        rect.y + (rect.h - style.metrics.line_height) / 2.,
        Some(Color::WHITE),
    );
}
