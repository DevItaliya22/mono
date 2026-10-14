// SPDX-License-Identifier: MPL-2.0

//! Thin standalone host for the reusable `koru-store-server` worker.

#![cfg_attr(not(test), windows_subsystem = "windows")]

fn main() {
    koru_store_server::host::run();
}
