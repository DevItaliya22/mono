// SPDX-License-Identifier: MPL-2.0

//! Process-level idempotency for windows replayed after an update.
//!
//! Agent persists a v4 token before it spawns a replacement window and keeps
//! that token until the shell completes its real `Hello`. If Agent or the
//! machine dies between those events, the same token is replayed. Holding a
//! per-token local endpoint turns that replay into an OS election: exactly one
//! process continues, including when two replays race before either has drawn.

use std::path::Path;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Held {
    ThisProcess,
    AnotherProcess,
}

pub fn hold(root: &Path, token: &str) -> Result<Held, String> {
    let token = uuid::Uuid::parse_str(token)
        .ok()
        .filter(|token| token.get_version_num() == 4)
        .ok_or("the reopen claim is not a v4 UUID")?
        .to_string();
    let endpoint = koru_ipc::Endpoint::for_service_claim(
        koru_ipc::REOPEN_CLAIM,
        koru_channel::segment(),
        root,
        &token,
    )
    .map_err(|error| error.to_string())?;
    let listener = match koru_ipc::Listener::bind(endpoint) {
        Ok(listener) => listener,
        Err(koru_ipc::IpcError::AlreadyBound(_)) => return Ok(Held::AnotherProcess),
        Err(error) => return Err(error.to_string()),
    };

    std::thread::Builder::new()
        .name("koru-reopen-claim".into())
        .spawn(move || {
            // Connections carry no protocol. Accepting and dropping them lets
            // `koru-ipc` distinguish a live holder from a stale Unix socket or
            // unrelated bind failure on every later replay.
            while let Ok(stream) = listener.accept() {
                drop(stream);
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(Held::ThisProcess)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claim_must_be_a_v4_uuid() {
        let root = std::env::temp_dir().join("koru-invalid-reopen-claim");
        assert!(hold(&root, "not-a-token").is_err());
        assert!(hold(&root, "00000000-0000-1000-8000-000000000000").is_err());
    }

    #[test]
    fn claim_holder_child() {
        let (Some(root), Some(token), Some(ready)) = (
            std::env::var_os("REOPEN_CLAIM_TEST_ROOT"),
            std::env::var_os("REOPEN_CLAIM_TEST_TOKEN"),
            std::env::var_os("REOPEN_CLAIM_TEST_READY"),
        ) else {
            return;
        };
        assert_eq!(
            hold(Path::new(&root), &token.to_string_lossy()).unwrap(),
            Held::ThisProcess
        );
        std::fs::write(ready, b"held").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(10));
    }

    #[test]
    fn two_processes_replaying_one_claim_elect_exactly_one_holder() {
        let directory = tempfile::tempdir().unwrap();
        let ready = directory.path().join("ready");
        let token = uuid::Uuid::new_v4().to_string();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "reopen_claim::tests::claim_holder_child",
                "--nocapture",
            ])
            .env("REOPEN_CLAIM_TEST_ROOT", directory.path())
            .env("REOPEN_CLAIM_TEST_TOKEN", &token)
            .env("REOPEN_CLAIM_TEST_READY", &ready)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        while !ready.is_file() && started.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(ready.is_file(), "claim-holder child did not become ready");
        assert_eq!(
            hold(directory.path(), &token).unwrap(),
            Held::AnotherProcess
        );
        let _ = child.kill();
        let _ = child.wait();
    }
}
