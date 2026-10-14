// SPDX-License-Identifier: MPL-2.0

//! Blocking Store calls live on one worker, never the drawing/input thread.
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use koru_api_client::Client;
use koru_socials::{Channel, Document, Network, POST_KIND, Post};
use koru_socials_store::{
    CHANNEL_KIND, Change, Watch, load_post, read_channel, save_channel, save_post,
};
use koru_store_core::{DocumentId, WorkspaceId};

pub enum Request {
    ConnectProvider {
        network: Network,
        organization: String,
        workspace: WorkspaceId,
        client_id: String,
        cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
        open_browser: fn(&str) -> std::io::Result<()>,
        success_html: String,
        error_html: String,
    },
    Load(WorkspaceId),
    Save {
        workspace: WorkspaceId,
        post: Post,
        base: Option<Post>,
    },
    AddChannel {
        workspace: WorkspaceId,
        channel: Channel,
    },
    Import {
        workspace: WorkspaceId,
        document: DocumentId,
        title: String,
    },
}

pub struct Listing {
    pub device: Option<uuid::Uuid>,
    pub posts: Vec<Post>,
    pub channels: Vec<Channel>,
    pub incomplete: bool,
    pub connected: std::collections::HashSet<uuid::Uuid>,
}

pub enum Reply {
    Loaded(WorkspaceId, Listing),
    PostChanged(WorkspaceId, Post),
    ListingChanged(WorkspaceId, Listing),
    WatchFailed,
    Saved(WorkspaceId, Post),
    ChannelSaved(WorkspaceId, Channel),
    ChannelConnected(WorkspaceId, Channel),
    Imported(WorkspaceId, Post),
    Failed(Failure),
}

#[derive(Clone, Copy)]
pub enum Failure {
    Unavailable,
    Changed,
    Invalid,
    OAuth(koru_socials_x::Error),
}

impl From<koru_socials_store::Error> for Failure {
    fn from(error: koru_socials_store::Error) -> Self {
        match error {
            koru_socials_store::Error::Unavailable => Self::Unavailable,
            koru_socials_store::Error::Changed => Self::Changed,
            koru_socials_store::Error::Invalid => Self::Invalid,
        }
    }
}

pub struct Backend {
    pub send: Sender<Request>,
    pub receive: Receiver<Reply>,
}

impl Backend {
    pub fn new(root: Option<PathBuf>) -> Self {
        let (send, requests) = mpsc::channel();
        let (replies, receive) = mpsc::channel();
        if !cfg!(any(test, feature = "no-reader")) {
            let _ = std::thread::Builder::new()
                .name("koru-socials-store".into())
                .spawn(move || Worker::new(root, replies).run(requests));
        }
        Self { send, receive }
    }
}

struct Worker {
    root: Option<PathBuf>,
    replies: Sender<Reply>,
    client: Option<Client>,
    watch: Option<Watch>,
    workspace: Option<WorkspaceId>,
    retry_watch: Instant,
    watch_failed: bool,
}

impl Worker {
    fn new(root: Option<PathBuf>, replies: Sender<Reply>) -> Self {
        Self {
            root,
            replies,
            client: None,
            watch: None,
            workspace: None,
            retry_watch: Instant::now(),
            watch_failed: false,
        }
    }

    fn connect(&mut self) -> Result<(), Failure> {
        if self.client.is_none() {
            let segment = koru_channel::segment();
            let client = match &self.root {
                Some(root) => Client::connect_at(segment, root),
                None => Client::connect(segment),
            }
            .map_err(|_| Failure::Unavailable)?;
            self.client = Some(client);
        }
        Ok(())
    }

    fn open_watch(&mut self, workspace: WorkspaceId) -> Result<(), Failure> {
        self.connect()?;
        self.watch = Some(Watch::open(
            self.client.as_ref().ok_or(Failure::Unavailable)?,
            &[workspace],
        )?);
        Ok(())
    }

    fn unavailable(&mut self) {
        self.client = None;
        self.watch = None;
        self.retry_watch = Instant::now() + Duration::from_secs(5);
        if self.workspace.is_some() && !self.watch_failed {
            self.watch_failed = true;
            let _ = self.replies.send(Reply::WatchFailed);
        }
    }

    fn request(&mut self, request: Request) -> Result<Reply, Failure> {
        if let Request::Load(workspace) = &request {
            self.workspace = Some(*workspace);
            self.watch = None;
            // Subscribe before taking the listing; initial room catch-up closes
            // the race between subscription setup and the HTTP snapshot reads.
            self.open_watch(*workspace)?;
        } else {
            self.connect()?;
        }
        let is_load = matches!(request, Request::Load(_));
        let answer = execute(
            self.client.as_ref().ok_or(Failure::Unavailable)?,
            request,
            self.root.as_deref(),
        );
        if is_load && answer.is_ok() {
            self.watch_failed = false;
        }
        answer
    }

    fn changed(&mut self, change: Change) -> Result<Reply, Failure> {
        let client = self.client.as_ref().ok_or(Failure::Unavailable)?;
        match change {
            Change::Post(workspace, id) => Ok(Reply::PostChanged(
                workspace,
                load_post(client, workspace, id)?,
            )),
            Change::Manifest(workspace) | Change::Channel(workspace, _) => {
                self.watch
                    .as_mut()
                    .ok_or(Failure::Unavailable)?
                    .refresh_workspace(client, workspace)?;
                listing_changed(client, workspace, self.root.as_deref())
            }
        }
    }

    fn run(mut self, requests: Receiver<Request>) {
        loop {
            match requests.recv_timeout(Duration::from_millis(50)) {
                Ok(request) => {
                    let answer = self.request(request);
                    if matches!(answer, Err(Failure::Unavailable)) {
                        self.unavailable();
                    }
                    if self
                        .replies
                        .send(answer.unwrap_or_else(Reply::Failed))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if self.watch.is_none()
                && Instant::now() >= self.retry_watch
                && let Some(workspace) = self.workspace
            {
                let answer = self.open_watch(workspace).and_then(|()| {
                    listing_changed(
                        self.client.as_ref().ok_or(Failure::Unavailable)?,
                        workspace,
                        self.root.as_deref(),
                    )
                });
                match answer {
                    Ok(reply) => {
                        self.watch_failed = false;
                        if self.replies.send(reply).is_err() {
                            break;
                        }
                    }
                    Err(_) => self.unavailable(),
                }
            }
            // One protocol event per iteration keeps queued UI requests ahead
            // of a busy subscription. No listing is read merely because time passed.
            let event = self.watch.as_mut().map(Watch::poll);
            match event {
                Some(Ok(Some(change))) => match self.changed(change) {
                    Ok(reply) => {
                        if self.replies.send(reply).is_err() {
                            break;
                        }
                    }
                    Err(_) => self.unavailable(),
                },
                Some(Err(_)) => self.unavailable(),
                _ => {}
            }
        }
    }
}

fn listing_changed(
    client: &Client,
    workspace: WorkspaceId,
    root: Option<&std::path::Path>,
) -> Result<Reply, Failure> {
    match execute(client, Request::Load(workspace), root)? {
        Reply::Loaded(workspace, listing) => Ok(Reply::ListingChanged(workspace, listing)),
        _ => Err(Failure::Invalid),
    }
}

fn acknowledged_device(root: Option<&std::path::Path>) -> Option<uuid::Uuid> {
    let endpoint = koru_api_client::grants::agent_endpoint(koru_channel::segment(), root).ok()?;
    let welcome = koru_api_client::grants::agent_welcome(&endpoint).ok()?;
    uuid::Uuid::parse_str(&welcome.device_id).ok()
}

fn execute(
    client: &Client,
    request: Request,
    root_path: Option<&std::path::Path>,
) -> Result<Reply, Failure> {
    match request {
        Request::ConnectProvider {
            network,
            organization,
            workspace,
            client_id,
            cancelled,
            open_browser,
            success_html,
            error_html,
        } => {
            let completion_cancelled = cancelled.clone();
            let options = koru_socials_auth::ConnectOptions {
                root: root_path.map(ToOwned::to_owned),
                client_id,
                cancelled,
                open_browser,
                success_html,
                error_html,
            };
            let account = match network {
                Network::X => koru_socials_x::connect(options),
                Network::Reddit => koru_socials_reddit::connect(options),
                Network::LinkedInPerson => koru_socials_linkedin::connect(options, None),
                Network::LinkedInPage => {
                    koru_socials_linkedin::connect(options, Some(&organization))
                }
            }
            .map_err(Failure::OAuth)?;
            let Reply::Loaded(_, listing) = execute(client, Request::Load(workspace), root_path)?
            else {
                return Err(Failure::Invalid);
            };
            if listing.incomplete {
                return Err(Failure::Unavailable);
            }
            if completion_cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return Err(Failure::OAuth(koru_socials_x::Error::Cancelled));
            }
            let name = match network {
                Network::X => format!("@{}", account.username),
                Network::Reddit => format!("u/{}", account.username),
                _ => account.display_name.clone(),
            };
            let mut channel = listing
                .channels
                .into_iter()
                .find(|c| {
                    c.network == network && c.remote_actor.as_deref() == Some(account.id.as_str())
                })
                .unwrap_or_else(|| Channel::new(network, name.clone()));
            channel.name = name;
            channel.remote_actor = Some(account.id);
            save_channel(client, workspace, &channel)?;
            Ok(Reply::ChannelConnected(workspace, channel))
        }
        Request::Import {
            workspace,
            document,
            title,
        } => {
            let bytes = client
                .store()
                .snapshot(workspace, document)
                .map_err(|_| Failure::Unavailable)?;
            let body = source_text(&bytes)?;
            let mut post = Post::draft(chrono::Local::now().timestamp_millis());
            post.title = title.clone();
            post.body = body;
            post.source = Some(koru_socials::Source {
                workspace: workspace.as_uuid(),
                document: document.as_uuid(),
                title,
                follows: false,
            });
            Ok(Reply::Imported(workspace, post))
        }
        Request::Load(workspace) => {
            let entries = client
                .store()
                .documents(workspace)
                .map_err(|_| Failure::Unavailable)?;
            let mut listing = Listing {
                device: acknowledged_device(root_path),
                posts: Vec::new(),
                channels: Vec::new(),
                incomplete: false,
                connected: Default::default(),
            };
            for entry in entries.into_iter().filter(|e| {
                e.trashed_at.is_none() && (e.kind == POST_KIND || e.kind == CHANNEL_KIND)
            }) {
                let loaded = client.store().snapshot(workspace, entry.id).ok();
                if let Some(bytes) = loaded {
                    if entry.kind == POST_KIND {
                        if let Ok(post) = Document::from_snapshot(&bytes).and_then(|d| d.read()) {
                            listing.posts.push(post);
                            continue;
                        }
                    } else if let Some(channel) = read_channel(&bytes) {
                        if let Some(actor) = channel.remote_actor.as_deref() {
                            let present = match channel.network {
                                Network::X => koru_socials_x::has_credentials(root_path, actor),
                                Network::Reddit => {
                                    koru_socials_reddit::has_credentials(root_path, actor)
                                }
                                _ => koru_socials_linkedin::has_credentials(root_path, actor),
                            };
                            if present.unwrap_or(false) {
                                listing.connected.insert(channel.id);
                            }
                        }
                        listing.channels.push(channel);
                        continue;
                    }
                }
                listing.incomplete = true;
            }
            listing
                .posts
                .sort_by_key(|p| p.scheduled_at_ms.unwrap_or(p.updated_at_ms));
            Ok(Reply::Loaded(workspace, listing))
        }
        Request::Save {
            workspace,
            mut post,
            base,
        } => {
            if let Some(base) = &base {
                if post.execution_device != base.execution_device
                    || (post.scheduled && base.execution_device.is_none())
                {
                    return Err(Failure::Invalid);
                }
            } else {
                // The Agent greeting is the identity authority. On a retry the
                // same stable device is applied to the otherwise exact request.
                post.execution_device =
                    Some(acknowledged_device(root_path).ok_or(Failure::Unavailable)?);
            }
            save_post(client, workspace, &post, base.as_ref())?;
            Ok(Reply::Saved(workspace, post))
        }
        Request::AddChannel { workspace, channel } => {
            save_channel(client, workspace, &channel)?;
            Ok(Reply::ChannelSaved(workspace, channel))
        }
    }
}

/// Import an explicit plain-text snapshot, excluding trash, media and pills.
fn source_text(bytes: &[u8]) -> Result<String, Failure> {
    let document = koru_core::WorkspaceDoc::from_snapshot(bytes).map_err(|_| Failure::Invalid)?;
    if !matches!(
        document.compatibility(),
        koru_core::schema::Compatibility::Writable
    ) {
        return Err(Failure::Invalid);
    }
    let mut blocks = Vec::new();
    for (id, _) in document.walk().map_err(|_| Failure::Invalid)? {
        let node = document.read(id).map_err(|_| Failure::Invalid)?;
        if let Some(text) = node.content.as_text() {
            let text = text.replace(koru_core::text::ANCHOR, "");
            if !text.trim().is_empty() {
                blocks.push(text);
            }
        }
    }
    Ok(blocks.join("\n\n"))
}
