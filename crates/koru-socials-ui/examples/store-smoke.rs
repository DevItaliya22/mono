// SPDX-License-Identifier: MPL-2.0

//! Explicit integration exercise; never runs under cargo test.
//! Only accepts a root below this worktree's target/verification/socials/.
use koru_socials::{Channel, DeliveryState, Destination, Network, Post};
use koru_socials_ui::backend::{Backend, Reply, Request};
use std::{path::PathBuf, time::Duration};

fn ask(backend: &Backend, request: Request) -> Reply {
    backend
        .send
        .send(request)
        .map_err(|_| "worker stopped")
        .unwrap();
    loop {
        let reply = backend
            .receive
            .recv_timeout(Duration::from_secs(30))
            .expect("worker timeout");
        match reply {
            Reply::PostChanged(..) | Reply::ListingChanged(..) => continue,
            Reply::Failed(_) | Reply::WatchFailed => panic!("Store operation failed"),
            _ => return reply,
        }
    }
}

fn main() {
    assert!(!cfg!(feature = "no-reader"), "run without no-reader");
    let root = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("explicit isolated root required"),
    )
    .canonicalize()
    .expect("root exists");
    let allowed = std::env::current_dir()
        .unwrap()
        .join("target/verification/socials")
        .canonicalize()
        .unwrap();
    assert!(
        root.starts_with(&allowed) && root != allowed,
        "root must be a child of target/verification/socials"
    );
    let client = koru_api_client::Client::connect_at(koru_channel::segment(), &root)
        .expect("rooted gateway");
    let profile = client
        .store()
        .profiles()
        .unwrap()
        .into_iter()
        .find(|p| p.name == "Socials acceptance")
        .unwrap_or_else(|| {
            client
                .store()
                .create_profile("Socials acceptance", None, None, Some("en-US"))
                .unwrap()
        });
    let workspace = client
        .store()
        .workspaces()
        .unwrap()
        .into_iter()
        .find(|w| w.name == "Socials acceptance")
        .unwrap_or_else(|| {
            client
                .store()
                .create_workspace(profile.id, "Socials acceptance")
                .unwrap()
        });
    let worker = Backend::new(Some(root));
    let Reply::Loaded(_, listing) = ask(&worker, Request::Load(workspace.id)) else {
        panic!("load reply")
    };
    assert!(!listing.incomplete);
    if let Some(post) = listing
        .posts
        .iter()
        .find(|p| p.title == "Socials persistence smoke")
    {
        assert_eq!(
            post.body,
            "Saved, edited, and reopened through the real Store."
        );
        assert!(post.scheduled);
        assert_eq!(
            post.destinations[0].delivery.state,
            DeliveryState::NeedsConnection
        );
        println!("REOPEN PASS: post {}, workspace {}", post.id, workspace.id);
        return;
    }
    let channel = Channel::new(Network::X, "Acceptance X (unconnected)".into());
    assert!(matches!(
        ask(
            &worker,
            Request::AddChannel {
                workspace: workspace.id,
                channel: channel.clone()
            }
        ),
        Reply::ChannelSaved(..)
    ));
    let mut post = Post::draft(chrono::Local::now().timestamp_millis());
    post.title = "Socials persistence smoke".into();
    post.body = "First save".into();
    post.destinations.push(Destination::new(channel));
    let Reply::Saved(_, saved) = ask(
        &worker,
        Request::Save {
            workspace: workspace.id,
            post: post.clone(),
            base: None,
        },
    ) else {
        panic!("save reply")
    };
    assert!(saved.execution_device.is_some());
    post = saved;
    let base = post.clone();
    post.body = "Saved, edited, and reopened through the real Store.".into();
    post.scheduled = true;
    post.scheduled_at_ms = Some(chrono::Local::now().timestamp_millis() + 86_400_000);
    post.destinations[0].delivery.state = DeliveryState::NeedsConnection;
    assert!(matches!(
        ask(
            &worker,
            Request::Save {
                workspace: workspace.id,
                post: post.clone(),
                base: Some(base.clone())
            }
        ),
        Reply::Saved(..)
    ));
    // Resubmitting the exact value after a lost reply must be harmless.
    assert!(matches!(
        ask(
            &worker,
            Request::Save {
                workspace: workspace.id,
                post: post.clone(),
                base: Some(base)
            }
        ),
        Reply::Saved(..)
    ));
    let Reply::Loaded(_, loaded) = ask(&worker, Request::Load(workspace.id)) else {
        panic!("reload reply")
    };
    assert_eq!(loaded.posts.iter().find(|p| p.id == post.id), Some(&post));
    println!(
        "CREATE/UPDATE/RETRY/LOAD PASS: post {}, workspace {}",
        post.id, workspace.id
    );
}
