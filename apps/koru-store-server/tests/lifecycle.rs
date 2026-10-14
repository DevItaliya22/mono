// SPDX-License-Identifier: MPL-2.0

//! Standalone Store worker lifecycle through its explicit, unshipped test seam.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use koru_store_client::Discovery;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_koru-store-server"))
}

struct Daemon {
    child: Child,
    root: PathBuf,
}

impl Daemon {
    fn start(root: &Path) -> Self {
        let mut command = Command::new(binary());
        command
            .arg("serve")
            .arg("--console")
            .arg("--root")
            .arg(root);
        common::configure(&mut command, root);
        let child = command.spawn().expect("spawn standalone Store");
        let daemon = Self {
            child,
            root: root.to_path_buf(),
        };
        daemon.wait_until_answering();
        daemon
    }

    fn wait_until_answering(&self) -> Discovery {
        common::wait_for_discovery(&self.root)
    }

    fn found(&self) -> Discovery {
        common::discovery(&self.root).expect("standalone Store answers")
    }

    fn stop(&self) {
        koru_store_client::Client::new(self.found())
            .shutdown()
            .expect("stop standalone Store");
    }

    fn wait_for_exit(&mut self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return true,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => return false,
            }
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn http() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

#[test]
fn a_standalone_store_publishes_an_explicit_endpoint_that_answers() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(dir.path());
    let found = daemon.found();

    assert!(common::ready_path(dir.path()).is_file());
    assert!(found.port > 0);
    assert!(!found.token.is_empty());
    assert!(koru_store_client::Client::new(found).status().is_ok());
}

#[test]
fn startup_opens_only_the_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let _daemon = Daemon::start(dir.path());

    assert!(dir.path().join("catalog.db").exists());
    assert!(!dir.path().join("workspaces").exists());
    assert!(!dir.path().join("store.json").exists());
    assert!(!dir.path().join("store.secret").exists());
}

#[test]
fn the_private_root_claim_refuses_a_second_owner() {
    let dir = tempfile::tempdir().unwrap();
    let _first = Daemon::start(dir.path());

    let second = Command::new(binary())
        .arg("serve")
        .arg("--console")
        .arg("--root")
        .arg(dir.path())
        .output()
        .expect("run second standalone Store");
    assert!(!second.status.success());
}

#[test]
fn root_qualified_claims_allow_hermetic_workers_side_by_side() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let one = Daemon::start(a.path());
    let two = Daemon::start(b.path());
    assert_ne!(one.found().port, two.found().port);
}

#[test]
fn every_http_route_requires_the_explicit_test_credential() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(dir.path());
    let port = daemon.found().port;

    for (method, path) in [("GET", "/v1/status"), ("POST", "/v1/shutdown")] {
        let url = format!("http://127.0.0.1:{port}{path}");
        let request = if method == "GET" {
            http().get(&url)
        } else {
            http().post(&url)
        };
        assert_eq!(request.send().unwrap().status().as_u16(), 401);
    }
}

#[test]
fn credentials_are_instance_bound() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let one = Daemon::start(a.path());
    let two = Daemon::start(b.path());

    let response = http()
        .get(format!("http://127.0.0.1:{}/v1/status", two.found().port))
        .bearer_auth(&one.found().token)
        .send()
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);
}

#[test]
fn credentials_do_not_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let stale = {
        let mut daemon = Daemon::start(dir.path());
        let token = daemon.found().token;
        daemon.stop();
        assert!(daemon.wait_for_exit(Duration::from_secs(30)));
        token
    };
    let daemon = Daemon::start(dir.path());
    let response = http()
        .get(format!(
            "http://127.0.0.1:{}/v1/status",
            daemon.found().port
        ))
        .bearer_auth(stale)
        .send()
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);
}

#[test]
fn the_http_adapter_binds_loopback_only() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(dir.path());
    assert!(std::net::TcpListener::bind(("0.0.0.0", daemon.found().port)).is_ok());
}

#[test]
fn graceful_shutdown_releases_the_private_claim_and_removes_readiness() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut daemon = Daemon::start(dir.path());
        daemon.stop();
        assert!(daemon.wait_for_exit(Duration::from_secs(30)));
    }
    assert!(!common::ready_path(dir.path()).exists());
    assert!(Daemon::start(dir.path()).found().port > 0);
}

#[test]
fn abrupt_exit_needs_no_claim_cleanup_before_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut daemon = Daemon::start(dir.path());
        daemon.child.kill().unwrap();
        daemon.child.wait().unwrap();
    }
    assert!(common::discovery(dir.path()).is_none());
    assert!(Daemon::start(dir.path()).found().port > 0);
}

#[test]
fn help_version_and_rejected_arguments_do_not_start_a_worker() {
    for arg in ["--help", "--version", "--roo"] {
        let dir = tempfile::tempdir().unwrap();
        let output = Command::new(binary())
            .arg(arg)
            .arg("--root")
            .arg(dir.path())
            .output()
            .unwrap();
        if arg == "--roo" {
            assert_eq!(output.status.code(), Some(2));
        } else {
            assert!(output.status.success());
        }
        assert!(common::discovery(dir.path()).is_none());
    }
}
