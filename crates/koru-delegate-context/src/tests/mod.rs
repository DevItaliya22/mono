// SPDX-License-Identifier: MPL-2.0

//! Pure tests: nothing here opens a store or touches a disk. The
//! process-and-disk scenario through the real store is `tests/store.rs`.

use super::*;

mod dependency;
mod layout;
mod no_logging;
mod pinning;
mod privacy;
mod state;
