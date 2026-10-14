// SPDX-License-Identifier: MPL-2.0

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use super::ProcessTree;

const PROCESS_DEADLINE: Duration = Duration::from_secs(5);

#[test]
fn terminating_the_provider_tree_reaches_a_real_descendant() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("descendant.pid");
    let process_tree = ProcessTree::new().unwrap();
    let mut command = descendant_command(&pid_file);
    process_tree.prepare(&mut command);
    let mut primary = command.spawn().unwrap();
    if let Err(failure) = process_tree.assign(&primary) {
        let _ = primary.kill();
        let _ = primary.wait();
        panic!("could not assign the fixture process tree: {failure}");
    }

    let descendant = match wait_for_pid(&pid_file, &mut primary) {
        Ok(pid) => pid,
        Err(why) => {
            let _ = process_tree.terminate(&mut primary);
            let _ = primary.wait();
            panic!("{why}");
        }
    };
    assert!(process_is_alive(descendant));

    process_tree.terminate(&mut primary).unwrap();
    let _ = primary.wait();
    let deadline = Instant::now() + PROCESS_DEADLINE;
    while process_is_alive(descendant) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !process_is_alive(descendant),
        "the provider's descendant {descendant} survived tree termination"
    );
}

fn wait_for_pid(pid_file: &Path, primary: &mut std::process::Child) -> Result<u32, String> {
    let deadline = Instant::now() + PROCESS_DEADLINE;
    loop {
        if let Ok(text) = std::fs::read_to_string(pid_file)
            && let Ok(pid) = text.trim().parse::<u32>()
        {
            return Ok(pid);
        }
        if primary
            .try_wait()
            .map_err(|error| format!("could not inspect the fixture parent: {error}"))?
            .is_some()
        {
            return Err("the fixture parent exited before publishing its child pid".into());
        }
        if Instant::now() >= deadline {
            return Err("the fixture parent did not publish its child pid".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn descendant_command(pid_file: &Path) -> Command {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg("sleep 60 & child=$!; printf '%s' \"$child\" > \"$KORU_PROCESS_TEST_PID\"; wait \"$child\"")
        .env("KORU_PROCESS_TEST_PID", pid_file);
    command
}

#[cfg(windows)]
fn descendant_command(pid_file: &Path) -> Command {
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$child = Start-Process powershell.exe -ArgumentList @('-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 60') -WindowStyle Hidden -PassThru; [System.IO.File]::WriteAllText($env:KORU_PROCESS_TEST_PID, [string]$child.Id); $child.WaitForExit()",
        ])
        .env("KORU_PROCESS_TEST_PID", pid_file);
    command
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return false;
    }
    let mut exit_code = 0;
    let queried = unsafe { GetExitCodeProcess(process, &mut exit_code) } != 0;
    unsafe { CloseHandle(process) };
    queried && exit_code == STILL_ACTIVE as u32
}
