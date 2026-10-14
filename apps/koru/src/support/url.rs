// SPDX-License-Identifier: MPL-2.0

//! Where the support page is, and what the surface may navigate to.
//!
//! `KORU_SUPPORT_URL` is an **origin** — scheme and host, no path — and the
//! app composes `{origin}/{segment}/support/desktop` from it and the person's
//! shown language. Three vocabularies meet here: Koru's own tags (`pt-BR`,
//! `zh-Hans`), the site's URL segments (`pt`, `zh`), and Intercom's language
//! codes, which the page derives from the segment on its side. The shared
//! website module owns the first mapping; the site owns the second. Tests pin
//! all eight rows.

use std::path::PathBuf;

use koru_i18n::Lang;
use koru_webview::Navigation;

/// The site's URL segment for a Koru language.
pub fn segment_for(lang: Lang) -> &'static str {
    crate::website::segment_for(lang)
}

/// Where the support page lives when nothing says otherwise: the marketing
/// site, which serves `/[locale]/support/desktop`.
#[cfg(test)]
pub const DEFAULT_ORIGIN: &str = crate::website::DEFAULT_ORIGIN;

/// The support origin: `KORU_SUPPORT_URL` when it is set to something
/// usable (a local marketing site during development), the site otherwise.
/// Contact support is always offered — the maintainers lifted the spike's switch on
/// 2026-09-05 — so this never says "no". A trailing slash is tolerated; a
/// path is not the contract and is cut.
pub fn origin() -> String {
    crate::website::origin()
}

/// The testable half of [`origin`].
#[cfg(test)]
pub fn origin_from(value: Option<&str>) -> Option<String> {
    crate::website::origin_from(value)
}

/// The page for one language, embedded and in the browser alike.
pub fn page_url(origin: &str, lang: Lang) -> String {
    format!("{origin}/{}/support/desktop", segment_for(lang))
}

/// The top-level navigation policy: the support origin and Intercom's own
/// origins stay inside; any other web URL goes to the browser; anything that
/// is not a web URL is refused.
pub fn navigation(origin: &str, url: &str) -> Navigation {
    if same_origin(origin, url) || is_intercom(url) {
        Navigation::Allow
    } else if url.starts_with("https://") || url.starts_with("http://") {
        Navigation::External
    } else {
        Navigation::Refuse
    }
}

/// Whether an IPC message's sender is the support page itself. Intercom's
/// frames post nothing to us, and nothing they could post is believed.
pub fn same_origin_for_ipc(origin: &str, source: &str) -> bool {
    same_origin(origin, source)
}

fn same_origin(origin: &str, url: &str) -> bool {
    url == origin
        || url
            .strip_prefix(origin)
            .is_some_and(|rest| rest.starts_with(['/', '?', '#']))
}

fn is_intercom(url: &str) -> bool {
    let host = koru_webview::host_of(url);
    url.starts_with("https://")
        && (host == "intercom.io"
            || host.ends_with(".intercom.io")
            || host.ends_with(".intercomcdn.com")
            || host.ends_with(".intercom-attachments-1.com")
            || host.ends_with(".intercom-messenger.com"))
}

/// Where the surface keeps cookies and storage: under the store root when
/// there is one (`--root`, so a dev root is isolated and deleting it removes
/// every browser byte — D11), otherwise beside Koru's other local data.
pub fn data_directory() -> PathBuf {
    data_directory_from(
        koru_workspace_host::cli::store_root_from_args(),
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
    )
}

/// The testable half of [`data_directory`].
pub fn data_directory_from(
    store_root: Option<PathBuf>,
    local_app_data: Option<PathBuf>,
) -> PathBuf {
    match store_root {
        Some(root) => root.join("webview").join("support"),
        None => local_app_data
            .unwrap_or_else(std::env::temp_dir)
            .join("Koru")
            .join("webview")
            .join("support"),
    }
}
