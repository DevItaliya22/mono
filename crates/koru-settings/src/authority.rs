// SPDX-License-Identifier: MPL-2.0

//! The authority: the one thing that reads and writes the settings files.
//!
//! **Serialization happens here, not at the caller.** Every patch takes one
//! lock, re-reads the file it is about to change, folds only the fields the
//! patch named onto what it found, and publishes the result by rename. That
//! single sentence is what fixes survey note 4: two windows changing two
//! different settings at the same moment both land, because neither one wrote
//! a field it was not asked to change. Two windows changing the *same* setting
//! is last-committed-wins, which is the only answer that does not involve
//! asking somebody to resolve a conflict about a checkbox.
//!
//! **Nothing is announced until it is durable.** The change fact goes out
//! after the rename has returned, so a window that reacts to one and refetches
//! is guaranteed to read the value the event was about.

use std::sync::{Arc, Mutex};

use crate::paths::SettingsPaths;
use crate::sections::{
    GlobalPatch, GlobalSettings, Invalid, Section, TerminalPatch, TerminalSettings,
};
use crate::storage;

#[cfg(test)]
mod tests;

/// Where a committed change fact leaves this authority.
///
/// **A port, for the same reason the Store's catalog sink is one.** The
/// journal an event lands in is the Gateway's, and this crate is the one that
/// knows what a setting is, not the one that knows where a journal lives. The
/// host hands an implementation down once, before anything is served; a build
/// that opened no journal leaves it empty and every publish is a silent no-op.
///
/// It carries a section and nothing else. No key, no old value, no new value —
/// a settings fact is an invalidation, and a consumer that wants the value
/// refetches it from the authority that just committed it.
///
/// **Never a reason to fail a write.** A journal that will not accept an
/// observation is a window that refreshes late, not a setting that was lost.
pub trait SettingsEventSink: Send + Sync + 'static {
    fn publish(&self, section: Section);
}

/// The OS side of `start_on_login`.
///
/// **The side effect must succeed before the value commits**, exactly as it
/// does today: `DesktopPreferences::set_start_on_login` calls the registry or
/// the launch agent first and only assigns the field if that returned `Ok`. A
/// stored `true` that the OS never got is a lie the person cannot see, and
/// they find out at the next sign-in.
///
/// It is a port rather than a direct `koru-autostart` call because the entry
/// this authority would write names `std::env::current_exe()`, and inside the
/// Gateway process that is the *Gateway's* executable. Resolving the product
/// executable is the host's knowledge, so the host supplies it. With no port
/// installed the field is refused rather than written — see Questions in the
/// SP-6a report.
pub trait StartOnLoginPort: Send + Sync + 'static {
    /// Enable or remove the startup entry. `Err` means nothing changed.
    fn set(&self, enabled: bool) -> Result<(), String>;
}

struct StartupChange {
    port: Option<Arc<dyn StartOnLoginPort>>,
    prior: bool,
    wanted: bool,
}

fn commit_with_startup<T>(
    change: Option<StartupChange>,
    authoritative_before: GlobalSettings,
    persist: impl FnOnce() -> std::io::Result<T>,
    reread: impl FnOnce() -> GlobalSettings,
) -> Result<T, PatchError<GlobalSettings>> {
    if let Some(change) = &change {
        let Some(port) = &change.port else {
            return Err(startup_refused(
                authoritative_before,
                "this host cannot change the startup entry".into(),
            ));
        };
        if let Err(message) = port.set(change.wanted) {
            return Err(startup_refused(authoritative_before, message));
        }
    }
    match persist() {
        Ok(value) => Ok(value),
        Err(write) => Err(rollback_after_write(change, write, reread)),
    }
}

fn startup_refused(authoritative: GlobalSettings, message: String) -> PatchError<GlobalSettings> {
    PatchError::SideEffect {
        authoritative: Box::new(authoritative),
        field: "start_on_login".into(),
        message,
    }
}

fn rollback_after_write(
    change: Option<StartupChange>,
    write: std::io::Error,
    reread: impl FnOnce() -> GlobalSettings,
) -> PatchError<GlobalSettings> {
    if let Some(change) = change
        && change.prior != change.wanted
        && let Some(port) = change.port
        && let Err(rollback) = port.set(change.prior)
    {
        return inconsistent_startup(reread(), write, rollback);
    }
    let authoritative = reread();
    PatchError::Write {
        authoritative: Box::new(authoritative),
        message: write.to_string(),
    }
}

fn inconsistent_startup(
    authoritative: GlobalSettings,
    write: std::io::Error,
    rollback_message: String,
) -> PatchError<GlobalSettings> {
    PatchError::Inconsistent {
        authoritative: Box::new(authoritative),
        field: "start_on_login".into(),
        message: write.to_string(),
        rollback_message,
    }
}

/// A patch this authority did not apply, and what is true instead.
#[derive(Clone, Debug, PartialEq)]
pub enum PatchError<T> {
    /// The patch was refused before anything was staged. The disk is
    /// untouched, so the caller's last read is still current.
    Invalid(Invalid),
    /// A required side effect refused. Nothing was written.
    SideEffect {
        authoritative: Box<T>,
        field: String,
        message: String,
    },
    /// The write did not land. `authoritative` is what is on disk *now*, re-read
    /// rather than assumed — so a control can reconcile to the truth instead of
    /// showing a value it only hoped for.
    Write {
        authoritative: Box<T>,
        message: String,
    },
    /// Persistence changed a required side effect, then both the file write
    /// and restoring that side effect failed. Disk and OS may disagree.
    Inconsistent {
        authoritative: Box<T>,
        field: String,
        message: String,
        rollback_message: String,
    },
}

impl<T> PatchError<T> {
    /// What is true now, whatever went wrong.
    pub fn authoritative(&self) -> Option<&T> {
        match self {
            Self::Invalid(_) => None,
            Self::SideEffect { authoritative, .. }
            | Self::Write { authoritative, .. }
            | Self::Inconsistent { authoritative, .. } => Some(authoritative),
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Invalid(invalid) => invalid.to_string(),
            Self::SideEffect { field, message, .. } => format!("{field}: {message}"),
            Self::Write { message, .. } => message.clone(),
            Self::Inconsistent {
                message,
                rollback_message,
                ..
            } => {
                format!("{message}; restoring start_on_login also failed: {rollback_message}")
            }
        }
    }
}

/// The settings authority.
#[derive(Clone)]
pub struct SettingsAuthority {
    inner: Arc<Inner>,
}

struct Inner {
    paths: SettingsPaths,
    /// Serializes patches inside this process.
    ///
    /// Guards the read-merge-write, not the data: the file is the state, and
    /// the lock exists so two concurrent patches cannot both read the same
    /// "before" and each write a body missing the other's field.
    write: Mutex<()>,
    sink: Mutex<Option<Arc<dyn SettingsEventSink>>>,
    start_on_login: Mutex<Option<Arc<dyn StartOnLoginPort>>>,
}

impl SettingsAuthority {
    pub fn new(paths: SettingsPaths) -> Self {
        Self {
            inner: Arc::new(Inner {
                paths,
                write: Mutex::new(()),
                sink: Mutex::new(None),
                start_on_login: Mutex::new(None),
            }),
        }
    }

    pub fn paths(&self) -> &SettingsPaths {
        &self.inner.paths
    }

    /// Hand the journal down once, before serving.
    pub fn set_event_sink(&self, sink: Arc<dyn SettingsEventSink>) {
        *self.inner.sink.lock().expect("settings sink") = Some(sink);
    }

    /// Hand the OS startup mechanism down once, before serving.
    pub fn set_start_on_login_port(&self, port: Arc<dyn StartOnLoginPort>) {
        *self
            .inner
            .start_on_login
            .lock()
            .expect("settings autostart") = Some(port);
    }

    fn publish(&self, section: Section) {
        let sink = self.inner.sink.lock().expect("settings sink").clone();
        if let Some(sink) = sink {
            sink.publish(section);
        }
    }

    // ------------------------------------------------------------ reading

    /// The Global section as it is on disk.
    ///
    /// Total: a missing or unreadable file is a first run and yields the
    /// registry's defaults, which is what makes a fresh install open into a
    /// working app rather than an error dialog.
    pub fn global(&self) -> GlobalSettings {
        self.read_global().0
    }

    fn read_global(
        &self,
    ) -> (
        GlobalSettings,
        storage::PreferencesFile,
        storage::DesktopFile,
    ) {
        let mut settings = GlobalSettings::default();
        let preferences = storage::read_optional(self.inner.paths.preferences_file())
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map(|text| storage::apply_preferences(&text, &mut settings))
            .unwrap_or_default();
        let desktop = storage::read_optional(self.inner.paths.desktop_file())
            .map(|bytes| storage::apply_desktop(&bytes, &mut settings))
            .unwrap_or_default();
        // The one-time migration: only when `desktop-v1.json` has never said
        // anything about automatic checks does `koru-update`'s state file get
        // a vote. After the first Global write it never speaks again.
        if !desktop.had_automatic_checks
            && let Some(bytes) = storage::read_optional(self.inner.paths.update_state_file())
            && let Some(flag) = storage::read_update_automatic_checks(&bytes)
        {
            settings.automatic_update_checks = flag;
        }
        (settings, preferences, desktop)
    }

    /// The Terminal section as it is on disk.
    ///
    /// Seeded once from this machine's `windows.conf` row when the roaming
    /// file does not exist yet, so promoting the setting from device-local to
    /// roaming does not read as "your terminal reset itself".
    pub fn terminal(&self) -> TerminalSettings {
        match storage::read_optional(self.inner.paths.terminal_file())
            .and_then(|bytes| String::from_utf8(bytes).ok())
        {
            Some(text) => {
                let mut settings = TerminalSettings::default();
                storage::apply_terminal(&text, &mut settings);
                settings
            }
            None => storage::read_optional(self.inner.paths.windows_file())
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .and_then(|text| storage::seed_terminal_from_window_row(&text))
                .unwrap_or_default(),
        }
    }

    // ------------------------------------------------------------ writing

    /// Apply a Global patch and publish the fact.
    pub fn patch_global(
        &self,
        patch: GlobalPatch,
    ) -> Result<GlobalSettings, PatchError<GlobalSettings>> {
        patch.validate().map_err(PatchError::Invalid)?;
        let _guard = self.inner.write.lock().expect("settings write");

        // Re-read under the lock. The caller's view may be stale, and only the
        // fields it actually named are allowed to move.
        let (mut settings, preferences_file, desktop_file) = self.read_global();
        if patch.is_empty() {
            return Ok(settings);
        }

        // The side effect first, for every explicit startup patch. Reasserting
        // the disk-visible value repairs the only ambiguous state this set-only
        // port permits: persistence and compensation both failed previously,
        // so disk says `prior` while the OS may still hold `wanted`.
        let authoritative_before = settings.clone();
        let mut startup_change = None;
        if let Some(wanted) = patch.start_on_login {
            let port = self
                .inner
                .start_on_login
                .lock()
                .expect("settings autostart")
                .clone();
            startup_change = Some(StartupChange {
                port,
                prior: settings.start_on_login,
                wanted,
            });
        }

        let touches_roaming = patch.touches_roaming();
        let touches_device = patch.touches_device();
        patch.apply(&mut settings);
        let settings = commit_with_startup(
            startup_change,
            authoritative_before,
            || {
                // A file nothing changed is a file that is not rewritten: a theme
                // change must not be able to fail on `desktop-v1.json`.
                if touches_roaming {
                    let body = storage::render_preferences(&settings, &preferences_file);
                    storage::write_atomic(self.inner.paths.preferences_file(), body.as_bytes())?;
                }
                if touches_device {
                    let body = storage::render_desktop(&settings, &desktop_file);
                    storage::write_atomic(self.inner.paths.desktop_file(), &body)?;
                }
                Ok(settings)
            },
            || self.read_global().0,
        )?;

        drop(_guard);
        self.publish(Section::Global);
        Ok(settings)
    }

    /// Apply a Terminal patch and publish the fact.
    pub fn patch_terminal(
        &self,
        patch: TerminalPatch,
    ) -> Result<TerminalSettings, PatchError<TerminalSettings>> {
        patch.validate().map_err(PatchError::Invalid)?;
        let _guard = self.inner.write.lock().expect("settings write");

        let mut settings = self.terminal();
        if patch.is_empty() {
            return Ok(settings);
        }
        patch.apply(&mut settings);

        let body = storage::render_terminal(&settings);
        if let Err(why) = storage::write_atomic(self.inner.paths.terminal_file(), body.as_bytes()) {
            return Err(PatchError::Write {
                authoritative: Box::new(self.terminal()),
                message: why.to_string(),
            });
        }

        drop(_guard);
        self.publish(Section::Terminal);
        Ok(settings)
    }
}

impl std::fmt::Debug for SettingsAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsAuthority")
            .field("paths", &self.inner.paths)
            .finish_non_exhaustive()
    }
}
