// SPDX-License-Identifier: MPL-2.0

//! **An argument that is not understood is not run.**
//!
//! The rule `koru_workspace_host::cli::parse` enforces for the window and
//! `koru_voice_bench::cli` enforces for the benchmark, applied here for the
//! reason §7.5 gives: a mistyped `--root` that fell back to a default would
//! point this runner at a workspace nobody named, and — worse than a benchmark
//! measuring the wrong corpus — at somebody's ordinary session.
//!
//! So the token is named, the nearest flag within three edits is suggested, the
//! usage is printed, and the process exits 2. [`FLAGS`] is the only table, and
//! `every_documented_flag_parses` fails if the parser and the help drift apart.
//!
//! ## There is no default root, no default window, and no discovery
//!
//! `--root <dir>`, `--pid <n>` and `--window <handle>` are all required, and
//! none of them has a fall-back. That is more typing than a runner that went
//! looking for a Koru window by itself, and it is the difference between a tool
//! that can only inspect the instance it was handed and one that can be pointed
//! at somebody's work by a typo.
//!
//! The handle is deliberately raw rather than something friendlier like a
//! window title: a title is content, §7.4 says content does not come near this
//! crate, and a tool that selected a window by title would select the wrong one
//! the first time two documents were called the same thing —
//! `ACCEPTANCE.md` 1.4 exists for exactly that case.

use std::path::PathBuf;
use std::time::Duration;

/// One accepted flag.
pub struct Flag {
    pub name: &'static str,
    pub argument: Option<&'static str>,
    pub help: &'static str,
}

/// The only table. The parser and the usage both read it.
pub const FLAGS: [Flag; 23] = [
    Flag {
        name: "--fixture",
        argument: Some("<manifest.json>"),
        help: "versioned synthetic fixture manifest; prepare fills each half once",
    },
    Flag {
        name: "--repeat",
        argument: Some("<1..10>"),
        help: "consecutive runs from the manifest (default 1)",
    },
    Flag {
        name: "--report",
        argument: Some("<new-report.json>"),
        help: "required new report file for test; existing files are refused",
    },
    Flag {
        name: "--snapshot-content",
        argument: None,
        help: "explicitly read synthetic fixture text; required for prepare, optional final test snapshots",
    },
    Flag {
        name: "--ui-language",
        argument: Some("<en>"),
        help: "prepare: compiled cases require English Koru UI",
    },
    Flag {
        name: "--locale",
        argument: Some("<OS-locale>"),
        help: "prepare: declared OS date/time locale, distinct from UI language",
    },
    Flag {
        name: "--hour-cycle",
        argument: Some("<h12|h23>"),
        help: "prepare: declared OS clock format",
    },
    Flag {
        name: "--time-zone",
        argument: Some("<zone>"),
        help: "prepare: declared OS time zone",
    },
    Flag {
        name: "--subject",
        argument: Some("<koru:...#text>"),
        help: "prepared synthetic paragraph locator for a compiled case",
    },
    Flag {
        name: "--document",
        argument: Some("<koru:...#root>"),
        help: "owning document's Explore row locator",
    },
    Flag {
        name: "--entry",
        argument: Some("<koru:...#text>"),
        help: "onscreen first paragraph for document-edit",
    },
    Flag {
        name: "--reminder-date",
        argument: Some("<localized-date>"),
        help: "explicit future date in the fixture system date format (e.g. 04/12/2099 for en-US)",
    },
    Flag {
        name: "--reminder-time",
        argument: Some("<localized-time>"),
        help: "explicit time in the fixture clock format (e.g. 3:30 PM); record locale, clock and zone",
    },
    Flag {
        name: "--root",
        argument: Some("<dir>"),
        help: "the isolated root the Koru under test was started on. Required: there is no \
               default and no search path",
    },
    Flag {
        name: "--pid",
        argument: Some("<n>"),
        help: "the process id of that Koru. Every match is proved to be in it",
    },
    Flag {
        name: "--window",
        argument: Some("<handle>"),
        help: "the window handle to inspect, decimal or 0x-prefixed. Nothing is enumerated and \
               nothing is guessed",
    },
    Flag {
        name: "--locator",
        argument: Some("<koru:...>"),
        help: "the control to find. `*` may stand for a window, view or instance number",
    },
    Flag {
        name: "--timeout",
        argument: Some("<ms>"),
        help: "how long to wait for the control to be there (default 20000)",
    },
    Flag {
        name: "--absent",
        argument: None,
        help: "wait for the control to be gone rather than present, which is how a retired \
               screen is proved retired from outside",
    },
    Flag {
        name: "--clip",
        argument: Some("<path.mp4>"),
        help: "record or sample-record: new H.264 file; existing paths are refused",
    },
    Flag {
        name: "--actions",
        argument: Some("<path.json>"),
        help: "record or sample-record: new versioned action JSON; existing paths are refused",
    },
    Flag {
        name: "--stills",
        argument: Some("<dir>"),
        help: "record or sample-record: directory for annotated stills and the validation view",
    },
    Flag {
        name: "--result",
        argument: Some("<path.json>"),
        help: "record: new result manifest; existing paths are refused",
    },
];

/// What a run was asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    pub case: Option<String>,
    pub prepare: bool,
    pub manifest: Option<PathBuf>,
    pub repeat: usize,
    pub report: Option<PathBuf>,
    pub snapshot_content: bool,
    pub environment: Option<crate::fixtures::Environment>,
    pub capture_date: Option<String>,
    pub capture_time: Option<String>,
    pub fixture: Option<crate::cases::Fixture>,
    pub root: PathBuf,
    pub process_id: u32,
    pub window_handle: i64,
    pub locator: String,
    pub timeout: Duration,
    pub absent: bool,
    pub sample_record: bool,
    pub record: bool,
    pub clip: Option<PathBuf>,
    pub actions_out: Option<PathBuf>,
    pub stills: Option<PathBuf>,
    pub result_out: Option<PathBuf>,
}

/// Why a command line was not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliError {
    pub message: String,
    pub suggestion: Option<&'static str>,
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)?;
        if let Some(suggestion) = self.suggestion {
            write!(f, "\n       did you mean {suggestion}?")?;
        }
        Ok(())
    }
}

/// Everything this binary would accept, printed from the one table.
pub fn usage() -> String {
    let mut out = String::from(
        "koru-demo — the accessibility acceptance runner\n\n\
         It inspects one named Koru instance through UI Automation and says which control a \
         locator found.\nUse test <document-edit|task-reminder> to execute a compiled workflow on a prepared isolated fixture. Use prepare <case> with --snapshot-content to capture its IDs through visible focus and scrolling; it waits for you to activate the named window.\n\n\
         Use record <document-edit|task-reminder> to capture an H.264 clip, versioned action JSON and result manifest after the first encoded frame exists. sample-record document-edit remains the Plan 4.1 sample verb. The JSON is koru-demo-recording/1.\n\n\
         usage: koru-demo --root <dir> --pid <n> --window <handle> --locator <koru:...>\n\n",
    );
    for flag in &FLAGS {
        let head = match flag.argument {
            Some(argument) => format!("{} {argument}", flag.name),
            None => flag.name.to_owned(),
        };
        out.push_str(&format!("  {head:<22}{}\n", flag.help));
    }
    out
}

/// Read a command line, or refuse it.
mod repeatability;
pub use repeatability::parse;
fn parse_base<I: IntoIterator<Item = String>>(
    arguments: I,
    deferred_fixture: bool,
) -> Result<Options, CliError> {
    let mut root = None;
    let mut process_id = None;
    let mut window_handle = None;
    let mut locator = None;
    let mut timeout = Duration::from_millis(20_000);
    let mut absent = false;
    let mut subject = None;
    let mut document = None;
    let mut entry = None;
    let mut reminder_date = None;
    let mut reminder_time = None;

    let mut arguments = arguments.into_iter().peekable();
    let case = if arguments.peek().is_some_and(|a| a == "test") {
        arguments.next();
        let name = required(arguments.next(), "test <case>")?;
        if !crate::cases::names().contains(&name.as_str()) {
            return Err(CliError {
                message: "unknown compiled case".into(),
                suggestion: None,
            });
        }
        Some(name)
    } else {
        None
    };
    while let Some(token) = arguments.next() {
        let Some(flag) = FLAGS.iter().find(|flag| flag.name == token) else {
            return Err(CliError {
                message: format!("{token} is not an argument koru-demo understands"),
                suggestion: nearest(&token),
            });
        };
        let value = match flag.argument {
            None => String::new(),
            Some(argument) => arguments.next().ok_or_else(|| CliError {
                message: format!("{} takes {argument}, and none was given", flag.name),
                suggestion: None,
            })?,
        };
        match flag.name {
            "--subject" => subject = Some(fixture_locator(&value, "text")?),
            "--document" => document = Some(fixture_locator(&value, "root")?),
            "--entry" => entry = Some(fixture_locator(&value, "text")?),
            "--reminder-date" => reminder_date = Some(value),
            "--reminder-time" => reminder_time = Some(value),
            "--root" => root = Some(PathBuf::from(value)),
            "--pid" => {
                process_id = Some(
                    value
                        .parse::<u32>()
                        .ok()
                        .filter(|pid| *pid != 0)
                        .ok_or_else(|| CliError {
                            message: format!("{value} is not a process id"),
                            suggestion: None,
                        })?,
                )
            }
            "--window" => window_handle = Some(parse_handle(&value)?),
            "--locator" => {
                // Parsed here rather than at the first search, so a typo costs a
                // line rather than a launch and a timeout.
                crate::locator::LocatorPattern::parse(&value).map_err(|error| CliError {
                    message: format!("{value} is not a locator: {error}"),
                    suggestion: None,
                })?;
                locator = Some(value);
            }
            "--timeout" => {
                timeout = value
                    .parse::<u64>()
                    .ok()
                    .map(Duration::from_millis)
                    .ok_or_else(|| CliError {
                        message: format!("{value} is not a number of milliseconds"),
                        suggestion: None,
                    })?
            }
            "--absent" => absent = true,
            _ => unreachable!("FLAGS and this match are the same list"),
        }
    }

    let capture_date = reminder_date.clone();
    let capture_time = reminder_time.clone();
    let fixture = if deferred_fixture {
        if absent || locator.is_some() || subject.is_some() || document.is_some() || entry.is_some()
        {
            return Err(CliError {
                message: "manifest mode refuses inline fixture/inspection arguments".into(),
                suggestion: None,
            });
        }
        None
    } else if let Some(name) = &case {
        if absent || locator.is_some() {
            return Err(CliError {
                message: "test does not accept --locator or --absent".into(),
                suggestion: None,
            });
        }
        let fixture = crate::cases::Fixture {
            subject: required(subject, "--subject")?,
            document: required(document, "--document")?,
            entry,
            reminder_date,
            reminder_time,
        };
        crate::cases::compile(name, &fixture).map_err(|code| CliError {
            message: code.into(),
            suggestion: None,
        })?;
        Some(fixture)
    } else {
        if subject.is_some()
            || document.is_some()
            || entry.is_some()
            || reminder_date.is_some()
            || reminder_time.is_some()
        {
            return Err(CliError {
                message: "fixture arguments require test <case>".into(),
                suggestion: None,
            });
        }
        None
    };
    Ok(Options {
        case: case.clone(),
        prepare: false,
        manifest: None,
        repeat: 1,
        report: None,
        snapshot_content: false,
        environment: None,
        capture_date,
        capture_time,
        fixture,
        root: required(root, "--root")?,
        process_id: required(process_id, "--pid")?,
        window_handle: required(window_handle, "--window")?,
        locator: if case.is_some() {
            "koru:w*/v*/s:body#root".into()
        } else {
            required(locator, "--locator")?
        },
        timeout,
        absent,
        sample_record: false,
        record: false,
        clip: None,
        actions_out: None,
        stills: None,
        result_out: None,
    })
}

fn fixture_locator(value: &str, part: &str) -> Result<crate::locator::LocatorPattern, CliError> {
    let pattern = crate::locator::LocatorPattern::parse(value).map_err(|_| CliError {
        message: "invalid fixture locator".into(),
        suggestion: None,
    })?;
    if !matches!(pattern.object, crate::locator::Object::Model(_))
        || pattern.part != part
        || pattern.view != crate::locator::Slot::Any
    {
        return Err(CliError {
            message:
                "fixture locators require a model UUID, the expected part and v* across reopen"
                    .into(),
            suggestion: None,
        });
    }
    Ok(pattern)
}

fn required<T>(value: Option<T>, flag: &str) -> Result<T, CliError> {
    value.ok_or_else(|| CliError {
        message: format!(
            "{flag} is required. Each of --root, --pid and --window has deliberately no default: \
             a runner that chose one for you could choose the session you are working in"
        ),
        suggestion: None,
    })
}

fn parse_handle(value: &str) -> Result<i64, CliError> {
    let parsed = match value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        Some(hex) => i64::from_str_radix(hex, 16).ok(),
        None => value.parse::<i64>().ok(),
    };
    parsed
        .filter(|handle| *handle != 0)
        .ok_or_else(|| CliError {
            message: format!(
                "{value} is not a window handle: expected a decimal or 0x-prefixed number"
            ),
            suggestion: None,
        })
}

/// The nearest flag within three edits.
///
/// Three, the distance `koru_workspace_host::cli::parse` uses, because a
/// suggestion that fires on anything is one nobody trusts.
fn nearest(token: &str) -> Option<&'static str> {
    FLAGS
        .iter()
        .map(|flag| (flag.name, edits(token, flag.name)))
        .filter(|(_, distance)| *distance <= 3)
        .min_by_key(|(_, distance)| *distance)
        .map(|(name, _)| name)
}

/// Levenshtein distance, over characters.
///
/// Written here rather than reached for: the two in the tree are behind crates
/// this one has no other reason to depend on — `strsim` through `koru-search`,
/// and `koru_voice_bench::metrics`, which is a benchmark harness. Fifteen lines
/// against a dependency edge from the accessibility runner to the search index.
fn edits(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, left) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, right) in b.iter().enumerate() {
            let substitute = previous[j] + usize::from(left != right);
            current[j + 1] = substitute.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests;
