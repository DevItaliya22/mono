// SPDX-License-Identifier: MPL-2.0

//! Google Docs import, behind the same `koru-import` contract as every other
//! format.
//!
//! The pipeline is deliberately thin because Google exports Markdown natively:
//! authenticate (PKCE + loopback), select documents in the browser Picker
//! (`drive.file`), export each as Markdown, and hand the text to
//! `koru-import-markdown`. Nothing here parses the Google Docs format.
//!
//! [`GoogleDocsImporter::inspect`] is the interactive half — OAuth and the
//! Picker — and [`PreparedGoogleDocs::build`] is the download-and-convert half.
//! Both run on the import worker thread; the window loop never touches the
//! browser or the network.
//!
//! **The browser opener is injected, and this crate opens nothing by itself.**
//! [`GoogleDocsImporter::new`] takes the host's URI opener and optional
//! Agent-acknowledged credential scope, and `koru-workspace-host` passes
//! `koru_window::open_uri` — the one door this workspace has for handing a URI
//! to the desktop. This crate deliberately links no browser crate of its own:
//! `webbrowser`'s Windows backend resolves the default *browser*'s command line
//! and runs it with the string as an argument, which is a different thing from
//! asking the shell, and a second door is a door the `no-browser` test guard
//! cannot see.

use std::path::PathBuf;
use std::sync::Arc;

use koru_import::{
    ImportContext, ImportError, ImportErrorCode, ImportOptions, ImportPreview, ImportProduct,
    ImportProgress, ImportSource, ImportStage, ImportWarning, ImportedContent, ImportedDocument,
    Importer, ImporterDescriptor, PreparedImport, PreviewNode, SourceRequest,
};

pub mod client;
pub mod drive;
pub mod oauth;
pub mod tokens;

use client::Registration;
use oauth::{AuthConfig, GoogleAuth};
use tokens::{ScopedTokenStore, SessionTokenStore, TokenStore};

const GOOGLE_SERVICE_ORIGIN: &str = "https://accounts.google.com";

pub static DESCRIPTOR: ImporterDescriptor = ImporterDescriptor {
    id: "google-docs",
    name: "Google Docs",
    source: SourceRequest::Google,
    menu_name: Some("Google Docs"),
    // No file dialog: the importer authenticates and opens the Google Picker
    // itself, on the worker thread.
    picker_title: "Import from Google Docs",
    claims_zip: None,
    roots_import: false,
    dedups_known_files: false,
};

/// Whether this build carries a Google OAuth registration for the importer.
///
/// `false` in a community build that supplied none: the shell then offers the
/// importer disabled, with a reason, and [`GoogleDocsImporter::inspect`]
/// refuses with [`ImportErrorCode::NotConfigured`] before opening anything.
pub fn is_configured() -> bool {
    Registration::configured().is_some()
}

/// The Google Docs importer, holding the things it cannot decide for itself:
/// how this process opens a URI, whether this installation has an
/// Agent-acknowledged durable credential scope, and which OAuth client this
/// build was configured with.
///
/// A host builds it with its platform opener — `koru_window::open_uri` in the
/// desktop shell — so nothing here links a browser crate and a test can capture
/// the consent URL instead of launching one. Without the credential context,
/// refresh state remains in this process only.
pub struct GoogleDocsImporter {
    open: fn(&str) -> std::io::Result<()>,
    tokens: Arc<dyn TokenStore>,
    registration: Option<Registration>,
}

impl GoogleDocsImporter {
    /// The importer with this build's registration ([`Registration::configured`]).
    pub fn new(
        open: fn(&str) -> std::io::Result<()>,
        credentials: Option<koru_oauth_credentials::CredentialContext>,
    ) -> Self {
        Self::with_registration(open, credentials, Registration::configured())
    }

    /// The importer with an explicit registration, or none.
    pub fn with_registration(
        open: fn(&str) -> std::io::Result<()>,
        credentials: Option<koru_oauth_credentials::CredentialContext>,
        registration: Option<Registration>,
    ) -> Self {
        let tokens: Arc<dyn TokenStore> = match credentials {
            Some(context) => Arc::new(ScopedTokenStore::os(
                context
                    .entry(GOOGLE_SERVICE_ORIGIN)
                    .expect("Google's static service origin is valid"),
            )),
            None => process_session_tokens(),
        };
        Self {
            open,
            tokens,
            registration,
        }
    }
}

fn process_session_tokens() -> Arc<dyn TokenStore> {
    static TOKENS: std::sync::OnceLock<Arc<SessionTokenStore>> = std::sync::OnceLock::new();
    let tokens = Arc::clone(TOKENS.get_or_init(|| Arc::new(SessionTokenStore::default())));
    tokens
}

/// A document the user picked, with its Drive title already fetched.
struct SelectedDoc {
    id: String,
    title: String,
}

impl Importer for GoogleDocsImporter {
    fn descriptor(&self) -> &'static ImporterDescriptor {
        &DESCRIPTOR
    }

    fn inspect(
        &self,
        source: ImportSource,
        progress: &dyn ImportProgress,
    ) -> Result<Box<dyn PreparedImport>, ImportError> {
        if !matches!(source, ImportSource::Google) {
            return Err(ImportError::new(
                "Google Docs import has no file source; it authenticates and picks on its own",
            ));
        }
        // No registration, no browser: a community build without its own
        // Google client says so here rather than at Google's consent screen.
        let Some(registration) = self.registration.clone() else {
            debug("oauth", "no Google OAuth registration in this build");
            return Err(ImportError::coded(ImportErrorCode::NotConfigured));
        };
        let mut auth = GoogleAuth::new(AuthConfig {
            registration,
            tokens: Arc::clone(&self.tokens),
            open: self.open,
        });

        debug("oauth", "signing in and opening the Google Picker");
        // The consent trip can take minutes — a password, a second factor, a
        // firewall prompt — so the wait watches the same cancellation flag the
        // rest of the import does rather than pinning this worker thread until
        // the five-minute timeout.
        let picked = match auth.authorize_with_picker(&|| progress.cancelled()) {
            Ok(picked) => picked,
            Err(error) => {
                if progress.cancelled() {
                    debug("oauth", "the import was cancelled during sign-in");
                    return Err(ImportError::cancelled());
                }
                return Err(ImportError::new(format!(
                    "could not connect to Google: {error}"
                )));
            }
        };
        if picked.picked_file_ids.is_empty() {
            debug("picker", "the user closed the Picker without selecting");
            progress.cancel();
            return Err(ImportError::cancelled());
        }

        let drive = drive::DriveClient::new(picked.access_token).map_err(ImportError::new)?;
        let total = picked.picked_file_ids.len();
        let mut docs = Vec::with_capacity(total);
        let mut warnings = Vec::new();
        for (index, id) in picked.picked_file_ids.into_iter().enumerate() {
            if progress.cancelled() {
                return Err(ImportError::cancelled());
            }
            progress.update(ImportStage::Inspecting, index, Some(total));
            match drive.doc_title(&id) {
                Ok(title) => docs.push(SelectedDoc { id, title }),
                // One unreadable document — a shared file the token cannot
                // reach, or one deleted after the Picker opened — must not sink
                // the whole batch. Import the rest and report this one.
                Err(error) => {
                    debug("skipped", &format!("a document could not be read: {error}"));
                    warnings.push(format!(
                        "Skipped a document that could not be read: {error}"
                    ));
                }
            }
        }
        if docs.is_empty() {
            let detail = warnings
                .first()
                .map(String::as_str)
                .unwrap_or("none of the selected documents could be read");
            return Err(ImportError::new(format!(
                "none of the selected Google Docs could be read: {detail}"
            )));
        }
        progress.update(ImportStage::Inspecting, total, Some(total));

        let documents = docs
            .iter()
            .map(|doc| PreviewNode {
                title: doc.title.clone(),
                children: Vec::new(),
                nodes: 0,
                trashed: false,
            })
            .collect();
        let preview = ImportPreview {
            native_workspace_id: None,
            workspace_name: "Google Docs".into(),
            document_count: docs.len(),
            node_count: 0,
            image_count: 0,
            documents,
            warnings,
            identity_modes: Vec::new(),
        };
        debug(
            "inspected",
            &format!("documents={}", preview.document_count),
        );
        Ok(Box::new(PreparedGoogleDocs {
            auth,
            docs,
            preview,
        }))
    }
}

struct PreparedGoogleDocs {
    auth: GoogleAuth,
    docs: Vec<SelectedDoc>,
    preview: ImportPreview,
}

impl PreparedImport for PreparedGoogleDocs {
    fn preview(&self) -> &ImportPreview {
        &self.preview
    }

    fn build(
        mut self: Box<Self>,
        _options: ImportOptions,
        context: &ImportContext<'_>,
        progress: &dyn ImportProgress,
    ) -> Result<ImportProduct, ImportError> {
        let target = context
            .target_workspace
            .ok_or_else(|| ImportError::new("Google Docs import needs an open workspace"))?;
        let workspace = koru_core::WorkspaceId::from_uuid(target.as_uuid());

        // A fresh token: enough may have passed since `inspect` that the
        // previous one expired, and `access_token` refreshes transparently.
        let token = self
            .auth
            .access_token()
            .map_err(|error| ImportError::new(format!("could not connect to Google: {error}")))?;
        let drive = drive::DriveClient::new(token).map_err(ImportError::new)?;

        let total = self.docs.len();
        let mut documents = Vec::with_capacity(total);
        let mut warnings = Vec::new();
        for (index, doc) in self.docs.into_iter().enumerate() {
            if progress.cancelled() {
                return Err(ImportError::cancelled());
            }
            progress.update(ImportStage::Converting, index, Some(total));
            match drive.export_markdown(&doc.id) {
                Ok(markdown) => {
                    debug(
                        "exported",
                        &format!("title={:?} bytes={}", doc.title, markdown.len()),
                    );
                    match koru_import_markdown::build_document_from_text(
                        workspace,
                        &doc.title,
                        &format!("google-{}.md", doc.id),
                        &markdown,
                        context,
                        progress,
                    ) {
                        Ok((title, snapshot, mut file_warnings)) => {
                            warnings.append(&mut file_warnings);
                            documents.push(ImportedDocument {
                                id: koru_store_core::ids::DocumentId::new(),
                                source: PathBuf::from(format!("google://docs/{}", doc.id)),
                                title,
                                kind: koru_core::explore::KIND_DOCUMENT.to_owned(),
                                mime: None,
                                parent: None,
                                trashed: false,
                                trashed_at: None,
                                snapshot,
                            });
                        }
                        Err(error) => warnings.push(ImportWarning {
                            source: PathBuf::from(format!("google://docs/{}", doc.id)),
                            code: None,
                            arguments: Vec::new(),
                            message: error.to_string(),
                        }),
                    }
                }
                Err(error) => warnings.push(ImportWarning {
                    source: PathBuf::from(format!("google://docs/{}", doc.id)),
                    code: None,
                    arguments: Vec::new(),
                    message: error,
                }),
            }
        }
        progress.update(ImportStage::BuildingBundle, total, Some(total));
        if documents.is_empty() {
            return Err(ImportError::new(
                "none of the selected Google Docs could be imported",
            ));
        }
        Ok(ImportProduct::Documents(ImportedContent {
            documents,
            warnings,
        }))
    }
}

/// Privacy-safe, opt-in diagnostics. Ids, counts and stages — never a token or
/// document text. Free when unset.
///
/// `pub(crate)` because [`tokens`] reports keychain outcomes through the same
/// flag: a credential store that refuses is the thing a person needs to see, and
/// a second `KORU_*` variable for one module would be a flag nobody guesses.
pub(crate) fn debug(stage: &str, detail: &str) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let enabled = *ENABLED.get_or_init(|| std::env::var_os("KORU_GOOGLE_DEBUG").is_some());
    if enabled {
        println!("[Koru][google] stage={stage} {detail}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_open(_: &str) -> std::io::Result<()> {
        Ok(())
    }

    fn opener_that_must_not_run(url: &str) -> std::io::Result<()> {
        panic!("an unconfigured importer opened {url}");
    }

    #[test]
    fn an_unconfigured_importer_refuses_before_opening_anything() {
        let importer = GoogleDocsImporter::with_registration(opener_that_must_not_run, None, None);
        let error = match importer.inspect(ImportSource::Google, &koru_import::NoProgress) {
            Ok(_) => panic!("an importer with no registration must not proceed"),
            Err(error) => error,
        };
        assert_eq!(error.code(), Some(ImportErrorCode::NotConfigured));
    }

    #[test]
    fn no_agent_registries_share_one_process_session_store() {
        let first = GoogleDocsImporter::new(fake_open, None);
        let second = GoogleDocsImporter::new(fake_open, None);
        assert!(Arc::ptr_eq(&first.tokens, &second.tokens));
        first
            .tokens
            .store(koru_oauth_credentials::SecretString::new("process-refresh").unwrap())
            .unwrap();
        assert_eq!(
            second.tokens.load().unwrap().unwrap().expose(),
            "process-refresh"
        );
        second.tokens.clear().unwrap();
    }
}
