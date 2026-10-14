// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn new_post_reveals_the_hidden_unsaved_composer_without_losing_its_content() {
    let (mut s, requests, _) = fixture();
    s.act(Action::New);
    s.body.set_text(&mut s.engine, "Retain this unsaved post");
    let id = s.working.as_ref().unwrap().id;
    s.act(Action::Page(Page::Accounts));
    assert!(!s.composer_visible);
    s.act(Action::New);
    assert!(
        s.composer_visible,
        "the blocked action must reveal what needs saving"
    );
    assert!(s.page != Page::Accounts);
    assert_eq!(s.working.as_ref().unwrap().id, id);
    assert_eq!(s.working.as_ref().unwrap().body, "Retain this unsaved post");
    assert!(s.message == Some(Label::Unsaved));
    assert!(requests.try_recv().is_err());
}

#[test]
fn legacy_schedule_requires_a_distinct_copy_and_does_not_reassign_ownership() {
    let (mut s, requests, _) = fixture();
    let mut post = Post::draft(0);
    post.body = "older saved post".into();
    post.scheduled = true;
    let old_id = post.id;
    s.open(post.clone(), Some(post));
    s.save(true, false);
    assert!(requests.try_recv().is_err());
    assert!(s.message == Some(Label::LegacySchedule));
    s.act(Action::Duplicate);
    let copy = s.working.as_ref().unwrap();
    assert_ne!(copy.id, old_id);
    assert!(!copy.scheduled);
    assert_eq!(copy.execution_device, None);
    assert_eq!(copy.body, "older saved post");
    assert!(s.base.is_none());
}

#[test]
fn delivery_updates_do_not_consume_pending_save_or_replace_unsaved_edits() {
    let (mut s, _requests, replies) = fixture();
    let mut post = Post::draft(0);
    post.body = "saved".into();
    s.open(post.clone(), Some(post.clone()));
    s.body.set_text(&mut s.engine, "unsaved");
    s.sync_fields();
    s.pending = 1;
    replies
        .send(backend::Reply::PostChanged(s.workspace.unwrap(), post))
        .unwrap();
    s.maintain(Instant::now());
    assert_eq!(s.pending, 1);
    assert_eq!(s.working.as_ref().unwrap().body, "unsaved");
    assert_eq!(s.posts[0].body, "saved");
}

#[test]
fn unconfirmed_has_no_retry_and_cancellation_is_only_an_intent() {
    let (mut s, requests, _) = fixture();
    let mut post = Post::draft(0);
    post.execution_device = Some(Uuid::new_v4());
    post.scheduled = true;
    let mut d = Destination::new(Channel::new(Network::X, "X".into()));
    d.delivery.state = DeliveryState::Unconfirmed;
    d.delivery.content = Some(vec!["possibly sent".into()]);
    post.destinations.push(d);
    s.open(post.clone(), Some(post));
    s.draw(&mut Scene::new(), Affine::IDENTITY);
    assert!(!s.hits.iter().any(|(_, a)| *a == Action::Retry));
    s.act(Action::Retry);
    assert!(requests.try_recv().is_err());
    s.act(Action::CancelDelivery);
    let backend::Request::Save { post, .. } =
        requests.recv_timeout(Duration::from_secs(1)).unwrap()
    else {
        panic!()
    };
    assert!(post.cancel_requested);
    assert_eq!(
        post.destinations[0].delivery.state,
        DeliveryState::Unconfirmed
    );
}
use std::sync::mpsc;

fn fixture() -> (
    Surface,
    mpsc::Receiver<backend::Request>,
    mpsc::Sender<backend::Reply>,
) {
    let mut surface = Surface::new(None);
    let (send, requests) = mpsc::channel();
    let (replies, receive) = mpsc::channel();
    surface.backend = backend::Backend { send, receive };
    surface.workspace = Some(WorkspaceId::from_uuid(Uuid::new_v4()));
    (surface, requests, replies)
}

#[test]
fn working_copy_survives_account_view_and_closes_only_after_ack() {
    let (mut s, requests, replies) = fixture();
    s.act(Action::New);
    s.body.set_text(&mut s.engine, "An unsaved idea");
    s.act(Action::Page(Page::Accounts));
    assert_eq!(s.working.as_ref().unwrap().body, "An unsaved idea");
    s.act(Action::Page(Page::Drafts));
    s.save(false, false);
    assert!(s.working.is_some());
    let backend::Request::Save {
        workspace, post, ..
    } = requests.recv().unwrap()
    else {
        panic!()
    };
    assert_eq!(post.body, "An unsaved idea");
    replies
        .send(backend::Reply::Saved(workspace, post))
        .unwrap();
    assert!(s.maintain(Instant::now()));
    assert!(s.working.is_none());
    assert_eq!(s.posts.len(), 1);
}

#[test]
fn failed_save_preserves_the_working_copy_and_base() {
    let (mut s, requests, replies) = fixture();
    let mut p = Post::draft(0);
    p.body = "original".into();
    s.open(p.clone(), Some(p.clone()));
    s.body.set_text(&mut s.engine, "still here");
    s.save(false, false);
    let _request = requests.recv().unwrap();
    replies
        .send(backend::Reply::Failed(backend::Failure::Changed))
        .unwrap();
    s.maintain(Instant::now());
    assert_eq!(s.working.as_ref().unwrap().body, "still here");
    assert_eq!(s.base, Some(p));
    assert_eq!(s.pending, 0);
}

#[test]
fn navigating_variant_does_not_detach_but_typing_does_and_reset_follows() {
    let (mut s, _, _) = fixture();
    let c = Channel::new(Network::X, "My X".into());
    let mut p = Post::draft(0);
    p.body = "base".into();
    p.destinations.push(Destination::new(c.clone()));
    s.open(p, None);
    s.select(c.id);
    s.focus = Some(Field::Variant);
    s.key(&Key::Named(NamedKey::End), None, ModifiersState::empty());
    assert!(
        s.working.as_ref().unwrap().destinations[0]
            .override_body
            .is_none()
    );
    s.key(
        &Key::Character("!".into()),
        Some("!"),
        ModifiersState::empty(),
    );
    assert!(
        s.working.as_ref().unwrap().destinations[0]
            .override_body
            .is_some()
    );
    s.act(Action::Reset);
    assert!(
        s.working.as_ref().unwrap().destinations[0]
            .override_body
            .is_none()
    );
    assert_eq!(s.variant.raw_text(), "base");
}

#[test]
fn unconnected_schedule_is_saved_as_needing_connection_without_publishing() {
    let (mut s, requests, _) = fixture();
    let mut p = Post::draft(Local::now().timestamp_millis());
    p.body = "hello".into();
    p.destinations
        .push(Destination::new(Channel::new(Network::X, "X".into())));
    p.scheduled_at_ms = Some(Local::now().timestamp_millis() + 86_400_000);
    s.open(p, None);
    s.save(true, false);
    let backend::Request::Save { post, .. } = requests.recv().unwrap() else {
        panic!()
    };
    assert!(post.scheduled);
    assert_eq!(
        post.status(Local::now().timestamp_millis()),
        Status::NeedsConnection
    );
    assert!(post.destinations[0].delivery.published.is_empty());
}

#[test]
fn narrow_composer_has_a_route_to_variant_fields() {
    let (mut s, _, _) = fixture();
    let mut p = Post::draft(0);
    let c = Channel::new(Network::Reddit, "Reddit".into());
    p.destinations.push(Destination::new(c.clone()));
    s.open(p, None);
    s.set_viewport(Rect::new(0., 0., 600., 700.));
    s.draw(&mut Scene::new(), Affine::IDENTITY);
    let rect = s
        .hits
        .iter()
        .find(|(_, a)| matches!(a, Action::Variants))
        .unwrap()
        .0;
    let (x, y) = rect.centre();
    assert!(s.mouse_down(x, y));
    s.select(c.id);
    s.draw(&mut Scene::new(), Affine::IDENTITY);
    assert!(
        s.hits
            .iter()
            .any(|(_, a)| matches!(a, Action::Field(Field::Subreddit)))
    );
    assert!(s.hits.iter().any(|(_, a)| matches!(a, Action::Original)));
}

#[test]
fn read_only_publication_refuses_text_changes() {
    let (mut s, _, _) = fixture();
    let mut p = Post::draft(0);
    p.body = "published".into();
    let mut d = Destination::new(Channel::new(Network::X, "X".into()));
    d.delivery.content = Some(vec!["published".into()]);
    d.delivery.state = DeliveryState::Published;
    p.destinations.push(d);
    s.open(p, None);
    s.focus = Some(Field::Body);
    assert!(!s.key(
        &Key::Character("x".into()),
        Some("x"),
        ModifiersState::empty()
    ));
    assert_eq!(s.body.raw_text(), "published");
}

#[test]
fn date_edit_vetoes_close_and_external_workspace_switch() {
    let (mut s, _, _) = fixture();
    let original = s.workspace;
    let p = Post::draft(0);
    s.open(p.clone(), Some(p));
    assert!(!s.request_close());
    s.date.set_text(&mut s.engine, "changed");
    assert!(s.request_close());
    s.set_context_workspace(Some(WorkspaceId::from_uuid(Uuid::new_v4())));
    assert_eq!(s.workspace, original);
    assert!(s.working.is_some());
}

#[test]
fn calendar_drag_uses_nearest_half_hour_and_does_not_publish() {
    let (mut s, requests, _) = fixture();
    let dt = Local::now() + chrono::Duration::days(1);
    let at = Local
        .with_ymd_and_hms(dt.year(), dt.month(), dt.day(), 10, 0, 0)
        .single()
        .unwrap();
    let mut post = Post::draft(0);
    post.body = "move this".into();
    post.execution_device = Some(Uuid::new_v4());
    post.scheduled = true;
    post.scheduled_at_ms = Some(at.timestamp_millis());
    post.destinations
        .push(Destination::new(Channel::new(Network::X, "X".into())));
    let id = post.id;
    s.posts.push(post);
    s.set_viewport(Rect::new(0., 0., 1200., 800.));
    s.draw(&mut Scene::new(), Affine::IDENTITY);
    let card = s
        .hits
        .iter()
        .find(|(_, a)| matches!(a,Action::Open(value) if *value==id))
        .unwrap()
        .0;
    let (x, y) = card.centre();
    s.mouse_down(x, y);
    let target = at.timestamp_millis() + 1_800_000;
    let rect = s
        .hits
        .iter()
        .find(|(_, a)| matches!(a,Action::Slot(value) if *value==target))
        .unwrap()
        .0;
    s.mouse_move(rect.x + 10., rect.y + 2.);
    s.mouse_up();
    let backend::Request::Save { post, .. } =
        requests.recv_timeout(Duration::from_secs(1)).unwrap()
    else {
        panic!()
    };
    assert_eq!(post.scheduled_at_ms, Some(target));
    assert_eq!(post.status(target), Status::NeedsConnection);
}

#[test]
fn save_retry_keeps_the_exact_submission_timestamp() {
    let (mut s, requests, replies) = fixture();
    s.act(Action::New);
    s.body.set_text(&mut s.engine, "retain me");
    s.save(false, false);
    let backend::Request::Save { mut post, .. } = requests.recv().unwrap() else {
        panic!()
    };
    post.updated_at_ms -= 10_000;
    s.submitted = Some(post.clone());
    replies
        .send(backend::Reply::Failed(backend::Failure::Unavailable))
        .unwrap();
    s.maintain(Instant::now());
    s.save(false, false);
    let backend::Request::Save { post: retry, .. } = requests.recv().unwrap() else {
        panic!()
    };
    assert_eq!(post, retry);
}

#[test]
fn late_night_and_overlapping_cards_remain_reachable() {
    let (mut s, _, _) = fixture();
    let date = Local::now().date_naive();
    for minute in [30, 30, 59] {
        let mut p = Post::draft(0);
        p.scheduled = true;
        p.scheduled_at_ms = Some(
            Local
                .from_local_datetime(&date.and_hms_opt(23, minute, 0).unwrap())
                .single()
                .unwrap()
                .timestamp_millis(),
        );
        s.posts.push(p);
    }
    s.scroll = 100_000.;
    s.set_viewport(Rect::new(0., 0., 1200., 700.));
    s.draw(&mut Scene::new(), Affine::IDENTITY);
    let cards: Vec<_> = s
        .hits
        .iter()
        .filter(|(_, a)| matches!(a, Action::Open(_)))
        .collect();
    assert_eq!(cards.len(), 3);
    let day_slot = s
        .hits
        .iter()
        .find_map(|(rect, action)| {
            let Action::Slot(at) = action else {
                return None;
            };
            Local
                .timestamp_millis_opt(*at)
                .single()
                .filter(|dt| dt.date_naive() == date)
                .map(|_| *rect)
        })
        .expect("the scheduled day's calendar column");
    for (rect, action) in &cards {
        assert!(rect.x >= day_slot.x);
        assert!(
            rect.x + rect.w <= day_slot.x + day_slot.w,
            "overlapping posts must stay inside their scheduled day"
        );
        let (x, y) = rect.centre();
        let hit = s.hits.iter().rev().find(|(r, _)| r.contains(x, y)).unwrap();
        let Action::Open(expected) = action else {
            panic!()
        };
        assert!(matches!(hit.1,Action::Open(id) if id==*expected));
    }
}

#[test]
fn short_composer_scrolls_required_reddit_fields_above_footer() {
    let (mut s, _, _) = fixture();
    let mut p = Post::draft(0);
    for index in 0..12 {
        let c = Channel::new(Network::Reddit, format!("channel {index}"));
        if index < 4 {
            p.destinations.push(Destination::new(c.clone()));
        }
        s.channels.push(c);
    }
    let id = p.destinations[0].channel.id;
    s.open(p, None);
    s.select(id);
    s.set_viewport(Rect::new(0., 0., 1200., 500.));
    s.draw(&mut Scene::new(), Affine::IDENTITY);
    s.form_scroll = s.form_max;
    s.variant_scroll = 160.;
    s.draw(&mut Scene::new(), Affine::IDENTITY);
    for field in [
        Field::Date,
        Field::Time,
        Field::Subreddit,
        Field::RedditTitle,
        Field::Flair,
    ] {
        assert!(
            s.hits.iter().any(
                |(rect, a)| matches!(a,Action::Field(f) if *f==field) && rect.y + rect.h < 406.
            )
        );
    }
}

#[test]
fn navigator_routes_existing_channel_and_keeps_unsaved_work() {
    let (mut s, requests, _) = fixture();
    let c = Channel::new(Network::X, "@example".into());
    s.channels.push(c.clone());
    s.act(Action::New);
    s.body.set_text(&mut s.engine, "still drafting");
    s.navigation_draw(
        &mut Scene::new(),
        Affine::IDENTITY,
        Rect::new(0., 0., 280., 500.),
    );
    assert_eq!(s.navigation_mouse_down(120., 112.), NavigationOutcome::Open);
    assert_eq!(s.channel_page, Some(c.id));
    assert_eq!(s.filter, Some(c.id));
    assert!(!s.composer_visible);
    assert_eq!(s.working.as_ref().unwrap().body, "still drafting");
    assert!(
        requests.try_recv().is_err(),
        "navigation itself opens no connection"
    );
    s.act(Action::Resume);
    assert!(s.composer_visible);
    assert!(s.request_close());
}

#[test]
fn connect_requires_client_id_and_cancel_remains_available_while_waiting() {
    let (mut s, requests, replies) = fixture();
    s.page = Page::Accounts;
    s.act(Action::ConnectProvider);
    assert_eq!(s.message, Some(Label::OAuthClientRequired));
    assert!(requests.try_recv().is_err());
    s.client_id.set_text(&mut s.engine, "public-test-client");
    s.act(Action::ConnectProvider);
    let backend::Request::ConnectProvider { cancelled, .. } = requests.recv().unwrap() else {
        panic!()
    };
    assert_eq!(s.pending, 1);
    assert!(!cancelled.load(std::sync::atomic::Ordering::Relaxed));
    s.key(&Key::Named(NamedKey::Escape), None, ModifiersState::empty());
    assert!(cancelled.load(std::sync::atomic::Ordering::Relaxed));
    replies
        .send(backend::Reply::Failed(backend::Failure::OAuth(
            koru_socials_x::Error::Cancelled,
        )))
        .unwrap();
    s.maintain(Instant::now());
    assert!(s.connecting.is_none());
    assert!(s.channels.is_empty());
    assert_eq!(s.message, Some(Label::OAuthCancelled));
}

#[test]
fn provider_choice_routes_connection_and_requires_page_identity() {
    for network in Network::ALL {
        let (mut s, requests, _) = fixture();
        s.page = Page::Accounts;
        s.act(Action::Provider(network));
        assert!(requests.try_recv().is_err());
        s.client_id.set_text(&mut s.engine, "public-client");
        if network == Network::LinkedInPage {
            s.act(Action::ConnectProvider);
            assert_eq!(s.message, Some(Label::OrganizationRequired));
            assert!(requests.try_recv().is_err());
            s.organization_id.set_text(&mut s.engine, "123456");
        }
        s.act(Action::ConnectProvider);
        let backend::Request::ConnectProvider {
            network: selected,
            organization,
            ..
        } = requests.recv().unwrap()
        else {
            panic!()
        };
        assert_eq!(selected, network);
        assert_eq!(
            organization,
            if network == Network::LinkedInPage {
                "123456"
            } else {
                ""
            }
        );
        // A pending browser flow cannot be silently retargeted to another app.
        s.act(Action::Provider(Network::X));
        assert_eq!(s.account_network, network);
        assert!(requests.try_recv().is_err());
    }
}

#[test]
fn every_provider_form_keeps_connect_reachable_in_a_short_window() {
    for network in Network::ALL {
        let (mut s, _, _) = fixture();
        s.page = Page::Accounts;
        s.account_network = network;
        s.set_viewport(Rect::new(0., 0., 620., 340.));
        s.scroll = 100_000.;
        s.draw(&mut Scene::new(), Affine::IDENTITY);
        let (rect, _) = s
            .hits
            .iter()
            .find(|(_, a)| *a == Action::ConnectProvider)
            .unwrap();
        assert!(rect.y >= 0. && rect.y + rect.h <= 292.);
    }
}

#[test]
fn short_account_form_keeps_connect_reachable_above_status_footer() {
    let (mut s, requests, _) = fixture();
    s.page = Page::Accounts;
    s.set_viewport(Rect::new(0., 0., 620., 340.));
    s.scroll = 100_000.;
    s.draw(&mut Scene::new(), Affine::IDENTITY);
    let (rect, _) = s
        .hits
        .iter()
        .find(|(_, action)| *action == Action::ConnectProvider)
        .unwrap();
    assert!(rect.y >= 0. && rect.y + rect.h <= 292.);
    let (x, y) = rect.centre();
    s.mouse_down(x, y);
    assert_eq!(s.message, Some(Label::OAuthClientRequired));
    assert!(requests.try_recv().is_err());
}

#[test]
fn verified_account_reply_updates_shared_navigation_without_inventing_posts() {
    let (mut s, _, replies) = fixture();
    let mut channel = Channel::new(Network::X, "@connected".into());
    channel.remote_actor = Some("12345".into());
    replies
        .send(backend::Reply::ChannelConnected(
            s.workspace.unwrap(),
            channel.clone(),
        ))
        .unwrap();
    s.maintain(Instant::now());
    assert!(s.connected.contains(&channel.id));
    assert_eq!(s.channel_page, Some(channel.id));
    assert!(s.posts.is_empty());
    s.navigation_draw(
        &mut Scene::new(),
        Affine::IDENTITY,
        Rect::new(0., 0., 280., 500.),
    );
    s.draw(&mut Scene::new(), Affine::IDENTITY);
    assert!(s.hits.iter().any(|(_, a)| *a == Action::New));
}
