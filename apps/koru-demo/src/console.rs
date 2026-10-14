// SPDX-License-Identifier: MPL-2.0

//! Ctrl+C cancels the supervisor before process teardown, permitting owned key release.
use crate::supervise::CancelToken;
use std::sync::OnceLock;
use windows::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler};
use windows_core::BOOL;

static TOKEN: OnceLock<CancelToken> = OnceLock::new();
unsafe extern "system" fn handle(event: u32) -> BOOL {
    if matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT) {
        if let Some(token) = TOKEN.get() {
            token.cancel();
        }
        BOOL(1)
    } else {
        BOOL(0)
    }
}
pub fn install(token: &CancelToken) -> Result<(), &'static str> {
    TOKEN
        .set(token.clone())
        .map_err(|_| "cancel-handler-already-installed")?;
    unsafe { SetConsoleCtrlHandler(Some(handle), true) }.map_err(|_| "cancel-handler-unavailable")
}
