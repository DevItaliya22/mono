// SPDX-License-Identifier: MPL-2.0

//! **A command line that is not understood is not run.**
//!
//! The same rule, and the same shape, as `crates/koru-workspace-host/src/cli.rs`:
//! one table that both the parser and the help read, so a flag cannot be
//! accepted without being documented or documented without being accepted.
//! An unknown argument names itself, suggests the nearest real flag, and
//! exits 2.
//!
//! For a relay the stakes are the usual ones plus a public one: a mistyped
//! `--config` silently ignored would start a relay with no TLS and no
//! hostnames on the open internet — so a run needs exactly one of
//! `--config <path>` or `--local`, and anything else is refused.
//!
//! One subcommand exists: `probe <url>`, the namespace the bare-word refusal
//! was keeping clear. It gets the same treatment from its own table
//! ([`PROBE_FLAGS`]): the URL must parse as a [`RelayUrl`] *here*, before any
//! endpoint is built, so a typo is a usage error and never a probe failure
//! the monitors would page on.

use std::path::PathBuf;
use std::time::Duration;

use iroh::RelayUrl;
use koru_relay::DEFAULT_PROBE_TIMEOUT;

/// What to serve.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Developer mode: loopback, ephemeral ports, plain HTTP, no QAD.
    /// For humans only — repository tests use `koru-relay-test-support`.
    Local,
    /// The deployment described by this TOML file.
    Config(PathBuf),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Command {
    Serve(Mode),
    /// Probe the relay at this URL: the library's `probe_relay`, one line on
    /// stdout, exit 0 only if the relay relayed (Plan 1.3).
    Probe {
        url: RelayUrl,
        timeout: Duration,
    },
    Help,
    Version,
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
        name: "--config",
        short: None,
        takes_value: true,
        args: "--config <path>",
        help: "serve the deployment described by this TOML file",
    },
    Flag {
        name: "--local",
        short: None,
        takes_value: false,
        args: "--local",
        help: "developer mode: loopback, ephemeral ports, plain HTTP, no QAD",
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

/// The `probe` subcommand's table, read by its parser and by the help. The
/// default timeout is the library's [`DEFAULT_PROBE_TIMEOUT`], appended to
/// the help dynamically so the documented number cannot drift from the code.
const PROBE_FLAGS: &[Flag] = &[Flag {
    name: "--timeout",
    short: None,
    takes_value: true,
    args: "--timeout <seconds>",
    help: "give up after this many whole seconds",
}];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    Unknown {
        arg: String,
        did_you_mean: Option<&'static str>,
    },
    MissingValue {
        flag: String,
    },
    /// `--local` and `--config` together, or the same flag twice.
    Combined {
        first: String,
        second: String,
    },
    /// Neither `--config` nor `--local`: there is nothing safe to default to.
    NoMode,
    /// `probe` appeared after other arguments; a subcommand comes first.
    ProbeNotFirst,
    /// `probe` with nothing to probe.
    NoUrl,
    /// `probe` with more than one URL.
    ExtraUrl {
        first: String,
        second: String,
    },
    /// The probe URL did not parse as a relay URL, or its scheme is not one a
    /// relay is reached over. Refused here so a typo is a usage error, never
    /// a probe failure.
    InvalidUrl {
        url: String,
        message: String,
    },
    /// `--timeout` with anything but a positive whole number of seconds.
    InvalidTimeout {
        value: String,
    },
}

impl Rejected {
    pub fn message(&self) -> String {
        match self {
            Rejected::Unknown {
                arg,
                did_you_mean: Some(near),
            } => format!("unknown argument `{arg}` — did you mean `{near}`?"),
            Rejected::Unknown {
                arg,
                did_you_mean: None,
            } => format!("unknown argument `{arg}`"),
            Rejected::MissingValue { flag } => format!("`{flag}` needs a value"),
            Rejected::Combined { first, second } if first == second => {
                format!("`{first}` was given twice")
            }
            Rejected::Combined { first, second } => {
                format!("`{first}` does not combine with `{second}`")
            }
            Rejected::NoMode => "nothing to serve: pass `--config <path>` or `--local`".to_string(),
            Rejected::ProbeNotFirst => {
                "`probe` is a subcommand and comes first: `koru-relay probe <url>`".to_string()
            }
            Rejected::NoUrl => "probe needs a relay URL: `koru-relay probe <url>`".to_string(),
            Rejected::ExtraUrl { first, second } => {
                format!("probe takes one URL; `{second}` came after `{first}`")
            }
            Rejected::InvalidUrl { url, message } => {
                format!("`{url}` is not a relay URL: {message}")
            }
            Rejected::InvalidTimeout { value } => format!(
                "`--timeout` takes a whole number of seconds greater than zero, not `{value}`"
            ),
        }
    }

    /// Say what was wrong, print the usage, exit 2.
    pub fn report(&self) -> ! {
        eprintln!("koru-relay: {}", self.message());
        eprintln!();
        eprint!("{}", help());
        std::process::exit(2)
    }
}

pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Command, Rejected> {
    let tokens: Vec<String> = args.into_iter().skip(1).collect();
    if tokens.first().map(String::as_str) == Some("probe") {
        return parse_probe(&tokens[1..]);
    }
    let mut mode: Option<(Mode, String)> = None;
    let mut i = 0;

    while i < tokens.len() {
        let token = tokens[i].clone();

        // `--help` and `--version` win wherever they appear, so
        // `koru-relay --config typo.toml --help` explains rather than fails.
        if token == "--help" || token == "-h" {
            return Ok(Command::Help);
        }
        if token == "--version" || token == "-V" {
            return Ok(Command::Version);
        }
        // A `probe` that is not the first argument is recognised, not merely
        // unknown: the refusal teaches where it goes.
        if token == "probe" {
            return Err(Rejected::ProbeNotFirst);
        }

        let (name, inline) = match token.split_once('=') {
            Some((n, v)) if n.starts_with('-') => (n.to_string(), Some(v.to_string())),
            _ => (token.clone(), None),
        };
        let Some(flag) = FLAGS
            .iter()
            .find(|f| f.name == name || f.short == Some(name.as_str()))
        else {
            return Err(Rejected::Unknown {
                did_you_mean: nearest(&name),
                arg: name,
            });
        };
        let chosen = if flag.takes_value {
            let value = match inline {
                Some(v) => {
                    i += 1;
                    v
                }
                None => {
                    let Some(v) = tokens.get(i + 1) else {
                        return Err(Rejected::MissingValue { flag: name });
                    };
                    i += 2;
                    v.clone()
                }
            };
            debug_assert_eq!(flag.name, "--config");
            Mode::Config(PathBuf::from(value))
        } else {
            // `--local=1` carries a value nothing understands; refuse it
            // rather than quietly dropping the `=1`.
            if inline.is_some() {
                return Err(Rejected::Unknown {
                    arg: token,
                    did_you_mean: Some(flag.name),
                });
            }
            debug_assert_eq!(flag.name, "--local");
            i += 1;
            Mode::Local
        };
        if let Some((_, first)) = &mode {
            return Err(Rejected::Combined {
                first: first.clone(),
                second: flag.name.to_string(),
            });
        }
        mode = Some((chosen, flag.name.to_string()));
    }

    match mode {
        Some((mode, _)) => Ok(Command::Serve(mode)),
        None => Err(Rejected::NoMode),
    }
}

/// `koru-relay probe <url> [--timeout <seconds>]`, from its own table.
///
/// The one positional is the relay URL, validated here (see
/// [`parse_relay_url`]); everything else must be a documented probe flag.
fn parse_probe(tokens: &[String]) -> Result<Command, Rejected> {
    let mut url: Option<RelayUrl> = None;
    let mut timeout: Option<Duration> = None;
    let mut i = 0;

    while i < tokens.len() {
        let token = tokens[i].clone();

        if token == "--help" || token == "-h" {
            return Ok(Command::Help);
        }
        if token == "--version" || token == "-V" {
            return Ok(Command::Version);
        }

        if !token.starts_with('-') {
            // The positional: the relay URL, exactly one of it.
            if let Some(first) = &url {
                return Err(Rejected::ExtraUrl {
                    first: first.to_string(),
                    second: token,
                });
            }
            url = Some(parse_relay_url(&token)?);
            i += 1;
            continue;
        }

        let (name, inline) = match token.split_once('=') {
            Some((n, v)) => (n.to_string(), Some(v.to_string())),
            None => (token.clone(), None),
        };
        let Some(flag) = PROBE_FLAGS
            .iter()
            .find(|f| f.name == name || f.short == Some(name.as_str()))
        else {
            return Err(Rejected::Unknown {
                did_you_mean: nearest_probe(&name),
                arg: name,
            });
        };
        debug_assert_eq!(flag.name, "--timeout");
        let value = match inline {
            Some(v) => {
                i += 1;
                v
            }
            None => {
                let Some(v) = tokens.get(i + 1) else {
                    return Err(Rejected::MissingValue { flag: name });
                };
                i += 2;
                v.clone()
            }
        };
        if timeout.is_some() {
            return Err(Rejected::Combined {
                first: flag.name.to_string(),
                second: flag.name.to_string(),
            });
        }
        timeout = Some(parse_timeout(&value)?);
    }

    match url {
        Some(url) => Ok(Command::Probe {
            url,
            timeout: timeout.unwrap_or(DEFAULT_PROBE_TIMEOUT),
        }),
        None => Err(Rejected::NoUrl),
    }
}

/// The probe URL, validated before any endpoint exists.
///
/// It must parse as a [`RelayUrl`] and be reachable the way relays are —
/// `http` or `https` (a bare `host:port` parses as a URL whose *scheme* is
/// the host, which a probe would then chase into a confusing timeout).
fn parse_relay_url(arg: &str) -> Result<RelayUrl, Rejected> {
    let url: RelayUrl = match arg.parse() {
        Ok(url) => url,
        Err(error) => {
            return Err(Rejected::InvalidUrl {
                url: arg.to_string(),
                message: error.to_string(),
            });
        }
    };
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(Rejected::InvalidUrl {
            url: arg.to_string(),
            message: format!(
                "a relay is reached over http or https, not `{}`",
                url.scheme()
            ),
        });
    }
    Ok(url)
}

/// Seconds for `--timeout`: a positive whole number. Zero would fail every
/// probe before it began, so it is refused rather than accepted as a trap.
fn parse_timeout(value: &str) -> Result<Duration, Rejected> {
    match value.parse::<u64>() {
        Ok(seconds) if seconds > 0 => Ok(Duration::from_secs(seconds)),
        _ => Err(Rejected::InvalidTimeout {
            value: value.to_string(),
        }),
    }
}

/// The nearest flag within three edits, so a typo is corrected rather than
/// merely refused. `probe` is a candidate too: it is the one bare word the
/// command line knows.
fn nearest(name: &str) -> Option<&'static str> {
    nearest_of(name, FLAGS.iter().map(|f| f.name).chain(["probe"]))
}

/// The nearest probe argument: the probe table plus the global `--help` and
/// `--version`, which the probe parser also accepts.
fn nearest_probe(name: &str) -> Option<&'static str> {
    nearest_of(
        name,
        PROBE_FLAGS
            .iter()
            .map(|f| f.name)
            .chain(["--help", "--version"]),
    )
}

fn nearest_of(
    name: &str,
    candidates: impl IntoIterator<Item = &'static str>,
) -> Option<&'static str> {
    candidates
        .into_iter()
        .map(|candidate| (candidate, distance(name, candidate)))
        .filter(|(_, d)| *d <= 3)
        .min_by_key(|(_, d)| *d)
        .map(|(candidate, _)| candidate)
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
    format!("koru-relay {}", env!("CARGO_PKG_VERSION"))
}

/// Built from [`FLAGS`] and the config example, so it cannot drift from the
/// parser or the schema.
pub fn help() -> String {
    let mut out = String::from(
        "Koru relay — the upstream iroh-relay server, configured for Koru.\n\n\
         Usage: koru-relay --config <path>\n\
         \x20      koru-relay --local\n\
         \x20      koru-relay probe <url> [--timeout <seconds>]\n\n\
         Options:\n",
    );
    for flag in FLAGS {
        let spelling = match flag.short {
            Some(short) => format!("{}, {short}", flag.args),
            None => flag.args.to_string(),
        };
        out.push_str(&format!("  {spelling:<20} {}\n", flag.help));
    }
    out.push_str(
        "\n`probe <url>` asks whether the relay at <url> actually relays: two\n\
         fresh endpoints whose only route is that relay connect through it and\n\
         round-trip a nonce. One line on stdout says what happened; the exit\n\
         code is 0 only if the relay relayed, so scripts read the code and\n\
         people read the line.\n\n\
         Probe options:\n",
    );
    for flag in PROBE_FLAGS {
        let spelling = match flag.short {
            Some(short) => format!("{}, {short}", flag.args),
            None => flag.args.to_string(),
        };
        // The default is the library constant, printed rather than retyped,
        // so the documented number cannot drift from the shipped one.
        let default_note = if flag.name == "--timeout" {
            format!(" (default {}s)", DEFAULT_PROBE_TIMEOUT.as_secs())
        } else {
            String::new()
        };
        out.push_str(&format!("  {spelling:<20} {}{default_note}\n", flag.help));
    }
    out.push_str(
        "\nThe config file is TOML. `hostnames` is required and has no default;\n\
         every other key is optional, so this example is equivalent to a file\n\
         holding only its `hostnames` line:\n\n",
    );
    for line in crate::config::EXAMPLE.lines() {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(
        "\nWith TLS, /healthz answers on the HTTPS bind and the HTTP bind serves\n\
         only the captive-portal probe; with --local, /healthz answers on the one\n\
         plain-HTTP bind. Metrics are Prometheus text on the private metrics\n\
         bind. KORU_RELAY_DEBUG turns on per-connection diagnostics.\n",
    );
    out
}

#[cfg(test)]
mod tests;
