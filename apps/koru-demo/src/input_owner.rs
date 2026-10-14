// SPDX-License-Identifier: MPL-2.0

//! Positive storage ownership before a runner may deliver input.
//!
//! A live root rendezvous and a Koru HWND do not link the two. Read the selected
//! process's explicit launch root, then retain its process object through every
//! action. The bounded, read-only CIM subprocess avoids private PEB layouts and
//! another COM stack. Its command line is never included in diagnostics.

#[cfg(windows)]
use std::path::Path;

/// Compare argv supplied by Windows, without filesystem access or guessing a
/// foreign process's working directory. Keep normalization aligned with
/// `koru-ipc/src/endpoint.rs::root_identity`: no canonicalization or dot folding.
// Both are the Windows proof's, and the tests' on every platform.
#[cfg(any(windows, test))]
fn matches_root(root: &str, argv: &[String]) -> Result<(), String> {
    let mut found = None;
    let mut args = argv.iter().skip(1);
    while let Some(arg) = args.next() {
        let value = if arg == "--root" {
            Some(
                args.next()
                    .ok_or("selected process has no explicit root value")?
                    .as_str(),
            )
        } else {
            arg.strip_prefix("--root=")
        };
        if let Some(value) = value {
            if found.replace(value).is_some() {
                return Err("selected process has ambiguous duplicate root arguments".into());
            }
        }
    }
    let found = found.ok_or("selected process was not launched with an explicit --root")?;
    let expected = root_identity(root)?;
    let actual = root_identity(found)?;
    if expected != actual {
        return Err("selected process belongs to a different explicit root".into());
    }
    Ok(())
}

pub(crate) fn root_identity(value: &str) -> Result<String, String> {
    if value.contains('\0') {
        return Err("root contains an invalid character".into());
    }
    let value = value.replace('/', "\\");
    let ordinary = if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        value.strip_prefix(r"\\?\").unwrap_or(&value).to_owned()
    };
    let bytes = ordinary.as_bytes();
    let drive_absolute =
        bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\';
    let unc_absolute = ordinary.strip_prefix(r"\\").is_some_and(|rest| {
        let mut parts = rest.split('\\');
        parts
            .next()
            .is_some_and(|p| !p.is_empty() && p != "." && p != "?")
            && parts.next().is_some_and(|p| !p.is_empty())
    });
    if !drive_absolute && !unc_absolute {
        return Err("input requires an absolute explicit launch root; relative and inherited roots are not proved".into());
    }
    Ok(ordinary.to_lowercase())
}

/// A completed query is evidence only before cancellation/deadline, and only
/// when the subprocess itself succeeded. Kept pure for fake process timelines.
#[cfg(any(windows, test))]
fn query_progress(cancelled: bool, expired: bool, status: Option<bool>) -> Result<bool, String> {
    if cancelled {
        return Err("process ownership proof cancelled".into());
    }
    if expired {
        return Err("process ownership query timed out".into());
    }
    match status {
        Some(false) => Err("selected process command line could not be proved".into()),
        Some(true) => Ok(true),
        None => Ok(false),
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::io::Read;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::{HANDLE, HLOCAL, LocalFree, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::{
        CREATE_NO_WINDOW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        WaitForSingleObject,
    };
    use windows::Win32::UI::Shell::CommandLineToArgvW;
    use windows::core::PCWSTR;

    const QUERY_LIMIT: usize = 128 * 1024;
    const QUERY_TIMEOUT: Duration = Duration::from_secs(5);

    /// Clones keep the original process object open; a reused numeric PID is
    /// never sufficient. Check `is_alive` immediately before each consequence.
    #[derive(Clone, Debug)]
    pub struct InputOwner {
        handle: Arc<OwnedHandle>,
        process_id: u32,
    }

    impl InputOwner {
        pub fn process_id(&self) -> u32 {
            self.process_id
        }

        pub fn is_alive(&self) -> bool {
            // SAFETY: OwnedHandle remains open for this borrow. A zero wait is
            // nonblocking; failures and exited processes both refuse input.
            unsafe { WaitForSingleObject(HANDLE(self.handle.as_raw_handle()), 0) == WAIT_TIMEOUT }
        }
    }

    pub fn prove_input_target(root: &Path, pid: u32) -> Result<InputOwner, String> {
        prove_input_target_with_cancel(root, pid, &AtomicBool::new(false))
    }

    pub fn prove_input_target_with_cancel(
        root: &Path,
        pid: u32,
        cancelled: &AtomicBool,
    ) -> Result<InputOwner, String> {
        prove_input_target_checked(root, pid, || cancelled.load(Ordering::Acquire))
    }

    pub fn prove_input_target_checked(
        root: &Path,
        pid: u32,
        cancelled: impl Fn() -> bool,
    ) -> Result<InputOwner, String> {
        let root = root.to_str().ok_or("root is not valid Unicode")?;
        root_identity(root)?;
        if pid == 0 || cancelled() {
            return Err("process ownership proof cancelled or invalid PID".into());
        }
        // SAFETY: query/synchronize only, no inheritance; ownership is moved
        // exactly once into OwnedHandle, whose final Arc closes it.
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                false,
                pid,
            )
        }
        .map_err(|_| "cannot open selected process for ownership proof")?;
        let owner = InputOwner {
            handle: Arc::new(unsafe { OwnedHandle::from_raw_handle(handle.0) }),
            process_id: pid,
        };
        if !owner.is_alive() {
            return Err("selected process exited before ownership proof".into());
        }
        let command_line = query_command_line(pid, &cancelled)?;
        matches_root(root, &parse_windows_argv(&command_line)?)?;
        if cancelled() || !owner.is_alive() {
            return Err("selected process exited or ownership proof was cancelled".into());
        }
        Ok(owner)
    }

    fn parse_windows_argv(command_line: &str) -> Result<Vec<String>, String> {
        if command_line.is_empty() || command_line.contains('\0') {
            return Err("selected process command line is unavailable".into());
        }
        let wide: Vec<u16> = command_line.encode_utf16().chain(Some(0)).collect();
        let mut count = 0;
        // SAFETY: input is terminated and count lives through the call. All
        // returned strings share one allocation, released after conversion.
        let args = unsafe { CommandLineToArgvW(PCWSTR(wide.as_ptr()), &mut count) };
        if args.is_null() {
            return Err("could not parse selected process command line".into());
        }
        let result = (|| {
            if count <= 0 || count as usize > QUERY_LIMIT {
                return Err("selected process command line has invalid arguments".into());
            }
            unsafe { std::slice::from_raw_parts(args, count as usize) }
                .iter()
                .map(|arg| {
                    unsafe { arg.to_string() }
                        .map_err(|_| "selected process arguments are not valid Unicode".into())
                })
                .collect()
        })();
        unsafe {
            let _ = LocalFree(Some(HLOCAL(args.cast())));
        }
        result
    }

    struct QueryChild(Child);
    impl Drop for QueryChild {
        fn drop(&mut self) {
            // Request termination of our helper, then close its process handle.
            // Never wait here: a failed kill must not turn the bounded query
            // into an unbounded cleanup. Windows needs no Unix-style reaping.
            // This never terminates the selected Koru process or Store worker.
            let _ = self.0.kill();
        }
    }

    fn query_command_line(pid: u32, cancelled: &impl Fn() -> bool) -> Result<String, String> {
        let system_root =
            std::env::var_os("SystemRoot").ok_or("Windows system directory is unavailable")?;
        let powershell = std::path::PathBuf::from(system_root)
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        // Only a Rust u32 is interpolated. The root and all process content
        // stay outside shell source. CIM emits one bounded UTF-8 value only.
        let script = format!(
            "$ErrorActionPreference='Stop'; \
             $p=Get-CimInstance Win32_Process -Filter 'ProcessId={pid}'; \
             if ($null -eq $p -or [string]::IsNullOrEmpty($p.CommandLine)) {{ exit 3 }}; \
             $b=[Text.Encoding]::UTF8.GetBytes($p.CommandLine); \
             if ($b.Length -gt {QUERY_LIMIT}) {{ exit 4 }}; \
             $s=[Console]::OpenStandardOutput(); $s.Write($b,0,$b.Length)"
        );
        let child = Command::new(powershell)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &script,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW.0)
            .spawn()
            .map_err(|_| "cannot start bounded process ownership query")?;
        let mut child = QueryChild(child);
        let output = child
            .0
            .stdout
            .take()
            .ok_or("ownership query has no output pipe")?;
        let (send, receive) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = output
                .take((QUERY_LIMIT + 1) as u64)
                .read_to_end(&mut bytes)
                .map(|_| bytes);
            let _ = send.send(result);
        });
        let started = Instant::now();
        let mut completed = false;
        loop {
            let status = if completed {
                Some(true)
            } else {
                child
                    .0
                    .try_wait()
                    .map_err(|_| "cannot observe ownership query completion")?
                    .map(|status| status.success())
            };
            completed = query_progress(cancelled(), started.elapsed() >= QUERY_TIMEOUT, status)?;
            if completed && let Ok(result) = receive.try_recv() {
                let bytes = result.map_err(|_| "ownership query output failed")?;
                if bytes.len() > QUERY_LIMIT {
                    return Err("ownership query exceeded its output limit".into());
                }
                return String::from_utf8(bytes)
                    .map_err(|_| "ownership query output is not valid Unicode".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(test)]
    pub(super) fn parse_for_test(value: &str) -> Vec<String> {
        parse_windows_argv(value).unwrap()
    }
}

#[cfg(windows)]
pub use platform::{
    InputOwner, prove_input_target, prove_input_target_checked, prove_input_target_with_cancel,
};

#[cfg(test)]
mod tests;
