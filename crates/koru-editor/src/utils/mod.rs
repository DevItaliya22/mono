// SPDX-License-Identifier: MPL-2.0

//! Pure helpers with no view state of their own.
//!
//! Everything here is a function of its arguments. If something needs to know
//! what is on screen, it belongs to a view instead.

pub mod dates;
// The grammar itself lives in `koru-when`; this is only the mapping onto the
// model's temporal types (ADR-191).
pub mod when;
pub mod zone;
