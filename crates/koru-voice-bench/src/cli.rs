// SPDX-License-Identifier: MPL-2.0

//! **An argument that is not understood is not run.**
//!
//! The rule `koru_workspace_host::cli::parse` already enforces for the window, applied
//! here for the same reason: ignoring an unknown argument sounds harmless and is
//! the same failure as guessing a directory. A mistyped `--corpus` that fell
//! back to an environment variable would measure a corpus nobody named and print
//! its hashes in the header as though they had been asked for.
//!
//! So: the token is named, the nearest flag within three edits is suggested, the
//! usage is printed and the process exits 2. [`FLAGS`] is the only table and
//! `every_documented_flag_parses` fails if the parser and the help drift apart.
//!
//! ## There is no default corpus and no search path
//!
//! `--corpus <dir>`, falling back to `KORU_VOICE_BENCH_CORPUS`, and otherwise a
//! refusal. Likewise `--models <dir>`. A benchmark that silently found *some*
//! corpus is exactly the failure the strict parser exists to prevent, one level
//! up.
//!
//! `--smoke` is the exception and is explicit about it: it names the checked-in
//! fixture, needs neither argument, and prints a header that says which fixture
//! it read.

use std::path::PathBuf;

/// Which stage of the run.
#[derive(Copy, Clone, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    /// n = 100, all five candidates, `W-desk` only. p95 figures print with an
    /// unbounded upper CI and are labelled `screen` — so a screen figure can
    /// never be pasted into a decision document by accident.
    Screen,
    /// n = 300 (600 for `ZhHans`), the survivors, all four classes. The numbers
    /// ADR-253 cites.
    Decision,
    /// The checked-in fixture, eight languages, no corpus. Proves the harness
    /// runs; produces no number anybody cites.
    Smoke,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Screen => "screen",
            Stage::Decision => "decision",
            Stage::Smoke => "smoke",
        }
    }

    /// How many utterances per language this stage draws.
    ///
    /// n = 100 is not enough for a p95 — the distribution-free 90% interval runs
    /// from the 91st to the 100th order statistic, so its upper end *is* the
    /// largest observation. That is why the screen can only screen.
    pub fn sample_size(self, language: koru_voice::VoiceLanguage) -> usize {
        match self {
            Stage::Screen => 100,
            Stage::Decision if language == koru_voice::VoiceLanguage::ZhHans => 600,
            Stage::Decision => 300,
            Stage::Smoke => 1,
        }
    }
}

/// One flag, as parsed and as printed. The only table.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Flag {
    pub name: &'static str,
    /// What it takes, or `None` for a switch.
    pub value: Option<&'static str>,
    pub help: &'static str,
}

/// Every flag this binary understands.
pub const FLAGS: [Flag; 8] = [
    Flag {
        name: "--stage",
        value: Some("screen|decision"),
        help: "which stage to run; the default is screen",
    },
    Flag {
        name: "--corpus",
        value: Some("dir"),
        help: "the decision corpus tree; falls back to KORU_VOICE_BENCH_CORPUS, and there is no default",
    },
    Flag {
        name: "--models",
        value: Some("dir"),
        help: "the provisioned model directories; falls back to KORU_VOICE_BENCH_MODELS",
    },
    Flag {
        name: "--class",
        value: Some("name"),
        help: "the hardware class this machine is, e.g. W-desk; printed in the header",
    },
    Flag {
        name: "--threads",
        value: Some("n"),
        help: "num_threads for every engine; the gate value is 2 and there is no default",
    },
    Flag {
        name: "--candidate",
        value: Some("n"),
        help: "run only candidate n; repeatable",
    },
    Flag {
        name: "--out",
        value: Some("dir"),
        help: "where bench-<run-id>.json is written; the default is the working directory",
    },
    Flag {
        name: "--smoke",
        value: None,
        help: "run the checked-in smoke fixture in all eight languages and exit",
    },
];

/// A parsed command line.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Options {
    pub stage: Stage,
    pub corpus: Option<PathBuf>,
    pub models: Option<PathBuf>,
    pub class: String,
    pub num_threads: i32,
    /// Empty means all of them.
    pub candidates: Vec<u8>,
    pub out: PathBuf,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            stage: Stage::Screen,
            corpus: None,
            models: None,
            // Not "unknown": a class is a claim about the machine and the
            // header has to carry one, so an unstated class prints as what it
            // is rather than as a plausible name.
            class: "unstated".to_owned(),
            // **The gate value, and it is here rather than nowhere** — but
            // `--threads` is still accepted, because `W-desk` additionally
            // reports every figure at 4 so the scaling is visible.
            num_threads: 2,
            candidates: Vec::new(),
            out: PathBuf::from("."),
        }
    }
}

/// What a bad command line is.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CliError {
    pub message: String,
    /// The nearest flag within three edits, if there is one.
    pub suggestion: Option<&'static str>,
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(suggestion) = self.suggestion {
            write!(f, " (did you mean {suggestion}?)")?;
        }
        Ok(())
    }
}

/// The usage text, generated from [`FLAGS`] so it cannot drift from the parser.
pub fn usage() -> String {
    let mut out = String::from("koru-voice-bench — the speech-engine benchmark\n\nusage:\n");
    for flag in &FLAGS {
        let head = match flag.value {
            Some(value) => format!("  {} <{}>", flag.name, value),
            None => format!("  {}", flag.name),
        };
        out.push_str(&format!("{head:<28}{}\n", flag.help));
    }
    out.push_str(
        "\nThe protocol is docs/features/voice/plans/ENGINE_BENCHMARK.md. No metric outside it \
         is printed.\n",
    );
    out
}

/// Parse. Strict: an unknown token is an error, never an ignore.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Options, CliError> {
    let mut options = Options::default();
    let mut args = args.into_iter().peekable();

    while let Some(token) = args.next() {
        let flag = FLAGS.iter().find(|f| f.name == token);
        let Some(flag) = flag else {
            return Err(CliError {
                message: format!("{token} is not an argument this program understands"),
                suggestion: nearest(&token),
            });
        };
        let value = if flag.value.is_some() {
            match args.next() {
                Some(value) => value,
                None => {
                    return Err(CliError {
                        message: format!("{token} needs a value"),
                        suggestion: None,
                    });
                }
            }
        } else {
            String::new()
        };

        match flag.name {
            "--stage" => {
                options.stage = match value.as_str() {
                    "screen" => Stage::Screen,
                    "decision" => Stage::Decision,
                    // Deliberately not accepted here: `--smoke` is its own
                    // switch, so nothing that says `--stage decision` can turn
                    // into a fixture run through a typo.
                    other => {
                        return Err(CliError {
                            message: format!("{other} is not a stage; it is screen or decision"),
                            suggestion: None,
                        });
                    }
                }
            }
            "--corpus" => options.corpus = Some(PathBuf::from(value)),
            "--models" => options.models = Some(PathBuf::from(value)),
            "--class" => options.class = value,
            "--threads" => {
                options.num_threads =
                    value
                        .parse::<i32>()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or(CliError {
                            message: format!("{value} is not a positive thread count"),
                            suggestion: None,
                        })?
            }
            "--candidate" => {
                let number = value.parse::<u8>().ok().ok_or(CliError {
                    message: format!("{value} is not a candidate number"),
                    suggestion: None,
                })?;
                if crate::candidate::Candidate::by_number(number).is_none() {
                    return Err(CliError {
                        message: format!("there is no candidate {number}; they are 1 to 4"),
                        suggestion: None,
                    });
                }
                options.candidates.push(number);
            }
            "--out" => options.out = PathBuf::from(value),
            "--smoke" => options.stage = Stage::Smoke,
            _ => unreachable!("FLAGS and this match are the same list"),
        }
    }

    if options.stage != Stage::Smoke {
        options.corpus = options
            .corpus
            .or_else(|| std::env::var_os("KORU_VOICE_BENCH_CORPUS").map(PathBuf::from));
        options.models = options
            .models
            .or_else(|| std::env::var_os("KORU_VOICE_BENCH_MODELS").map(PathBuf::from));
        if options.corpus.is_none() {
            return Err(CliError {
                message: "no corpus: pass --corpus <dir> or set KORU_VOICE_BENCH_CORPUS. \
                          There is deliberately no default and no search path — a benchmark \
                          that silently found some corpus is not a protocol"
                    .to_owned(),
                suggestion: None,
            });
        }
    }

    Ok(options)
}

/// The nearest flag within three edits.
///
/// Three, the same distance `koru_workspace_host::cli::parse` uses, because a suggestion
/// that fires on anything is one nobody trusts.
fn nearest(token: &str) -> Option<&'static str> {
    FLAGS
        .iter()
        .map(|f| (f.name, edits(token, f.name)))
        .filter(|(_, d)| *d <= 3)
        .min_by_key(|(_, d)| *d)
        .map(|(name, _)| name)
}

fn edits(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    crate::metrics::levenshtein(&a, &b)
}

/// The environment variables this binary reads.
///
/// **Named here because a variable read without a row in `koru-env`'s `VARS` is
/// invisible to `koru.bat envs` and to `reference/ENV.md`.** These two are
/// not in that table yet: `koru-env` is owned by another plan's working tree and
/// adding rows to it from here would be two agents editing one list. Recorded as
/// `docs/features/voice/archive/QUESTIONS.md` Q-V54, with the two rows written out so it is
/// a paste rather than a decision.
pub const ENV_VARS: [&str; 2] = ["KORU_VOICE_BENCH_CORPUS", "KORU_VOICE_BENCH_MODELS"];

#[cfg(test)]
mod tests;
