// SPDX-License-Identifier: MPL-2.0

//! Contact support: the Intercom Messenger in a window of its own.
//!
//! The domain, beside `feedback/`: what the page is, what the surface may
//! navigate to, when the chat is ready, what closing means. The mechanism
//! lives below — `koru-webview` hosts the page, `koru-workspace-host`'s
//! `native_surface` places it — and neither knows this module exists.
//!
//! Product decisions this implements are in ADR-276 (docs/decisions): one
//! singleton window that contains only the chat, never a tab; anonymous
//! until Koru Identity exists; *Open support in browser* when the chat
//! cannot load; End chat keeps the visitor cookie; a reply to "hide until
//! support replies" opens the window itself.

pub mod fab;
pub mod session;
pub mod surface;
pub mod url;
mod view;

#[cfg(test)]
mod tests;

pub use fab::FabView;
pub use session::{Session, SupportSession};
pub use view::SupportView;

/// `KORU_SUPPORT_DEBUG`: the dialog choice, boot requested, ready — routine
/// lines. The unconditional ones (`webview_unavailable`, `load_failed`,
/// `chat_ended`, `fallback_browser`) go through `tracing` at their sites.
pub(crate) mod debug {
    use std::sync::OnceLock;

    pub fn enabled() -> bool {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| koru_env::switch(koru_env::name::KORU_SUPPORT_DEBUG))
    }

    pub fn log(line: &str) {
        if enabled() {
            eprintln!("[Koru][support] {line}");
        }
    }
}
