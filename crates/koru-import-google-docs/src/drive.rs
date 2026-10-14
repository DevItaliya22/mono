// SPDX-License-Identifier: MPL-2.0

//! The Drive v3 REST surface the importer needs: a document's title and its
//! Markdown export. Two endpoints, so there is no generated `google-drive3`
//! dependency — the whole client is smaller than the generated crate's
//! `files` module would be.

use serde::Deserialize;

const DRIVE_BASE: &str = "https://www.googleapis.com/drive/v3";

/// A Drive client scoped to one bearer token. Built per import so a refreshed
/// token never has to propagate through callers mid-flight.
pub struct DriveClient {
    http: reqwest::blocking::Client,
    token: String,
}

#[derive(Deserialize)]
struct FileMeta {
    name: String,
}

impl DriveClient {
    pub fn new(token: String) -> Result<Self, String> {
        let http = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self { http, token })
    }

    /// The document's title, from `files.get`. The Picker hands back ids; the
    /// title is authoritative from Drive rather than guessed from the export.
    pub fn doc_title(&self, file_id: &str) -> Result<String, String> {
        let meta: FileMeta = self
            .http
            .get(format!("{DRIVE_BASE}/files/{file_id}"))
            .query(&[("fields", "name")])
            .bearer_auth(&self.token)
            .send()
            .map_err(|error| format!("could not reach Google Drive: {error}"))?
            .error_for_status()
            .map_err(|error| format!("Google Drive refused the metadata request: {error}"))?
            .json()
            .map_err(|error| format!("Google Drive sent unreadable metadata: {error}"))?;
        Ok(meta.name)
    }

    /// The document as Markdown, from `files.export`. This is the whole point
    /// of the Markdown-first path: Google's own export, not a re-implementation
    /// of the Docs format.
    pub fn export_markdown(&self, file_id: &str) -> Result<String, String> {
        self.http
            .get(format!("{DRIVE_BASE}/files/{file_id}/export"))
            .query(&[("mimeType", "text/markdown")])
            .bearer_auth(&self.token)
            .send()
            .map_err(|error| format!("could not reach Google Drive: {error}"))?
            .error_for_status()
            .map_err(|error| format!("Google Drive refused the export: {error}"))?
            .text()
            .map_err(|error| format!("Google Drive sent unreadable Markdown: {error}"))
    }
}
