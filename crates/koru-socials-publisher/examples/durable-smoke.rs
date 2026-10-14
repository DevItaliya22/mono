// SPDX-License-Identifier: MPL-2.0

//! Explicit isolated disk/Store exercise. The transport is synthetic and
//! cannot make an X request. Run three times to exercise separate processes.
use koru_socials::{Channel, Destination, Network, Post, Publication};
use koru_socials_publisher::*;
use koru_store_core::WorkspaceId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

struct Store(koru_api_client::Client);
impl Repository for Store {
    fn load(&mut self, k: Key) -> Result<Option<Post>, Fault> {
        koru_socials_store::load_post(&self.0, WorkspaceId::from_uuid(k.workspace), k.post)
            .map(Some)
            .map_err(|_| Fault::Store)
    }
    fn save(&mut self, k: Key, p: &Post, b: &Post) -> Result<(), Fault> {
        koru_socials_store::save_post(&self.0, WorkspaceId::from_uuid(k.workspace), p, Some(b))
            .map_err(|_| Fault::Store)
    }
}
#[derive(Default)]
struct Synthetic {
    sent: Vec<Option<String>>,
}
impl Provider for Synthetic {
    fn prepare(
        &mut self,
        _: &koru_socials::PublicationTarget,
        _: &str,
    ) -> Result<(), ProviderError> {
        Ok(())
    }
    fn send(
        &mut self,
        _target: &koru_socials::PublicationTarget,
        _: &str,
        _: &str,
        reply: Option<&str>,
    ) -> Result<Publication, ProviderError> {
        self.sent.push(reply.map(str::to_owned));
        Ok(Publication {
            id: if reply.is_some() {
                "synthetic-part-2"
            } else {
                "synthetic-part-1"
            }
            .into(),
            url: None,
            published_at_ms: 0,
        })
    }
}
#[derive(Serialize, Deserialize)]
struct Saved {
    workspace: Uuid,
    post: Uuid,
    channel: Uuid,
    device: Uuid,
}
fn main() {
    assert!(
        !cfg!(feature = "no-network"),
        "explicit production Store adapter required"
    );
    let root = std::path::PathBuf::from(std::env::args().nth(1).expect("isolated root required"))
        .canonicalize()
        .unwrap();
    let allowed = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/verification/socials")
        .canonicalize()
        .unwrap();
    assert!(root.starts_with(&allowed) && root != allowed);
    let client = koru_api_client::Client::connect_at(koru_channel::segment(), &root).unwrap();
    let marker = root.join("synthetic-publisher-smoke.json");
    let saved: Saved = if marker.exists() {
        serde_json::from_slice(&std::fs::read(&marker).unwrap()).unwrap()
    } else {
        let profile = client
            .store()
            .create_profile(
                "Synthetic publishing verification",
                None,
                None,
                Some("en-US"),
            )
            .unwrap();
        let ws = client
            .store()
            .create_workspace(profile.id, "Synthetic publishing verification")
            .unwrap();
        let mut channel = Channel::new(Network::X, "Synthetic transport only".into());
        channel.remote_actor = Some("synthetic-no-real-account".into());
        let mut p = Post::draft(0);
        p.title = "SYNTHETIC: durable delivery process test".into();
        p.body = "A long synthetic paragraph that is never sent to a real provider. ".repeat(6);
        let device = Uuid::new_v4(); // Never the real Agent's publishing identity.
        p.execution_device = Some(device);
        p.scheduled = true;
        p.scheduled_at_ms = Some(0);
        p.destinations.push(Destination::new(channel.clone()));
        assert_eq!(p.destinations[0].parts(&p.body).len(), 2);
        koru_socials_store::save_post(&client, ws.id, &p, None).unwrap();
        let saved = Saved {
            workspace: ws.id.as_uuid(),
            post: p.id,
            channel: channel.id,
            device,
        };
        std::fs::write(&marker, serde_json::to_vec(&saved).unwrap()).unwrap();
        saved
    };
    let key = Key {
        workspace: saved.workspace,
        post: saved.post,
        channel: saved.channel,
    };
    let ledger = SqliteLedger::open(&root.join("synthetic-publisher-smoke.db")).unwrap();
    let mut engine = Engine {
        repository: Store(client),
        journal: ledger,
        provider: Synthetic::default(),
        device: saved.device,
    };
    let prior = engine.journal.load(key).unwrap();
    engine.step(key, 1_000).unwrap();
    let after = engine.journal.load(key).unwrap().unwrap();
    let post = engine.repository.load(key).unwrap().unwrap();
    assert_eq!(post.destinations[0].delivery.published, after.published);
    match prior {
        None => {
            assert_eq!(engine.provider.sent, vec![None]);
            assert_eq!(after.published.len(), 1);
            println!("PASS first process: confirmed part 1 durable in SQLite and Store");
        }
        Some(a) if a.phase == Phase::Ready => {
            assert_eq!(engine.provider.sent, vec![Some("synthetic-part-1".into())]);
            assert_eq!(after.phase, Phase::Published);
            println!("PASS restarted process: sent only part 2, replying to confirmed part 1");
        }
        Some(a) => {
            assert_eq!(a.phase, Phase::Published);
            assert!(engine.provider.sent.is_empty());
            println!("PASS reopened published attempt: no sends");
        }
    }
}
