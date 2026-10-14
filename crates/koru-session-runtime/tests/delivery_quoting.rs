// SPDX-License-Identifier: MPL-2.0

use std::path::PathBuf;
use std::process::Command;

use koru_session_protocol::ShellQuoting;
use koru_session_runtime::quote_path;

const ADVERSARIAL_PATH: &str =
    "C:\\Koru space\\quote'\" and newline\nUnicode-雪-مرحبا;$()&|<>*.png";

#[test]
fn real_powershell_parser_receives_one_literal_adversarial_path() {
    #[cfg(not(windows))]
    let executable = "pwsh";
    #[cfg(windows)]
    let executable = "powershell.exe";

    if Command::new(executable)
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg("exit 0")
        .status()
        .is_err()
    {
        return;
    }

    let temporary = tempfile::tempdir().unwrap();
    let output_path = temporary.path().join("literal.txt");
    let output = output_path.to_string_lossy();
    let script = format!(
        "$value = {}; [IO.File]::WriteAllText({}, $value, (New-Object Text.UTF8Encoding($false)))",
        quote_path(ADVERSARIAL_PATH, ShellQuoting::PowerShell),
        quote_path(&output, ShellQuoting::PowerShell),
    );
    let result = Command::new(executable)
        .arg("-NoLogo")
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg(script)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "PowerShell parser failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        std::fs::read(output_path).unwrap(),
        ADVERSARIAL_PATH.as_bytes()
    );
}

#[test]
fn real_bash_parser_receives_one_literal_adversarial_path() {
    let Some(executable) = bash_executable() else {
        return;
    };
    let script = format!(
        "printf '%s' {}",
        quote_path(ADVERSARIAL_PATH, ShellQuoting::Posix)
    );
    let result = Command::new(executable)
        .arg("--noprofile")
        .arg("--norc")
        .arg("-c")
        .arg(script)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "Bash parser failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, ADVERSARIAL_PATH.as_bytes());
}

fn bash_executable() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let program_files = std::env::var_os("ProgramFiles")?;
        let candidate = PathBuf::from(program_files).join("Git/bin/bash.exe");
        candidate.is_file().then_some(candidate)
    }
    #[cfg(not(windows))]
    {
        Some(PathBuf::from("bash"))
    }
}
