// SPDX-License-Identifier: MPL-2.0

//! Turning input into intent.
//!
//! [`action`] names what can be done; [`keymap`] says which chord does it. The
//! split is what makes remapping possible later without touching the editor —
//! and, today, what lets each view perform only the actions that are its own.

pub mod action;
pub mod keymap;
