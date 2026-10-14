// SPDX-License-Identifier: MPL-2.0

//! What `koru.bat envs` falls back to when no built `koru.exe` is beside the
//! target directory, and what `setup-envs` parses.
//!
//! No arguments: the whole table for humans — the same string
//! `koru.exe --envs` prints, from [`koru_env::table`]. `--tsv`:
//! `name<TAB>kind<TAB>purpose` of the **runtime** variables only, for the
//! script's interactive walk to parse.

use koru_env::{Kind, Scope, VARS};

fn main() {
    if std::env::args().any(|arg| arg == "--markdown") {
        print!("{}", koru_env::markdown());
        return;
    }
    if std::env::args().any(|arg| arg == "--tsv") {
        for var in VARS.iter().filter(|var| var.scope == Scope::Runtime) {
            let kind = match var.kind {
                Kind::Switch => "switch",
                Kind::Value => "value",
            };
            println!("{}\t{}\t{}", var.name, kind, var.purpose);
        }
        return;
    }
    print!("{}", koru_env::table());
}
