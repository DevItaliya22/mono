// SPDX-License-Identifier: MPL-2.0

//! Pending Profile-to-account binding markers: the record of intent that has
//! to survive a restart between "asked the Agent to bind this Profile" and
//! "the Agent answered".
//!
//! One file per Profile under the profile directory,
//! `pending-profile-account-binding-<id>.conf` — the name and the place are a
//! contract, because the OAuth test plan counts them on disk. What changed is
//! who touches the disk and how.
//!
//! **The window is the only writer, so the truth is the map in memory.** The
//! directory is read once, when the runtime opens, and every change is
//! mirrored to disk by one worker thread through
//! [`koru_settings::storage::write_atomic`]. The winit thread neither writes
//! nor lists nor fsyncs: it remembers, forgets, and drains the worker's
//! answers on its next pass. The Agent is asked to bind only once the marker
//! is durable, which is the order the synchronous version kept.
//!
//! **Why atomic, and why that mattered more than it looks.** The marker used
//! to be `fs::write` — truncate, then write — and the reconcile that ran on
//! every account snapshot read the whole directory with `?`. A crash between
//! the truncate and the write left an empty file, the read failed on it, and
//! the app died at launch with *"could not reconcile the saved Profile account
//! connection safely"* until somebody deleted a hundred-byte file by hand. A
//! marker is now either the previous complete one or the next; and a marker
//! that still cannot be read is reported and skipped, never a reason to stop.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use koru_core::ProfileId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingProfileAccountBinding {
    pub profile: ProfileId,
    pub expected_identity_id: Option<String>,
    pub new_identity_id: String,
}

/// The worker's answer to one [`PendingBindings::remember`]: the marker is on
/// disk, or it is not and the person has to be told.
pub(crate) struct Written {
    pub profile: ProfileId,
    pub result: Result<(), String>,
}

enum Job {
    Write(PendingProfileAccountBinding),
    Clear(ProfileId),
}

pub(crate) struct PendingBindings {
    known: BTreeMap<ProfileId, PendingProfileAccountBinding>,
    jobs: mpsc::Sender<Job>,
    written: mpsc::Receiver<Written>,
}

impl PendingBindings {
    /// Read what a previous run left behind — once — and start the writer.
    ///
    /// `wake` is called from the worker thread after every write so the loop
    /// drains [`Self::drain`] promptly rather than on its next unrelated tick.
    pub(crate) fn open(profile_dir: PathBuf, wake: impl Fn() + Send + 'static) -> Self {
        let known = read_markers(&profile_dir);
        let (jobs, job_rx) = mpsc::channel::<Job>();
        let (written_tx, written) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("koru-binding-markers".into())
            .spawn(move || {
                while let Ok(job) = job_rx.recv() {
                    match job {
                        Job::Write(marker) => {
                            let result = write_marker(&profile_dir, &marker);
                            if written_tx
                                .send(Written {
                                    profile: marker.profile,
                                    result,
                                })
                                .is_err()
                            {
                                return;
                            }
                            wake();
                        }
                        Job::Clear(profile) => {
                            if let Err(error) = clear_marker(&profile_dir, profile) {
                                // Not fatal: the next launch reconciles it as
                                // already bound and clears it again.
                                eprintln!(
                                    "[Koru][profiles] could not remove the Profile binding marker for {}: {error}",
                                    profile.to_storage()
                                );
                            }
                        }
                    }
                }
            });
        if let Err(error) = spawned {
            eprintln!("[Koru][profiles] could not start the binding-marker writer: {error}");
        }
        Self {
            known,
            jobs,
            written,
        }
    }

    /// Record the intent and queue its marker. The Agent must not be asked
    /// until [`Self::drain`] reports the marker written.
    pub(crate) fn remember(&mut self, marker: PendingProfileAccountBinding) {
        self.known.insert(marker.profile, marker.clone());
        let _ = self.jobs.send(Job::Write(marker));
    }

    /// The binding is settled one way or the other; the marker goes.
    pub(crate) fn forget(&mut self, profile: ProfileId) {
        if self.known.remove(&profile).is_some() {
            let _ = self.jobs.send(Job::Clear(profile));
        }
    }

    pub(crate) fn get(&self, profile: ProfileId) -> Option<&PendingProfileAccountBinding> {
        self.known.get(&profile)
    }

    /// Every marker, oldest Profile id first — a copy, because the callers
    /// that walk it also forget while walking.
    pub(crate) fn all(&self) -> Vec<PendingProfileAccountBinding> {
        self.known.values().cloned().collect()
    }

    /// What the writer has finished since the last pass, in order.
    pub(crate) fn drain(&mut self) -> Vec<Written> {
        let mut done = Vec::new();
        while let Ok(written) = self.written.try_recv() {
            done.push(written);
        }
        done
    }
}

const PENDING_PROFILE_BINDING_PREFIX: &str = "pending-profile-account-binding-";

fn marker_path(profile_dir: &Path, profile: ProfileId) -> PathBuf {
    profile_dir.join(format!(
        "{PENDING_PROFILE_BINDING_PREFIX}{}.conf",
        profile.to_storage()
    ))
}

fn encode(binding: &PendingProfileAccountBinding) -> String {
    let expected = binding.expected_identity_id.as_deref().unwrap_or("-");
    format!("expected={expected}\nnew={}\n", binding.new_identity_id)
}

fn decode(profile: ProfileId, body: &str) -> std::io::Result<PendingProfileAccountBinding> {
    let expected = body
        .lines()
        .find_map(|line| line.strip_prefix("expected="))
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "missing expected identity")
        })?;
    let new_identity_id = body
        .lines()
        .find_map(|line| line.strip_prefix("new="))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "missing new identity")
        })?
        .to_owned();
    Ok(PendingProfileAccountBinding {
        profile,
        expected_identity_id: (expected != "-").then(|| expected.to_owned()),
        new_identity_id,
    })
}

fn write_marker(profile_dir: &Path, marker: &PendingProfileAccountBinding) -> Result<(), String> {
    koru_settings::storage::write_atomic(
        &marker_path(profile_dir, marker.profile),
        encode(marker).as_bytes(),
    )
    .map_err(|error| error.to_string())
}

fn clear_marker(profile_dir: &Path, profile: ProfileId) -> std::io::Result<()> {
    match std::fs::remove_file(marker_path(profile_dir, profile)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Every marker that can be read. One that cannot is named once and skipped:
/// it is a lost intent, which the person can redo, and never a reason for the
/// app not to start.
fn read_markers(profile_dir: &Path) -> BTreeMap<ProfileId, PendingProfileAccountBinding> {
    let mut known = BTreeMap::new();
    let entries = match std::fs::read_dir(profile_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return known,
        Err(error) => {
            eprintln!(
                "[Koru][profiles] could not list Profile binding markers in {}: {error}",
                profile_dir.display()
            );
            return known;
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(raw_profile) = name
            .strip_prefix(PENDING_PROFILE_BINDING_PREFIX)
            .and_then(|name| name.strip_suffix(".conf"))
        else {
            continue;
        };
        let path = entry.path();
        let decoded = koru_core::ProfileId::parse(raw_profile)
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid Profile id")
            })
            .and_then(|profile| std::fs::read_to_string(&path).map(|body| (profile, body)))
            .and_then(|(profile, body)| decode(profile, &body));
        match decoded {
            Ok(marker) => {
                known.insert(marker.profile, marker);
            }
            Err(error) => eprintln!(
                "[Koru][profiles] ignoring a Profile binding marker that could not be read: {} ({error})",
                path.display()
            ),
        }
    }
    known
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn binding(new: &str) -> PendingProfileAccountBinding {
        PendingProfileAccountBinding {
            profile: ProfileId::new(),
            expected_identity_id: Some("00000000-0000-4000-8000-000000000001".into()),
            new_identity_id: new.into(),
        }
    }

    fn drain_one(bindings: &mut PendingBindings) -> Written {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(written) = bindings.drain().pop() {
                return written;
            }
            assert!(Instant::now() < deadline, "the writer never answered");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_marker_round_trips_without_identity_inference() {
        let marker = binding("00000000-0000-4000-8000-000000000002");
        let encoded = encode(&marker);
        assert!(!encoded.contains("email"));
        assert!(!encoded.contains("name"));
        assert_eq!(decode(marker.profile, &encoded).unwrap(), marker);
    }

    #[test]
    fn a_marker_refuses_incomplete_state() {
        let profile = ProfileId::new();
        assert!(decode(profile, "new=identity\n").is_err());
        assert!(decode(profile, "expected=-\n").is_err());
    }

    #[test]
    fn remembered_markers_reach_the_disk_and_are_read_back_on_the_next_open() {
        let dir = tempfile::tempdir().unwrap();
        let marker = binding("00000000-0000-4000-8000-000000000002");
        let mut bindings = PendingBindings::open(dir.path().to_path_buf(), || {});
        assert!(bindings.all().is_empty());

        bindings.remember(marker.clone());
        assert_eq!(bindings.get(marker.profile), Some(&marker), "known at once");
        let written = drain_one(&mut bindings);
        assert_eq!(written.profile, marker.profile);
        assert_eq!(written.result, Ok(()));
        assert!(marker_path(dir.path(), marker.profile).is_file());

        let reopened = PendingBindings::open(dir.path().to_path_buf(), || {});
        assert_eq!(reopened.all(), vec![marker.clone()]);

        bindings.forget(marker.profile);
        assert!(bindings.get(marker.profile).is_none());
        let gone = Instant::now() + Duration::from_secs(10);
        while marker_path(dir.path(), marker.profile).exists() {
            assert!(Instant::now() < gone, "the marker was never removed");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_torn_marker_is_skipped_and_the_others_are_still_read() {
        let dir = tempfile::tempdir().unwrap();
        let good = binding("00000000-0000-4000-8000-000000000002");
        std::fs::write(marker_path(dir.path(), good.profile), encode(&good)).unwrap();
        // What `fs::write` left behind when the process died after the
        // truncate: a name and no bytes.
        std::fs::write(marker_path(dir.path(), ProfileId::new()), "").unwrap();
        std::fs::write(dir.path().join("settings-default.conf"), "unrelated\n").unwrap();

        let bindings = PendingBindings::open(dir.path().to_path_buf(), || {});
        assert_eq!(bindings.all(), vec![good]);
    }

    #[test]
    fn a_missing_profile_directory_is_an_empty_set() {
        let dir = tempfile::tempdir().unwrap();
        let bindings = PendingBindings::open(dir.path().join("never-created"), || {});
        assert!(bindings.all().is_empty());
    }
}
