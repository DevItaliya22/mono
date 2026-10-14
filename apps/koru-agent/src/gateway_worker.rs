// SPDX-License-Identifier: MPL-2.0

//! The gateway role of `koru-agent.exe`.
//!
//! `koru-agent.exe --worker gateway [--root <path>]` **is** the Koru API
//! Gateway, and since Plan 7 of the Gateway worker roadmap it is the only host
//! there is: the process reads the Agent's one-line launch bootstrap from
//! stdin and drives the `koru_api_gateway` composition. Everything else about
//! being a worker — spawn, Job Object, readiness, restart — is the
//! supervisor's side and is not this file.
//!
//! ## Two things a supervised host does that the retired standalone one did not
//!
//! Kept as a pair because each is easy to undo by accident and neither fails
//! loudly when it is.
//!
//! - **One bounded line, not a read to EOF.** `koru-api-gateway.exe` read its
//!   whole stdin, because its launcher wrote one document and closed the pipe.
//!   A supervised worker's parent holds the write end open for the child's
//!   whole life — that *is* the parent-death signal — so a read to EOF would
//!   never return. [`koru_agent_worker::read_bootstrap_line`] reads exactly one
//!   newline-terminated, bounded line and leaves the pipe open.
//! - **The pipe is then watched, not dropped.** EOF on it means the Agent is
//!   gone, and a gateway whose parent is gone has nobody to validate a grant
//!   for; it asks itself to shut down.
//!   [`koru_agent_worker::ParentPipe`] owns that named thread, beside the
//!   reader, because this is the second role that needs it — the session host
//!   is the first — and a third copy is how a platform fix ends up applied in
//!   one place out of three.
//!
//! ## The one switch a released build does not have
//!
//! `--deterministic-ai-test` composes the deterministic mock AI provider. It
//! was a *host CLI* switch on `koru-api-gateway.exe`, and the AI integration
//! suite is the only caller; when that binary went, the suite moved to
//! `apps/koru-agent/tests/gateway_ai.rs` and the switch had to move with it or
//! the suite would have had no provider. It is accepted here on exactly the
//! terms the retired host accepted it on — **only under `cfg!(debug_assertions)`**
//! — so a released build refuses it through the same strict unknown-argument
//! path as any other token and never prints it in the usage line.
//!
//! ## What is never logged
//!
//! The bootstrap line carries the bridge capability and the session-lifecycle
//! authority. It is parsed into a `LaunchBootstrap` and handed to the runtime;
//! neither the bytes nor the value is ever formatted into a message, which is
//! also why `GatewayConfig` deliberately has no `Debug`. A refusal names *what
//! was wrong*, never what was read.

use std::path::PathBuf;

/// The role the Agent's `dispatch` call registers.
pub fn role() -> koru_agent_worker::Role {
    koru_agent_worker::Role {
        name: "gateway",
        run,
    }
}

/// Everything `--worker gateway` accepts: one optional flag, and one more that
/// a released build does not have at all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// `--root <path>`: what the supervisor was rooted at. `None` means this
    /// build's channel root, resolved once, in [`run`] — never guessed twice.
    pub root: Option<PathBuf>,
    /// `--deterministic-ai-test`: compose the deterministic mock AI provider
    /// instead of a real one. **Accepted only by a build with debug
    /// assertions**, exactly as the `debug_only` rule of the retired standalone
    /// gateway's command line accepted it — a released build refuses the token
    /// through the ordinary unknown-argument path and never prints it in the
    /// usage line, so there is no way for it to reach a production composition
    /// quietly.
    pub deterministic_ai_test: bool,
}

/// **An argument that is not understood is not run.**
///
/// Strict in both directions, following `koru_workspace_host::cli::parse`: an
/// unknown token is refused, and a *flag* where `--root`'s value should be is a
/// forgotten path rather than a directory literally named `--supervised`.
/// Accepting that second one is the silent wrong-root failure the rule exists
/// to prevent, and it is a live bug in the sessions role's parser that this one
/// deliberately does not copy.
///
/// Pure, and it returns rather than exits: only [`run`] knows that a refusal
/// here costs exit 2.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut options = Options::default();
    let mut words = args.into_iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--root" => match words.next() {
                Some(path) if !path.starts_with('-') => options.root = Some(PathBuf::from(path)),
                _ => return Err("--root needs a path".to_owned()),
            },
            // **A debug-only switch does not exist in a released build.** The
            // guard is on the arm rather than inside it, so a build without
            // debug assertions falls through to the strict unknown-argument
            // refusal below and says the same thing it would say about any
            // other token it has never heard of.
            "--deterministic-ai-test" if cfg!(debug_assertions) => {
                options.deterministic_ai_test = true;
            }
            unknown => return Err(format!("unknown argument `{unknown}`")),
        }
    }
    Ok(options)
}

fn run(args: Vec<String>) -> ! {
    // **Ordering (1), which only a single-threaded process can hold.**
    // `dispatch` is the first statement of `main`, so nothing in this image
    // has started a thread or a runtime yet, and setting a process variable is
    // a data race against anything reading one. The standalone host does this
    // in the first statement of its own `main` for the same reason.
    koru_api_gateway::load_dotenv();

    let options = match parse(args) {
        Ok(options) => options,
        Err(why) => refuse(&why),
    };

    // **One bounded line, and stdin is deliberately not consumed further.**
    // What is left of the pipe is the parent-death signal, armed below once
    // there is a runtime to ask for shutdown.
    let mut stdin = std::io::stdin();
    let line = match koru_agent_worker::read_bootstrap_line(&mut stdin) {
        Ok(line) => line,
        // A supervisor that closed the pipe without writing, or wrote more
        // than the bound, is a broken contract rather than a runtime failure —
        // the same class as a malformed argument, and the same exit code.
        Err(error) => refuse(&format!("could not read the Agent bootstrap: {error}")),
    };
    let bootstrap: koru_network_bridge::LaunchBootstrap = match serde_json::from_slice(&line) {
        Ok(bootstrap) => bootstrap,
        // `error` names the JSON fault and never quotes the input. The line
        // carries the bridge capability.
        Err(error) => refuse(&format!("invalid Agent bootstrap: {error}")),
    };
    drop(line);

    // **The one place this role resolves a root.** `--root` wins; otherwise
    // this build's channel root. There is no third answer, and the runtime
    // itself resolves none.
    let Some(root) = options.root.or_else(koru_channel::channel_root) else {
        fail("could not determine the channel root");
    };

    // The channel and the version are this *image's*: a worker role proves its
    // own image against the bootstrap's fingerprint, which is what makes "the
    // Agent launched this exact binary" mean something. Both packages take the
    // workspace version, so this says the same thing the standalone host says.
    let config = koru_api_gateway::GatewayConfig::new(
        root,
        koru_api_gateway::BuildIdentity::new(crate::segment(), env!("CARGO_PKG_VERSION")),
    )
    .with_bootstrap(Some(bootstrap))
    // False in every build that did not accept the switch, because a build
    // without debug assertions cannot parse it at all.
    .with_deterministic_ai_test(options.deterministic_ai_test);

    // **The sink first**, and held for the rest of the process. The parent
    // writes `koru-agent.log` and this child writes `koru-api-gateway.log`
    // beside it; initialising here rather than inside the composition is what
    // keeps the two out of one destination, and it is what makes the line
    // explaining a refused start survive the refusal.
    let logging = koru_api_gateway::init_logging(&config);

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => fail(&format!("could not start a runtime: {error}")),
    };

    let outcome = runtime.block_on(async move {
        let gateway = koru_api_gateway::start(config).await?;
        // **Armed after start, and the window before it is not a hole.** The
        // Agent's private bridge is connected inside `start`, before anything
        // is discoverable; a parent that died earlier takes its listener with
        // it, so `start` fails and this process exits. Once Plan 4 supervises
        // this role, the kill-on-close Job Object covers the same window a
        // second time on Windows. What EOF adds is the cross-platform answer
        // for a parent that dies *after* the gateway is up.
        let request = gateway.shutdown_request();
        if let Err(error) = koru_agent_worker::ParentPipe::new(stdin)
            .watch("koru-gateway parent", move || request.request())
        {
            // **A refusal here still goes through the one teardown.** A
            // gateway with no parent-death watcher must not serve, but
            // returning from this block would drop the runtime with the Store
            // open and leave the join to a destructor that discards it —
            // exactly the exit path this plan closed. So it is asked to stop
            // and *then* served: `serve` observes the request immediately,
            // runs the whole teardown, and reports what the Store said.
            gateway.shutdown_request().request();
            let completion = gateway.serve().await;
            return Err(format!(
                "could not watch the Agent bootstrap pipe: {error}; {completion}"
            ));
        }
        Ok(gateway.serve().await)
    });

    let code = match outcome {
        Ok(completion) if completion.is_clean() => 0,
        Ok(completion) => {
            tracing::error!(
                completion = %completion,
                "the Koru API Gateway worker did not stop cleanly"
            );
            eprintln!("koru-agent --worker gateway: {completion}");
            1
        }
        Err(error) => {
            tracing::error!(%error, "Koru API Gateway worker stopped with an error");
            eprintln!("koru-agent --worker gateway: {error}");
            1
        }
    };
    // **Dropped before the exit, not by it.** `process::exit` runs no
    // destructors, and the sink is a non-blocking writer whose guard is what
    // flushes it — including the line just above saying why this stopped. The
    // standalone host gets this for free by returning from `main`; a role
    // cannot return, so it has to say so.
    drop(logging);
    std::process::exit(code)
}

/// A refused command line or a broken launch contract: exit 2, the strict-CLI
/// code `dispatch` itself uses.
fn refuse(why: &str) -> ! {
    eprintln!("koru-agent --worker gateway: {why}");
    eprintln!("{}", usage());
    std::process::exit(2)
}

/// The usage line, which names only what this build actually accepts. A
/// released build neither parses `--deterministic-ai-test` nor advertises it.
fn usage() -> &'static str {
    if cfg!(debug_assertions) {
        "usage: koru-agent --worker gateway [--root <path>] [--deterministic-ai-test]"
    } else {
        "usage: koru-agent --worker gateway [--root <path>]"
    }
}

/// A runtime failure — a refused bootstrap, a root that cannot be resolved, a
/// gateway that would not start. Exit 1, so a supervisor can tell it from a
/// command line it got wrong.
fn fail(why: &str) -> ! {
    eprintln!("koru-agent --worker gateway: {why}");
    std::process::exit(1)
}

#[cfg(test)]
mod tests;
