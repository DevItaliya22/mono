// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_draw::scene;
use koru_ui::{ButtonRole, InputStyle};

struct Card {
    id: Uuid,
    title: String,
    at: Option<i64>,
    status: Status,
}
impl Card {
    fn from_post(post: &Post, now: i64) -> Self {
        let title = if post.title.is_empty() {
            post.body.lines().next().unwrap_or(label(Label::Draft))
        } else {
            &post.title
        };
        Self {
            id: post.id,
            title: title.chars().take(200).collect(),
            at: post.scheduled_at_ms,
            status: post.status(now),
        }
    }
}

impl Surface {
    fn text(&mut self, s: &mut Scene, r: Affine, text: &str, x: f32, y: f32, dim: bool) {
        let layout = self.engine.simple_layout(text, theme::role::row());
        scene::draw_layout(
            s,
            r,
            &layout,
            x,
            y,
            Some(if dim {
                theme::p().text_dim
            } else {
                theme::p().text
            }),
        );
    }
    fn button(
        &mut self,
        s: &mut Scene,
        r: Affine,
        rect: Rect,
        text: &str,
        action: Action,
        primary: bool,
    ) {
        let text = self.engine.elided(text, theme::role::row(), rect.w - 20.);
        koru_ui::button::draw_button_enabled(
            s,
            r,
            &mut self.engine,
            rect,
            &text,
            if primary {
                ButtonRole::Primary
            } else {
                ButtonRole::Secondary
            },
            rect.contains(self.pointer.0, self.pointer.1),
            self.keyboard_target.as_ref() == Some(&action),
            self.pending == 0 || action == Action::CancelConnection,
        );
        self.hits.push((rect, action));
    }
    fn input(&mut self, s: &mut Scene, r: Affine, rect: Rect, field: Field, placeholder: Label) {
        let focused = self.focus == Some(field);
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
        input.draw(
            s,
            r,
            &mut self.engine,
            rect,
            InputStyle::search(),
            Some(label(placeholder)),
            focused,
        );
        self.hits.push((rect, Action::Field(field)));
    }
    pub(super) fn paint(&mut self, s: &mut Scene, r: Affine) {
        self.hits.clear();
        self.preview_rect = None;
        self.form_bar = None;
        self.variant_bar = None;
        let v = self.viewport;
        scene::fill(s, r, theme::p().bg, &scene::rect(v.x, v.y, v.w, v.h));
        if self.page == Page::Accounts {
            self.accounts(s, r, 32.);
        } else if self.working.is_some() && self.composer_visible {
            self.composer(s, r, 32.);
        } else if let Some(channel) = self
            .channels
            .iter()
            .find(|c| Some(c.id) == self.channel_page)
            .cloned()
        {
            self.channel(s, r, &channel);
        } else {
            self.list(s, r, 28.);
        }
        if self.working.is_some() && !self.composer_visible {
            self.button(
                s,
                r,
                Rect::new(v.w - 152., v.h - 42., 128., 30.),
                label(Label::Edited),
                Action::Resume,
                false,
            );
        }
        if let Some(message) = self.message {
            let layout = self.engine.simple_layout_wrapped(
                label(message),
                theme::role::meta(),
                (v.w - 64.).max(100.),
            );
            scene::draw_layout(s, r, &layout, 32., v.h - 30., Some(theme::p().text_dim));
        } else if self.pending > 0 {
            self.text(s, r, label(Label::Loading), 32., v.h - 25., true);
        }
    }
    fn heading(&mut self, s: &mut Scene, r: Affine, text: &str, x: f32, y: f32, width: f32) {
        let style = theme::TextStyle::new(theme::text_metrics::_3XL, theme::FontWeight::SemiBold);
        let text = self.engine.elided(text, style, width);
        let layout = self.engine.simple_layout(&text, style);
        scene::draw_layout(s, r, &layout, x, y, Some(theme::p().text));
    }
    fn accounts(&mut self, s: &mut Scene, r: Affine, x: f32) {
        let width = (self.viewport.w - x - 32.).max(100.);
        let hint_label = match self.account_network {
            Network::X => Label::OAuthHint,
            Network::Reddit => Label::RedditOAuthHint,
            _ => Label::LinkedInOAuthHint,
        };
        let hint_height = self
            .engine
            .simple_layout_wrapped(label(hint_label), theme::role::row(), width.min(620.))
            .height();
        let tail_height = self
            .engine
            .simple_layout_wrapped(
                label(Label::PublishingThisDevice),
                theme::role::row(),
                width.min(620.),
            )
            .height();
        let page_height = if self.account_network == Network::LinkedInPage {
            110. + self
                .engine
                .simple_layout_wrapped(
                    label(Label::LinkedInPageHint),
                    theme::role::row(),
                    width.min(620.),
                )
                .height()
        } else {
            0.
        };
        let content_height = 498. + hint_height + tail_height + page_height;
        let band = Rect::new(0., 0., self.viewport.w, (self.viewport.h - 48.).max(0.));
        self.scroll = self.scroll.min((content_height - band.h).max(0.));
        let hit_start = self.hits.len();
        s.push_clip_layer(
            vello::peniko::Fill::NonZero,
            r,
            &scene::rect(band.x, band.y, band.w, band.h),
        );
        let original_root = r;
        let r = r * Affine::translate((0., -(self.scroll as f64)));
        let original_pointer = self.pointer;
        self.pointer.1 += self.scroll;
        self.heading(
            s,
            r,
            label(Label::ConnectAccount),
            x,
            32.,
            self.viewport.w - x - 32.,
        );
        let width = (self.viewport.w - x - 32.).max(100.);
        let button_width = ((width.min(620.) - 8.) / 2.).max(46.);
        for (index, network) in [
            Network::X,
            Network::Reddit,
            Network::LinkedInPerson,
            Network::LinkedInPage,
        ]
        .into_iter()
        .enumerate()
        {
            let name = match network {
                Network::LinkedInPerson => format!("LinkedIn · {}", label(Label::Person)),
                Network::LinkedInPage => format!("LinkedIn · {}", label(Label::Page)),
                _ => network.brand().to_owned(),
            };
            self.button(
                s,
                r,
                Rect::new(
                    x + (index % 2) as f32 * (button_width + 8.),
                    100. + (index / 2) as f32 * 42.,
                    button_width,
                    34.,
                ),
                &name,
                Action::Provider(network),
                self.account_network == network,
            );
        }
        let hint = self.engine.simple_layout_wrapped(
            label(hint_label),
            theme::role::row(),
            width.min(620.),
        );
        scene::draw_layout(s, r, &hint, x, 214., Some(theme::p().text_dim));
        let mut y = 214. + hint.height() + 24.;
        self.text(s, r, label(Label::ClientId), x, y, false);
        y += 26.;
        self.input(
            s,
            r,
            Rect::new(x, y, width.min(500.), 36.),
            Field::ClientId,
            Label::ClientId,
        );
        y += 60.;
        if self.account_network == Network::LinkedInPage {
            self.text(s, r, label(Label::OrganizationId), x, y, false);
            y += 26.;
            self.input(
                s,
                r,
                Rect::new(x, y, width.min(500.), 36.),
                Field::OrganizationId,
                Label::OrganizationId,
            );
            y += 60.;
            let hint = self.engine.simple_layout_wrapped(
                label(Label::LinkedInPageHint),
                theme::role::row(),
                width.min(620.),
            );
            scene::draw_layout(s, r, &hint, x, y, Some(theme::p().text_dim));
            y += hint.height() + 24.;
        }
        self.text(s, r, label(Label::RedirectUri), x, y, false);
        y += 26.;
        let callback = match self.account_network {
            Network::X => koru_socials_x::CALLBACK_URL,
            Network::Reddit => koru_socials_reddit::CALLBACK_URL,
            _ => koru_socials_linkedin::CALLBACK_URL,
        };
        self.text(s, r, callback, x, y, true);
        y += 48.;
        self.button(
            s,
            r,
            Rect::new(x, y, 200., 34.),
            label(if self.connecting.is_some() {
                Label::Cancel
            } else {
                Label::ConnectAccount
            }),
            if self.connecting.is_some() {
                Action::CancelConnection
            } else {
                Action::ConnectProvider
            },
            true,
        );
        y += 60.;
        let hint = self.engine.simple_layout_wrapped(
            label(Label::PublishingThisDevice),
            theme::role::row(),
            width.min(620.),
        );
        scene::draw_layout(s, r, &hint, x, y, Some(theme::p().text_dim));
        self.pointer = original_pointer;
        s.pop_layer();
        for (rect, _) in &mut self.hits[hit_start..] {
            rect.y -= self.scroll;
        }
        self.clip_form_hits(hit_start, band);
        if let Some(bar) =
            koru_ui::scrollbar::ScrollbarGeom::for_content(band, content_height, self.scroll)
        {
            koru_ui::scrollbar::draw(s, original_root, bar, false);
        }
    }
    fn channel(&mut self, s: &mut Scene, r: Affine, channel: &Channel) {
        let x = 48.;
        let width = (self.viewport.w - 96.).max(180.);
        self.text(
            s,
            r,
            &format!("{} / {}", label(Label::Title), label(Label::Channels)),
            x,
            28.,
            true,
        );
        navigation::network_badge(
            s,
            r,
            &mut self.engine,
            channel.network,
            Rect::new(x, 66., 38., 38.),
        );
        self.heading(s, r, &channel.name, x + 52., 62., (width - 210.).max(100.));
        self.button(
            s,
            r,
            Rect::new(self.viewport.w - 194., 70., 146., 32.),
            label(Label::NewPost),
            Action::New,
            false,
        );
        self.text(s, r, &network_name(channel.network), x, 120., true);
        let status = if self.connected.contains(&channel.id) {
            Label::Connected
        } else if channel.remote_actor.is_some() {
            Label::LocalCredentialsMissing
        } else {
            Label::NotConnected
        };
        let status_text = self
            .engine
            .elided(label(status), theme::role::meta(), width);
        self.text(s, r, &status_text, x, 146., true);
        let tab_y = 188.;
        for (index, (key, action, active)) in [
            (Label::Feed, None, false),
            (Label::Inbox, None, false),
            (
                Label::Queue,
                Some(Action::ChannelQueue),
                self.page == Page::Scheduled,
            ),
            (
                Label::Posts,
                Some(Action::ChannelPosts),
                self.page == Page::History,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let rect = Rect::new(x + index as f32 * 94., tab_y, 86., 30.);
            self.text(s, r, label(key), rect.x + 4., rect.y + 4., action.is_none());
            if active {
                scene::fill(
                    s,
                    r,
                    theme::p().text,
                    &scene::rect(rect.x, rect.y + 31., rect.w, 2.),
                );
            }
            if let Some(action) = action {
                self.hits.push((rect, action));
            }
        }
        scene::fill(
            s,
            r,
            theme::p().panel_border,
            &scene::rect(x, 222., width, 0.5),
        );
        let hint = if self.page == Page::Scheduled {
            Label::ChannelQueueHint
        } else {
            Label::Posts
        };
        self.text(s, r, label(hint), x, 246., true);
        self.button(
            s,
            r,
            Rect::new(self.viewport.w - 222., 238., 174., 32.),
            label(Label::OpenSchedule),
            Action::OpenSchedule,
            false,
        );
        let now = Local::now().timestamp_millis();
        let mut posts: Vec<_> = self
            .posts
            .iter()
            .filter(|p| {
                !p.deleted
                    && p.destinations.iter().any(|d| {
                        d.channel.id == channel.id
                            && if self.page == Page::History {
                                !d.delivery.published.is_empty()
                            } else {
                                p.scheduled
                                    && !matches!(
                                        d.delivery.state,
                                        DeliveryState::Published | DeliveryState::Cancelled
                                    )
                            }
                    })
            })
            .map(|p| Card::from_post(p, now))
            .collect();
        posts.sort_by_key(|p| p.at);
        self.scroll = self
            .scroll
            .min((posts.len() as f32 * 70. + 62. - (self.viewport.h - 334.)).max(0.));
        let band = Rect::new(x, 282., width, (self.viewport.h - 334.).max(0.));
        s.push_clip_layer(
            vello::peniko::Fill::NonZero,
            r,
            &scene::rect(band.x, band.y, band.w, band.h),
        );
        let start = self.hits.len();
        for (i, post) in posts.iter().enumerate() {
            let rect = Rect::new(x, 282. + i as f32 * 70. - self.scroll, width, 58.);
            if !rect.intersects(band) {
                continue;
            }
            scene::stroke(
                s,
                r,
                theme::p().panel_border,
                1.,
                &scene::rrect(rect.x, rect.y, rect.w, rect.h, 7.),
            );
            let time = post
                .at
                .and_then(|at| Local.timestamp_millis_opt(at).single())
                .map(|dt| LocaleFormat::for_system(None, None).date_time(parts(dt)))
                .unwrap_or_default();
            let time = self.engine.elided(&time, theme::role::meta(), 180.);
            self.text(s, r, &time, x + 14., rect.y + 9., true);
            self.text(
                s,
                r,
                label(status_label(post.status)),
                x + 14.,
                rect.y + 31.,
                true,
            );
            let title = self.engine.elided(
                &post.title,
                theme::role::section_heading(),
                (width - 220.).max(60.),
            );
            self.text(s, r, &title, x + 210., rect.y + 18., false);
            self.hits.push((rect, Action::Open(post.id)));
        }
        if posts.is_empty() && self.page == Page::History {
            self.text(s, r, label(Label::NoChannelPosts), x, 294., true);
        }
        if self.page == Page::Scheduled {
            self.button(
                s,
                r,
                Rect::new(x, 282. + posts.len() as f32 * 70. - self.scroll, width, 40.),
                label(Label::AddToQueue),
                Action::New,
                false,
            );
        }
        s.pop_layer();
        self.clip_form_hits(start, band);
    }
    fn list(&mut self, s: &mut Scene, r: Affine, x: f32) {
        if self.page == Page::Scheduled && !self.agenda {
            self.calendar(s, r, x);
            return;
        }
        let key = if self.page == Page::Scheduled {
            Label::Scheduled
        } else if self.page == Page::Drafts {
            Label::Drafts
        } else {
            Label::History
        };
        self.text(s, r, label(key), x, 24., false);
        self.button(
            s,
            r,
            Rect::new(self.viewport.w - 156., 16., 132., 32.),
            label(Label::NewPost),
            Action::New,
            true,
        );
        self.button(
            s,
            r,
            Rect::new(x, self.viewport.h - 42., 96., 30.),
            label(Label::Refresh),
            Action::Refresh,
            false,
        );
        if self.page == Page::Scheduled {
            self.button(
                s,
                r,
                Rect::new(self.viewport.w - 292., 16., 126., 32.),
                label(Label::Calendar),
                Action::Agenda,
                false,
            );
        }
        let now = Local::now().timestamp_millis();
        let posts: Vec<_> = self
            .posts
            .iter()
            .filter(|p| {
                !p.deleted
                    && self.matches_filter(p)
                    && if self.page == Page::Scheduled {
                        p.scheduled
                    } else if self.page == Page::Drafts {
                        !p.scheduled
                    } else {
                        matches!(
                            p.status(now),
                            Status::Published
                                | Status::Cancelled
                                | Status::Partial
                                | Status::Failed
                                | Status::Unconfirmed
                        )
                    }
            })
            .map(|post| Card::from_post(post, now))
            .collect();
        self.scroll = self
            .scroll
            .min((posts.len() as f32 * 82. - (self.viewport.h - 130.)).max(0.));
        if posts.is_empty() {
            self.text(
                s,
                r,
                label(if self.page == Page::Scheduled {
                    Label::EmptySchedule
                } else if self.page == Page::Drafts {
                    Label::EmptyDrafts
                } else {
                    Label::EmptyHistory
                }),
                x,
                90.,
                true,
            );
        }
        for (i, p) in posts.iter().enumerate() {
            let y = 64. + i as f32 * 82. - self.scroll;
            if y < 60. || y + 72. > self.viewport.h - 48. {
                continue;
            }
            self.card(
                s,
                r,
                Rect::new(x, y, (self.viewport.w - x - 24.).max(90.), 72.),
                p,
                now,
            );
        }
    }
    fn matches_filter(&self, p: &Post) -> bool {
        self.filter
            .is_none_or(|id| p.destinations.iter().any(|d| d.channel.id == id))
    }
    fn calendar(&mut self, s: &mut Scene, r: Affine, x: f32) {
        self.text(s, r, label(Label::Scheduled), x, 24., false);
        self.button(
            s,
            r,
            Rect::new(self.viewport.w - 156., 16., 132., 32.),
            label(Label::NewPost),
            Action::New,
            true,
        );
        self.button(
            s,
            r,
            Rect::new(self.viewport.w - 120., 60., 96., 32.),
            label(Label::Refresh),
            Action::Refresh,
            false,
        );
        self.button(
            s,
            r,
            Rect::new(self.viewport.w - 260., 16., 96., 32.),
            label(Label::List),
            Action::Agenda,
            false,
        );
        self.button(
            s,
            r,
            Rect::new(x, 60., 95., 32.),
            label(Label::Previous),
            Action::Week(-7),
            false,
        );
        self.button(
            s,
            r,
            Rect::new(x + 104., 60., 95., 32.),
            label(Label::Today),
            Action::Today,
            false,
        );
        self.button(
            s,
            r,
            Rect::new(x + 208., 60., 95., 32.),
            label(Label::Next),
            Action::Week(7),
            false,
        );
        let start = Local::now().date_naive() + chrono::Duration::days(self.week);
        let width = ((self.viewport.w - x - 84.) / 7.).max(36.);
        let top = 144.;
        let row = 54.;
        self.scroll = self
            .scroll
            .min((row * 24. + 46. - (self.viewport.h - top - 48.)).max(0.));
        let format = LocaleFormat::for_system(None, None);
        for day in 0..7 {
            let date = start + chrono::Duration::days(day);
            let dx = x + 72. + day as f32 * width;
            if let Some(dt) = date
                .and_hms_opt(12, 0, 0)
                .and_then(|d| Local.from_local_datetime(&d).single())
            {
                let date = koru_i18n::time::absolute_date(koru_i18n::current(), parts(dt), false);
                let date = self.engine.elided(&date, theme::role::row(), width - 6.);
                self.text(s, r, &date, dx + 3., 112., false);
            }
            for half in 0..48 {
                let y = top + half as f32 * row / 2. - self.scroll;
                if y < top || y + row / 2. > self.viewport.h - 48. {
                    continue;
                }
                let hour = half / 2;
                let minute = (half % 2) * 30;
                let rect = Rect::new(dx, y, width, row / 2.);
                scene::fill(
                    s,
                    r,
                    theme::p().panel_border,
                    &scene::rect(dx, y, width, 0.5),
                );
                if let Some(at) = date
                    .and_hms_opt(hour, minute, 0)
                    .and_then(|d| Local.from_local_datetime(&d).single())
                {
                    self.hits.push((rect, Action::Slot(at.timestamp_millis())));
                    if day == 0 && half % 2 == 0 {
                        self.text(s, r, &format.time_input(parts(at)), x, y, true);
                    }
                }
            }
            scene::fill(
                s,
                r,
                theme::p().panel_border,
                &scene::rect(dx, top, 0.5, (self.viewport.h - top - 48.).max(0.)),
            );
        }
        let now = Local::now().timestamp_millis();
        let cards: Vec<_> = self
            .posts
            .iter()
            .filter(|p| p.scheduled && !p.deleted && self.matches_filter(p))
            .map(|p| Card::from_post(p, now))
            .collect();
        let mut positions = Vec::new();
        for post in cards {
            let Some(dt) = post
                .at
                .and_then(|at| Local.timestamp_millis_opt(at).single())
            else {
                continue;
            };
            let day = (dt.date_naive() - start).num_days();
            if !(0..7).contains(&day) {
                continue;
            }
            let y = top + (dt.hour() as f32 + dt.minute() as f32 / 60.) * row - self.scroll;
            if y < top || y + 46. > self.viewport.h - 48. {
                continue;
            }
            positions.push((day, y, post));
        }
        positions.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        for day in 0..7 {
            let mut ends = Vec::<f32>::new();
            let mut placed = Vec::new();
            for (_, y, card) in positions.iter().filter(|(d, _, _)| *d == day) {
                let lane = ends.iter().position(|end| *end <= *y).unwrap_or(ends.len());
                if lane == ends.len() {
                    ends.push(*y + 48.);
                } else {
                    ends[lane] = *y + 48.;
                }
                placed.push((*y, card, lane));
            }
            let lane_width = (width - 4.) / ends.len().max(1) as f32;
            for (y, card, lane) in placed {
                self.card(
                    s,
                    r,
                    Rect::new(
                        x + 74. + day as f32 * width + lane as f32 * lane_width,
                        y,
                        (lane_width - 2.).max(1.),
                        46.,
                    ),
                    card,
                    now,
                );
            }
        }
        if self.drag_post.as_ref().is_some_and(|(_, _, moved)| *moved) {
            if let Some(at) = self.drop_time() {
                if let Some((rect, _)) = self
                    .hits
                    .iter()
                    .find(|(_, a)| matches!(a,Action::Slot(value) if *value==at))
                {
                    scene::fill(
                        s,
                        r,
                        theme::p().accent,
                        &scene::rect(rect.x, rect.y, rect.w, 3.),
                    );
                }
            }
        }
    }
    fn card(&mut self, s: &mut Scene, r: Affine, rect: Rect, p: &Card, _now: i64) {
        s.push_clip_layer(
            vello::peniko::Fill::NonZero,
            r,
            &scene::rect(rect.x, rect.y, rect.w, rect.h),
        );
        scene::fill(
            s,
            r,
            theme::p().chip_bg,
            &scene::rrect(rect.x, rect.y, rect.w, rect.h, 5.),
        );
        let title = &p.title;
        let limit = (rect.w / 8.).max(3.) as usize;
        let title: String = title.chars().take(limit).collect();
        self.text(s, r, &title, rect.x + 8., rect.y + 7., false);
        self.text(
            s,
            r,
            label(status_label(p.status)),
            rect.x + 8.,
            rect.y + 26.,
            true,
        );
        if rect.h >= 70. {
            if let Some(dt) = p.at.and_then(|at| Local.timestamp_millis_opt(at).single()) {
                self.text(
                    s,
                    r,
                    &LocaleFormat::for_system(None, None).date_time(parts(dt)),
                    rect.x + 8.,
                    rect.y + 47.,
                    true,
                );
            }
        }
        s.pop_layer();
        self.hits.push((rect, Action::Open(p.id)));
    }
    fn composer(&mut self, s: &mut Scene, r: Affine, x: f32) {
        let post = self.working.clone().unwrap();
        if !post.editable() {
            self.delivery_details(s, r, x, &post);
            return;
        }
        let width = (self.viewport.w - x - 24.).max(160.);
        let split = width >= 640.;
        let main = if split { width * 0.55 - 16. } else { width };
        if !split && self.show_variants {
            self.button(
                s,
                r,
                Rect::new(x, 20., 120., 32.),
                label(Label::Back),
                Action::Original,
                false,
            );
            self.variants(s, r, x, width);
            return;
        }
        self.input(
            s,
            r,
            Rect::new(x, 20., main - 100., 36.),
            Field::Title,
            Label::PostTitle,
        );
        self.button(
            s,
            r,
            Rect::new(self.viewport.w - 124., 20., 100., 32.),
            label(Label::Close),
            Action::Close,
            false,
        );
        let body_y = 76.
            + if post.source.is_some() { 30. } else { 0. }
            + if self.channels.is_empty() {
                44.
            } else {
                self.channels.len() as f32 * 38.
            };
        let body_h = (self.viewport.h - body_y - 188. - if split { 0. } else { 44. }).max(160.);
        let form_bottom = body_y + body_h + 12. + if split { 34. } else { 74. };
        let clip = Rect::new(x, 70., main, (self.viewport.h - 176.).max(80.));
        self.form_max = (form_bottom - (clip.y + clip.h)).max(0.);
        self.form_scroll = self.form_scroll.min(self.form_max);
        let hit_start = self.hits.len();
        s.push_clip_layer(
            vello::peniko::Fill::NonZero,
            r,
            &scene::rect(clip.x, clip.y, clip.w, clip.h),
        );
        let mut y = 76. - self.form_scroll;
        if let Some(source) = &post.source {
            self.text(
                s,
                r,
                &format!(
                    "{}: {} · {}",
                    label(Label::Source),
                    source.title,
                    label(Label::SourceDetached)
                ),
                x,
                y,
                true,
            );
            y += 30.;
        }
        for channel in self.channels.clone() {
            self.button(
                s,
                r,
                Rect::new(x, y, main, 30.),
                &format!("{} · {}", channel.name, network_name(channel.network)),
                Action::Toggle(channel.id),
                post.destinations.iter().any(|d| d.channel.id == channel.id),
            );
            y += 38.;
        }
        if self.channels.is_empty() {
            self.button(
                s,
                r,
                Rect::new(x, y, main, 32.),
                label(Label::AddChannel),
                Action::Page(Page::Accounts),
                true,
            );
            y += 44.;
        }
        let body_rect = Rect::new(x, y, main, body_h);
        self.body.draw(
            s,
            r,
            &mut self.engine,
            body_rect,
            Some(label(Label::BodyPlaceholder)),
            self.focus == Some(Field::Body),
        );
        self.hits.push((body_rect, Action::Field(Field::Body)));
        y += body_h + 12.;
        self.input(
            s,
            r,
            Rect::new(x, y, main * 0.57, 34.),
            Field::Date,
            Label::Date,
        );
        self.input(
            s,
            r,
            Rect::new(x + main * 0.6, y, main * 0.4, 34.),
            Field::Time,
            Label::Time,
        );
        if !split {
            self.button(
                s,
                r,
                Rect::new(x, y + 44., main, 30.),
                label(Label::PerNetwork),
                Action::Variants,
                false,
            );
        }
        s.pop_layer();
        self.clip_form_hits(hit_start, clip);
        self.form_bar = self.form_scrollbar(s, r, clip, self.form_max, self.form_scroll);
        self.variant_x = if split { x + main + 24. } else { f32::MAX };
        if split {
            self.variants(s, r, self.variant_x, width - main - 24.);
        }
        let footer = self.viewport.h - 94.;
        if post.editable() {
            let bw = ((width - 20.) / 3.).min(140.);
            self.button(
                s,
                r,
                Rect::new(x, footer, bw, 34.),
                label(Label::SaveDraft),
                Action::Save(false),
                false,
            );
            self.button(
                s,
                r,
                Rect::new(x + bw + 10., footer, bw, 34.),
                label(if self.base.is_some() && post.execution_device.is_none() {
                    Label::Duplicate
                } else if post.scheduled {
                    Label::Update
                } else {
                    Label::Schedule
                }),
                if self.base.is_some() && post.execution_device.is_none() {
                    Action::Duplicate
                } else {
                    Action::Save(true)
                },
                true,
            );
            if self.base.is_some() {
                self.button(
                    s,
                    r,
                    Rect::new(x + 2. * (bw + 10.), footer, bw, 34.),
                    label(if post.scheduled && post.execution_device.is_some() {
                        Label::CancelDelivery
                    } else if self.confirm_delete {
                        Label::ConfirmDelete
                    } else {
                        Label::Delete
                    }),
                    if post.scheduled && post.execution_device.is_some() {
                        Action::CancelDelivery
                    } else if self.confirm_delete {
                        Action::ConfirmDelete
                    } else {
                        Action::Delete
                    },
                    false,
                );
            }
        } else {
            self.text(s, r, label(Label::ReadOnly), x, footer, true);
        }
        let hint = self.owner_hint(&post);
        let hint = self.engine.elided(label(hint), theme::role::row(), width);
        self.text(s, r, &hint, x, footer + 42., true);
    }

    fn owner_hint(&self, post: &Post) -> Label {
        match post.execution_device {
            None if self.base.is_some() => Label::LegacySchedule,
            Some(id) if Some(id) != self.device => Label::PublishingOtherDevice,
            _ => Label::PublishingThisDevice,
        }
    }

    fn delivery_details(&mut self, s: &mut Scene, r: Affine, x: f32, post: &Post) {
        let width = (self.viewport.w - x - 24.).max(160.);
        let title = self
            .engine
            .elided(&post.title, theme::role::ui(), width - 120.);
        self.text(s, r, &title, x, 24., false);
        self.button(
            s,
            r,
            Rect::new(self.viewport.w - 124., 20., 100., 32.),
            label(Label::Close),
            Action::Close,
            false,
        );
        let clip = Rect::new(x, 70., width, (self.viewport.h - 170.).max(80.));
        let rows = post
            .destinations
            .iter()
            .map(|d| 38. + d.delivery.published.len() as f32 * 38.)
            .sum::<f32>();
        self.form_max = (328. + rows - clip.h).max(0.);
        self.form_scroll = self.form_scroll.min(self.form_max);
        let start = self.hits.len();
        s.push_clip_layer(
            vello::peniko::Fill::NonZero,
            r,
            &scene::rect(clip.x, clip.y, clip.w, clip.h),
        );
        let mut y = 76. - self.form_scroll;
        self.text(
            s,
            r,
            label(status_label(post.status(Local::now().timestamp_millis()))),
            x,
            y,
            false,
        );
        y += 30.;
        let hint = self
            .engine
            .elided(label(self.owner_hint(post)), theme::role::row(), width);
        self.text(s, r, &hint, x, y, true);
        y += 32.;
        if post
            .destinations
            .iter()
            .any(|d| d.delivery.state == DeliveryState::Unconfirmed)
        {
            let hint = self
                .engine
                .elided(label(Label::UnconfirmedHint), theme::role::row(), width);
            self.text(s, r, &hint, x, y, true);
            y += 34.;
        }
        for d in &post.destinations {
            let status = match d.delivery.state {
                DeliveryState::Pending => {
                    if d.delivery.retry_at_ms.is_some() {
                        Label::Waiting
                    } else {
                        Label::Queued
                    }
                }
                DeliveryState::Publishing => Label::Publishing,
                DeliveryState::Published => Label::Published,
                DeliveryState::Failed => Label::Failed,
                DeliveryState::NeedsConnection => Label::NeedsConnection,
                DeliveryState::Unconfirmed => Label::Unconfirmed,
                DeliveryState::Cancelled => Label::Cancelled,
            };
            self.text(
                s,
                r,
                &format!(
                    "{} · {} · {}/{}",
                    d.channel.name,
                    label(status),
                    d.delivery.published.len(),
                    d.delivery.content.as_ref().map_or(0, Vec::len)
                ),
                x,
                y,
                false,
            );
            y += 36.;
            for (i, p) in d.delivery.published.iter().enumerate() {
                // Construct a provider URL from a validated ID; never open an
                // arbitrary URL carried by a synced document.
                if let Some(url) = koru_socials::publication_url(d.channel.network, &p.id) {
                    self.button(
                        s,
                        r,
                        Rect::new(x, y, width.min(260.), 30.),
                        &format!("{} {}", label(Label::OpenPublished), i + 1),
                        Action::OpenPublished(url),
                        false,
                    );
                }
                y += 38.;
            }
        }
        if self.preview_dirty {
            let text = post
                .destinations
                .iter()
                .map(|d| {
                    let content = d
                        .delivery
                        .content
                        .clone()
                        .unwrap_or_else(|| d.parts(&post.body));
                    let heading = match d.delivery.target.as_ref() {
                        Some(koru_socials::PublicationTarget::Reddit {
                            subreddit,
                            title,
                            flair_id,
                        }) => format!(
                            "{} · r/{}\n{}\n{}",
                            d.channel.name,
                            subreddit,
                            title,
                            flair_id.as_deref().unwrap_or("")
                        ),
                        _ => d.channel.name.clone(),
                    };
                    format!("{}\n\n{}", heading, content.join("\n\n"))
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            self.preview.set_text(&mut self.engine, &text);
            self.preview_dirty = false;
        }
        let preview = Rect::new(x, y + 8., width - 16., 180.);
        self.preview
            .draw(s, r, &mut self.engine, preview, None, false);
        if preview.intersects(clip) {
            let top = preview.y.max(clip.y);
            self.preview_rect = Some(Rect::new(
                preview.x,
                top,
                preview.w,
                (preview.y + preview.h).min(clip.y + clip.h) - top,
            ));
        }
        s.pop_layer();
        self.clip_form_hits(start, clip);
        self.form_bar = self.form_scrollbar(s, r, clip, self.form_max, self.form_scroll);
        let footer = self.viewport.h - 86.;
        let bw = ((width - 20.) / 3.).min(220.);
        self.button(
            s,
            r,
            Rect::new(x, footer, bw, 34.),
            label(Label::Duplicate),
            Action::Duplicate,
            false,
        );
        if !post.cancel_requested
            && post.destinations.iter().any(|d| {
                matches!(
                    d.delivery.state,
                    DeliveryState::Failed | DeliveryState::NeedsConnection
                )
            })
        {
            self.button(
                s,
                r,
                Rect::new(x + bw + 10., footer, bw, 34.),
                label(Label::RetryDelivery),
                Action::Retry,
                true,
            );
        }
        if !post.cancel_requested
            && post.destinations.iter().any(|d| {
                !matches!(
                    d.delivery.state,
                    DeliveryState::Published | DeliveryState::Cancelled
                )
            })
        {
            self.button(
                s,
                r,
                Rect::new(x + 2. * (bw + 10.), footer, bw, 34.),
                label(Label::CancelDelivery),
                Action::CancelDelivery,
                false,
            );
        }
    }
    fn variants(&mut self, s: &mut Scene, r: Affine, x: f32, width: f32) {
        self.text(s, r, label(Label::PerNetwork), x, 76., false);
        let post = self.working.clone().unwrap();
        let selected = post
            .destinations
            .iter()
            .find(|d| Some(d.channel.id) == self.selected);
        let preview_y = 108.
            + post.destinations.len() as f32 * 38.
            + 70.
            + if selected.is_some_and(|d| d.channel.network == Network::Reddit) {
                120.
            } else {
                0.
            };
        let preview_h = (self.viewport.h - preview_y - 155.).max(120.);
        let clip = Rect::new(x, 104., width, (self.viewport.h - 210.).max(80.));
        self.variant_max = (preview_y + preview_h + 30. - clip.y - clip.h).max(0.);
        self.variant_scroll = self.variant_scroll.min(self.variant_max);
        let hit_start = self.hits.len();
        s.push_clip_layer(
            vello::peniko::Fill::NonZero,
            r,
            &scene::rect(clip.x, clip.y, clip.w, clip.h),
        );
        let mut y = 108. - self.variant_scroll;
        for d in &post.destinations {
            self.button(
                s,
                r,
                Rect::new(x, y, width, 30.),
                &d.channel.name,
                Action::Select(d.channel.id),
                self.selected == Some(d.channel.id),
            );
            y += 38.;
        }
        let Some(d) = post
            .destinations
            .iter()
            .find(|d| Some(d.channel.id) == self.selected)
        else {
            s.pop_layer();
            self.clip_form_hits(hit_start, clip);
            return;
        };
        self.text(
            s,
            r,
            label(if d.override_body.is_some() {
                Label::Edited
            } else {
                Label::FollowsOriginal
            }),
            x,
            y,
            true,
        );
        y += 28.;
        self.button(
            s,
            r,
            Rect::new(x, y, 100., 30.),
            label(if d.override_body.is_some() {
                Label::Reset
            } else {
                Label::Edit
            }),
            if d.override_body.is_some() {
                Action::Reset
            } else {
                Action::Edit
            },
            false,
        );
        y += 42.;
        if d.channel.network == Network::Reddit {
            for (field, key) in [
                (Field::Subreddit, Label::Subreddit),
                (Field::RedditTitle, Label::RedditTitle),
                (Field::Flair, Label::FlairOptional),
            ] {
                self.input(s, r, Rect::new(x, y, width, 32.), field, key);
                y += 40.;
            }
        }
        let rect = Rect::new(x, y, width, preview_h);
        if self.preview_dirty {
            let parts = d.parts(&post.body);
            let text = parts.join("\n\n────────\n\n");
            if self.preview.raw_text() != text {
                self.preview.set_text(&mut self.engine, &text);
            }
            self.preview_parts = parts.len();
            self.preview_count = if d.channel.network == Network::X {
                koru_socials::weighted_length(d.body(&post.body))
            } else {
                d.body(&post.body).chars().count()
            };
            self.preview_dirty = false;
        }
        if d.override_body.is_some() {
            self.variant.draw(
                s,
                r,
                &mut self.engine,
                rect,
                Some(label(Label::BodyPlaceholder)),
                self.focus == Some(Field::Variant),
            );
            self.hits.push((rect, Action::Field(Field::Variant)));
        } else {
            self.preview.draw(s, r, &mut self.engine, rect, None, false);
            self.preview_rect = Some(rect);
        }
        self.text(
            s,
            r,
            &format!(
                "{}: {} · {}: {}",
                label(Label::Characters),
                self.preview_count,
                label(Label::Thread),
                self.preview_parts
            ),
            x,
            rect.y + rect.h + 8.,
            true,
        );
        s.pop_layer();
        self.clip_form_hits(hit_start, clip);
        self.variant_bar = self.form_scrollbar(s, r, clip, self.variant_max, self.variant_scroll);
        if let Some(rect) = self.preview_rect {
            self.preview_rect = rect.intersects(clip).then(|| {
                Rect::new(
                    rect.x,
                    rect.y.max(clip.y),
                    rect.w,
                    (rect.y + rect.h).min(clip.y + clip.h) - rect.y.max(clip.y),
                )
            });
        }
    }
    fn clip_form_hits(&mut self, start: usize, clip: Rect) {
        let visible: Vec<_> = self
            .hits
            .drain(start..)
            .filter(|(rect, _)| rect.y >= clip.y && rect.y + rect.h <= clip.y + clip.h)
            .collect();
        self.hits.extend(visible);
    }
    fn form_scrollbar(
        &self,
        s: &mut Scene,
        r: Affine,
        clip: Rect,
        max: f32,
        scroll: f32,
    ) -> Option<koru_ui::scrollbar::ScrollbarGeom> {
        let area = Rect::new(clip.x, clip.y, clip.w + 14., clip.h);
        let bar = koru_ui::scrollbar::ScrollbarGeom::for_content(area, area.h + max, scroll);
        if let Some(bar) = bar {
            koru_ui::scrollbar::draw(s, r, bar, bar.in_gutter(self.pointer.0, self.pointer.1));
        }
        bar
    }
}
fn network_name(network: Network) -> String {
    match network {
        Network::LinkedInPerson => format!("LinkedIn · {}", label(Label::Person)),
        Network::LinkedInPage => format!("LinkedIn · {}", label(Label::Page)),
        _ => network.brand().into(),
    }
}
