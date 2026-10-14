// SPDX-License-Identifier: MPL-2.0

//! The gateway, as this surface reaches it — the Artifacts column's handle,
//! for the AI routes.
//!
//! One [`Client`] shared by the surface's two threads, and one rule for what
//! to do when it stops working: rediscover, never a remembered address, and
//! back off rather than dial on every refresh.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use koru_ai_wire::Failure;
use koru_api_client::Client;

/// The routes this surface calls: `ai.chat` lists and deletes chats, and
/// `ai.manage_providers` is what the provider settings — the agents' names —
/// are read under.
const SCOPES: &[&str] = &["ai.chat", "ai.manage_providers"];

const RETRY_FIRST: Duration = Duration::from_millis(250);
const RETRY_MAX: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(crate) struct Gateway {
    connected: Arc<Mutex<Connected>>,
    /// `--root`, when the host was given one. A reconnect after the gateway
    /// restarts must dial the *same* root.
    root: Option<PathBuf>,
}

#[derive(Default)]
struct Connected {
    client: Option<Client>,
    /// Bumped whenever a fresh client is stored, so a call that failed clears
    /// only the client it actually used.
    generation: u64,
    retry_after: Option<Instant>,
    wait: Option<Duration>,
}

pub(crate) fn unreachable() -> Failure {
    Failure {
        code: "gateway_unavailable".into(),
        message: "The Koru API gateway is not answering.".into(),
        retryable: true,
        details: Default::default(),
    }
}

impl Gateway {
    pub(crate) fn new(root: Option<PathBuf>) -> Self {
        Self {
            connected: Arc::new(Mutex::new(Connected::default())),
            root,
        }
    }

    /// Run one call. **The lock is never held across it**: it guards the
    /// handle, not the request, so a delete never sits behind a listing.
    pub(crate) fn call<T>(
        &self,
        work: impl Fn(&Client) -> Result<T, Failure>,
    ) -> Result<T, Failure> {
        let (client, generation) = self.client()?;
        match work(&client) {
            Ok(answer) => {
                self.succeeded();
                Ok(answer)
            }
            // A gateway process that went away must be found again. Nothing
            // else is: a service that answered and refused is answering.
            Err(failure) if failure.code == "gateway_unavailable" => {
                self.discard(generation);
                let (fresh, _) = self.client()?;
                match work(&fresh) {
                    Ok(answer) => {
                        self.succeeded();
                        Ok(answer)
                    }
                    Err(failure) => {
                        self.failed();
                        Err(failure)
                    }
                }
            }
            Err(failure) => Err(failure),
        }
    }

    fn client(&self) -> Result<(Client, u64), Failure> {
        let mut connected = self.lock();
        if let Some(client) = &connected.client {
            return Ok((client.clone(), connected.generation));
        }
        if let Some(after) = connected.retry_after
            && Instant::now() < after
        {
            return Err(unreachable());
        }
        drop(connected);

        // Outside the lock: the handshake dials a pipe and waits. **This
        // build's own channel**, asked rather than written down. `open`, never
        // `ensure`: a list is not a reason to start the service.
        let client = match &self.root {
            Some(root) => Client::open_at_with_scopes(koru_channel::segment(), root, SCOPES),
            None => Client::open_with_scopes(koru_channel::segment(), SCOPES),
        };
        let Ok(client) = client else {
            self.failed();
            return Err(unreachable());
        };

        connected = self.lock();
        connected.client = Some(client.clone());
        connected.generation = connected.generation.wrapping_add(1);
        Ok((client, connected.generation))
    }

    fn discard(&self, generation: u64) {
        let mut connected = self.lock();
        if connected.generation == generation {
            connected.client = None;
        }
    }

    fn succeeded(&self) {
        let mut connected = self.lock();
        connected.retry_after = None;
        connected.wait = None;
    }

    fn failed(&self) {
        let mut connected = self.lock();
        connected.client = None;
        let wait = connected
            .wait
            .map(|wait| (wait * 2).min(RETRY_MAX))
            .unwrap_or(RETRY_FIRST);
        connected.wait = Some(wait);
        connected.retry_after = Some(Instant::now() + wait);
    }

    /// Poisoning is not a reason to stop: nothing here is state a panic
    /// could have left half-written.
    fn lock(&self) -> std::sync::MutexGuard<'_, Connected> {
        self.connected.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A gateway that is down is not dialled on every refresh.** Armed by
    /// hand, so the test does not depend on whether this machine has Koru
    /// open — and never dials anything.
    #[test]
    fn a_failing_gateway_backs_off_rather_than_dialling_every_time() {
        let gateway = Gateway::new(None);
        gateway.failed();
        let refused = gateway.call(|_| Ok::<(), Failure>(())).unwrap_err();
        assert_eq!(refused.code, "gateway_unavailable");
        assert!(refused.retryable);
        assert!(gateway.lock().client.is_none());
    }

    #[test]
    fn the_wait_doubles_to_a_ceiling_and_one_answer_clears_it() {
        let gateway = Gateway::new(None);
        gateway.failed();
        assert_eq!(gateway.lock().wait, Some(RETRY_FIRST));
        gateway.failed();
        assert_eq!(gateway.lock().wait, Some(RETRY_FIRST * 2));
        for _ in 0..10 {
            gateway.failed();
        }
        assert_eq!(gateway.lock().wait, Some(RETRY_MAX));
        gateway.succeeded();
        assert!(gateway.lock().wait.is_none());
        assert!(gateway.lock().retry_after.is_none());
    }
}
