// SPDX-License-Identifier: MPL-2.0

//! Account session and Profile-scoped enrollment state machines with injected host ports.
//! Hosts own UI, IPC framing, discovery, sign-in delivery and platform secret storage.
pub mod account;
pub mod cloud;
pub mod delivery;
pub mod enrollment;
pub mod identity;
pub mod ports;
pub mod regional_bootstrap;
