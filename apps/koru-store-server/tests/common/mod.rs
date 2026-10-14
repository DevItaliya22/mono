#![allow(dead_code)]
// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub fn ready_path(root: &Path) -> PathBuf {
    root.join("standalone-ready.json")
}

pub fn configure(command: &mut std::process::Command, root: &Path) {
    let ready = ready_path(root);
    let _ = std::fs::remove_file(&ready);
    command.env(koru_store_server::INTERNAL_READY_ENV, ready);
}

pub fn discovery(root: &Path) -> Option<koru_store_client::Discovery> {
    let bytes = std::fs::read(ready_path(root)).ok()?;
    let discovery = serde_json::from_slice::<koru_store_server::StandaloneReady>(&bytes)
        .ok()?
        .discovery()
        .ok()?;
    koru_store_client::Client::new(discovery.clone())
        .status()
        .ok()
        .map(|_| discovery)
}

pub fn wait_for_discovery(root: &Path) -> koru_store_client::Discovery {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(discovery) = discovery(root) {
            return discovery;
        }
        assert!(
            Instant::now() < deadline,
            "standalone Store did not become ready"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}
