// SPDX-License-Identifier: MPL-2.0

//! Tab/document navigation diagnostics, on demand.
//!
//! ```sh
//! KORU_NAV_DEBUG=1 ./koru.sh run
//! ```
//!
//! Off, this is a load of a `bool` and one branch. On, it prints one line when
//! a tab is left, selected, caught up, landed or restored — ids, counts and
//! stages, never a title or row text.

use std::fmt;
use std::sync::OnceLock;

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| koru_env::switch(koru_env::name::KORU_NAV_DEBUG))
}

pub fn write(args: fmt::Arguments<'_>) {
    if enabled() {
        eprintln!("{args}");
    }
}

macro_rules! nav_debug {
    ($($t:tt)*) => {
        $crate::mainview::nav_debug::write(format_args!($($t)*))
    };
}

pub(crate) use nav_debug;
