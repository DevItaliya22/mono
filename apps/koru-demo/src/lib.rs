// SPDX-License-Identifier: MPL-2.0

//! **The accessibility acceptance runner.**
//!
//! A separate process selects one root-owned Koru window through UI Automation,
//! resolves unique locators, and runs compiled workflows with visible native
//! input and state assertions. The exact-target mechanism lives in the shared
//! `koru-windows-input` crate; this crate owns traversal, supervision and cases.
//! A revoked per-call permit prevents an abandoned provider thread from later
//! delivering input. UI Automation stays on its owning worker thread.
//!
//! ## Its logs are developer text
//!
//! Nothing this crate prints is shown to a person using Koru, so none of it
//! goes through `koru-i18n` — the same rule `{ code, message }` APIs follow in
//! [PRINCIPLES.md](../../../guide/contributing/PRINCIPLES.md). What it *may* print is
//! constrained much harder than language: §7.4 is absolute, so **by default no accessible
//! name, window title or document text reaches a report; explicit scoped snapshots may include synthetic fixture text**. A candidate in
//! an ambiguity failure is identified by its locator, its control type, its
//! state and its rectangle — never by what it is called. See
//! [`inspect::ElementFacts`], which has no name field at all, on purpose.

pub mod cli;
pub mod fixtures;
pub mod inspect;
pub mod instance;
pub mod locator;
pub mod prepare;
pub mod repeat;
pub mod report;
pub mod select;
pub mod supervise;
pub mod viewport;
pub mod wait;

pub mod cases;
#[cfg(windows)]
pub mod console;
pub mod execute;
pub mod input;
pub mod input_owner;
pub mod recording;
#[cfg(windows)]
pub mod uia;
