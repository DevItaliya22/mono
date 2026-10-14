// SPDX-License-Identifier: MPL-2.0

// Windows starts this executable through the sparse package's COM declaration.
// It must stay invisible and short-lived: the long-running scheduler belongs
// to koru-agent, whose lifetime Windows must never own.
#![cfg_attr(
    all(windows, not(debug_assertions), not(test)),
    windows_subsystem = "windows"
)]

#[cfg(windows)]
fn main() {
    let paths = match koru_agent::paths_for(None) {
        Ok(paths) => paths,
        Err(error) => fatal(&error.to_string()),
    };
    let _logging = koru_log::init_file(
        &paths.store_dir(),
        "koru-com-notification.log",
        "warn,koru=info,koru_agent=info,koru_com_notification=info",
    );
    tracing::info!(
        target: "koru::notification_activation",
        pid = std::process::id(),
        "notification COM broker started"
    );
    match koru_agent::run_windows_notification_com_server() {
        Ok(()) => tracing::info!(
            target: "koru::notification_activation",
            pid = std::process::id(),
            "notification COM broker stopped cleanly"
        ),
        Err(error) => fatal(&error),
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("koru-com-notification is available only on Windows");
}

#[cfg(windows)]
fn fatal(message: &str) -> ! {
    tracing::error!(
        target: "koru::notification_activation",
        error = message,
        "notification COM broker failed"
    );
    std::process::exit(1)
}
