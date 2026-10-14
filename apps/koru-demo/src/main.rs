// SPDX-License-Identifier: MPL-2.0

//! **The runner, as a command.**
//!
//! Inspect a locator or explicitly run a compiled case on a prepared isolated
//! instance. Input requires verified root ownership, the named foreground
//! window and the shared native exact-target consequence mechanism.
//!
//! Exit codes, because a runner is read by a script as often as by a person:
//!
//! | | |
//! | --- | --- |
//! | `0` | exactly one match, in the named instance, meeting the requirements |
//! | `1` | it ran and the answer was no: not found, ambiguous, unusable, wrong process, timed out |
//! | `2` | it was not asked a question it understood, or was pointed at something that is not a Koru window |
//! | `3` | UI Automation stopped answering and the run gave up rather than hanging |

use std::process::ExitCode;

use koru_demo::cli;

// **Under `cfg(windows)` because every one of them is only reachable from the
// Windows arm of `run`.** Not tidiness: the mac branch compiles this crate, and
// unconditional imports that only the Windows path uses would arrive there as a
// wall of `unused_import` warnings around the one thing that actually has to be
// ported — see `docs/features/macos/CATCHUP.md`.
#[cfg(windows)]
use std::sync::Arc;

#[cfg(windows)]
use koru_demo::{
    inspect::{Request, Response},
    instance::{self, RootElement, Target},
    locator::LocatorPattern,
    select::{self, Requirements, Selection},
    supervise::{CancelToken, Limits, Outcome, Start, Supervisor},
    wait::{self, Goal, Interrupted, Look, Patience, SystemClock},
};

#[path = "main/fixture_text.rs"]
mod fixture_text;
#[cfg(windows)]
#[path = "main/workflows.rs"]
mod workflows;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().is_some_and(|arg| arg == "fixture-text") {
        return fixture_text::run(&arguments);
    }
    let options = match cli::parse(arguments) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("koru-demo: {error}\n\n{}", cli::usage());
            return ExitCode::from(2);
        }
    };
    run(options)
}

#[cfg(not(windows))]
fn run(_options: cli::Options) -> ExitCode {
    // The seam, not the port. Every module above the apartment is portable and
    // tested on every platform; what is missing is a UI Automation client,
    // which is a Windows interface. `docs/features/macos/CATCHUP.md` holds what Apple is
    // owed — an `NSAccessibility` client behind the same `Mechanism` trait.
    eprintln!(
        "koru-demo: the accessibility runner needs a UI Automation client, which exists on \
         Windows only. See docs/features/macos/CATCHUP.md."
    );
    ExitCode::from(2)
}

#[cfg(windows)]
fn run(options: cli::Options) -> ExitCode {
    let cancel = CancelToken::new();
    if options.case.is_some() {
        return workflows::run(options);
    }
    if let Err(code) = koru_demo::console::install(&cancel) {
        eprintln!("koru-demo: {code}");
        return ExitCode::from(3);
    }
    let start: Start = Arc::new(move |generation| {
        koru_demo::uia::Client::open(generation)
            .map(|client| Box::new(client) as Box<dyn koru_demo::supervise::Mechanism>)
    });
    let mut supervisor = Supervisor::new(start, Limits::default());

    // 1. What are we pointed at? Nothing is inspected until this answers.
    let bound = match supervisor.call(
        Request::BindWindow {
            native_handle: options.window_handle,
        },
        &cancel,
    ) {
        Outcome::Answered(Response::Bound(bound)) => *bound,
        Outcome::Answered(Response::Failed(error)) => {
            eprintln!("koru-demo: {error}");
            return ExitCode::from(2);
        }
        other => return report_interruption(&other),
    };

    // **Resolved once, here, and used for every question after.** A relative
    // `--root` names a different rendezvous from the absolute one the Agent
    // published under, because `koru-ipc` digests the path lexically — see
    // `instance::resolved`. Resolving at one point means the path that is
    // probed, the path that is checked for existence and the path that is
    // printed cannot be three different things.
    let root = instance::resolved(&options.root);
    let target = Target {
        root: root.clone(),
        process_id: options.process_id,
        window_handle: options.window_handle,
    };
    let element = RootElement {
        automation_id: bound.facts.locator.clone().unwrap_or_default(),
        class_name: bound.class_name.clone(),
        framework: bound.framework.clone(),
        process_id: bound.facts.process_id,
    };
    let identified = match instance::identify(
        &target,
        root.is_dir(),
        instance::root_is_serving(&root),
        &element,
    ) {
        Ok(identified) => identified,
        Err(refusal) => {
            eprintln!("koru-demo: {refusal}");
            return ExitCode::from(2);
        }
    };
    println!("{identified}\n");
    // 2. Bind the pattern to the window that actually answered, so the ordinary
    //    case is one property condition rather than a walk of the subtree.
    let pattern = LocatorPattern::parse(&options.locator)
        .expect("the command line already parsed this")
        .bound_to_window(identified.window_slot);

    // 3. Wait for the state the run was asked about.
    let goal = if options.absent {
        Goal::Absent
    } else {
        Goal::Present
    };
    let waited = wait::until(
        goal,
        Patience {
            limit: options.timeout,
            ..Patience::default()
        },
        &SystemClock,
        || {
            look(
                &mut supervisor,
                &cancel,
                bound.facts.handle,
                &pattern,
                options.process_id,
            )
        },
    );

    println!("{waited}");
    match waited {
        wait::Waited::Reached(_) => ExitCode::SUCCESS,
        wait::Waited::TimedOut { .. } => ExitCode::from(1),
        wait::Waited::Cancelled { .. } | wait::Waited::Wedged { .. } => ExitCode::from(3),
    }
}

#[cfg(windows)]
fn look(
    supervisor: &mut Supervisor,
    cancel: &CancelToken,
    root: koru_demo::inspect::Handle,
    pattern: &LocatorPattern,
    process_id: u32,
) -> Look {
    match supervisor.call(
        Request::Matches {
            root,
            pattern: pattern.clone(),
        },
        cancel,
    ) {
        Outcome::Answered(Response::Found(found)) => Ok(select::select(
            found,
            Some(process_id),
            Requirements::INSPECT,
        )),
        // A destroyed root is not "the control is not there yet": the window
        // this run was pointed at has gone, and waiting out the deadline would
        // report the wrong thing for twenty seconds.
        Outcome::Answered(Response::Failed(error)) => Err(Interrupted::Wedged(error.to_string())),
        Outcome::Answered(_) => Ok(Selection::NotFound),
        Outcome::Cancelled => Err(Interrupted::Cancelled),
        Outcome::Lost => Err(Interrupted::Wedged(
            "a UI Automation call did not return within its budget; the apartment was abandoned \
             rather than waited for"
                .to_owned(),
        )),
        Outcome::Unavailable(why) => Err(Interrupted::Wedged(why)),
        Outcome::Wedged { abandoned } => Err(Interrupted::Wedged(format!(
            "{abandoned} UI Automation apartments are not answering; this run will not open \
             another"
        ))),
    }
}

#[cfg(windows)]
fn report_interruption(outcome: &Outcome) -> ExitCode {
    match outcome {
        Outcome::Lost => {
            eprintln!(
                "koru-demo: UI Automation did not answer within its budget. The call was \
                 abandoned rather than waited for, so this process is not hung — but nothing was \
                 learned about that window."
            );
            ExitCode::from(3)
        }
        Outcome::Cancelled => {
            eprintln!("koru-demo: cancelled");
            ExitCode::from(3)
        }
        Outcome::Unavailable(why) => {
            eprintln!("koru-demo: {why}");
            ExitCode::from(3)
        }
        Outcome::Wedged { abandoned } => {
            eprintln!("koru-demo: {abandoned} UI Automation apartments are not answering");
            ExitCode::from(3)
        }
        Outcome::Answered(response) => {
            eprintln!("koru-demo: unexpected answer while binding: {response:?}");
            ExitCode::from(3)
        }
    }
}
