// SPDX-License-Identifier: MPL-2.0

//! The `koru-relay` binary: the upstream iroh-relay server, configured.
//!
//! Two ways to run it, and nothing in between (decision №3 in
//! docs/architecture/iroh-relay/README.md — the server is the library, configured,
//! never forked):
//!
//! - `--config <path>`: the deployment. One TOML file (schema and defaults in
//!   [`config`]) describing the served hostnames, ports, ACME cache and
//!   metrics bind; one server instance, one ACME certificate carrying every
//!   hostname as a SAN, QAD sharing that certificate on UDP.
//! - `--local`: a developer's relay. Loopback, ephemeral ports, plain HTTP,
//!   no QAD. Humans only — repository tests start their relay through
//!   `koru-relay-test-support` instead.
//!
//! And one question to ask of a relay that is already running somewhere:
//!
//! - `probe <url>`: the probe from this crate's library — two relay-only
//!   endpoints and a nonce round trip through the relay at `<url>`. Exactly
//!   one line on stdout, exit 0 only if the relay relayed; the deploy gate
//!   and the scheduled monitor (Milestone 3) branch on the code and keep the
//!   line for humans, so this path initialises no logging at all.
//!
//! Logging reports news, not activity: the startup summary with the bound
//! addresses, TLS/ACME failure, and shutdown are unconditional; everything
//! per-connection sits at debug behind `KORU_RELAY_DEBUG` (or a `RUST_LOG`
//! override). Logs go to stdout, where the container runtime keeps them.

mod cli;
mod config;

use std::net::{Ipv4Addr, SocketAddr};
use std::process::ExitCode;

use iroh_relay::server::Server;

use crate::cli::{Command, Mode};

/// What is recorded when nothing says otherwise: our news and the library's
/// (ACME order failures arrive as `iroh_relay` error/info records), everything
/// else's warnings. `RUST_LOG` overrides the whole string.
const DEFAULT_FILTER: &str = "warn,koru_relay=info,iroh_relay=info";

/// `KORU_RELAY_DEBUG`: per-connection accepts and disconnects, ACME event
/// detail, relay frame handling — the routine records the default filter
/// deliberately leaves out.
const DEBUG_FILTER: &str = "warn,koru_relay=debug,iroh_relay=debug";

fn main() -> ExitCode {
    let command = match cli::parse(std::env::args()) {
        Ok(command) => command,
        Err(rejected) => rejected.report(),
    };
    let mode = match command {
        Command::Help => {
            print!("{}", cli::help());
            return ExitCode::SUCCESS;
        }
        Command::Version => {
            println!("{}", cli::version());
            return ExitCode::SUCCESS;
        }
        Command::Probe { url, timeout } => return probe(url, timeout),
        Command::Serve(mode) => mode,
    };

    let _logging = koru_log::init_stdout(if koru_env::switch(koru_env::name::KORU_RELAY_DEBUG) {
        DEBUG_FILTER
    } else {
        DEFAULT_FILTER
    });
    raise_nofile_limit();

    // iroh compiles more than one rustls crypto provider in, so the process
    // default is ambiguous until somebody names one; the ACME client and the
    // QUIC server both reach for it. Another component may have won the race
    // already, which is fine — everything here passes `ring` explicitly where
    // an API takes a provider.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!("failed to start async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(serve(mode)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            tracing::error!("{message}");
            ExitCode::FAILURE
        }
    }
}

/// The `probe` subcommand: run the library probe, print its one line, map
/// the result to the exit code.
///
/// Stdout is the contract — exactly the one-line result and nothing else —
/// which is why no logging is initialised here: a stray warning on stdout
/// would corrupt the line the monitors keep for humans. The URL was already
/// validated by the parser, so nothing here can fail for usage reasons.
fn probe(url: iroh::RelayUrl, timeout: std::time::Duration) -> ExitCode {
    // Same crypto-provider ambiguity as serving; see the comment in `main`.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("koru-relay: failed to start async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(koru_relay::probe_relay(url, timeout));
    println!("{}", koru_relay::one_line_result(&result));
    if result.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Docker's default hard limit is 1024. iroh-relay's HTTP accept loop logs
/// `Too many open files` and retries immediately when it hits that, which
/// fills the log and still cannot accept. Raise to the process hard ceiling
/// (a container deployment should set `ulimits.nofile` so that ceiling is
/// actually high).
#[cfg(unix)]
fn raise_nofile_limit() {
    let mut lim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } != 0 {
        tracing::warn!("could not read open-file limit");
        return;
    }
    let hard = lim.rlim_max;
    let want = 65_536.min(hard);
    if lim.rlim_cur >= want {
        tracing::info!(soft = lim.rlim_cur, hard, "open-file limit");
        return;
    }
    lim.rlim_cur = want;
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) } != 0 {
        tracing::warn!(
            soft = want,
            hard,
            "could not raise open-file limit; compose ulimits.nofile must allow it"
        );
        return;
    }
    tracing::info!(soft = lim.rlim_cur, hard, "raised open-file limit");
}

#[cfg(not(unix))]
fn raise_nofile_limit() {}

async fn serve(mode: Mode) -> Result<(), String> {
    let server_config = match &mode {
        Mode::Local => config::local_server_config(ephemeral_metrics_addr()?),
        Mode::Config(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|error| format!("cannot read config {}: {error}", path.display()))?;
            let parsed = config::parse(&text)
                .map_err(|error| format!("config {}: {error}", path.display()))?;
            tracing::info!(
                hostnames = ?parsed.hostnames,
                acme_cache = %parsed.acme_cache_dir.display(),
                "one certificate will carry every hostname"
            );
            parsed.server_config()
        }
    };
    let metrics_addr = server_config.metrics_addr;

    let mut server = Server::spawn(server_config)
        .await
        .map_err(|error| format!("failed to start relay server: {error}"))?;

    // The startup summary: the one line that says where everything bound.
    // With TLS, /healthz and the relay live on `https`; without, on `http`.
    tracing::info!(
        http = ?server.http_addr(),
        https = ?server.https_addr(),
        quic = ?server.quic_addr(),
        metrics = ?metrics_addr,
        "relay serving"
    );

    tokio::select! {
        biased;
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("interrupt received; shutting down");
        }
        outcome = server.join() => {
            // The supervisor never finishes on its own in health: this is a
            // TLS/ACME failure or a task loss, and the process must exit so
            // the container's restart policy can act on it.
            let reason = match outcome {
                Ok(Ok(())) => "server tasks ended".to_string(),
                Ok(Err(error)) => error.to_string(),
                Err(join_error) => join_error.to_string(),
            };
            return Err(format!("relay server stopped on its own: {reason}"));
        }
    }
    server
        .shutdown()
        .await
        .map_err(|error| format!("relay server did not shut down cleanly: {error}"))?;
    tracing::info!("relay stopped");
    Ok(())
}

/// An ephemeral loopback port for `--local` metrics, chosen before spawning
/// because the library does not report where its metrics server bound. Bound
/// and released, so there is a benign window in which another process could
/// take it — acceptable for a developer mode that tests never run.
fn ephemeral_metrics_addr() -> Result<SocketAddr, String> {
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|error| format!("cannot pick an ephemeral metrics port: {error}"))?;
    listener
        .local_addr()
        .map_err(|error| format!("cannot read the ephemeral metrics port: {error}"))
}
