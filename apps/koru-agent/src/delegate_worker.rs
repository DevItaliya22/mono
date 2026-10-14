// SPDX-License-Identifier: MPL-2.0

//! The delegate role of `koru-agent.exe`.
//!
//! `koru-agent.exe --worker delegate [--root <path>]` **is** the delegated
//! run orchestrator (D12): the process reads the one-line Agent bootstrap
//! from stdin and runs the `koru_delegate_host::run_supervised`
//! composition, the way `--worker sessions` runs the session host's.
//! Everything else about being a worker — spawn, Job Object, readiness,
//! restart — is the supervisor's side, in [`crate::delegate_host`].
//!
//! The argument parser is strict in both directions, following the
//! gateway role: an unknown token is refused, and a flag where `--root`'s
//! path should be is a forgotten path rather than a directory literally
//! named `--supervised`. A refusal costs exit 2 and prints this role's
//! usage, never the Agent's help.

use std::path::PathBuf;

/// The role the Agent's `dispatch` call registers.
pub fn role() -> koru_agent_worker::Role {
    koru_agent_worker::Role {
        name: "delegate",
        run,
    }
}

/// Everything `--worker delegate` accepts: one optional flag.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// `--root <path>`: what the supervisor was rooted at. `None` means
    /// this build's channel root, resolved once, inside the composition.
    pub root: Option<PathBuf>,
}

/// **An argument that is not understood is not run.** Pure, and it returns
/// rather than exits: only [`run`] knows that a refusal costs exit 2.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut options = Options::default();
    let mut words = args.into_iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--root" => match words.next() {
                Some(path) if !path.starts_with('-') => options.root = Some(PathBuf::from(path)),
                _ => return Err("--root needs a path".to_owned()),
            },
            unknown => return Err(format!("unknown argument `{unknown}`")),
        }
    }
    Ok(options)
}

fn run(args: Vec<String>) -> ! {
    let options = match parse(args) {
        Ok(options) => options,
        Err(why) => refuse(&why),
    };
    match koru_delegate_host::run_supervised(options.root.as_deref()) {
        Ok(()) => std::process::exit(0),
        Err(why) => {
            // A refused bootstrap is a broken launch contract, the same
            // class as a malformed argument; anything else is a runtime
            // failure a supervisor tells apart by its code.
            if why.starts_with("could not read the Agent bootstrap")
                || why.starts_with("invalid Agent bootstrap")
            {
                refuse(&why);
            }
            eprintln!("koru-agent --worker delegate: {why}");
            std::process::exit(1)
        }
    }
}

fn refuse(why: &str) -> ! {
    eprintln!("koru-agent --worker delegate: {why}");
    eprintln!("{USAGE}");
    std::process::exit(2)
}

pub const USAGE: &str = "usage: koru-agent --worker delegate [--root <path>]";

#[cfg(test)]
mod tests;
