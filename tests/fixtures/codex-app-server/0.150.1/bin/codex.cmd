@echo off
rem Absolute path on purpose: under `cargo test` the harness PATH carries
rem every native build-script directory (59 whisper-rs-sys entries, ~13k
rem chars) and cmd.exe stops searching PATH past its 8191-character
rem expansion limit, so a bare `powershell.exe` is not found. The real npm
rem shim resolves via PATH; this fixture must work where the harness runs.
%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%~dp0codex-fixture.ps1" %*
