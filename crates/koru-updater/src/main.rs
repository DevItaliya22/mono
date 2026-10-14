#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
// SPDX-License-Identifier: MPL-2.0

mod install;
mod progress;

use install::{Outcome, run};
use std::path::PathBuf;
use std::time::Duration;

fn main() {
    let command = match parse_args(std::env::args()) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("koru-updater: {error}");
            std::process::exit(2);
        }
    };
    if command == Command::Test {
        progress::show(
            "Koru update",
            "Installing a test update. Koru will reopen when it is ready.",
            || std::thread::sleep(Duration::from_secs(3)),
        );
        return;
    }
    let Command::Install { request, wait_pid } = command else {
        unreachable!("the test command returned above");
    };
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    progress::show(
        "Koru update",
        "Installing the update. Koru will reopen when it is ready.",
        move || {
            let _ = result_tx.send(run(&request, wait_pid));
        },
    );
    match result_rx
        .recv()
        .unwrap_or_else(|_| Err("the updater stopped unexpectedly".into()))
    {
        Ok(Outcome::Installed) => {}
        Ok(Outcome::RolledBack(reason)) => {
            eprintln!("koru-updater: update rolled back: {reason}");
            std::process::exit(3);
        }
        Err(error) => {
            eprintln!("koru-updater: {error}");
            std::process::exit(1);
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Install { request: PathBuf, wait_pid: u32 },
    Test,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut args = args.into_iter().skip(1);
    let mut request = None;
    let mut wait_pid = None;
    while let Some(arg) = args.next() {
        let value = if let Some((name, value)) = arg.split_once('=') {
            (name.to_string(), Some(value.to_string()))
        } else {
            (arg, None)
        };
        match value.0.as_str() {
            "--test-upgrade" if request.is_none() && wait_pid.is_none() => {
                return Ok(Command::Test);
            }
            "--request" => {
                request = Some(PathBuf::from(
                    value
                        .1
                        .or_else(|| args.next())
                        .ok_or("--request needs a path")?,
                ));
            }
            "--wait-pid" => {
                wait_pid = Some(
                    value
                        .1
                        .or_else(|| args.next())
                        .ok_or("--wait-pid needs a process id")?
                        .parse()
                        .map_err(|_| "--wait-pid is not a process id")?,
                );
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Command::Install {
        request: request.ok_or("missing --request")?,
        wait_pid: wait_pid.ok_or("missing --wait-pid")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_arguments_accept_both_value_spellings() {
        let parsed = parse_args([
            "updater".into(),
            "--request=C:/request.json".into(),
            "--wait-pid".into(),
            "42".into(),
        ])
        .unwrap();
        assert_eq!(
            parsed,
            Command::Install {
                request: PathBuf::from("C:/request.json"),
                wait_pid: 42,
            }
        );
        assert_eq!(
            parse_args(["updater".into(), "--test-upgrade".into()]),
            Ok(Command::Test)
        );
        assert!(parse_args(["updater".into(), "--nope".into()]).is_err());
    }
}
