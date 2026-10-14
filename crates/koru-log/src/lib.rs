// SPDX-License-Identifier: MPL-2.0

//! Process-level tracing setup shared by Koru executables.
//!
//! The application chooses its targets, filter, file name, and whether a
//! console layer exists. This crate owns the mechanics that must not drift:
//! logs live under `<store root>/logs`, roll daily, retain seven files, write
//! off-thread, and keep the worker guard alive for the process lifetime.
//!
//! This is deliberately a leaf used by executable crates. Model, Store core,
//! transport, and UI libraries do not inherit a logging framework merely
//! because an embedder links them.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// The directory logs live in, under the Store root.
pub const LOG_DIR: &str = "logs";

/// Held for the logger's life.
///
/// Dropping it stops the non-blocking file writer and can silently lose later
/// records, so initializers return it instead of hiding it in a global.
#[must_use = "dropping the guard stops the log writer, silently"]
pub struct Logging {
    _guard: Option<WorkerGuard>,
}

fn filter(default_filter: &'static str) -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter))
}

/// Log only to stdout. Used by the Store's attached developer console.
pub fn init_stdout(default_filter: &'static str) -> Logging {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter(default_filter))
        .with_target(false)
        .with_writer(std::io::stdout)
        .try_init();
    Logging { _guard: None }
}

/// Log only to one daily rotating file.
pub fn init_file(
    root: &Path,
    filename_prefix: &'static str,
    default_filter: &'static str,
) -> Logging {
    let Some((writer, guard)) = file_writer(root, filename_prefix) else {
        // Logging must never decide whether the process itself can start.
        return Logging { _guard: None };
    };
    let initialized = tracing_subscriber::registry()
        .with(filter(default_filter))
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_ansi(false)
                .with_writer(writer),
        )
        .try_init()
        .is_ok();
    if initialized {
        install_panic_hook();
    }
    Logging {
        _guard: Some(guard),
    }
}

/// Log to stderr and the same rotating file.
///
/// MCP needs both: its host owns stderr, while the shared file is what a person
/// can tail when the host-launched child has no visible console. If the file
/// cannot be opened, stderr logging still starts.
pub fn init_file_and_stderr(
    root: &Path,
    filename_prefix: &'static str,
    default_filter: &'static str,
) -> Logging {
    let Some((writer, guard)) = file_writer(root, filename_prefix) else {
        let initialized = tracing_subscriber::fmt()
            .with_env_filter(filter(default_filter))
            .with_target(false)
            .with_writer(std::io::stderr)
            .try_init()
            .is_ok();
        if initialized {
            install_panic_hook();
        }
        return Logging { _guard: None };
    };
    let initialized = tracing_subscriber::registry()
        .with(filter(default_filter))
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_ansi(false)
                .with_writer(writer),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_writer(std::io::stderr),
        )
        .try_init()
        .is_ok();
    if initialized {
        install_panic_hook();
    }
    Logging {
        _guard: Some(guard),
    }
}

/// Log to stdout and the same rotating file.
///
/// An attached daemon needs the live console while it is being developed, but
/// the file is what survives after the terminal has closed. If the file cannot
/// be opened, stdout logging still starts; diagnostics must never decide
/// whether the process itself may run.
pub fn init_file_and_stdout(
    root: &Path,
    filename_prefix: &'static str,
    default_filter: &'static str,
) -> Logging {
    let Some((writer, guard)) = file_writer(root, filename_prefix) else {
        return init_stdout(default_filter);
    };
    let initialized = tracing_subscriber::registry()
        .with(filter(default_filter))
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_ansi(false)
                .with_writer(writer),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_writer(std::io::stdout),
        )
        .try_init()
        .is_ok();
    if initialized {
        install_panic_hook();
    }
    Logging {
        _guard: Some(guard),
    }
}

fn file_writer(
    root: &Path,
    filename_prefix: &str,
) -> Option<(tracing_appender::non_blocking::NonBlocking, WorkerGuard)> {
    let directory = root.join(LOG_DIR);
    std::fs::create_dir_all(&directory).ok()?;
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(filename_prefix)
        .max_log_files(7)
        .build(&directory)
        .ok()?;
    Some(tracing_appender::non_blocking(appender))
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // A panic in an async transport worker can be followed immediately by
        // a second panic in dependency cleanup, which aborts before a user can
        // scroll back to the original failure. Force the first backtrace into
        // the durable log even when RUST_BACKTRACE was not set.
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!(panic = %info, backtrace = %backtrace, "panic");
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdout_logging_creates_no_log_directory() {
        let dir = tempfile::tempdir().unwrap();
        let _logging = init_stdout("warn,koru_log=info");
        assert!(!dir.path().join(LOG_DIR).exists());
    }

    #[test]
    fn file_logging_creates_the_shared_log_directory() {
        let dir = tempfile::tempdir().unwrap();
        let logging = init_file(dir.path(), "koru-test.log", "warn,koru_log=info");
        assert!(dir.path().join(LOG_DIR).is_dir());
        drop(logging);
    }

    #[test]
    fn stderr_and_file_logging_creates_the_shared_log_directory() {
        let dir = tempfile::tempdir().unwrap();
        let logging = init_file_and_stderr(dir.path(), "koru-test.log", "warn,koru_log=info");
        assert!(dir.path().join(LOG_DIR).is_dir());
        drop(logging);
    }

    #[test]
    fn stdout_and_file_logging_creates_the_shared_log_directory() {
        let dir = tempfile::tempdir().unwrap();
        let logging = init_file_and_stdout(dir.path(), "koru-test.log", "warn,koru_log=info");
        assert!(dir.path().join(LOG_DIR).is_dir());
        drop(logging);
    }

    #[test]
    fn the_requested_prefix_names_the_rotating_file() {
        let dir = tempfile::tempdir().unwrap();
        let (_writer, guard) = file_writer(dir.path(), "koru-mcp.log").unwrap();
        drop(guard);
        let names = std::fs::read_dir(dir.path().join(LOG_DIR))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            names.iter().any(|name| name.starts_with("koru-mcp.log.")),
            "rotating file did not use the requested prefix: {names:?}"
        );
    }

    #[test]
    fn a_root_that_cannot_be_created_is_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("a-file");
        std::fs::write(&blocked, b"not a directory").unwrap();
        let _logging = init_file_and_stderr(&blocked, "koru-test.log", "warn");
    }
}
