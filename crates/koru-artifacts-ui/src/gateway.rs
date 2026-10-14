// SPDX-License-Identifier: MPL-2.0

//! The gateway, as this surface reaches it.
//!
//! One [`Client`] shared by the surface's two worker threads, and one rule
//! for what to do when it stops working — the same shape
//! `koru-search-ui`'s store handle has, and for the same reasons: dialling
//! per keystroke would be absurd, and a thread that sleeps through a
//! backoff is a thread that swallows the newest request.
//!
//! **Re-discovery, never a remembered address.** A port is a promise
//! nobody made.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use koru_api_client::Client;
use koru_artifact_wire::Failure;

/// How long to wait before dialling again after a failure. A gateway that
/// is down must not be re-discovered on every refresh: the handshake is not
/// free and the answer will not have changed.
const RETRY_FIRST: Duration = Duration::from_millis(250);
const RETRY_MAX: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(crate) struct Gateway {
    connected: Arc<Mutex<Connected>>,
    /// `--root`, when the host was given one. Beside the handle rather than
    /// inside [`Connected`] because it never changes: a reconnect after the
    /// gateway restarts must dial the *same* root.
    root: Option<PathBuf>,
}

#[derive(Default)]
struct Connected {
    client: Option<Client>,
    /// Bumped whenever a fresh client is stored, so a call that failed
    /// clears only the client it actually used.
    generation: u64,
    retry_after: Option<Instant>,
    wait: Option<Duration>,
}

fn unreachable() -> Failure {
    Failure::new(
        "gateway_unavailable",
        "The Koru API gateway is not answering.",
        true,
    )
}

impl Gateway {
    pub(crate) fn new(root: Option<PathBuf>) -> Self {
        Self {
            connected: Arc::new(Mutex::new(Connected::default())),
            root,
        }
    }

    /// Run one call. **The lock is never held across it**: it guards the
    /// handle, not the request, so a Register can never sit behind a
    /// refresh.
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
            // else is: a store that answered and refused is answering.
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
        // build's own channel**, asked rather than written down.
        let client = match &self.root {
            Some(root) => Client::open_at(koru_channel::segment(), root),
            None => Client::open(koru_channel::segment()),
        }
        .map_err(|_| unreachable())?;

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

    /// **A gateway that is down must not be dialled on every refresh.** The
    /// back-off is armed by hand rather than by failing to connect, which
    /// is also the only way to assert what happens inside the window
    /// without a sleep — and the only way that does not depend on whether
    /// this machine happens to have Koru open.
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

    /// A discard for a generation somebody has already replaced leaves the
    /// newer client alone — two threads share this handle.
    #[test]
    fn a_stale_discard_leaves_a_newer_client_alone() {
        let gateway = Gateway::new(None);
        gateway.lock().generation = 9;
        gateway.discard(7);
        assert_eq!(gateway.lock().generation, 9);
    }
}
