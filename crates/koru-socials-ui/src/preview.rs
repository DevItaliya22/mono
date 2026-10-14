// SPDX-License-Identifier: MPL-2.0

//! Synthetic visual fixtures. Available only in the explicitly offline build.
use super::*;

pub fn fixture(view: &str) -> Surface {
    let mut surface = Surface::new(None);
    surface.workspace = Some(WorkspaceId::from_uuid(Uuid::new_v4()));
    surface.channels = Network::ALL
        .into_iter()
        .zip(["@koru_app", "Koru", "Mira Okafor", "u/koru_team"])
        .map(|(network, name)| Channel::new(network, name.into()))
        .collect();
    let mut post = Post::draft(Local::now().timestamp_millis());
    post.title = "A calmer way to plan your week".into();
    post.body="Your ideas already live in your workspace. Now they can become posts, too.\n\nWrite once, adapt the message for each network, and keep the original close by. Use the calendar to find a good time, then see what needs attention.\n\nThis is a preview of Koru Socials. Every account shown here is synthetic and unconnected.".into();
    post.destinations = surface
        .channels
        .iter()
        .cloned()
        .map(Destination::new)
        .collect();
    if let Some(reddit) = post.destinations.last_mut() {
        reddit.subreddit = "SideProject".into();
        reddit.reddit_title = "Building a workspace for the whole idea".into();
    }
    if view == "calendar" {
        let day = Local::now().date_naive();
        for index in 0..6 {
            let mut p = post.clone();
            p.id = Uuid::new_v4();
            p.scheduled = true;
            let date = day + chrono::Duration::days(index);
            p.scheduled_at_ms = Some(
                Local
                    .from_local_datetime(&date.and_hms_opt(9 + index as u32, 0, 0).unwrap())
                    .single()
                    .unwrap()
                    .timestamp_millis(),
            );
            for d in &mut p.destinations {
                d.delivery.state = DeliveryState::NeedsConnection;
            }
            surface.posts.push(p);
        }
    } else if view == "delivery" || view == "unconfirmed" {
        post.destinations.truncate(1);
        let device = Uuid::new_v4();
        surface.device = Some(device);
        post.execution_device = Some(device);
        post.scheduled = true;
        let d = &mut post.destinations[0];
        d.delivery.content = Some(vec![
            "Synthetic confirmed part".into(),
            "Synthetic second part".into(),
        ]);
        d.delivery.published.push(koru_socials::Publication {
            id: "123456789".into(),
            url: None,
            published_at_ms: 0,
        });
        d.delivery.state = if view == "delivery" {
            DeliveryState::Failed
        } else {
            DeliveryState::Unconfirmed
        };
        surface.open(post.clone(), Some(post));
    } else if view.starts_with("accounts") {
        surface.page = Page::Accounts;
        surface.scroll = if view.ends_with("short") {
            100_000.
        } else {
            0.
        };
        surface.account_network = if view.contains("linkedin-page") {
            Network::LinkedInPage
        } else if view.contains("linkedin") {
            Network::LinkedInPerson
        } else if view.contains("reddit") {
            Network::Reddit
        } else {
            Network::X
        };
    } else {
        surface.open(post, None);
        let id = if view == "reddit" {
            surface.channels.last().unwrap().id
        } else {
            surface.channels[0].id
        };
        surface.select(id);
    }
    surface
}
