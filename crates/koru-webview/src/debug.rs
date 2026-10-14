// SPDX-License-Identifier: MPL-2.0

//! `KORU_WEBVIEW_DEBUG`: what the surface is doing, without the URL.
//!
//! Every line is a stage, a decision and a host — never a full URL (a signed
//! attachment link is a URL), never an IPC body (a message is a body). Free
//! when off: one `OnceLock<bool>` and a branch.

use std::sync::OnceLock;

pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| koru_env::switch(koru_env::name::KORU_WEBVIEW_DEBUG))
}

pub(crate) fn log(stage: &str, decision: &str, host: &str) {
    if enabled() {
        if host.is_empty() {
            eprintln!("[Koru][webview] {stage}: {decision}");
        } else {
            eprintln!("[Koru][webview] {stage}: {decision} ({host})");
        }
    }
}
