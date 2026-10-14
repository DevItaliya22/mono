// SPDX-License-Identifier: MPL-2.0

//! **The run header, the four tables, the probe, and `bench-<run-id>.json`.**
//!
//! Plan 0.4 pastes the tables; anyone re-aggregating or re-plotting reads the
//! JSON. **The two are generated from one structure so they cannot disagree** —
//! which is why [`Run`] is the only thing either of them is rendered from and
//! there is no second path that formats a number.
//!
//! ## The rules that apply to every cell, enforced rather than remembered
//!
//! - **A number without an `n` is not printed.**
//! - **A quantile without an interval is not printed.**
//! - `n < 30` prints `n/a (n=…)`.
//! - **No cell averages an error rate across languages**, and no cell places a
//!   WER and a CER in the same aggregate. There is no function here that could:
//!   an [`AccuracyRow`] carries its language and its unit, and nothing sums two
//!   of them.
//! - A screen-stage p95 prints as `≥ x` and carries the label, so a screen figure
//!   can never be pasted into a decision document by accident.
//!
//! The first three are [`cell`]'s job and it is the only formatter of a number
//! with an `n`.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use koru_voice::VoiceLanguage;
use serde::Serialize;

use crate::candidate::Candidate;
use crate::cli::Stage;
use crate::corpus::SourceId;
use crate::metrics::Unit;
use crate::stats::{Censored, Interval, MINIMUM_N, Quantile};

/// The header block, once, at the top of every run.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct Header {
    pub harness: String,
    pub protocol_rev: u32,
    pub run_id: String,
    pub stage: Stage,
    pub host_class: String,
    pub host: String,
    pub num_threads: i32,
    /// One line per pinned source: revision, source hash, converted hash.
    pub corpus: Vec<String>,
    pub converter: String,
    /// Every note the run has to carry: a placeholder fixture, an absent t2s
    /// table, a candidate substitution. **Notes are not warnings that scroll
    /// past** — they are in the header because a run whose header is incomplete
    /// is a run whose numbers cannot be compared.
    pub notes: Vec<String>,
}

impl Header {
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "koru-voice-bench {}    protocol BENCHMARK_PROTOCOL.md rev {}",
            self.harness, self.protocol_rev
        );
        let _ = writeln!(out, "run-id        {}", self.run_id);
        let _ = writeln!(out, "stage         {}", self.stage.as_str());
        let _ = writeln!(out, "host-class    {}", self.host_class);
        let _ = writeln!(out, "host          {}", self.host);
        let _ = writeln!(out, "num-threads   {}", self.num_threads);
        if self.corpus.is_empty() {
            let _ = writeln!(out, "corpus        none (smoke fixture)");
        } else {
            for (index, line) in self.corpus.iter().enumerate() {
                let label = if index == 0 {
                    "corpus      "
                } else {
                    "            "
                };
                let _ = writeln!(out, "{label}  {line}");
            }
        }
        let _ = writeln!(out, "converter     {}", self.converter);
        if self.notes.is_empty() {
            let _ = writeln!(out, "notes         none");
        } else {
            for (index, note) in self.notes.iter().enumerate() {
                let label = if index == 0 {
                    "notes       "
                } else {
                    "            "
                };
                let _ = writeln!(out, "{label}  {note}");
            }
        }
        out
    }
}

/// Table 1 — latency, from the paced pass over Common Voice.
#[derive(Clone, PartialEq, Debug, Serialize)]
pub struct LatencyRow {
    pub cand: u8,
    pub lang: String,
    pub n: usize,
    /// The censored count, printed beside every `TTFUP` figure **without
    /// exception**.
    pub cens: usize,
    pub ttfup_p50_ms: Option<f64>,
    pub ttfup_p95: Option<CensoredFigure>,
    pub fin_p50_ms: Option<f64>,
    pub fin_p95_ms: Option<f64>,
    pub revisions_per_utterance: Option<(f64, Interval)>,
    pub cpu_percent: Option<f64>,
    /// Frames the engine could not keep up with. See [`crate::pace`] — an engine
    /// that cannot be paced is measuring a queue.
    pub overruns: usize,
}

/// A p95 that may not be estimable.
#[derive(Clone, PartialEq, Debug, Serialize)]
pub enum CensoredFigure {
    Estimable {
        ms: f64,
        ci: (f64, f64),
        /// True at the screen stage, where the upper rank is `n`.
        unbounded_above: bool,
    },
    NotEstimable {
        at_least_ms: Option<f64>,
        fraction: f64,
    },
}

impl CensoredFigure {
    pub fn from(censored: Censored) -> CensoredFigure {
        match censored {
            Censored::Estimable(q) => CensoredFigure::Estimable {
                ms: q.value,
                ci: (q.interval.low, q.interval.high),
                unbounded_above: q.unbounded_above,
            },
            Censored::NotEstimable {
                largest_uncensored,
                fraction,
            } => CensoredFigure::NotEstimable {
                at_least_ms: largest_uncensored,
                fraction,
            },
        }
    }

    fn render(&self) -> String {
        match self {
            CensoredFigure::Estimable {
                ms,
                unbounded_above: true,
                ..
            } => format!(">={ms:.0} ms"),
            CensoredFigure::Estimable { ms, .. } => format!("{ms:.0} ms"),
            CensoredFigure::NotEstimable {
                at_least_ms: Some(ms),
                fraction,
            } => format!(">{ms:.0} ({:.1}% cens)", fraction * 100.0),
            CensoredFigure::NotEstimable {
                at_least_ms: None,
                fraction,
            } => format!("all censored ({:.1}%)", fraction * 100.0),
        }
    }

    fn render_ci(&self) -> String {
        match self {
            CensoredFigure::Estimable { ci, .. } => format!("[{:.0}, {:.0}]", ci.0, ci.1),
            CensoredFigure::NotEstimable { .. } => "not estimable".to_owned(),
        }
    }

    /// Whether this row fails its language under the protocol's censoring rule.
    pub fn is_failure(&self) -> bool {
        matches!(self, CensoredFigure::NotEstimable { .. })
    }
}

/// Table 2 — accuracy, from the paced pass.
#[derive(Clone, PartialEq, Debug, Serialize)]
pub struct AccuracyRow {
    pub cand: u8,
    pub lang: String,
    pub corpus: String,
    pub n: usize,
    /// Excluded by the digit rule and the duration bounds.
    pub excl: usize,
    /// `WER` or `CER`. **Carried per row**, so nothing can put two of them in
    /// one aggregate.
    pub metric: &'static str,
    pub value: Option<f64>,
    pub ci: Option<(f64, f64)>,
    /// Utterances whose final transcript differs between the paced and unpaced
    /// passes. A nonzero value is called out rather than averaged in: both a
    /// fixed chunk schedule and a chunk-and-redecode policy are supposed to be
    /// timing-independent, and this says one of them is not.
    pub drift: usize,
}

/// Table 3 — cost, from the unpaced pass, per candidate per class.
#[derive(Clone, PartialEq, Debug, Serialize)]
pub struct CostRow {
    pub cand: u8,
    pub class: String,
    pub thr: i32,
    pub load_first_ms: Option<f64>,
    pub load_warm_p50_ms: Option<f64>,
    pub load_warm_p95_ms: Option<f64>,
    pub rtf_p50: Option<f64>,
    pub rtf_p95: Option<f64>,
    pub peak_rss_bytes: Option<u64>,
    pub model_on_disk_bytes: Option<u64>,
}

/// Table 4 — Simplified Chinese, by source and by speaker. **Never a pooled
/// row**: AISHELL-1 is clean read speech on controlled hardware and Common Voice
/// is crowd-sourced on unknown microphones, so a pooled CER describes neither —
/// and which of the two a candidate is bad at is exactly the decision.
#[derive(Clone, PartialEq, Debug, Serialize)]
pub struct ChineseRow {
    pub cand: u8,
    pub source: String,
    pub n: usize,
    pub cer: Option<f64>,
    pub ci: Option<(f64, f64)>,
    pub speakers: usize,
    /// The point of this table: a max far above the median is the failure mode a
    /// mean would hide.
    pub speaker_cer_min: Option<f64>,
    pub speaker_cer_median: Option<f64>,
    pub speaker_cer_max: Option<f64>,
    pub cens: usize,
}

/// The proper-noun probe, smoke fixture only. **Not scored, not aggregated, and
/// not a gate.**
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct ProbeRow {
    pub cand: u8,
    pub lang: String,
    /// Name → found.
    pub hits: BTreeMap<String, bool>,
}

/// The `L(language)` check the **screen** runs, and only the screen.
///
/// The check is that the mean number of words in a qualifying prefix falls in
/// `[1.5, 3.0]`. A language outside that band has its `L` adjusted once, the
/// screen rerun for it, and the adjusted table recorded in
/// `ENGINE_DECISION.md`. **Adjusting after seeing decision-run results is
/// forbidden**, which is why this exists at the screen stage and nowhere else.
#[derive(Clone, PartialEq, Debug, Serialize)]
pub struct PrefixCheckRow {
    pub lang: String,
    pub l_graphemes: usize,
    pub mean_words: Option<f64>,
    pub in_band: bool,
}

/// The whole run, and the only thing either output is rendered from.
#[derive(Clone, PartialEq, Debug, Serialize)]
pub struct Run {
    pub header: Header,
    pub latency: Vec<LatencyRow>,
    pub accuracy: Vec<AccuracyRow>,
    pub cost: Vec<CostRow>,
    pub chinese: Vec<ChineseRow>,
    pub probe: Vec<ProbeRow>,
    pub prefix_check: Vec<PrefixCheckRow>,
    /// One line per candidate that could not be built, with the reason.
    pub unavailable: Vec<String>,
}

/// Format a number that has an `n` behind it.
///
/// **The one formatter**, so the three rules — a number without an `n` is not
/// printed, `n < 30` prints `n/a (n=…)`, an absent value prints as an absence —
/// are enforced in a single place rather than remembered at every call site.
pub fn cell(value: Option<f64>, n: usize, format: impl Fn(f64) -> String) -> String {
    if n < MINIMUM_N {
        return format!("n/a (n={n})");
    }
    match value {
        Some(value) => format(value),
        None => "—".to_owned(),
    }
}

/// A quantile that has to print with its interval or not at all.
pub fn quantile_cell(q: Option<Quantile>, n: usize) -> (String, String) {
    if n < MINIMUM_N {
        return (format!("n/a (n={n})"), String::new());
    }
    match q {
        Some(q) => (
            format!("{:.0} ms", q.value),
            format!("[{:.0}, {:.0}]", q.interval.low, q.interval.high),
        ),
        None => ("—".to_owned(), String::new()),
    }
}

impl Run {
    /// The printed report: the header, then every table that has a row.
    ///
    /// A table with no rows is **omitted with a line saying so** rather than
    /// printed empty: an empty table reads as "nothing happened", and a line
    /// saying which table is empty reads as what it is.
    pub fn render(&self) -> String {
        let mut out = self.header.render();

        if !self.unavailable.is_empty() {
            out.push_str("\ncandidates not run\n");
            for line in &self.unavailable {
                let _ = writeln!(out, "  {line}");
            }
        }

        out.push_str("\nTable 1 — Latency (paced pass, Common Voice)\n");
        if self.latency.is_empty() {
            out.push_str("  (no rows: no candidate produced a paced session)\n");
        } else {
            let _ = writeln!(
                out,
                "{:<5} {:<8} {:>4} {:>5} {:>10} {:>16} {:>14} {:>9} {:>9} {:>14} {:>5} {:>4}",
                "cand",
                "lang",
                "n",
                "cens",
                "ttfup_p50",
                "ttfup_p95",
                "ttfup_p95_ci",
                "fin_p50",
                "fin_p95",
                "rev/utt",
                "cpu%",
                "over"
            );
            for row in &self.latency {
                let (p95, ci) = match &row.ttfup_p95 {
                    Some(figure) if row.n >= MINIMUM_N => (figure.render(), figure.render_ci()),
                    Some(_) => (format!("n/a (n={})", row.n), String::new()),
                    None => ("—".to_owned(), String::new()),
                };
                let revisions = match row.revisions_per_utterance {
                    Some((mean, interval)) if row.n >= MINIMUM_N => {
                        format!("{mean:.1} [{:.1},{:.1}]", interval.low, interval.high)
                    }
                    Some(_) => format!("n/a (n={})", row.n),
                    None => "—".to_owned(),
                };
                let _ = writeln!(
                    out,
                    "{:<5} {:<8} {:>4} {:>5} {:>10} {:>16} {:>14} {:>9} {:>9} {:>14} {:>5} {:>4}",
                    row.cand,
                    row.lang,
                    row.n,
                    row.cens,
                    cell(row.ttfup_p50_ms, row.n, |v| format!("{v:.0} ms")),
                    p95,
                    ci,
                    cell(row.fin_p50_ms, row.n, |v| format!("{v:.0} ms")),
                    cell(row.fin_p95_ms, row.n, |v| format!("{v:.0} ms")),
                    revisions,
                    cell(row.cpu_percent, row.n, |v| format!("{v:.0}")),
                    row.overruns
                );
            }
        }

        out.push_str("\nTable 2 — Accuracy (paced pass)\n");
        if self.accuracy.is_empty() {
            out.push_str("  (no rows: no candidate produced a final transcript)\n");
        } else {
            let _ = writeln!(
                out,
                "{:<5} {:<8} {:<8} {:>4} {:>5} {:<7} {:>8} {:>16} {:>6}",
                "cand", "lang", "corpus", "n", "excl", "metric", "value", "ci_95", "drift"
            );
            for row in &self.accuracy {
                let ci = match row.ci {
                    Some((low, high)) if row.n >= MINIMUM_N => {
                        format!("[{:.1}, {:.1}]", low * 100.0, high * 100.0)
                    }
                    _ => String::new(),
                };
                let _ = writeln!(
                    out,
                    "{:<5} {:<8} {:<8} {:>4} {:>5} {:<7} {:>8} {:>16} {:>6}",
                    row.cand,
                    row.lang,
                    row.corpus,
                    row.n,
                    row.excl,
                    row.metric,
                    cell(row.value, row.n, |v| format!("{:.1}%", v * 100.0)),
                    ci,
                    row.drift
                );
            }
        }

        out.push_str("\nTable 3 — Cost (unpaced pass, per candidate per class)\n");
        if self.cost.is_empty() {
            out.push_str("  (no rows: no candidate was loaded)\n");
        } else {
            let _ = writeln!(
                out,
                "{:<5} {:<10} {:>4} {:>11} {:>14} {:>14} {:>8} {:>8} {:>10} {:>14}",
                "cand",
                "class",
                "thr",
                "load_first",
                "load_warm_p50",
                "load_warm_p95",
                "rtf_p50",
                "rtf_p95",
                "peak_rss",
                "model_on_disk"
            );
            for row in &self.cost {
                let _ = writeln!(
                    out,
                    "{:<5} {:<10} {:>4} {:>11} {:>14} {:>14} {:>8} {:>8} {:>10} {:>14}",
                    row.cand,
                    row.class,
                    row.thr,
                    optional_ms(row.load_first_ms),
                    optional_ms(row.load_warm_p50_ms),
                    optional_ms(row.load_warm_p95_ms),
                    row.rtf_p50.map_or("—".to_owned(), |v| format!("{v:.2}")),
                    row.rtf_p95.map_or("—".to_owned(), |v| format!("{v:.2}")),
                    optional_mib(row.peak_rss_bytes),
                    optional_mib(row.model_on_disk_bytes),
                );
            }
        }

        if !self.chinese.is_empty() {
            out.push_str("\nTable 4 — Simplified Chinese, by source and by speaker\n");
            let _ = writeln!(
                out,
                "{:<5} {:<8} {:>4} {:>7} {:>16} {:>6} {:>12} {:>12} {:>12} {:>5}",
                "cand",
                "source",
                "n",
                "CER",
                "ci_95",
                "spk_n",
                "spk_cer_min",
                "spk_cer_med",
                "spk_cer_max",
                "cens"
            );
            for row in &self.chinese {
                let ci = match row.ci {
                    Some((low, high)) if row.n >= MINIMUM_N => {
                        format!("[{:.1}, {:.1}]", low * 100.0, high * 100.0)
                    }
                    _ => String::new(),
                };
                let _ = writeln!(
                    out,
                    "{:<5} {:<8} {:>4} {:>7} {:>16} {:>6} {:>12} {:>12} {:>12} {:>5}",
                    row.cand,
                    row.source,
                    row.n,
                    cell(row.cer, row.n, |v| format!("{:.1}%", v * 100.0)),
                    ci,
                    row.speakers,
                    optional_percent(row.speaker_cer_min),
                    optional_percent(row.speaker_cer_median),
                    optional_percent(row.speaker_cer_max),
                    row.cens
                );
            }
        }

        if !self.prefix_check.is_empty() {
            out.push_str("\nThe L(language) check (screen only; the band is [1.5, 3.0] words)\n");
            let _ = writeln!(
                out,
                "{:<8} {:>4} {:>12} {:>8}",
                "lang", "L", "mean_words", "in_band"
            );
            for row in &self.prefix_check {
                let _ = writeln!(
                    out,
                    "{:<8} {:>4} {:>12} {:>8}",
                    row.lang,
                    row.l_graphemes,
                    row.mean_words.map_or("—".to_owned(), |v| format!("{v:.2}")),
                    if row.in_band { "ok" } else { "ADJUST" }
                );
            }
        }

        if !self.probe.is_empty() {
            out.push_str("\nThe proper-noun probe (smoke fixture only; not scored, not a gate)\n");
            let mut header = format!("{:<5} {:<8}", "cand", "lang");
            for name in crate::corpus::PROPER_NOUNS {
                let _ = write!(header, " {name:>7}");
            }
            let _ = writeln!(out, "{header}");
            for row in &self.probe {
                let mut line = format!("{:<5} {:<8}", row.cand, row.lang);
                for name in crate::corpus::PROPER_NOUNS {
                    let hit = row.hits.get(name).copied().unwrap_or(false);
                    let _ = write!(line, " {:>7}", if hit { "ok" } else { "--" });
                }
                let _ = writeln!(out, "{line}");
            }
        }

        out
    }

    /// `bench-<run-id>.json`, from the same structure the tables came from.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a Run is plain data and always serializes")
    }

    /// The file name the JSON is written under.
    ///
    /// **The colons come out.** The protocol's run id is an RFC 3339 stamp and
    /// a colon is illegal in a Windows file name — `bench-2026-09-14T09:12:03Z`
    /// fails with `ERROR_INVALID_NAME` on the platform two of the four hardware
    /// classes run on. The id keeps its colons everywhere it is *printed*, so
    /// the header and the JSON still carry the stamp the protocol specifies;
    /// only the file name is sanitised, and only for the one character that
    /// needs it.
    pub fn json_file_name(&self) -> String {
        format!("bench-{}.json", self.header.run_id.replace(':', ""))
    }
}

fn optional_ms(value: Option<f64>) -> String {
    value.map_or("—".to_owned(), |v| format!("{v:.0} ms"))
}

fn optional_mib(value: Option<u64>) -> String {
    value.map_or("—".to_owned(), |v| {
        format!("{:.0} MiB", v as f64 / (1024.0 * 1024.0))
    })
}

fn optional_percent(value: Option<f64>) -> String {
    value.map_or("—".to_owned(), |v| format!("{:.1}%", v * 100.0))
}

/// A run id: an RFC 3339-ish UTC stamp and six hex characters.
///
/// The stamp is formatted by hand from the system clock rather than through
/// `chrono`, which this crate does not otherwise need: a benchmark that acquired
/// a date library to name a file would be paying for one dependency's worth of
/// build for one string. The suffix is derived from the process id and the
/// nanoseconds, so two runs started in the same second on one machine do not
/// share a file name.
pub fn run_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let (year, month, day, hour, minute, second) = civil_from_unix(secs as i64);
    let salt = (u64::from(std::process::id()) ^ u64::from(now.subsec_nanos())) & 0xff_ffff;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z-{salt:06x}")
}

/// Days-from-civil, inverted. Howard Hinnant's algorithm, which is the standard
/// one and is here because the alternative was a dependency.
fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (
        year,
        m,
        d,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    )
}

/// A language's column value.
pub fn lang_column(language: VoiceLanguage) -> String {
    language.tag().to_owned()
}

/// A source's column value.
pub fn source_column(source: SourceId) -> String {
    source.column().to_owned()
}

/// A candidate's number.
pub fn candidate_column(candidate: &Candidate) -> u8 {
    candidate.number
}

/// The metric name for a language.
pub fn metric_for(language: VoiceLanguage) -> &'static str {
    Unit::of(language).metric()
}

#[cfg(test)]
mod tests;
