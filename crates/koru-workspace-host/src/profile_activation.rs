// SPDX-License-Identifier: MPL-2.0

//! Machine-local routing for a stable search target to an already-open profile.
//!
//! The taskbar activation pipe predates profiles and is intentionally global.
//! Its `open <node>` message cannot route a global-search hit: node ids have no
//! meaning without their profile and workspace. This channel is therefore
//! keyed by `ProfileId` and carries the complete target. One process per
//! profile owns the listener; another process for that profile may still edit
//! through its own Loro replica, but activations converge on one visible window.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use koru_core::{DocumentId, NodeId, ProfileId, WorkspaceId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub workspace: WorkspaceId,
    pub document: Option<DocumentId>,
    pub node: Option<NodeId>,
    pub match_text: Option<String>,
}

impl Target {
    #[cfg(any(windows, test))]
    fn encode(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.workspace.to_storage(),
            self.document.map(|id| id.to_storage()).unwrap_or_default(),
            self.node.map(|id| id.to_storage()).unwrap_or_default(),
            self.match_text
                .as_deref()
                .map(encode_text)
                .unwrap_or_default(),
        )
    }

    #[cfg(any(windows, test))]
    fn decode(line: &str) -> Option<Self> {
        let mut fields = line.split('|');
        let workspace = WorkspaceId::parse(fields.next()?)?;
        let document = parse_optional_document(fields.next()?)?;
        let node = parse_optional_node(fields.next()?)?;
        let match_text = match fields.next() {
            None | Some("") => None,
            Some(encoded) => Some(decode_text(encoded)?),
        };
        (fields.next().is_none()).then_some(Self {
            workspace,
            document,
            node,
            match_text,
        })
    }
}

#[cfg(any(windows, test))]
fn encode_text(text: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(text.len() * 2);
    for byte in text.as_bytes() {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[cfg(any(windows, test))]
fn decode_text(encoded: &str) -> Option<String> {
    if encoded.len() % 2 != 0 {
        return None;
    }
    let bytes = encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            Some(((high << 4) | low) as u8)
        })
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}

#[cfg(any(windows, test))]
fn parse_optional_node(value: &str) -> Option<Option<NodeId>> {
    if value.is_empty() {
        Some(None)
    } else {
        NodeId::parse(value).map(Some)
    }
}

#[cfg(any(windows, test))]
fn parse_optional_document(value: &str) -> Option<Option<DocumentId>> {
    if value.is_empty() {
        Some(None)
    } else {
        DocumentId::parse(value).map(Some)
    }
}

pub(crate) struct Inbox {
    // `profile` and `owned` are read by Drop's cfg(windows) arm only — the
    // route to unbind exists on no other platform.
    #[cfg_attr(not(windows), allow(dead_code))]
    profile: Option<ProfileId>,
    pending: Arc<Mutex<VecDeque<Target>>>,
    stop: Arc<AtomicBool>,
    #[cfg_attr(not(windows), allow(dead_code))]
    owned: bool,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Inbox {
    /// `None` is a window with no profile seated — first run (S21). There is
    /// nothing to name the mailbox after and nobody who could route to it, so
    /// the inbox exists and stays empty rather than the caller holding an
    /// `Option<Inbox>` every reader would have to unwrap.
    pub(crate) fn bind(profile: Option<ProfileId>) -> Self {
        let pending = Arc::new(Mutex::new(VecDeque::new()));
        let stop = Arc::new(AtomicBool::new(false));
        #[cfg(windows)]
        let thread =
            profile.and_then(|profile| windows_impl::bind(profile, pending.clone(), stop.clone()));
        #[cfg(not(windows))]
        let thread = None;
        let owned = thread.is_some();
        let _ = profile;
        Self {
            profile,
            pending,
            stop,
            owned,
            thread,
        }
    }

    pub(crate) fn take(&self) -> Option<Target> {
        self.pending.lock().ok()?.pop_front()
    }
}

impl Drop for Inbox {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        #[cfg(windows)]
        if let Some(profile) = self.profile.filter(|_| self.owned) {
            windows_impl::stop(profile);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(crate) fn signal(profile: ProfileId, target: Target) -> bool {
    #[cfg(windows)]
    {
        windows_impl::signal(profile, target)
    }
    #[cfg(not(windows))]
    {
        let _ = (profile, target);
        false
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_PIPE_CONNECTED, HANDLE, HWND, LPARAM, WPARAM,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_GENERIC_WRITE,
        FILE_SHARE_MODE, OPEN_EXISTING, PIPE_ACCESS_INBOUND, ReadFile, WriteFile,
    };
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId,
        PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, EnumWindows, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        PostMessageW, SW_RESTORE, SetForegroundWindow, ShowWindowAsync, WM_NULL,
    };
    use windows::core::{BOOL, Error, HRESULT, PCWSTR, Result};

    const MESSAGE_MAX: usize = 4096;

    pub(super) fn bind(
        profile: ProfileId,
        pending: Arc<Mutex<VecDeque<Target>>>,
        stop: Arc<AtomicBool>,
    ) -> Option<std::thread::JoinHandle<()>> {
        let pipe = match first_pipe(profile) {
            Ok(pipe) => pipe,
            Err(error) => {
                // Another process for this profile normally owns routing. Log
                // the OS result because the same branch also covers genuine
                // pipe-creation failures.
                crate::search_debug!(
                    "[Koru][search-open][pid={}] listener not owned: profile={} error={error}",
                    std::process::id(),
                    profile.to_storage(),
                );
                return None;
            }
        };
        crate::search_debug!(
            "[Koru][search-open][pid={}] listener owned: profile={}",
            std::process::id(),
            profile.to_storage(),
        );
        let raw_pipe = pipe.0 as usize;
        match std::thread::Builder::new()
            .name("koru-profile-activation".into())
            .spawn(move || {
                serve(
                    profile,
                    HANDLE(raw_pipe as *mut std::ffi::c_void),
                    pending,
                    stop,
                )
            }) {
            Ok(thread) => Some(thread),
            Err(error) => {
                crate::search_debug!(
                    "[Koru][search-open][pid={}] listener thread failed: profile={} error={error}",
                    std::process::id(),
                    profile.to_storage(),
                );
                let _ = unsafe { CloseHandle(HANDLE(raw_pipe as *mut std::ffi::c_void)) };
                None
            }
        }
    }

    /// Wake a listener blocked in `ConnectNamedPipe` so dropping an inbox can
    /// release that profile's single-instance route before rebinding another.
    pub(super) fn stop(profile: ProfileId) {
        let name = pipe_name(profile);
        unsafe {
            let pipe = CreateFileW(
                PCWSTR(name.as_ptr()),
                FILE_GENERIC_WRITE.0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            );
            if let Ok(pipe) = pipe {
                let _ = CloseHandle(pipe);
            }
        }
    }

    pub(super) fn signal(profile: ProfileId, target: Target) -> bool {
        let name = pipe_name(profile);
        let name = PCWSTR(name.as_ptr());
        unsafe {
            if !WaitNamedPipeW(name, 0).as_bool() {
                let error = Error::from_thread();
                crate::search_debug!(
                    "[Koru][navigate][pid={}] ipc unavailable: profile={} workspace={} document={} node={} error={error}",
                    std::process::id(),
                    profile.to_storage(),
                    target.workspace.to_storage(),
                    target
                        .document
                        .map(|id| id.to_storage())
                        .as_deref()
                        .unwrap_or("-"),
                    target
                        .node
                        .map(|id| id.to_storage())
                        .as_deref()
                        .unwrap_or("-"),
                );
                return false;
            }
            let pipe = match CreateFileW(
                name,
                FILE_GENERIC_WRITE.0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            ) {
                Ok(pipe) => pipe,
                Err(error) => {
                    crate::search_debug!(
                        "[Koru][search-open][pid={}] ipc connect failed: profile={} workspace={} error={error}",
                        std::process::id(),
                        profile.to_storage(),
                        target.workspace.to_storage(),
                    );
                    return false;
                }
            };
            let mut server = 0;
            match GetNamedPipeServerProcessId(pipe, &mut server) {
                Ok(()) => {
                    let allowed = AllowSetForegroundWindow(server).is_ok();
                    crate::search_debug!(
                        "[Koru][search-open][pid={}] ipc connected: profile={} server_pid={server} foreground_permission={allowed}",
                        std::process::id(),
                        profile.to_storage(),
                    );
                }
                Err(error) => crate::search_debug!(
                    "[Koru][search-open][pid={}] ipc server lookup failed: profile={} error={error}",
                    std::process::id(),
                    profile.to_storage(),
                ),
            }
            let line = target.encode();
            let bytes = line.as_bytes();
            let mut written = 0;
            let sent = if bytes.len() > MESSAGE_MAX {
                crate::search_debug!(
                    "[Koru][search-open][pid={}] ipc message too large: profile={} bytes={} limit={MESSAGE_MAX}",
                    std::process::id(),
                    profile.to_storage(),
                    bytes.len(),
                );
                false
            } else {
                match WriteFile(pipe, Some(bytes), Some(&mut written), None) {
                    Ok(()) if written as usize == bytes.len() => true,
                    Ok(()) => {
                        crate::search_debug!(
                            "[Koru][search-open][pid={}] ipc short write: profile={} expected={} written={written}",
                            std::process::id(),
                            profile.to_storage(),
                            bytes.len(),
                        );
                        false
                    }
                    Err(error) => {
                        crate::search_debug!(
                            "[Koru][search-open][pid={}] ipc write failed: profile={} error={error}",
                            std::process::id(),
                            profile.to_storage(),
                        );
                        false
                    }
                }
            };
            let _ = CloseHandle(pipe);
            crate::search_debug!(
                "[Koru][navigate][pid={}] ipc send complete: profile={} workspace={} document={} node={} sent={sent}",
                std::process::id(),
                profile.to_storage(),
                target.workspace.to_storage(),
                target
                    .document
                    .map(|id| id.to_storage())
                    .as_deref()
                    .unwrap_or("-"),
                target
                    .node
                    .map(|id| id.to_storage())
                    .as_deref()
                    .unwrap_or("-"),
            );
            sent
        }
    }

    fn first_pipe(profile: ProfileId) -> Result<HANDLE> {
        let name = pipe_name(profile);
        unsafe {
            let pipe = CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                0,
                1,
                0,
                None,
            );
            if pipe.is_invalid() {
                Err(Error::from_thread())
            } else {
                Ok(pipe)
            }
        }
    }

    fn pipe_name(profile: ProfileId) -> Vec<u16> {
        format!(r"\\.\pipe\Koru.Profile.{}", profile.to_storage())
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect()
    }

    fn serve(
        profile: ProfileId,
        pipe: HANDLE,
        pending: Arc<Mutex<VecDeque<Target>>>,
        stop: Arc<AtomicBool>,
    ) {
        loop {
            if stop.load(Ordering::Acquire) {
                break;
            }
            let connected = unsafe {
                match ConnectNamedPipe(pipe, None) {
                    Ok(()) => true,
                    Err(error) if error.code() == HRESULT::from_win32(ERROR_PIPE_CONNECTED.0) => {
                        true
                    }
                    Err(error) => {
                        crate::search_debug!(
                            "[Koru][search-open][pid={}] listener connect failed; listener stopping: profile={} error={error}",
                            std::process::id(),
                            profile.to_storage(),
                        );
                        false
                    }
                }
            };
            if !connected {
                break;
            }
            if stop.load(Ordering::Acquire) {
                break;
            }
            let mut buffer = [0_u8; MESSAGE_MAX];
            let mut read = 0;
            let received = unsafe { ReadFile(pipe, Some(&mut buffer), Some(&mut read), None) };
            let received = match received {
                Ok(()) if read > 0 => true,
                Ok(()) => {
                    crate::search_debug!(
                        "[Koru][search-open][pid={}] listener received empty message: profile={}",
                        std::process::id(),
                        profile.to_storage(),
                    );
                    false
                }
                Err(error) => {
                    crate::search_debug!(
                        "[Koru][search-open][pid={}] listener read failed: profile={} error={error}",
                        std::process::id(),
                        profile.to_storage(),
                    );
                    false
                }
            };
            if received {
                match std::str::from_utf8(&buffer[..read as usize])
                    .ok()
                    .and_then(Target::decode)
                {
                    Some(target) => {
                        let workspace = target.workspace;
                        let document = target.document;
                        let node = target.node;
                        let queued = match pending.lock() {
                            Ok(mut queue) => {
                                queue.push_back(target);
                                Some(queue.len())
                            }
                            Err(error) => {
                                crate::search_debug!(
                                    "[Koru][search-open][pid={}] listener queue poisoned: profile={} error={error}",
                                    std::process::id(),
                                    profile.to_storage(),
                                );
                                None
                            }
                        };
                        crate::search_debug!(
                            "[Koru][navigate][pid={}] ipc received: profile={} workspace={} document={} node={} queue_depth={}",
                            std::process::id(),
                            profile.to_storage(),
                            workspace.to_storage(),
                            document.map(|id| id.to_storage()).as_deref().unwrap_or("-"),
                            node.map(|id| id.to_storage()).as_deref().unwrap_or("-"),
                            queued
                                .map(|depth| depth.to_string())
                                .as_deref()
                                .unwrap_or("error"),
                        );
                        if queued.is_some() {
                            foreground_koru_window(profile);
                        }
                    }
                    None => crate::search_debug!(
                        "[Koru][search-open][pid={}] listener rejected malformed message: profile={} bytes={read}",
                        std::process::id(),
                        profile.to_storage(),
                    ),
                }
            }
            let _ = unsafe { DisconnectNamedPipe(pipe) };
            if stop.load(Ordering::Acquire) {
                break;
            }
        }
        let _ = unsafe { CloseHandle(pipe) };
    }

    fn foreground_koru_window(profile: ProfileId) {
        struct Search {
            pid: u32,
            any_window: HWND,
            visible_window: HWND,
        }

        unsafe extern "system" fn visit(window: HWND, state: LPARAM) -> BOOL {
            let state = unsafe { &mut *(state.0 as *mut Search) };
            let mut pid = 0;
            unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
            if pid == state.pid {
                if state.any_window.0.is_null() {
                    state.any_window = window;
                }
                if unsafe { IsWindowVisible(window).as_bool() } {
                    state.visible_window = window;
                    return BOOL(0);
                }
            }
            BOOL(1)
        }

        let mut search = Search {
            pid: std::process::id(),
            any_window: HWND(std::ptr::null_mut()),
            visible_window: HWND(std::ptr::null_mut()),
        };
        let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize)) };
        let wake_window = if search.visible_window.0.is_null() {
            search.any_window
        } else {
            search.visible_window
        };
        if wake_window.0.is_null() {
            crate::search_debug!(
                "[Koru][search-open][pid={}] foreground skipped: profile={} no_process_window=true",
                std::process::id(),
                profile.to_storage(),
            );
            return;
        }
        unsafe {
            // WM_NULL changes no state; it only wakes winit's Windows message
            // loop so `about_to_wait` can drain the queue immediately. Hidden
            // tray windows still have an HWND and therefore remain routable.
            let event_woken =
                PostMessageW(Some(wake_window), WM_NULL, WPARAM(0), LPARAM(0)).is_ok();
            let visible = !search.visible_window.0.is_null();
            let minimized = visible && IsIconic(search.visible_window).as_bool();
            let restored = !visible
                || !minimized
                || ShowWindowAsync(search.visible_window, SW_RESTORE).as_bool();
            let foregrounded = visible && SetForegroundWindow(search.visible_window).as_bool();
            crate::search_debug!(
                "[Koru][search-open][pid={}] foreground attempted: profile={} hwnd={} visible={visible} event_woken={event_woken} minimized={minimized} restored={restored} foregrounded={foregrounded}",
                std::process::id(),
                profile.to_storage(),
                wake_window.0 as usize,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_wire_format_round_trips_and_rejects_partial_messages() {
        let target = Target {
            workspace: WorkspaceId::new(),
            document: Some(DocumentId::new()),
            node: Some(NodeId::new()),
            match_text: Some("emoji 🤝".into()),
        };
        assert_eq!(Target::decode(&target.encode()), Some(target.clone()));
        assert_eq!(Target::decode("not-a-workspace||"), None);
        assert_eq!(Target::decode(&format!("{}|", target.workspace)), None);
    }

    #[cfg(windows)]
    #[test]
    fn a_target_is_delivered_only_to_its_profile_inbox() {
        let profile = ProfileId::new();
        let other_profile = ProfileId::new();
        let inbox = Inbox::bind(Some(profile));
        let target = Target {
            workspace: WorkspaceId::new(),
            document: Some(DocumentId::new()),
            node: Some(NodeId::new()),
            match_text: Some("match".into()),
        };

        assert!(!signal(other_profile, target.clone()));
        assert!(signal(profile, target.clone()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let received = loop {
            if let Some(received) = inbox.take() {
                break Some(received);
            }
            if std::time::Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(received, Some(target));
    }

    #[cfg(windows)]
    #[test]
    fn dropping_an_inbox_releases_the_profile_route_for_rebinding() {
        let profile = ProfileId::new();
        let first = Inbox::bind(Some(profile));
        assert!(first.owned, "the first inbox should own its fresh route");
        drop(first);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            let rebound = Inbox::bind(Some(profile));
            if rebound.owned {
                break;
            }
            drop(rebound);
            assert!(
                std::time::Instant::now() < deadline,
                "the dropped profile route was not released"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
