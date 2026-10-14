// SPDX-License-Identifier: MPL-2.0

//! Thin host for the voice worker.
//!
//! **No `windows_subsystem = "windows"`.** The CLI verbs print, and a
//! supervised launch is spawned with `CREATE_NO_WINDOW` by the agent that
//! starts it — which is where the decision belongs, because the parent knows
//! whether anybody is watching and this binary does not.

fn main() -> std::process::ExitCode {
    koru_voice_worker::run()
}
