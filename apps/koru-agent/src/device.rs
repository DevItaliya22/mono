// SPDX-License-Identifier: MPL-2.0

//! Which device this is.
//!
//! A stub, deliberately: an id, a name and a date. **No keypair, no peers, no
//! iroh** — those are agent phase 2, and inventing their file format now would
//! be deciding a protocol from the wrong end.
//!
//! ## The id is minted once and never derived
//!
//! A v4 uuid, written on first run and read forever after. Not from the machine
//! name, not from a MAC address, not from the root path — the rule in
//! [DATAMODEL](../../../docs/architecture/DATAMODEL.md) §Identity and
//! [ADR-39](../../../docs/decisions/ADR-026-050.md#adr-39-identity--identity-is-client-minted-document-scoped-and-cloud-ready).
//! A derived id looks stable until somebody renames a laptop or restores a
//! backup onto a second one, and then two devices are the same device.
//!
//! **It is not an account id and not a person.** It names this installation, so
//! that when phase 2 gives devices trust between them there is something for
//! trust to be *about*.
//!
//! ## A file that will not parse is replaced, not fatal
//!
//! The opposite of the workspace rule, and on purpose. Losing a device id costs
//! this installation its name in a list; losing a document costs somebody their
//! writing. So a truncated or hand-edited `device.json` is rewritten with a
//! fresh id and a line in the log, rather than stopping the agent — which would
//! take the hotkey, the overlay and the store's supervisor down with it.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// A v4 uuid in its storage spelling. See the module note.
    pub id: String,
    /// What a person would call this machine. A label, never an identity — it
    /// is allowed to be wrong, and renaming it changes nothing.
    pub name: String,
    /// Epoch milliseconds. A number, so no calendar or locale is baked in — the
    /// rule the store's `Timestamp` follows.
    pub created_at_millis: i64,
}

impl Device {
    fn mint(now_millis: i64) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: host_name(),
            created_at_millis: now_millis,
        }
    }

    /// Read the device, or mint and write one.
    ///
    /// `now_millis` is passed in rather than read here, so a test can assert on
    /// the stamp without a clock — the same rule
    /// `SessionCredential::has_expired` follows.
    pub fn open(path: &Path, now_millis: i64) -> std::io::Result<Self> {
        if let Ok(bytes) = std::fs::read(path)
            && let Ok(device) = serde_json::from_slice::<Self>(&bytes)
            && !device.id.is_empty()
        {
            return Ok(device);
        }
        let device = Self::mint(now_millis);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_vec_pretty(&device).map_err(std::io::Error::other)?;
        std::fs::write(path, body)?;
        Ok(device)
    }
}

/// What to call this machine, best effort.
///
/// `COMPUTERNAME` on Windows, `HOSTNAME` elsewhere, and a plain fallback when
/// neither is set. **Never fatal**: this is a label, and an agent that would not
/// start because it could not read an environment variable would be trading
/// something that matters for something that does not.
fn host_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "this device".to_string())
}

#[cfg(test)]
mod tests;
