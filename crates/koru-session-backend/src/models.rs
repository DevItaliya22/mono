// SPDX-License-Identifier: MPL-2.0

//! One coalescing worker and one independent connection; model probing never
//! holds the connection that carries input, cancellation or approval answers.
use super::*;
use std::sync::{Condvar, Mutex};
struct Job {
    request: IdempotencyKey,
    provider: koru_session_protocol::ProviderKind,
    cwd: PathBuf,
}
#[derive(Default)]
struct State {
    next: Option<Job>,
    stopped: bool,
}
pub(crate) struct Worker {
    shared: Arc<(Mutex<State>, Condvar)>,
}
impl Worker {
    pub fn spawn(
        endpoint: koru_ipc::Endpoint,
        generation: u64,
        internal: SyncSender<BackendCommand>,
    ) -> Self {
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let worker = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("koru-session-models".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                loop {
                    let job = {
                        let (lock, ready) = &*worker;
                        let mut state = lock.lock().unwrap_or_else(|held| held.into_inner());
                        while !state.stopped && state.next.is_none() {
                            state = ready.wait(state).unwrap_or_else(|held| held.into_inner());
                        }
                        if state.stopped {
                            break;
                        }
                        state.next.take().expect("pending model request")
                    };
                    let result =
                        SessionClient::connect(&endpoint, "koru-session-models", protocol_offer())
                            .and_then(|client| {
                                client.discover_provider_models(job.provider, job.cwd)
                            })
                            .map_err(|error| client_failure(error).code);
                    if internal
                        .send(BackendCommand::ModelsDiscovered {
                            generation,
                            request: job.request,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            });
        if spawned.is_err() {
            shared
                .0
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .stopped = true;
        }
        Self { shared }
    }
    pub fn discover(
        &self,
        request: IdempotencyKey,
        provider: koru_session_protocol::ProviderKind,
        cwd: PathBuf,
    ) -> bool {
        let mut state = self
            .shared
            .0
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        if state.stopped {
            return false;
        }
        state.next = Some(Job {
            request,
            provider,
            cwd,
        });
        self.shared.1.notify_one();
        true
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let mut state = self
            .shared
            .0
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        state.stopped = true;
        state.next = None;
        self.shared.1.notify_one();
    }
}
