// SPDX-License-Identifier: MPL-2.0

//! The notification-area icon, and the Win32 message loop it cannot live
//! without.
//!
//! ## The rule this module exists to keep
//!
//! **A store you cannot see is a store you cannot stop.** Started from the Run
//! key it has no console and no
//! window. Without an icon it is a process holding your workspace that only Task
//! Manager can end. So the menu always offers **Quit**, and the tooltip always
//! names the channel — two stores can run at once and two identical icons would
//! be worse than one.
//!
//! ## The pump is the part `koru-workspace-host` did not need
//!
//! `tray-icon` creates a hidden window and registers the icon against it with a
//! callback message; clicks arrive at that window's procedure. Nothing delivers
//! them unless somebody pumps messages for it. Koru's own tray works because
//! winit is already pumping — the crate's documentation is explicit: *"an event
//! loop must be running on the thread... you have to create the tray icon on the
//! same thread as the event loop."*
//!
//! A daemon has no event loop, so this makes one: a dedicated OS thread that
//! creates the icon and runs `GetMessageW` / `TranslateMessage` /
//! `DispatchMessageW` until told to stop.
//!
//! Three consequences, each of which is a bug if forgotten:
//!
//! - the `TrayIcon` is **thread-affine** — created *and dropped* on that thread,
//!   never sent anywhere;
//! - menu events are drained **immediately after each dispatch**, on this same
//!   thread. A click *is* a window message, so by the time `DispatchMessageW`
//!   returns muda has already queued it — which makes this push-driven despite
//!   reading a channel. `koru-workspace-host` polls every 120 ms instead, because it has
//!   a caret's clock to ride on; a daemon has no clock and inventing one would
//!   be a wake per second, forever, to discover nothing. It also cannot use
//!   `set_event_handler`, whose closure must be `Send` while `CheckMenuItem`
//!   holds `Rc` internals — and that item's post-toggle state is exactly what
//!   has to be read;
//! - the thread has to be woken to exit, because `GetMessageW` blocks. That is
//!   what [`Tray::stop`] posts.
//!
//! ## Failure is never fatal
//!
//! No icon resource, no shell, another process holding the notification area —
//! the daemon logs it and serves anyway. A store that refused to run because it
//! could not draw a picture would be a worse outcome than the one it was
//! avoiding.
//!
//! ## Windows only
//!
//! `tray-icon` reaches for gtk and libxdo elsewhere, which is a large dependency
//! for a convenience, so it is declared under
//! `[target.'cfg(windows)'.dependencies]` and everything here has a stub. The
//! stub's `start` returning `None` is not a degraded mode: a daemon on a
//! platform with no notification area is exactly the console daemon it was.

/// What the person asked the icon for.
///
/// Only `windows_impl` constructs these — the stub has no icon to be asked
/// through — so off Windows the variants are matched but never made.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum TrayEvent {
    /// Show the store's directory in the file manager. The question people
    /// actually ask of a store is "where is my data", and the debug view can
    /// only answer it in text you then have to copy.
    OpenStoreLocation,
    /// Write the complete Store to one portable file chosen by the person.
    BackupStore,
    /// Start this store when Windows starts, or stop doing so. Carries the
    /// state the menu item now shows.
    StartOnLogin(bool),
    /// Stop the daemon, gracefully, through the same path `/v1/shutdown` uses.
    Quit,
}

#[cfg(windows)]
pub use windows_impl::Tray;

#[cfg(not(windows))]
pub use stub::Tray;

#[cfg(windows)]
mod windows_impl {
    use std::sync::mpsc::{Receiver, Sender, channel};

    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem};
    use tray_icon::{Icon, TrayIconBuilder};
    use windows_sys::Win32::Foundation::{LPARAM, WPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MSG, PostThreadMessageW, TranslateMessage, WM_QUIT,
    };

    use super::TrayEvent;

    /// A live icon, and the thread pumping messages for it.
    ///
    /// Holding one is what keeps the icon up; dropping it takes the icon down
    /// and ends the pump.
    pub struct Tray {
        /// The thread to wake so it can exit. `GetMessageW` blocks, so dropping
        /// the receiver is not enough to end it.
        thread_id: u32,
        joiner: Option<std::thread::JoinHandle<()>>,
    }

    impl Tray {
        /// Put an icon in the notification area for `channel`.
        ///
        /// `None` means there is no icon and the daemon carries on without one.
        ///
        /// The receiver is handed back rather than polled, so the caller can
        /// block on it — there is no clock here to hang a poll on.
        pub fn start(segment: &str, start_on_login: bool) -> Option<(Self, Receiver<TrayEvent>)> {
            let (events, receiver) = channel();
            let (ready, is_ready) = channel();
            let tooltip = format!("Koru Store — {segment}");

            // A real OS thread, not a task: the icon belongs to this thread for
            // its whole life and the message loop blocks.
            let joiner = std::thread::Builder::new()
                .name("koru-store-server tray".to_string())
                .spawn(move || pump(&tooltip, start_on_login, events, &ready))
                .ok()?;

            // Wait for the thread to say whether it got an icon, so a caller
            // that was told "yes" can rely on there being one.
            match is_ready.recv() {
                Ok(Some(thread_id)) => Some((
                    Self {
                        thread_id,
                        joiner: Some(joiner),
                    },
                    receiver,
                )),
                _ => {
                    let _ = joiner.join();
                    None
                }
            }
        }

        /// Take the icon down and end the thread.
        ///
        /// Posting `WM_QUIT` is what wakes `GetMessageW`; without it the thread
        /// sits in the loop and the process never exits.
        pub fn stop(&mut self) {
            // SAFETY: a thread id and a message, neither of which is a pointer.
            unsafe {
                PostThreadMessageW(
                    self.thread_id,
                    WM_QUIT,
                    WPARAM::default(),
                    LPARAM::default(),
                )
            };
            if let Some(joiner) = self.joiner.take() {
                let _ = joiner.join();
            }
        }
    }

    impl Drop for Tray {
        fn drop(&mut self) {
            // So the notification area never keeps an entry for a process that
            // has gone.
            self.stop();
        }
    }

    /// The thread body: build the icon, then pump until `WM_QUIT`.
    fn pump(
        tooltip: &str,
        start_on_login: bool,
        events: Sender<TrayEvent>,
        ready: &Sender<Option<u32>>,
    ) {
        let Some((_icon, _menu, ids)) = build(tooltip, start_on_login) else {
            let _ = ready.send(None);
            return;
        };

        // SAFETY: `GetCurrentThreadId` takes nothing and returns an integer.
        let thread_id = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
        let _ = ready.send(Some(thread_id));

        let mut message: MSG = unsafe { std::mem::zeroed() };
        // SAFETY: `message` is a valid, writable `MSG` for the whole loop, and
        // a null window filter means "every message for this thread".
        unsafe {
            while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
                // **Drained here, and only here.** A menu click *is* a window
                // message, so muda has filled its channel by the time dispatch
                // returns — this is push-driven despite reading a channel, and
                // there is no timer anywhere.
                //
                // Not `MenuEvent::set_event_handler`, which demands a `Send`
                // closure: `CheckMenuItem` holds `Rc` internals and cannot
                // leave this thread, and its post-toggle state is exactly what
                // has to be read.
                drain(&ids, &events);
            }
        }
        // The icon is dropped here, on the thread that created it, which is the
        // only thread allowed to.
    }

    /// Turn whatever muda has queued into events the daemon understands.
    fn drain(ids: &Ids, events: &Sender<TrayEvent>) {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let id = event.id().0.clone();
            let sent = if id == ids.location_id {
                Some(TrayEvent::OpenStoreLocation)
            } else if id == ids.backup_id {
                Some(TrayEvent::BackupStore)
            } else if id == ids.startup_id {
                // Read back *after* muda has toggled it, so this is the state
                // the person now sees rather than the one they saw.
                Some(TrayEvent::StartOnLogin(ids.startup.is_checked()))
            } else if id == ids.quit_id {
                Some(TrayEvent::Quit)
            } else {
                None
            };
            if let Some(event) = sent {
                let _ = events.send(event);
            }
        }
    }

    /// The menu item ids, kept so a click can be matched back.
    struct Ids {
        location_id: String,
        backup_id: String,
        startup_id: String,
        quit_id: String,
        /// Held whole, because its *state* has to be read after a toggle.
        startup: CheckMenuItem,
    }

    fn build(tooltip: &str, start_on_login: bool) -> Option<(tray_icon::TrayIcon, Menu, Ids)> {
        // The executable's own icon, resource 1 — read from the binary rather
        // than from `assets/`, so there is no path to resolve and a bundle
        // missing its asset folder still shows the right picture.
        let icon = Icon::from_resource(1, None).ok()?;

        let menu = Menu::new();
        let location = MenuItem::new("Open store location", true, None);
        let backup = MenuItem::new("Backup Store…", true, None);
        let startup = CheckMenuItem::new("Start on Windows startup", true, start_on_login, None);
        let quit = MenuItem::new("Quit", true, None);

        let ids = Ids {
            location_id: location.id().0.clone(),
            backup_id: backup.id().0.clone(),
            startup_id: startup.id().0.clone(),
            quit_id: quit.id().0.clone(),
            startup: startup.clone(),
        };

        // A partially built menu is treated as no tray at all: an icon whose
        // menu is missing Quit is the unstoppable process this exists to
        // prevent.
        menu.append(&location).ok()?;
        menu.append(&backup).ok()?;
        menu.append(&tray_icon::menu::PredefinedMenuItem::separator())
            .ok()?;
        menu.append(&startup).ok()?;
        menu.append(&quit).ok()?;

        let icon = TrayIconBuilder::new()
            .with_tooltip(tooltip)
            .with_icon(icon)
            .with_menu(Box::new(menu.clone()))
            .build()
            .ok()?;
        Some((icon, menu, ids))
    }
}

#[cfg(not(windows))]
mod stub {
    use super::TrayEvent;

    /// No notification area here. `start` answering `None` leaves the daemon
    /// exactly the console daemon it was, which is what a platform with no tray
    /// should do.
    pub struct Tray;

    impl Tray {
        pub fn start(
            _segment: &str,
            _start_on_login: bool,
        ) -> Option<(Self, std::sync::mpsc::Receiver<TrayEvent>)> {
            None
        }

        // Unused here by construction: the stub's point is that both
        // platforms present one surface to the daemon.
        #[allow(dead_code)]
        pub fn stop(&mut self) {}
    }
}

// ------------------------------------------------------- wiring it to a store

/// The base name this store's startup entry lives under.
///
/// **Not `Koru`**, which is the app's. Two programs writing one value would mean
/// enabling either silently disabled the other.
///
/// A base, and never the value name itself: [`run_entry`] qualifies it with this
/// build's channel first.
pub const RUN_ENTRY: &str = "Koru Store";

/// The value name this build actually reads and writes.
///
/// `Koru Store` on stable, `Koru Store (nightly)` on a nightly. The five
/// channels install side by side and share nothing else — separate binaries,
/// separate `%LOCALAPPDATA%\Koru\<segment>\` — and the Run key was the last
/// place two of them still met: enabling *start on Windows startup* in a nightly
/// overwrote stable's entry with the nightly path, and unticking it in either
/// removed it for both.
///
/// **Every mention of the entry goes through this**, the tick in
/// [`attach`] included. A menu item read from one value name and written to
/// another would be a worse bug than the one being fixed, because it would look
/// like the setting simply did not take.
pub fn run_entry() -> String {
    koru_autostart::channel_entry(RUN_ENTRY)
}

/// Put an icon up for a running store, and act on what it is asked.
///
/// The returned value must be held: dropping it takes the icon down, which is
/// what stops the notification area keeping an entry for a process that has
/// gone.
pub fn attach(segment: &str, state: std::sync::Arc<koru_store_api::AppState>) -> Option<Tray> {
    let starts_with_windows = koru_autostart::is_set(&run_entry());
    let Some((tray, events)) = Tray::start(segment, starts_with_windows) else {
        tracing::warn!("no notification-area icon; the store is running without one");
        return None;
    };
    tracing::info!("in the notification area ({segment})");

    let root = state.root.clone();
    // A plain thread, blocking on the channel. Not a tokio task: it does no
    // async work, and blocking a runtime worker on a `std` receiver would be
    // exactly the thing tokio warns about.
    std::thread::Builder::new()
        .name("koru-store-server tray events".to_string())
        .spawn(move || {
            while let Ok(event) = events.recv() {
                match event {
                    TrayEvent::OpenStoreLocation => open_store_location(&root),
                    TrayEvent::BackupStore => backup_store(&root, &state),
                    TrayEvent::StartOnLogin(wanted) => set_start_on_login(wanted),
                    TrayEvent::Quit => {
                        tracing::info!("quit, from the notification area");
                        // `notify_one`, not `notify_waiters`: this can arrive
                        // before `axum::serve` has parked on the signal, and
                        // `notify_waiters` wakes only what is already waiting —
                        // a click in that window would be swallowed. `notify_one`
                        // leaves a permit, so a later wait completes at once.
                        state.shutdown.notify_one();
                        return;
                    }
                }
            }
        })
        .ok()?;
    Some(tray)
}

/// Show the store's directory in the file manager.
///
/// **The one place the daemon hands out its own path deliberately.** The debug
/// route reports it as an opaque string that a client must not take apart —
/// knowing where a database is is not permission to open it — but the person
/// looking at their own tray icon is not a client, and "where is my data" is the
/// first question anybody asks of a store.
fn open_store_location(root: &str) {
    // `explorer` returns a non-zero exit code even when it worked, so the
    // status is deliberately not checked — only whether it could be started.
    let opener = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(opener).arg(root).spawn() {
        tracing::warn!("could not open {root}: {e}");
    }
}

/// Ask for — or stop asking for — this store at sign-in.
#[cfg(windows)]
fn backup_store(root: &str, state: &std::sync::Arc<koru_store_api::AppState>) {
    let suggested = format!(
        "Koru Store {}.koru-store",
        koru_store_core::model::Timestamp::now().millis()
    );
    let Some(destination) = rfd::FileDialog::new()
        .set_title("Backup the Koru Store")
        .set_file_name(&suggested)
        .add_filter("Koru Store backup", &["koru-store"])
        .save_file()
    else {
        return;
    };
    let result = state
        .store
        .lock()
        .map_err(|_| "the Store lock was poisoned".to_string())
        .and_then(|_store| {
            crate::store_backup::create(std::path::Path::new(root), &destination)
                .map_err(|error| error.to_string())
        });
    let (level, title, description) = match result {
        Ok(()) => (
            rfd::MessageLevel::Info,
            "Store backup complete",
            destination.display().to_string(),
        ),
        Err(error) => (
            rfd::MessageLevel::Error,
            "The Store could not be backed up",
            error,
        ),
    };
    rfd::MessageDialog::new()
        .set_level(level)
        .set_title(title)
        .set_description(description)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

#[cfg(not(windows))]
fn backup_store(_root: &str, _state: &std::sync::Arc<koru_store_api::AppState>) {}

fn set_start_on_login(wanted: bool) {
    let entry = run_entry();
    let outcome = if wanted {
        match std::env::current_exe() {
            // No arguments: serving is already the default, and a tray launch
            // is what it does with none.
            Ok(exe) => koru_autostart::set(&entry, &koru_autostart::quoted(&exe)),
            Err(e) => Err(e),
        }
    } else {
        koru_autostart::clear(&entry)
    };
    match outcome {
        Ok(()) => tracing::info!("start on Windows startup: {wanted}"),
        // The menu item has already toggled itself, so it now disagrees with
        // the registry. Saying so is all that can be done from here; the next
        // launch reads the registry and the two agree again.
        Err(e) => tracing::error!("could not change the startup setting: {e}"),
    }
}
