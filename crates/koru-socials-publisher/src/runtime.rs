// SPDX-License-Identifier: MPL-2.0

//! The Agent owns this worker. The UI only writes intent to the Store.
use crate::{Engine, Fault, Key, Provider, ProviderError, Repository, SqliteLedger};
use koru_api_client::{CatalogChange, Client};
use koru_socials::{POST_KIND, Post, Publication, PublicationTarget};
use koru_socials_store::{Change, Watch};
use koru_store_core::WorkspaceId;
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

pub struct Worker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Worker {
    pub fn start(root: Option<PathBuf>, agent_dir: PathBuf, device: Uuid) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = if cfg!(any(test, feature = "no-network")) {
            None
        } else {
            std::thread::Builder::new()
                .name("koru-socials-publisher".into())
                .spawn(move || {
                    if run(root, agent_dir, device, &flag).is_err() {
                        tracing::error!(
                            "Socials publisher stopped: local delivery journal unavailable"
                        );
                    }
                })
                .ok()
        };
        Self { stop, thread }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Store(Client);
impl Repository for Store {
    fn load(&mut self, key: Key) -> Result<Option<Post>, Fault> {
        koru_socials_store::load_post(&self.0, WorkspaceId::from_uuid(key.workspace), key.post)
            .map(Some)
            .map_err(|_| Fault::Store)
    }
    fn save(&mut self, key: Key, post: &Post, base: &Post) -> Result<(), Fault> {
        koru_socials_store::save_post(
            &self.0,
            WorkspaceId::from_uuid(key.workspace),
            post,
            Some(base),
        )
        .map_err(|_| Fault::Store)
    }
}
struct Providers {
    root: Option<PathBuf>,
    x: Option<koru_socials_x::Publisher>,
    linkedin: Option<koru_socials_linkedin::Publisher>,
    reddit: Option<koru_socials_reddit::Publisher>,
}
fn provider_error(error: koru_socials_auth::PublishError) -> ProviderError {
    use koru_socials_auth::PublishError as E;
    match error {
        E::NeedsConnection => ProviderError::NeedsConnection,
        E::Retryable { retry_after_ms } => ProviderError::Retryable { retry_after_ms },
        E::Rejected => ProviderError::Rejected,
        E::Unconfirmed => ProviderError::Unconfirmed,
        E::Unavailable => ProviderError::Unavailable,
    }
}
impl Provider for Providers {
    fn prepare(&mut self, target: &PublicationTarget, actor: &str) -> Result<(), ProviderError> {
        match target {
            PublicationTarget::X => {
                if self.x.is_none() {
                    self.x = Some(
                        koru_socials_x::Publisher::new(self.root.clone())
                            .map_err(provider_error)?,
                    );
                }
                self.x.as_mut().unwrap().prepare(actor)
            }
            PublicationTarget::LinkedInPerson | PublicationTarget::LinkedInPage => {
                if self.linkedin.is_none() {
                    self.linkedin = Some(
                        koru_socials_linkedin::Publisher::new(self.root.clone())
                            .map_err(provider_error)?,
                    );
                }
                self.linkedin.as_mut().unwrap().prepare(actor)
            }
            PublicationTarget::Reddit { .. } => {
                if self.reddit.is_none() {
                    self.reddit = Some(
                        koru_socials_reddit::Publisher::new(self.root.clone())
                            .map_err(provider_error)?,
                    );
                }
                self.reddit.as_mut().unwrap().prepare(actor)
            }
        }
        .map_err(provider_error)
    }
    fn send(
        &mut self,
        target: &PublicationTarget,
        actor: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> Result<Publication, ProviderError> {
        let result = match target {
            PublicationTarget::X => self
                .x
                .as_mut()
                .ok_or(ProviderError::Unavailable)?
                .send(actor, text, reply_to),
            PublicationTarget::LinkedInPerson | PublicationTarget::LinkedInPage
                if reply_to.is_none() =>
            {
                self.linkedin
                    .as_mut()
                    .ok_or(ProviderError::Unavailable)?
                    .send(actor, text)
            }
            PublicationTarget::Reddit {
                subreddit,
                title,
                flair_id,
            } if reply_to.is_none() => self
                .reddit
                .as_mut()
                .ok_or(ProviderError::Unavailable)?
                .send(actor, text, subreddit, title, flair_id.as_deref()),
            _ => return Err(ProviderError::Rejected),
        };
        result
            .map(|p| Publication {
                id: p.id,
                url: Some(p.url),
                published_at_ms: now(),
            })
            .map_err(provider_error)
    }
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
fn pause(stop: &AtomicBool, millis: u64) {
    for _ in 0..millis.div_ceil(100) {
        if stop.load(Ordering::Acquire) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn run(
    root: Option<PathBuf>,
    agent_dir: PathBuf,
    device: Uuid,
    stop: &AtomicBool,
) -> Result<(), Fault> {
    std::fs::create_dir_all(&agent_dir).map_err(|_| Fault::Ledger)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(agent_dir.join("socials-delivery.lock"))
        .map_err(|_| Fault::Ledger)?;
    fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| Fault::Ledger)?;
    // Keep the file lock for this worker's entire lifetime, including reconnects.
    let mut ledger = SqliteLedger::open(&agent_dir.join("socials-delivery.db"))?;
    while !stop.load(Ordering::Acquire) {
        let connection = match &root {
            Some(root) => Client::connect_at(koru_channel::segment(), root),
            None => Client::connect(koru_channel::segment()),
        };
        if let Ok(client) = connection {
            let provider = Providers {
                root: root.clone(),
                x: None,
                linkedin: None,
                reddit: None,
            };
            match connected(client, &mut ledger, provider, device, stop) {
                Err(Fault::Ledger) => return Err(Fault::Ledger),
                _ => {}
            }
        }
        pause(stop, 5_000);
    }
    Ok(())
}
impl crate::Journal for &mut SqliteLedger {
    fn load(&mut self, key: Key) -> Result<Option<crate::Attempt>, Fault> {
        (**self).load(key)
    }
    fn store(&mut self, key: Key, a: &crate::Attempt) -> Result<(), Fault> {
        (**self).store(key, a)
    }
}

fn connected(
    client: Client,
    ledger: &mut SqliteLedger,
    provider: Providers,
    device: Uuid,
    stop: &AtomicBool,
) -> Result<(), Fault> {
    let catalog = client
        .events()
        .watch_catalog(|_| {})
        .map_err(|_| Fault::Store)?;
    let workspaces: Vec<_> = client
        .store()
        .workspaces()
        .map_err(|_| Fault::Store)?
        .into_iter()
        .filter(|w| w.deleted_at.is_none())
        .map(|w| w.id)
        .collect();
    let mut watch = Watch::open(&client, &workspaces).map_err(|_| Fault::Store)?;
    let mut due = BTreeMap::new();
    for ws in workspaces {
        refresh(&client, ws, device, &mut due)?;
    }
    let mut engine = Engine {
        repository: Store(client.clone()),
        journal: ledger,
        provider,
        device,
    };
    while !stop.load(Ordering::Acquire) {
        if catalog.take(CatalogChange::Workspaces) || catalog.take(CatalogChange::Profiles) {
            return Err(Fault::Store);
        }
        match watch.poll().map_err(|_| Fault::Store)? {
            Some(Change::Manifest(ws)) => {
                watch
                    .refresh_workspace(&client, ws)
                    .map_err(|_| Fault::Store)?;
                refresh(&client, ws, device, &mut due)?;
            }
            Some(Change::Post(ws, id)) => {
                let p = koru_socials_store::load_post(&client, ws, id).map_err(|_| Fault::Store)?;
                add_post(ws, &p, device, &mut due);
            }
            _ => {}
        }
        let clock = now();
        if let Some(key) = due
            .iter()
            .find_map(|(key, at)| (*at <= clock).then_some(*key))
        {
            match engine.step_with_clock(key, now)? {
                Some(at) => {
                    due.insert(key, at);
                }
                None => {
                    due.remove(&key);
                }
            }
        }
        // Poll the wall clock after sleep/resume without rereading documents.
        std::thread::sleep(Duration::from_millis(40));
    }
    Ok(())
}
fn refresh(
    client: &Client,
    ws: WorkspaceId,
    device: Uuid,
    due: &mut BTreeMap<Key, i64>,
) -> Result<(), Fault> {
    let entries = client.store().documents(ws).map_err(|_| Fault::Store)?;
    due.retain(|key, _| key.workspace != ws.as_uuid());
    for entry in entries
        .into_iter()
        .filter(|e| e.trashed_at.is_none() && e.kind == POST_KIND)
    {
        let post = koru_socials_store::load_post(client, ws, entry.id.as_uuid())
            .map_err(|_| Fault::Store)?;
        add_post(ws, &post, device, due);
    }
    Ok(())
}
fn add_post(ws: WorkspaceId, post: &Post, device: Uuid, due: &mut BTreeMap<Key, i64>) {
    due.retain(|key, _| key.workspace != ws.as_uuid() || key.post != post.id);
    if post.execution_device != Some(device) {
        return;
    }
    // Even cancelled intent must be observed by its owner to acknowledge it.
    for d in &post.destinations {
        due.insert(
            Key {
                workspace: ws.as_uuid(),
                post: post.id,
                channel: d.channel.id,
            },
            now(),
        );
    }
}
