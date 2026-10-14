// SPDX-License-Identifier: MPL-2.0

//! The sessions role of `koru-agent.exe`.
//!
//! `koru-agent.exe --worker sessions [--root <path>]` **is** the session
//! host: the process reads the one-line Agent bootstrap from stdin and runs
//! the `koru_session_host::run_supervised` composition. It is now the *only*
//! launch path — the standalone `koru-session-host` binary was deleted in
//! Plan 7 of the Gateway worker roadmap, and the package it lived in keeps its
//! library for exactly this. Everything else about being a worker — spawn, Job
//! Object, readiness, restart — is the supervisor's side, in
//! [`crate::session_host`].

pub fn role() -> koru_agent_worker::Role {
    koru_agent_worker::Role {
        name: "sessions",
        run,
    }
}

fn run(args: Vec<String>) -> ! {
    let mut root: Option<std::path::PathBuf> = None;
    let mut words = args.into_iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            // A *flag* where the path should be is a forgotten value, not a
            // directory literally named `--frobnicate`. This accepted one
            // until Plan 3, which is the silent wrong-root failure the strict
            // rule exists to refuse; `koru_workspace_host::cli::parse`
            // refuses it this way.
            "--root" => match words.next() {
                Some(path) if !path.starts_with('-') => root = Some(path.into()),
                _ => refuse("--root needs a path"),
            },
            unknown => refuse(&format!("unknown argument `{unknown}`")),
        }
    }
    match koru_session_host::run_supervised(root.as_deref()) {
        Ok(()) => std::process::exit(0),
        Err(why) => {
            eprintln!("koru-agent --worker sessions: {why}");
            std::process::exit(1)
        }
    }
}

fn refuse(why: &str) -> ! {
    eprintln!("koru-agent --worker sessions: {why}");
    eprintln!("usage: koru-agent --worker sessions [--root <path>]");
    std::process::exit(2)
}
