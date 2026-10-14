// SPDX-License-Identifier: MPL-2.0

//! **A command line that is not understood is not run.**
//!
//! The same rule and the same shape as `crates/koru-workspace-host/src/cli.rs` and
//! `apps/koru-store-server/src/cli.rs`: one table that both the parser and the
//! help read, so a flag cannot be accepted without being documented or
//! documented without being accepted. An unknown argument names itself,
//! suggests the nearest real flag within three edits, prints the usage and
//! exits 2.
//!
//! Ignoring an unrecognised argument sounds harmless and is the same failure as
//! guessing a directory. Here it has a specific shape: a mistyped `--root`
//! would make this worker bind the *channel's* endpoint rather than the test
//! root's, so a suite that meant to run in isolation would find — and stop —
//! the worker a person was dictating through.
//!
//! ## This is the third copy of one mechanism
//!
//! The table, the Levenshtein suggestion and the `report()` exit are ~90 lines
//! that now exist three times. It is not extracted because the three tables
//! have nothing in common — different flags, different verbs, different
//! defaults — and a shared crate would be a generic bucket whose only charter
//! was *"the three of us parse arguments"*, which is the shape
//! [PRINCIPLES](../../../guide/contributing/PRINCIPLES.md) says to refuse. Written up in
//! `docs/contributing/techdebt/README.md` with what would make it worth doing.

use std::path::PathBuf;

/// What this invocation is for.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Command {
    /// Run the worker. The default when no verb is given, and what a
    /// supervising Agent launches.
    Serve,
    /// Ask a running worker what it is doing, and print it.
    Status,
    /// Ask a running worker to end. Silent and successful when none is
    /// running — *stop* is a state to reach, not an action to perform.
    Stop,
    /// Ask a running worker to drop the model and carry on. **Not `stop`**:
    /// voice keeps working and the next dictation pays the load again.
    Release,
    Help,
    Version,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Invocation {
    pub command: Command,
    /// Use this directory as the root instead of the channel's Store root.
    ///
    /// **A rooted worker is not refused**, unlike the machine-wide hotkey and
    /// the Run-key entry: those reach outside the installation, and a rooted
    /// worker is what makes voice testable in isolation. It reaches the
    /// endpoint name — `Endpoint::for_service_root` qualifies a name with a
    /// digest of the root — so a rooted worker and a rooted client meet while a
    /// rooted client and an unrooted worker find nothing.
    pub root: Option<PathBuf>,
    /// Read an Agent bootstrap from stdin before serving. See
    /// [`crate::bootstrap`].
    pub supervised: bool,
    /// Log to the console as well as to the file. The developer's switch: a
    /// supervised launch has no console to stay in.
    pub console: bool,
}

struct Flag {
    name: &'static str,
    short: Option<&'static str>,
    takes_value: bool,
    args: &'static str,
    help: &'static str,
}

/// The single table. Parsing reads it and so does the help.
const FLAGS: &[Flag] = &[
    Flag {
        name: "--root",
        short: None,
        takes_value: true,
        args: "--root <path>",
        help: "use this directory as the root instead of the channel's",
    },
    Flag {
        name: "--supervised",
        short: None,
        takes_value: false,
        args: "--supervised",
        help: "read the Agent launch bootstrap from stdin (serve only)",
    },
    Flag {
        name: "--console",
        short: None,
        takes_value: false,
        args: "--console",
        help: "log to the console as well as to the rotating file",
    },
    Flag {
        name: "--version",
        short: Some("-V"),
        takes_value: false,
        args: "--version",
        help: "print the version and exit",
    },
    Flag {
        name: "--help",
        short: Some("-h"),
        takes_value: false,
        args: "--help",
        help: "print this message and exit",
    },
];

const COMMANDS: &[(&str, &str)] = &[
    ("serve", "run the voice worker (the default)"),
    ("status", "what a running worker is doing"),
    ("stop", "ask a running worker to end"),
    (
        "release",
        "ask a running worker to drop the model and carry on",
    ),
];

#[derive(Debug, PartialEq, Eq)]
pub struct Rejected {
    pub arg: String,
    pub why: Why,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Why {
    Unknown {
        did_you_mean: Option<&'static str>,
    },
    MissingValue,
    /// A flag that is real but does not belong with this verb.
    NotHere {
        because: &'static str,
    },
}

impl Rejected {
    pub fn message(&self) -> String {
        match &self.why {
            Why::Unknown {
                did_you_mean: Some(near),
            } => format!("unknown argument `{}` — did you mean `{near}`?", self.arg),
            Why::Unknown { did_you_mean: None } => format!("unknown argument `{}`", self.arg),
            Why::MissingValue => format!("`{}` needs a value", self.arg),
            Why::NotHere { because } => format!("`{}` {because}", self.arg),
        }
    }

    /// Say what was wrong, print the usage, exit 2.
    pub fn report(&self) -> ! {
        eprintln!("koru-voice-worker: {}", self.message());
        eprintln!();
        eprint!("{}", help());
        std::process::exit(2)
    }
}

pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Invocation, Rejected> {
    let tokens: Vec<String> = args.into_iter().skip(1).collect();
    let mut command: Option<Command> = None;
    let mut root: Option<PathBuf> = None;
    let mut supervised = false;
    let mut console = false;
    let mut at = 0;

    while at < tokens.len() {
        let token = tokens[at].clone();

        // `--help` and `--version` win wherever they appear, so
        // `koru-voice-worker serve --help` explains rather than starting a worker.
        if token == "--help" || token == "-h" {
            return Ok(Invocation {
                command: Command::Help,
                root,
                supervised,
                console,
            });
        }
        if token == "--version" || token == "-V" {
            return Ok(Invocation {
                command: Command::Version,
                root,
                supervised,
                console,
            });
        }

        if token.starts_with("--") {
            let (name, inline) = match token[2..].split_once('=') {
                Some((name, value)) => (format!("--{name}"), Some(value.to_string())),
                None => (token.clone(), None),
            };
            let Some(flag) = FLAGS
                .iter()
                .find(|flag| flag.name == name || flag.short == Some(name.as_str()))
            else {
                return Err(Rejected {
                    why: Why::Unknown {
                        did_you_mean: nearest(&name),
                    },
                    arg: name,
                });
            };
            if flag.takes_value {
                let value = match inline {
                    Some(value) => {
                        at += 1;
                        value
                    }
                    None => {
                        let Some(value) = tokens.get(at + 1) else {
                            return Err(Rejected {
                                arg: name,
                                why: Why::MissingValue,
                            });
                        };
                        at += 2;
                        value.clone()
                    }
                };
                if flag.name == "--root" {
                    root = Some(PathBuf::from(value));
                }
            } else {
                match flag.name {
                    "--supervised" => supervised = true,
                    "--console" => console = true,
                    _ => {}
                }
                at += 1;
            }
            continue;
        }

        // A bare word is a verb.
        let chosen = match token.as_str() {
            "serve" => Command::Serve,
            "status" => Command::Status,
            "stop" => Command::Stop,
            "release" => Command::Release,
            _ => {
                return Err(Rejected {
                    why: Why::Unknown {
                        did_you_mean: nearest_command(&token),
                    },
                    arg: token,
                });
            }
        };
        if command.is_some() {
            return Err(Rejected {
                why: Why::Unknown { did_you_mean: None },
                arg: token,
            });
        }
        command = Some(chosen);
        at += 1;
    }

    let command = command.unwrap_or(Command::Serve);
    // **A bootstrap belongs to a launch, not to a question.** `--supervised`
    // reads stdin and validates a launch against this executable; asking for it
    // beside `status` would make a diagnostic block on a pipe nobody wrote to.
    if supervised && command != Command::Serve {
        return Err(Rejected {
            arg: "--supervised".into(),
            why: Why::NotHere {
                because: "is valid only while serving",
            },
        });
    }
    Ok(Invocation {
        command,
        root,
        supervised,
        console,
    })
}

/// The nearest flag within three edits, so a typo is corrected rather than
/// merely refused.
fn nearest(name: &str) -> Option<&'static str> {
    FLAGS
        .iter()
        .map(|flag| (flag.name, distance(name, flag.name)))
        .filter(|(_, edits)| *edits <= 3)
        .min_by_key(|(_, edits)| *edits)
        .map(|(name, _)| name)
}

fn nearest_command(name: &str) -> Option<&'static str> {
    COMMANDS
        .iter()
        .map(|(verb, _)| (*verb, distance(name, verb)))
        .filter(|(_, edits)| *edits <= 3)
        .min_by_key(|(_, edits)| *edits)
        .map(|(verb, _)| verb)
}

/// Levenshtein, over **characters not bytes** — a mistyped flag in any script
/// should still find its neighbour, and byte distance over UTF-8 measures
/// encoding rather than typing.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        current[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            current[j] = (previous[j] + 1)
                .min(current[j - 1] + 1)
                .min(previous[j - 1] + cost);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

pub fn version() -> String {
    format!("koru-voice-worker {}", env!("CARGO_PKG_VERSION"))
}

/// Built from [`FLAGS`] and [`COMMANDS`], so it cannot drift from the parser.
pub fn help() -> String {
    let mut out = String::from(
        "Koru voice worker — it opens the microphone, runs a speech engine, and\n\
         sends back transcripts. Audio never leaves this process.\n\n\
         Usage: koru-voice-worker [<command>] [options]\n\n\
         Commands:\n",
    );
    for (name, help) in COMMANDS {
        out.push_str(&format!("  {name:<10} {help}\n"));
    }
    out.push_str("\nOptions:\n");
    for flag in FLAGS {
        // The short spelling is listed because it is *accepted*. A flag that
        // works and is not documented is the same drift the table exists to
        // prevent, in the other direction.
        let spelling = match flag.short {
            Some(short) => format!("{}, {short}", flag.args),
            None => flag.args.to_string(),
        };
        out.push_str(&format!("  {spelling:<22} {}\n", flag.help));
    }
    out.push_str(&format!(
        "\nChannels: {}\nThis build uses `{}`.\n",
        koru_channel::SEGMENTS.join(", "),
        koru_channel::segment()
    ));
    out.push_str(
        "\nThe worker is started on demand by koru-agent and ends itself after a\n\
         spell with nothing to do. Starting one by hand is a diagnostic, not a\n\
         step anybody needs.\n",
    );
    out
}

#[cfg(test)]
mod tests;
