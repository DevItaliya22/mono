// SPDX-License-Identifier: MPL-2.0

//! The store, as a search surface reaches it.
//!
//! One [`Client`] shared by the surface's two worker threads, and one rule for
//! what to do when it stops working.
//!
//! ## Why a handle rather than a client
//!
//! `Client::open` runs the rendezvous handshake and mints a session. Doing that
//! per keystroke would be absurd. The Store client renews its bearer credential
//! itself; this handle owns only the separate concern of replacing a client
//! whose Store process has gone away on a new port.
//!
//! **Re-discovery, never a remembered address.** That is the discipline
//! `koru_workspace::store::link` already follows for the document socket, and
//! for the same reason: a port is a promise nobody made.
//!
//! ## Why it does not block and retry in a loop
//!
//! The link thread owns a socket and has nobody waiting on it, so it can sleep
//! through a backoff. This one answers a keystroke. A thread asleep for five
//! seconds would swallow the newest thing typed, and — because the surface is
//! built in unit tests with no store running at all — would keep test threads
//! alive after the surface was dropped.
//!
//! So a failed call **answers with the failure** and the *next* call retries.
//! Same discipline, in the shape this thread has.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use koru_api_client::Client;
use koru_store_core::errors::{Code, Error, Result};

/// How long to wait before dialling again after a second consecutive failure.
///
/// A store that is down must not be re-discovered on every keystroke: the
/// handshake is not free and the answer will not have changed.
const RETRY_FIRST: Duration = Duration::from_millis(250);
const RETRY_MAX: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(super) struct StoreSearch {
    connected: Arc<Mutex<Connected>>,
    /// `--root`, when the host was given one.
    ///
    /// **Beside the handle rather than inside `Connected`**, because it never
    /// changes: a reconnect after the store restarts must dial the *same* root,
    /// and a field the retry path cannot clear is how that is guaranteed rather
    /// than remembered.
    root: Option<PathBuf>,
}

#[derive(Default)]
struct Connected {
    client: Option<Client>,
    /// Bumped whenever a fresh client is stored.
    ///
    /// **Two threads share this handle**, so a call that failed must clear only
    /// the client it actually used — never one the other thread has just
    /// re-discovered in the meantime.
    generation: u64,
    /// Not before this instant. `None` means "try now".
    retry_after: Option<Instant>,
    wait: Option<Duration>,
}

impl StoreSearch {
    pub(super) fn new(root: Option<PathBuf>) -> Self {
        Self {
            connected: Arc::new(Mutex::new(Connected::default())),
            root,
        }
    }

    /// Run one call against the store, re-discovering once if the client we had
    /// has stopped working.
    ///
    /// **The lock is never held across the call.** It guards the handle, not
    /// the HTTP request, so recording a navigation can never sit in front of a
    /// keystroke's query.
    /// **Both of the query box's tables, in one round trip**: every live tag in
    /// `workspaces` as completion rows — name, colour and live usage, folded
    /// and de-duplicated — and every filterable attribute definition in scope,
    /// digested for the `/key` chooser.
    ///
    /// **One call, because it is one answer.** `AttributeQueryResponse` already
    /// carries `definitions` beside `tags`; asking twice would put two
    /// handshakes and two sweeps behind a box that needs both the moment
    /// anybody types a sigil into it.
    ///
    /// **No new endpoint.** The attribute query already answers this: with
    /// `group_by: "tag"` it returns the scoped tag tables (merge tombstones
    /// resolved away) beside corpus-wide counts. `limit: 1` because the rows
    /// are not wanted at all — only the tables and the counts, and a page of
    /// one is the smallest thing the endpoint will carry them on.
    ///
    /// **De-duplicated by fold key, summing counts.** Tags are workspace-scoped
    /// and this surface is usually global, so two workspaces can each hold a
    /// `#fun`; but a search filter takes tag **names**, deliberately — "a
    /// person types the word they read" — so one row per name is what matches
    /// the filter's own semantics. The surviving spelling is the most-used
    /// one, which is the one most likely to be the way the person writes it.
    ///
    /// **Ordered by live usage**, `count - trashed`: a tag whose uses are
    /// mostly in binned documents must not outrank one in front of you.
    pub(super) fn scope_tables(
        &self,
        workspaces: &[koru_store_core::ids::WorkspaceId],
    ) -> Result<(
        Vec<super::TagCompletion>,
        Vec<crate::filters::FilterDefinition>,
    )> {
        use koru_store_wire::attributes::AttributeQuery;

        let response = self.call(|client| {
            client.store().query_attributes(&AttributeQuery {
                workspaces: workspaces.to_vec(),
                group_by: Some("tag".to_owned()),
                limit: Some(1),
                ..AttributeQuery::default()
            })
        })?;

        // `groups` keys usage by TagId; `tags` is where an id becomes a word.
        let mut live: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for group in &response.groups {
            *live.entry(group.tag.as_str()).or_default() +=
                group.count.saturating_sub(group.trashed);
        }

        let mut by_key: std::collections::HashMap<String, super::TagCompletion> =
            std::collections::HashMap::new();
        for tag in &response.tags {
            let uses = live.get(tag.id.as_str()).copied().unwrap_or(0);
            let key = koru_tagname::fold_key(&tag.name);
            if key.is_empty() {
                continue;
            }
            match by_key.get_mut(&key) {
                Some(existing) => {
                    // Keep the more-used spelling, and let the counts add up:
                    // picking this row filters on the name, so it matches every
                    // workspace's tag that folds to it.
                    if uses > existing.uses {
                        existing.name = tag.name.clone();
                        existing.color = tag.color.clone();
                    }
                    existing.uses += uses;
                }
                None => {
                    by_key.insert(
                        key.clone(),
                        super::TagCompletion {
                            name: tag.name.clone(),
                            color: tag.color.clone(),
                            uses,
                            folded: koru_tagname::fold_segments(&tag.name),
                        },
                    );
                }
            }
        }

        let mut rows: Vec<super::TagCompletion> = by_key.into_values().collect();
        // Most-used first, then by name so the order is stable between fetches
        // rather than however the map happened to iterate.
        rows.sort_by(|a, b| b.uses.cmp(&a.uses).then_with(|| a.name.cmp(&b.name)));
        // **Registry order, not usage order.** The definitions arrive system
        // first and then per workspace, which is the order somebody reading the
        // list expects — Color, Due, Priority before whatever this workspace
        // invented. A chooser that reordered itself as the corpus changed would
        // move a row out from under a hand that had learned where it was.
        Ok((rows, crate::filters::digest(&response.definitions)))
    }

    pub(super) fn call<T>(&self, work: impl Fn(&Client) -> Result<T>) -> Result<T> {
        let (client, generation) = self.client()?;
        match work(&client) {
            Ok(answer) => {
                self.succeeded();
                Ok(answer)
            }
            // A Store process that went away must be found again. Bearer
            // renewal is deliberately not here: `koru-store-client` retries a
            // rejected credential once after re-rendezvousing, so every caller
            // gets the same long-lived-session behaviour. A `StorageFailed` is
            // not in the set: that store is answering, and dialling it again
            // reaches the same broken disk.
            Err(why) if reconnectable(&why) => {
                self.discard(generation);
                let (fresh, _) = self.client()?;
                match work(&fresh) {
                    Ok(answer) => {
                        self.succeeded();
                        Ok(answer)
                    }
                    Err(why) => {
                        self.failed();
                        Err(why)
                    }
                }
            }
            Err(why) => Err(why),
        }
    }

    fn client(&self) -> Result<(Client, u64)> {
        let mut connected = self.lock();
        if let Some(client) = &connected.client {
            return Ok((client.clone(), connected.generation));
        }
        if let Some(after) = connected.retry_after
            && Instant::now() < after
        {
            return Err(Error::new(
                Code::SearchUnavailable,
                "the Koru Store is not answering",
            ));
        }
        drop(connected);

        // Outside the lock: the handshake dials a pipe and waits.
        // **This build's own channel**, asked rather than written down: this
        // surface and the store it queries are built by one cargo invocation,
        // so a `--release` build moves both to `internal-release` together and
        // a hardcoded string would move neither.
        let client = match &self.root {
            // A host with an explicit root queries the store at that root, and
            // nothing else. Without this the overlay of a `--root` window
            // searches the *shared* store — every hit real, none of them in the
            // workspace on screen, which reads as a broken index rather than as
            // the wrong store.
            Some(root) => Client::open_at(koru_channel::segment(), root)?,
            None => Client::open(koru_channel::segment())?,
        };

        connected = self.lock();
        connected.client = Some(client.clone());
        connected.generation = connected.generation.wrapping_add(1);
        Ok((client, connected.generation))
    }

    /// Forget the client, unless somebody has already replaced it.
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

    /// Poisoning is not a reason to stop searching: nothing here is state a
    /// panic could have left half-written.
    fn lock(&self) -> std::sync::MutexGuard<'_, Connected> {
        self.connected.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Whether this failure means the connection is spent rather than the request
/// being wrong.
fn reconnectable(why: &Error) -> bool {
    matches!(why.code(), Code::StoreUnreachable)
}

/// Whether a failure should be shown as *search is unavailable* rather than as
/// an empty page.
///
/// **The distinction a person actually feels.** Without it a store that is down
/// paints "No results", which is indistinguishable from a query that matched
/// nothing — and the second is a fact about their writing while the first is a
/// fact about ours.
pub(super) fn unavailable(why: &Error) -> bool {
    matches!(
        why.code(),
        Code::SearchUnavailable
            | Code::StoreUnreachable
            | Code::StorageFailed
            | Code::Unauthenticated
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn err(code: Code) -> Error {
        Error::new(code, "for the test")
    }

    /// **A store that is down is not an empty page.** The whole reason the
    /// surface carries an unavailable state.
    #[test]
    fn a_transport_failure_reads_as_unavailable_and_a_missing_id_does_not() {
        assert!(unavailable(&err(Code::StoreUnreachable)));
        assert!(unavailable(&err(Code::StorageFailed)));
        assert!(unavailable(&err(Code::SearchUnavailable)));
        assert!(unavailable(&err(Code::Unauthenticated)));
        assert!(!unavailable(&err(Code::WorkspaceNotFound)));
        assert!(!unavailable(&err(Code::InvalidId)));
    }

    /// The one failure that means the Store process must be found again, and
    /// the ones that do not. An `InvalidId` re-discovering would dial the store
    /// again on every malformed request.
    #[test]
    fn only_a_dead_transport_is_reconnectable() {
        assert!(reconnectable(&err(Code::StoreUnreachable)));
        assert!(!reconnectable(&err(Code::Unauthenticated)));
        assert!(!reconnectable(&err(Code::InvalidId)));
        assert!(!reconnectable(&err(Code::SearchUnavailable)));
        // The store answered and its disk failed. Dialling it again reaches the
        // same store and the same disk.
        assert!(!reconnectable(&err(Code::StorageFailed)));
    }

    /// **A store that is down must not be dialled on every keystroke.**
    ///
    /// The back-off is armed by hand rather than by failing to connect. This
    /// test used to say "no store runs under `cargo test`" and dial for real —
    /// which is false on any machine with Koru open, where `Client::open`
    /// *succeeds* and the test failed on a correct handle. Arming the state is
    /// also the only way to assert what happens inside the retry window without
    /// a sleep.
    #[test]
    fn a_failing_store_backs_off_rather_than_dialling_every_time() {
        let store = StoreSearch::new(None);
        store.failed();
        let calls = AtomicUsize::new(0);

        // Inside the window: this fails in `client()`, before `work` is reached
        // and before anything is dialled.
        let first = store.call(|_| {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Error>(())
        });
        assert_eq!(
            first.err().map(|why| why.code()),
            Some(Code::SearchUnavailable)
        );
        assert_eq!(
            calls.load(Ordering::Relaxed),
            0,
            "the work ran without a client"
        );

        // Having failed, the handle must not be left claiming a client it does
        // not have.
        assert!(store.lock().client.is_none());
    }

    /// The wait doubles and stops at [`RETRY_MAX`], so a store that stays down
    /// costs one handshake every five seconds rather than one per keystroke.
    #[test]
    fn the_wait_doubles_up_to_the_cap() {
        let store = StoreSearch::new(None);
        store.failed();
        assert_eq!(store.lock().wait, Some(RETRY_FIRST));
        store.failed();
        assert_eq!(store.lock().wait, Some(RETRY_FIRST * 2));
        for _ in 0..10 {
            store.failed();
        }
        assert_eq!(store.lock().wait, Some(RETRY_MAX));

        // And one answer clears it: a store that comes back is dialled at once.
        store.succeeded();
        assert_eq!(store.lock().wait, None);
        assert_eq!(store.lock().retry_after, None);
    }

    /// A window left open over a weekend outlives its session, and the fix is
    /// to dial again rather than to keep presenting a dead token.
    #[test]
    fn an_expired_session_clears_the_client_it_was_holding() {
        let store = StoreSearch::new(None);
        {
            let mut connected = store.lock();
            connected.generation = 7;
        }
        store.discard(7);
        assert!(store.lock().client.is_none());

        // And a discard for a generation somebody has already replaced leaves
        // the newer client alone.
        {
            let mut connected = store.lock();
            connected.generation = 9;
        }
        store.discard(7);
        assert_eq!(store.lock().generation, 9);
    }

    /// The backoff grows and stops growing.
    #[test]
    fn the_backoff_doubles_to_a_ceiling() {
        let store = StoreSearch::new(None);
        store.failed();
        assert_eq!(store.lock().wait, Some(RETRY_FIRST));
        for _ in 0..10 {
            store.failed();
        }
        assert_eq!(store.lock().wait, Some(RETRY_MAX));

        // A success clears it, so one bad moment does not slow the next hour.
        store.succeeded();
        assert!(store.lock().wait.is_none());
        assert!(store.lock().retry_after.is_none());
    }
}
