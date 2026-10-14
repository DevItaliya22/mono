// SPDX-License-Identifier: MPL-2.0

//! Live spike for the Google Docs integration: sign in (which opens the Google
//! Picker in the browser), export each selected document as Markdown and write
//! it to a temp dir so you can read exactly what `files.export(text/markdown)`
//! returns.
//!
//! This is deliberately not a test — it needs a real Google account and a real
//! browser. Run it with:
//!
//! ```sh
//! cargo run -p koru-import-google-docs --example spike
//! ```
//!
//! The two questions it answers (the plan's phase 1): does the one-pick OAuth
//! flow return `picked_file_ids`, and what does the export do with images,
//! tables, lists and checklists. Set `KORU_GOOGLE_DEBUG=1` for the stage trace.
//! It needs a Google OAuth registration in `KORU_GOOGLE_DOCS_CLIENT_ID` and
//! `KORU_GOOGLE_DOCS_CLIENT_SECRET` (CONTRIBUTING.md).

use koru_import_google_docs::client::Registration;
use koru_import_google_docs::drive::DriveClient;
use koru_import_google_docs::oauth::{AuthConfig, GoogleAuth};
use koru_import_google_docs::tokens::SessionTokenStore;

/// This spike's own opener.
///
/// The product's is `koru_window::open_uri`, which this crate cannot reach —
/// `koru-workspace-host` depends on *it*, not the other way round — and linking the
/// window crate as a dev-dependency would pull winit and vello into a manual
/// spike. So the three lines are written out here on purpose, in an example
/// nothing ships, rather than as a second production door.
fn open_in_browser(url: &str) -> std::io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer.exe"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(url)
        .spawn()
        .map(|_| ())
}

fn main() {
    let Some(registration) = Registration::configured() else {
        eprintln!(
            "Set KORU_GOOGLE_DOCS_CLIENT_ID and KORU_GOOGLE_DOCS_CLIENT_SECRET to a Desktop app              OAuth client first (CONTRIBUTING.md, \"Google Docs import\")."
        );
        std::process::exit(2);
    };

    let mut auth = GoogleAuth::new(AuthConfig {
        registration,
        tokens: std::sync::Arc::new(SessionTokenStore::default()),
        open: open_in_browser,
    });

    let picked = match auth.authorize_with_picker(&|| false) {
        Ok(picked) => picked,
        Err(error) => {
            eprintln!("could not connect to Google: {error}");
            std::process::exit(1);
        }
    };
    if picked.picked_file_ids.is_empty() {
        println!("no documents selected");
        return;
    }

    let drive = DriveClient::new(picked.access_token).expect("drive client");
    let dir = std::env::temp_dir().join("koru-google-spike");
    std::fs::create_dir_all(&dir).expect("create spike dir");
    for id in picked.picked_file_ids {
        let title = drive.doc_title(&id).unwrap_or_else(|_| "<no title>".into());
        let markdown = drive.export_markdown(&id).unwrap_or_else(|error| {
            eprintln!("export of {title} failed: {error}");
            std::process::exit(1);
        });
        let path = dir.join(format!("{id}.md"));
        std::fs::write(&path, &markdown).expect("write spike output");
        println!("{title}: {} bytes -> {}", markdown.len(), path.display());
    }
}
