// SPDX-License-Identifier: MPL-2.0

//! Windows UI Automation identity and exact-target input mechanism.
//!
//! Agent owns consent and restoration policy; koru-demo owns selection and
//! waits. This crate owns neither application state nor window lifecycle.
pub mod gesture;
pub mod permit;
pub use permit::DeliveryPermit;

#[cfg(windows)]
pub mod workflow;

#[cfg(windows)]
mod automation;
#[cfg(windows)]
mod types;
#[cfg(windows)]
pub mod windows;

#[cfg(windows)]
pub use automation::AutomationClient;
#[cfg(windows)]
pub use types::{DirectInsertion, Refusal, TargetDisplay, TargetState};
#[cfg(windows)]
pub use windows::{OrdinaryDesktop, WindowsInsertBackend, WindowsRestoreHint, WindowsTarget};
#[cfg(windows)]
pub mod dpi;
