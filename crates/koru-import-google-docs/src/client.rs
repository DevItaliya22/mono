// SPDX-License-Identifier: MPL-2.0

//! The Google OAuth client registration and endpoints.
//!
//! For an installed (desktop) application Google treats the client as a
//! *public* client: neither value is confidential, because anything shipped in
//! the binary can be extracted. PKCE is what authenticates the code exchange.
//! The **client secret is not an authentication factor here** — but Google's
//! token endpoint still expects the field to be present on a "Desktop app"
//! client, so it is shipped and sent, never treated as a secret.
//!
//! ## Where the registration comes from
//!
//! **Not from this file.** The registration is build configuration
//! (`docs/contributing/operations/RUNBOOK.md`, "What a release refuses to build without"): an official build bakes
//! Koru's own from `.env.production` through `koru_env::BAKED`, and a
//! development launch may set the same two variables in its environment —
//! `KORU_GOOGLE_DOCS_CLIENT_ID` and `KORU_GOOGLE_DOCS_CLIENT_SECRET`. A build
//! with neither has no registration, and the importer is offered disabled
//! with a reason rather than failing at Google's consent screen.
//! `CONTRIBUTING.md` says how to create your own.

use std::fmt;

/// One Google OAuth client of type *Desktop app*: the id and the secret from
/// the same client JSON download (APIs & Services → Credentials).
#[derive(Clone, PartialEq, Eq)]
pub struct Registration {
    pub client_id: String,
    /// Sent alongside PKCE because Google's token endpoint asks for it; never
    /// proof of identity, and not confidential for an installed app.
    pub client_secret: String,
}

impl Registration {
    /// Both halves, trimmed, or nothing. A half-configured client would only
    /// fail later at Google's token endpoint with a message about the wrong
    /// thing, so a missing half is the same answer as no registration.
    pub fn from_values(client_id: Option<&str>, client_secret: Option<&str>) -> Option<Self> {
        let client_id = client_id.map(str::trim).filter(|value| !value.is_empty())?;
        let client_secret = client_secret
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        Some(Self {
            client_id: client_id.to_owned(),
            client_secret: client_secret.to_owned(),
        })
    }

    /// This build's registration: the process environment first, then what
    /// the release baked in (`koru_env::value`).
    pub fn configured() -> Option<Self> {
        Self::from_values(
            koru_env::value(koru_env::name::KORU_GOOGLE_DOCS_CLIENT_ID).as_deref(),
            koru_env::value(koru_env::name::KORU_GOOGLE_DOCS_CLIENT_SECRET).as_deref(),
        )
    }
}

/// The id is public; the secret is not confidential either, but a log line is
/// still no place for it.
impl fmt::Debug for Registration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Registration")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .finish()
    }
}

/// Google's OAuth authorization endpoint.
pub const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
/// Google's OAuth token endpoint.
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// `drive.file` — access only to the files the user selects in the Picker.
/// The point of this scope over `drive.readonly`/`drive` is philosophical and
/// practical: non-sensitive (light verification) and "Koru can access only the
/// Google Docs you choose to import."
pub const DRIVE_FILE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";

/// The Google Docs MIME type, used to filter the Picker to documents (not
/// Sheets, Slides or folders).
pub const GOOGLE_DOCS_MIME: &str = "application/vnd.google-apps.document";

/// The loopback path Google redirects to after consent. The port is chosen at
/// runtime (`127.0.0.1:0`), so this is only the path component; Google allows
/// any loopback port for *Desktop app* clients.
pub const REDIRECT_PATH: &str = "/oauth/google/callback";

#[cfg(test)]
mod tests;
