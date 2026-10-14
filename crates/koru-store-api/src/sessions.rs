// SPDX-License-Identifier: MPL-2.0

//! Credentials minted over the rendezvous, held for as long as the store runs.
//!
//! ## Why these are in memory and not in `api_clients`
//!
//! The plan said to record them in that table with an expiry column. Building
//! it made the opposite obvious: **a session must not outlive the process that
//! minted it.** The whole point of replacing `store.secret` is that no
//! credential survives a restart, and a row in `catalog.db` survives everything
//! — a restart, a reboot, a backup, a copied directory.
//!
//! It is also the wrong lifetime in the other direction. Every connection mints
//! one, so persisting would be a disk write per client per launch, and rows
//! that accumulate forever against a store that has already forgotten them.
//!
//! `api_clients` keeps the job it already had: **registered third parties**,
//! long-lived, revocable, scoped, deliberately durable. Two different things
//! that were nearly given one table.
//!
//! ## A restart invalidating every session costs nothing
//!
//! It sounds severe and it is free: the store's port is ephemeral, so a client
//! has to rediscover after a restart anyway. The session it gets back is minted
//! by the same handshake that tells it the new port.
//!
//! ## What is stored is a hash
//!
//! The token is handed out once and never held. A memory dump of the store —
//! or a debug route that got careless — yields hashes, not keys. The same
//! BLAKE3 the persistent credentials use, so there is one function rather than
//! two that must agree.

use std::collections::HashMap;
use std::sync::Mutex;

use koru_store_core::model::Scope;

use crate::auth::Caller;

/// How long a minted session lasts.
///
/// Long enough that an editing session never trips over it, short enough that a
/// token which somehow escaped is not useful tomorrow. It is renewed by
/// reconnecting, which a client does anyway whenever the store restarts.
pub const DEFAULT_SESSION_TTL_MILLIS: i64 = 12 * 60 * 60 * 1000;
const MAX_SESSION_TTL_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// The lifetime the Store grants to credentials minted over its local
/// rendezvous. Kept as milliseconds because the wire contract carries an epoch
/// millisecond expiry too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionTtl(i64);

impl SessionTtl {
    pub const DEFAULT: Self = Self(DEFAULT_SESSION_TTL_MILLIS);

    pub const fn millis(self) -> i64 {
        self.0
    }

    /// Parse a compact duration spelling such as `5m`,
    /// `10m`, `1h`, or a seconds value useful to deterministic tests.
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        let text = text.trim();
        if text.len() < 2 {
            return Err("expected a positive duration ending in s, m, or h");
        }
        let unit = text
            .chars()
            .next_back()
            .expect("duration length was checked above");
        let number = &text[..text.len() - unit.len_utf8()];
        let multiplier = match unit {
            's' => 1_000_i64,
            'm' => 60_000_i64,
            'h' => 3_600_000_i64,
            _ => return Err("expected a positive duration ending in s, m, or h"),
        };
        let value = number
            .parse::<i64>()
            .map_err(|_| "expected a whole positive duration")?;
        let millis = value
            .checked_mul(multiplier)
            .ok_or("duration is too large")?;
        if !(1_000..=MAX_SESSION_TTL_MILLIS).contains(&millis) {
            return Err("duration must be between 1s and 24h");
        }
        Ok(Self(millis))
    }
}

struct Session {
    scopes: Vec<Scope>,
    label: String,
    expires_at_millis: i64,
}

/// Every live session, by token hash.
pub struct Sessions {
    live: Mutex<HashMap<Vec<u8>, Session>>,
    ttl: SessionTtl,
}

impl Default for Sessions {
    fn default() -> Self {
        Self::new()
    }
}

impl Sessions {
    pub fn new() -> Self {
        Self::with_ttl(SessionTtl::DEFAULT)
    }

    pub fn with_ttl(ttl: SessionTtl) -> Self {
        Self {
            live: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    /// Mint a credential for a client that has just introduced itself.
    ///
    /// Returns the token — the only time it exists outside the client — and the
    /// instant it stops working.
    ///
    /// 244 bits of randomness from two v4 UUIDs, which come from `getrandom`, a
    /// CSPRNG. Two rather than one, and written down here so nobody tidies it
    /// into a single UUID and halves it.
    pub fn mint(&self, label: &str, scopes: Vec<Scope>, now_millis: i64) -> (String, i64) {
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let expires_at_millis = now_millis + self.ttl.millis();
        let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        // Swept here rather than on a timer: the only thing that grows this map
        // is minting, so the only moment it can need pruning is this one.
        // Nothing walks it in the background, which is the rule the app's
        // `about_to_wait` loop learned the expensive way.
        live.retain(|_, s| s.expires_at_millis > now_millis);
        live.insert(
            crate::state::token_hash(&token),
            Session {
                scopes,
                label: label.to_string(),
                expires_at_millis,
            },
        );
        (token, expires_at_millis)
    }

    /// Resolve a presented token, if it is a live session.
    ///
    /// **An expired session resolves to `None` and is indistinguishable from an
    /// unknown one.** Saying "that was valid until Tuesday" would confirm the
    /// token was real, which is a better answer than an attacker deserves.
    pub fn resolve(&self, presented: &str, now_millis: i64) -> Option<Caller> {
        let hash = crate::state::token_hash(presented);
        let live = self.live.lock().ok()?;
        let session = live.get(&hash)?;
        if session.expires_at_millis <= now_millis {
            return None;
        }
        Some(Caller {
            scopes: session.scopes.clone(),
            label: session.label.clone(),
        })
    }

    /// How many sessions are live. For the status route, and for tests.
    pub fn count(&self, now_millis: i64) -> usize {
        self.live
            .lock()
            .map(|live| {
                live.values()
                    .filter(|s| s.expires_at_millis > now_millis)
                    .count()
            })
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests;
