// SPDX-License-Identifier: MPL-2.0

//! The rooted session-host harness the `sessions_*` suites share.
//!
//! Every case starts `koru-agent.exe --worker sessions --root <root>` on its
//! own temporary root and drives it through `koru-session-client` exactly as
//! a window would. The role calls straight into
//! `koru_session_host::run_supervised`, which is the same composition the
//! retired `koru-session-host.exe` ran. Endpoint names carry a digest of the
//! root, so these hosts never meet each other or the sessions host a
//! developer has open.
//!
//! It lives under `apps/koru-agent` because `CARGO_BIN_EXE_*` is set only for
//! binaries the *testing package itself* declares, and `koru-agent` is the
//! only package that declares an image hosting this role.

#![allow(dead_code)]

use std::ffi::OsStr;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use koru_session_client::{ClientError, SessionClient};
use koru_session_wire::LaunchBootstrap;

pub struct Host {
    pub child: Child,
    pub bootstrap_pipe: Option<std::process::ChildStdin>,
    pub endpoint: koru_ipc::Endpoint,
    pub authority: String,
    pub lifecycle_authority: String,
}

impl Host {
    pub fn start(root: &Path) -> Self {
        Self::start_with(root, None, &[])
    }

    pub fn start_with_provider_path(root: &Path, provider_path: Option<&Path>) -> Self {
        Self::start_with(root, provider_path, &[])
    }

    /// Start a host with a provider directory prepended to its `PATH` and
    /// extra environment for the provider processes it spawns — the fixture
    /// shim's trace file, for a test that must see what argv reached it.
    pub fn start_with(root: &Path, provider_path: Option<&Path>, env: &[(&str, &OsStr)]) -> Self {
        std::fs::create_dir_all(root).unwrap();
        let launch_id = uuid::Uuid::new_v4().to_string();
        let authority = uuid::Uuid::new_v4().to_string();
        let lifecycle_authority = uuid::Uuid::new_v4().to_string();
        let bootstrap = LaunchBootstrap::new(
            launch_id.clone(),
            koru_channel::segment().into(),
            root.to_path_buf(),
            root.join("agent/sessions"),
            root.join("store"),
            true,
            authority.clone(),
        )
        .with_lifecycle_authority(lifecycle_authority.clone());
        let mut command = Command::new(PathBuf::from(env!("CARGO_BIN_EXE_koru-agent")));
        command
            .args(["--worker", "sessions", "--root"])
            .arg(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        if let Some(provider_path) = provider_path {
            let inherited = std::env::var_os("PATH").unwrap_or_default();
            let path = std::env::join_paths(
                std::iter::once(provider_path.to_path_buf())
                    .chain(std::env::split_paths(&inherited)),
            )
            .unwrap();
            command.env("PATH", path);
        }
        for (name, value) in env {
            command.env(name, value);
        }
        let mut child = command.spawn().unwrap();
        let mut bootstrap_pipe = child.stdin.take().unwrap();
        bootstrap_pipe
            .write_all(&serde_json::to_vec(&bootstrap).unwrap())
            .unwrap();
        bootstrap_pipe.write_all(b"\n").unwrap();
        bootstrap_pipe.flush().unwrap();
        let endpoint = koru_ipc::Endpoint::for_service_channel_root(
            koru_ipc::SESSION_HOST,
            koru_channel::segment(),
            root,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match client(&endpoint, "koru-session-host-test") {
                Ok(client) => {
                    let health = client.health().unwrap();
                    assert_eq!(health.supervisor_launch_id, launch_id);
                    break;
                }
                Err(_) if Instant::now() < deadline => {
                    assert!(
                        child.try_wait().unwrap().is_none(),
                        "host exited during startup"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("host did not start: {error}"),
            }
        }
        Self {
            child,
            bootstrap_pipe: Some(bootstrap_pipe),
            endpoint,
            authority,
            lifecycle_authority,
        }
    }

    pub fn client(&self, product: &str) -> SessionClient {
        client(&self.endpoint, product).unwrap()
    }

    pub fn stop(mut self) {
        self.client("koru-agent-supervisor")
            .stop_host(self.authority.clone())
            .unwrap();
        wait_until(|| self.child.try_wait().unwrap().is_some());
        self.bootstrap_pipe.take();
        assert!(!koru_ipc::probe(&self.endpoint));
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// The checked-in Codex app-server fixture, laid out the way npm lays the
/// real CLI out on this platform, in a directory a host can be handed on
/// its `PATH`.
pub fn fixture_codex(root: &Path) -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/codex-app-server/0.150.1/bin")
        .canonicalize()
        .expect("checked-in Codex fixture directory");
    let destination = root.join("codex-fixture-bin");
    std::fs::create_dir_all(&destination).unwrap();

    #[cfg(windows)]
    {
        // Match npm's real layout: the extensionless sibling is a Unix shim
        // and must never win over the PATHEXT command shim on Windows.
        std::fs::write(destination.join("codex"), "not a Windows executable").unwrap();
        std::fs::copy(source.join("codex.cmd"), destination.join("codex.cmd")).unwrap();
        std::fs::copy(
            source.join("codex-fixture.ps1"),
            destination.join("codex-fixture.ps1"),
        )
        .unwrap();
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let executable = destination.join("codex");
        std::fs::copy(source.join("codex"), &executable).unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(executable, permissions).unwrap();
    }

    destination
}

pub fn client(endpoint: &koru_ipc::Endpoint, product: &str) -> Result<SessionClient, ClientError> {
    SessionClient::connect(endpoint, product, koru_session_host::protocol_offer())
}

pub fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(Instant::now() < deadline, "condition did not settle");
        std::thread::sleep(Duration::from_millis(20));
    }
}
