// SPDX-License-Identifier: MPL-2.0

use std::path::PathBuf;
use std::sync::mpsc;

use koru_session_runtime::SessionLifecycleSink;

/// Fixed-capacity, best-effort handoff to the already-rooted Events client.
/// Runtime workers never perform journal, network, or gateway discovery I/O.
pub struct LifecycleQueue {
    send: mpsc::SyncSender<koru_events::Draft>,
}

impl LifecycleQueue {
    pub fn start(segment: String, channel_root: PathBuf) -> Self {
        let (send, receive) = mpsc::sync_channel::<koru_events::Draft>(64);
        let _ = std::thread::Builder::new()
            .name("koru-session-host events".into())
            .stack_size(128 * 1024)
            .spawn(move || {
                while let Ok(draft) = receive.recv() {
                    let Ok(client) = koru_api_client::Client::connect_at(&segment, &channel_root)
                    else {
                        continue;
                    };
                    if client.events().available() {
                        let _ = client.events().send(draft);
                    }
                }
            });
        Self { send }
    }
}

impl SessionLifecycleSink for LifecycleQueue {
    fn publish(&self, event: koru_events::Draft) {
        let _ = self.send.try_send(event);
    }
}
