// SPDX-License-Identifier: MPL-2.0

use directories::BaseDirs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdatePaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub logs_dir: PathBuf,
}

impl UpdatePaths {
    pub fn discover() -> Option<Self> {
        let base = BaseDirs::new()?;
        #[cfg(windows)]
        let (config, data, cache) = {
            let roaming = std::env::var_os("APPDATA").map(PathBuf::from)?;
            let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
            (
                roaming.join("koru"),
                local.join("koru"),
                local.join("koru").join("Cache"),
            )
        };
        #[cfg(not(windows))]
        let (config, data, cache) = (
            base.config_dir().join("koru"),
            base.data_dir().join("koru"),
            base.cache_dir().join("koru"),
        );
        let _ = base;
        Some(Self {
            config_dir: config,
            logs_dir: data.join("Logs"),
            data_dir: data,
            cache_dir: cache,
        })
    }

    /// Resolve every update-owned file beneath an explicit channel root.
    ///
    /// This is the `--root` and signed-rehearsal counterpart to
    /// [`Self::discover`]. It performs no I/O: callers can first prove that the
    /// executable they might replace is inside the same root, then call
    /// [`Self::ensure`] only after that proof.
    /// Keeping the four directories separate mirrors the installed layout
    /// without allowing a rooted run to fall back to `%APPDATA%` or
    /// `%LOCALAPPDATA%`.
    pub fn under(root: impl AsRef<Path>) -> Self {
        let base = root.as_ref().join("agent").join("updates");
        Self {
            config_dir: base.join("config"),
            data_dir: base.join("data"),
            cache_dir: base.join("cache"),
            logs_dir: base.join("logs"),
        }
    }

    /// Resolve update paths from an already-decided root policy.
    ///
    /// The optional root stays explicit at binary boundaries. Libraries take
    /// the resulting `UpdatePaths` and never rediscover a process-global home.
    pub fn resolve(root: Option<&Path>) -> Option<Self> {
        match root {
            Some(root) => Some(Self::under(root)),
            None => Self::discover(),
        }
    }

    /// Prove that a rooted updater would replace only a copied bundle owned by
    /// that root. Canonicalization closes `..`, symlink, junction, and reparse
    /// aliases; equality is refused because the bundle must be a child of the
    /// channel root, not the root itself.
    pub fn root_owns_install(root: &Path, install_dir: &Path) -> bool {
        let Ok(root) = std::fs::canonicalize(root) else {
            return false;
        };
        let Ok(install) = std::fs::canonicalize(install_dir) else {
            return false;
        };
        install != root && install.starts_with(root)
    }

    /// Resolve paths only when this installation is eligible under the chosen
    /// root policy. Ordinary installs preserve discovery; rooted installs must
    /// be real copied children of that root.
    pub fn for_install(root: Option<&Path>, install_dir: &Path) -> Option<Self> {
        if root.is_some_and(|root| !Self::root_owns_install(root, install_dir)) {
            return None;
        }
        Self::resolve(root)
    }

    /// Everything an update needs, **scoped to this build's channel**.
    ///
    /// It was `cache_dir/updates` — one directory for every channel, holding
    /// one `state.json`. That file is single-valued about things that are not:
    /// `cached_verified_package`, `last_manifest`, `last_seen_build` and
    /// `deferred_build` all describe *a* channel, so two installed side by side
    /// overwrote each other's answers. The ETags survived only by accident,
    /// being keyed by the manifest path.
    ///
    /// The sharpest edge was a cached package. Build numbers come from one
    /// counter across all channels, so a nightly's build 40 is simply a bigger
    /// number than a stable's build 20 — and "offer what is already
    /// downloaded" compares numbers. A stable install could be offered a
    /// nightly package that a nightly agent had downloaded and left in the
    /// shared cache. `offer_what_is_already_downloaded` also checks the
    /// manifest's own channel now, because a rule this important should not
    /// rest on a directory layout alone.
    ///
    /// Same segments as the store root — `koru_channel::segment` — so
    /// `stable`, `beta`, `nightly`, `internal-debug` and `internal-release`
    /// mean one thing across the product. The channel goes *inside* `updates/`
    /// rather than above it so the legacy directory becomes this one's parent,
    /// which is what makes [`Self::adopt_legacy_updates`] a move rather than a
    /// guess.
    pub fn updates_dir(&self) -> PathBuf {
        self.cache_dir.join("updates").join(koru_channel::segment())
    }

    /// Where every channel used to share one update directory.
    pub fn legacy_updates_dir(&self) -> PathBuf {
        self.cache_dir.join("updates")
    }

    pub fn downloads_dir(&self) -> PathBuf {
        self.updates_dir().join("downloads")
    }

    pub fn verified_dir(&self) -> PathBuf {
        self.updates_dir().join("verified")
    }

    pub fn staging_dir(&self) -> PathBuf {
        self.updates_dir().join("staging")
    }

    pub fn health_dir(&self) -> PathBuf {
        self.updates_dir().join("health")
    }

    pub fn instances_dir(&self) -> PathBuf {
        self.updates_dir().join("instances")
    }

    pub fn state_file(&self) -> PathBuf {
        self.updates_dir().join("state.json")
    }

    pub fn updater_log(&self) -> PathBuf {
        self.logs_dir.join("updater.log")
    }

    /// Tell the updater that the build it installed came up. The updater
    /// passes the token on the restarted process's command line and watches
    /// this directory; a token that never appears is what makes a bad install
    /// roll back rather than sit there broken.
    pub fn signal_health(&self, token: &str) -> std::io::Result<()> {
        use std::io::Write as _;
        if token.len() != 32 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the update health token is not a 32-digit hexadecimal value",
            ));
        }
        let dir = self.health_dir();
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{token}.ok"));
        match std::fs::read(&path) {
            Ok(bytes) if bytes == b"ok\n" => return Ok(()),
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "the existing update health marker is invalid",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let temporary = dir.join(format!(".{token}-{}.tmp", uuid::Uuid::new_v4().simple()));
        let result = (|| -> std::io::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(b"ok\n")?;
            file.sync_all()?;
            drop(file);
            match std::fs::rename(&temporary, &path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if std::fs::read(&path)? != b"ok\n" {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "the existing update health marker is invalid",
                        ));
                    }
                    std::fs::remove_file(&temporary)?;
                }
                Err(error) => return Err(error),
            }
            // Flush through the published name as well. The updater accepts
            // only this complete file; a crash before the rename leaves a
            // uniquely named temporary file that readers ignore.
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)?
                .sync_all()?;
            #[cfg(unix)]
            std::fs::File::open(&dir)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }

    /// Take what the shared directory held into this channel's, once.
    ///
    /// **Only when this channel has no state of its own**, so it runs at most
    /// once per channel and never overwrites an answer this channel gave. Every
    /// installed channel adopts the same legacy file, which is the honest
    /// outcome: it was one file describing all of them, and this is the last
    /// point at which it still describes any of them. They diverge from here.
    ///
    /// The state file is **copied**, not moved: a second channel that has not
    /// started yet would otherwise find nothing. The one *package* the state
    /// points at is moved, because two channels cannot both install it and a
    /// 60 MB file should not be duplicated to say so — whichever channel starts
    /// first takes it, and the others re-download if they ever want it. The
    /// remaining loose zips are left where they are; they belong to no channel
    /// now, and deleting somebody's files to tidy a layout is not this
    /// function's business. Pruning only ever touches this channel's own
    /// directory.
    ///
    /// Best effort. A failed copy leaves a channel with default settings and
    /// one re-download, which is a worse morning than it needed to be and not
    /// a reason to refuse to start.
    fn adopt_legacy_updates(&self) {
        let legacy = self.legacy_updates_dir().join("state.json");
        let mine = self.state_file();
        if mine.exists() || !legacy.is_file() {
            return;
        }
        let Some(parent) = mine.parent() else { return };
        if std::fs::create_dir_all(parent).is_err() || std::fs::copy(&legacy, &mine).is_err() {
            return;
        }
        // And the package it names, if it is still where it said.
        let state = crate::state::UpdateState::load(&mine);
        let Some(cached) = state.cached_verified_package else {
            return;
        };
        let Some(name) = cached.path.file_name() else {
            return;
        };
        let moved = self.verified_dir().join(name);
        if cached.path == moved || !cached.path.is_file() {
            return;
        }
        let _ = std::fs::create_dir_all(self.verified_dir());
        if std::fs::rename(&cached.path, &moved).is_ok() {
            let mut state = crate::state::UpdateState::load(&mine);
            if let Some(cached) = state.cached_verified_package.as_mut() {
                cached.path = moved;
            }
            let _ = state.save(&mine);
        }
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        for dir in [
            &self.config_dir,
            &self.data_dir,
            &self.cache_dir,
            &self.logs_dir,
            &self.downloads_dir(),
            &self.verified_dir(),
            &self.staging_dir(),
            &self.health_dir(),
            &self.instances_dir(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        self.adopt_legacy_updates();
        Ok(())
    }
}

/// Signal that this process started successfully after an update.
///
/// There are two callers and there must only ever be one copy of this: the
/// shell still needs it, because an update installed by the *previous* build
/// named `koru.exe` as its restart target and that build's token is on this
/// command line; and the agent needs it for every update it installs from now
/// on. Parsing the token off the command line stays with each binary — that is
/// where the argument tables live — and only the writing is shared.
pub fn signal_health(token: &str) -> std::io::Result<()> {
    let paths = UpdatePaths::discover().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "could not resolve Koru update paths",
        )
    })?;
    paths.signal_health(token)
}
#[cfg(test)]
mod tests {
    use super::*;

    fn temp(root: &std::path::Path) -> UpdatePaths {
        UpdatePaths {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            logs_dir: root.join("logs"),
        }
    }

    #[test]
    fn an_explicit_root_contains_every_update_owned_path() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let paths = UpdatePaths::under(root.path());

        for path in [
            paths.config_dir.clone(),
            paths.data_dir.clone(),
            paths.cache_dir.clone(),
            paths.logs_dir.clone(),
            paths.updates_dir(),
            paths.legacy_updates_dir(),
            paths.downloads_dir(),
            paths.verified_dir(),
            paths.staging_dir(),
            paths.health_dir(),
            paths.instances_dir(),
            paths.state_file(),
            paths.updater_log(),
        ] {
            assert!(path.starts_with(root.path()), "{} escaped", path.display());
            assert!(
                !path.starts_with(other.path()),
                "{} crossed roots",
                path.display()
            );
        }
        if let Some(ordinary) = UpdatePaths::discover() {
            for rooted in [
                paths.config_dir,
                paths.data_dir,
                paths.cache_dir,
                paths.logs_dir,
            ] {
                assert_ne!(rooted, ordinary.config_dir);
                assert_ne!(rooted, ordinary.data_dir);
                assert_ne!(rooted, ordinary.cache_dir);
                assert_ne!(rooted, ordinary.logs_dir);
            }
        }
    }

    #[test]
    fn two_roots_write_state_health_logs_and_cache_independently() {
        let root_a = tempfile::tempdir().unwrap();
        let root_b = tempfile::tempdir().unwrap();
        let a = UpdatePaths::under(root_a.path());
        let b = UpdatePaths::under(root_b.path());
        a.ensure().unwrap();
        b.ensure().unwrap();
        let token_a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let token_b = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        a.signal_health(token_a).unwrap();
        b.signal_health(token_b).unwrap();
        std::fs::write(a.state_file(), b"a").unwrap();
        std::fs::write(b.state_file(), b"b").unwrap();
        std::fs::write(a.updater_log(), b"log-a").unwrap();
        std::fs::write(b.updater_log(), b"log-b").unwrap();
        std::fs::write(a.staging_dir().join("sentinel-a"), b"a").unwrap();
        std::fs::write(b.staging_dir().join("sentinel-b"), b"b").unwrap();

        assert_eq!(std::fs::read(a.state_file()).unwrap(), b"a");
        assert_eq!(std::fs::read(b.state_file()).unwrap(), b"b");
        assert!(a.health_dir().join(format!("{token_a}.ok")).is_file());
        assert!(!a.health_dir().join(format!("{token_b}.ok")).exists());
        assert!(b.health_dir().join(format!("{token_b}.ok")).is_file());
        assert!(!b.health_dir().join(format!("{token_a}.ok")).exists());
        assert!(a.staging_dir().join("sentinel-a").is_file());
        assert!(!a.staging_dir().join("sentinel-b").exists());
        assert!(b.staging_dir().join("sentinel-b").is_file());
        assert!(!b.staging_dir().join("sentinel-a").exists());
    }

    #[test]
    fn constructing_rooted_paths_writes_nothing() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("not-created");
        let _ = UpdatePaths::under(&root);
        assert!(!root.exists());
    }

    #[test]
    fn a_root_owns_only_real_child_installations() {
        let root = tempfile::tempdir().unwrap();
        let inside = root.path().join("copied-bundle");
        std::fs::create_dir(&inside).unwrap();
        let outside = tempfile::tempdir().unwrap();
        assert!(UpdatePaths::root_owns_install(root.path(), &inside));
        assert!(!UpdatePaths::root_owns_install(root.path(), root.path()));
        assert!(!UpdatePaths::root_owns_install(root.path(), outside.path()));
        assert!(!UpdatePaths::root_owns_install(
            root.path(),
            &root.path().join("missing")
        ));
    }

    /// **Nobody loses their settings or re-downloads for a layout change.**
    /// The shared `state.json` is copied into this channel's directory, and the
    /// package it names is moved and re-pointed.
    #[test]
    fn the_shared_update_directory_is_adopted_once() {
        let root = tempfile::tempdir().unwrap();
        let p = temp(root.path());
        let legacy = p.legacy_updates_dir();
        std::fs::create_dir_all(legacy.join("verified")).unwrap();
        let package = legacy.join("verified").join("777-koru.zip");
        std::fs::write(&package, b"z").unwrap();
        let mut state = crate::state::UpdateState {
            automatic_checks: false,
            last_seen_build: Some(777),
            ..Default::default()
        };
        state.cached_verified_package = Some(crate::state::CachedPackage {
            build_number: 777,
            path: package.clone(),
            sha256: "abc".into(),
        });
        state.save(&legacy.join("state.json")).unwrap();

        p.ensure().unwrap();

        let mine = crate::state::UpdateState::load(&p.state_file());
        assert!(!mine.automatic_checks, "the answer they gave came across");
        assert_eq!(mine.last_seen_build, Some(777));
        let cached = mine.cached_verified_package.expect("the package came too");
        assert_eq!(cached.path, p.verified_dir().join("777-koru.zip"));
        assert!(cached.path.is_file(), "moved, not just re-pointed");
        assert!(!package.is_file(), "and not left as a second 60 MB copy");
        // The legacy file stays: a channel that has not started yet still has
        // to be able to find it.
        assert!(legacy.join("state.json").is_file());
    }

    /// It never runs over an answer this channel has already given.
    #[test]
    fn adoption_does_not_overwrite_state_this_channel_already_has() {
        let root = tempfile::tempdir().unwrap();
        let p = temp(root.path());
        std::fs::create_dir_all(p.updates_dir()).unwrap();
        crate::state::UpdateState {
            last_seen_build: Some(1),
            ..Default::default()
        }
        .save(&p.state_file())
        .unwrap();
        std::fs::create_dir_all(p.legacy_updates_dir()).unwrap();
        crate::state::UpdateState {
            last_seen_build: Some(999),
            ..Default::default()
        }
        .save(&p.legacy_updates_dir().join("state.json"))
        .unwrap();

        p.ensure().unwrap();

        assert_eq!(
            crate::state::UpdateState::load(&p.state_file()).last_seen_build,
            Some(1),
            "ours, not the legacy one"
        );
    }

    /// The channel is in the path, and it is the segment the store already uses.
    #[test]
    fn the_updates_directory_names_this_channel() {
        let root = tempfile::tempdir().unwrap();
        let p = temp(root.path());
        assert!(p.updates_dir().ends_with(koru_channel::segment()));
        assert_ne!(p.updates_dir(), p.legacy_updates_dir());
        assert!(p.updates_dir().starts_with(p.legacy_updates_dir()));
    }

    #[test]
    fn signalling_health_writes_the_token_the_updater_waits_for() {
        // Through a temp `UpdatePaths` rather than the free function, which
        // discovers the real `%LOCALAPPDATA%\koru` and must never be reached
        // from a test.
        let dir = tempfile::tempdir().unwrap();
        let paths = UpdatePaths {
            config_dir: dir.path().join("config"),
            data_dir: dir.path().join("data"),
            cache_dir: dir.path().join("cache"),
            logs_dir: dir.path().join("logs"),
        };
        let token = "0123456789abcdef0123456789abcdef";
        paths.signal_health(token).unwrap();
        assert_eq!(
            std::fs::read_to_string(paths.health_dir().join(format!("{token}.ok"))).unwrap(),
            "ok\n"
        );
        assert_eq!(
            std::fs::read_dir(paths.health_dir()).unwrap().count(),
            1,
            "the atomic publication must not leave its temporary file behind"
        );
    }

    #[test]
    fn signalling_health_does_not_accept_an_existing_torn_marker() {
        let dir = tempfile::tempdir().unwrap();
        let paths = temp(dir.path());
        let token = "0123456789abcdef0123456789abcdef";
        std::fs::create_dir_all(paths.health_dir()).unwrap();
        let marker = paths.health_dir().join(format!("{token}.ok"));
        std::fs::write(&marker, b"o").unwrap();

        let error = paths.signal_health(token).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read(marker).unwrap(), b"o");
        assert_eq!(std::fs::read_dir(paths.health_dir()).unwrap().count(), 1);
    }

    #[test]
    fn an_explicit_root_contains_every_update_path_and_health_marker() {
        let dir = tempfile::tempdir().unwrap();
        let paths = UpdatePaths::under(dir.path());
        for path in [
            paths.config_dir.clone(),
            paths.data_dir.clone(),
            paths.cache_dir.clone(),
            paths.logs_dir.clone(),
            paths.state_file(),
            paths.health_dir(),
            paths.instances_dir(),
        ] {
            assert!(path.starts_with(dir.path()), "{} escaped", path.display());
        }
        let token = "fedcba9876543210fedcba9876543210";
        paths.signal_health(token).unwrap();
        assert!(
            paths.health_dir().join(format!("{token}.ok")).is_file(),
            "health was not written beneath the explicit root"
        );
    }
}
