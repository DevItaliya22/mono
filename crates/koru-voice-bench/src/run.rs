// SPDX-License-Identifier: MPL-2.0

//! **Orchestration: which passes are run over which utterances, and how a
//! measurement becomes a row.**
//!
//! Everything decided here is decided in the protocol; this module is the order
//! of operations, and the order is itself normative in two places.
//!
//! **A warm-up utterance is decoded and discarded after every load, before any
//! measured utterance**, so first-utterance lazy initialisation inside a runtime
//! does not land in the first `TTFUP` sample.
//!
//! **Accuracy comes from the paced pass**, because that is the condition the
//! product runs in; the unpaced pass exists for RTF and peak memory and to
//! produce a final transcript the paced one can be compared against. That
//! comparison is `drift`.
//!
//! ## The smoke path, which is the one that runs today
//!
//! Eight languages, the checked-in fixture, no corpus, no model. Every candidate
//! reports *model missing* (or *runtime not built*), the tables come out with
//! their reasons in them, and **it exits zero** — a harness that failed when
//! nothing was provisioned would be a harness nobody could run to find out
//! whether it worked.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use koru_voice::{Requested, SessionId, SessionRequest, SpeechEngine, VoiceLanguage};

use crate::candidate::{self, CANDIDATES, Candidate, Unavailable};
use crate::cli::{Options, Stage};
use crate::corpus::{Corpus, PROPER_NOUNS, SmokeFixture, SourceId, Utterance};
use crate::host::{self, HostFacts};
use crate::metrics::{self, ErrorCount, Unit};
use crate::normalise::Normaliser;
use crate::pace;
use crate::report::{
    AccuracyRow, CensoredFigure, ChineseRow, CostRow, Header, LatencyRow, PrefixCheckRow, ProbeRow,
    Run, run_id,
};
use crate::stats::{self, Z_90};

/// One utterance measured under one candidate, before it becomes a row.
#[derive(Clone, PartialEq, Debug)]
pub struct Measurement {
    pub language: VoiceLanguage,
    pub source: SourceId,
    /// Who said it, where the source knows. FLEURS carries `gender` but no
    /// speaker id, so this is `None` there — which is why the per-speaker
    /// spread in Table 4 is reportable only for Common Voice and AISHELL-1.
    pub speaker: Option<String>,
    /// `None` when no partial qualified — a **censored** observation, counted in
    /// the p95 and never dropped.
    pub ttfup: Option<Duration>,
    /// The lower bound for a censored observation: `t_final − t0`.
    pub censored_at: Duration,
    pub final_latency: Option<Duration>,
    pub revisions: usize,
    pub partials: usize,
    /// Words in the first qualifying prefix, for the screen's `L` check.
    pub prefix_words: Option<usize>,
    pub error: ErrorCount,
    /// Whether the paced and unpaced finals disagreed.
    pub drift: bool,
    pub overruns: usize,
    pub rtf: Option<f64>,
    pub cpu_percent: Option<f64>,
    /// The final transcript, kept only for the smoke fixture's probe. **Never
    /// logged**: `Transcript`'s `Debug` is redacted for a reason and this is a
    /// `String` only because the probe has to look inside it.
    probe_source: Option<String>,
}

/// Run everything the options ask for.
pub fn execute(options: &Options) -> Result<Run, String> {
    let mut notes: Vec<String> = Vec::new();
    let facts = HostFacts::detect();

    let (utterances, normaliser, corpus_lines, converter) = match options.stage {
        Stage::Smoke => {
            let fixture = SmokeFixture::load(&SmokeFixture::default_root())
                .map_err(|e| format!("the smoke fixture would not load: {e}"))?;
            if fixture.is_placeholder() {
                notes.push(
                    "smoke fixture: PLACEHOLDER — the audio is synthetic and nobody said the \
                     references; see fixtures/smoke/PLACEHOLDER and QUESTIONS.md Q-V2"
                        .to_owned(),
                );
            }
            notes.push("smoke fixture: no number here appears in a decision table".to_owned());
            (
                fixture.utterances.clone(),
                Normaliser::without_t2s(),
                Vec::new(),
                "n/a".to_owned(),
            )
        }
        _ => {
            let root = options
                .corpus
                .as_deref()
                .ok_or_else(|| "no corpus".to_owned())?;
            let corpus =
                Corpus::open(root).map_err(|e| format!("the corpus would not open: {e}"))?;
            let lines: Vec<String> = corpus
                .manifest
                .sources
                .iter()
                .map(|pin| {
                    format!(
                        "{:<12} rev {}  src sha256:{}  conv sha256:{}",
                        pin.source_id.as_str(),
                        pin.source_revision,
                        short(&pin.source_sha256),
                        short(&pin.converted_sha256)
                    )
                })
                .collect();
            let converter = corpus.manifest.converter.clone();
            let normaliser = Normaliser::new(corpus.t2s.clone());
            if !normaliser.has_t2s() {
                let message = format!(
                    "zh-Hans normalisation: no {} in the corpus, so Traditional output is \
                     scored as-is and the zh-Hans figures are NOT the protocol's",
                    crate::normalise::T2S_FILE
                );
                if options.stage == Stage::Decision {
                    return Err(message);
                }
                notes.push(message);
            } else {
                notes.push(format!(
                    "zh-Hans t2s: {} entries, {} of them phrases",
                    normaliser.t2s_table().len(),
                    normaliser.t2s_table().phrases()
                ));
            }
            let (utterances, absent) = collect(&corpus, options)?;
            // **Named, never silent.** A corpus with only some of its sources
            // provisioned is a corpus somebody is part-way through building,
            // and a run over it produces fewer rows than the protocol asks for.
            // Skipping quietly is how a table ends up describing a subset
            // nobody chose.
            for line in absent {
                notes.push(line);
            }
            (utterances, normaliser, lines, converter)
        }
    };

    let header = Header {
        harness: crate::VERSION.to_owned(),
        protocol_rev: crate::PROTOCOL_REV,
        run_id: run_id(),
        stage: options.stage,
        host_class: options.class.clone(),
        host: facts.line(),
        num_threads: options.num_threads,
        corpus: corpus_lines,
        converter,
        notes,
    };

    let wanted: Vec<&Candidate> = CANDIDATES
        .iter()
        .filter(|c| options.candidates.is_empty() || options.candidates.contains(&c.number))
        .collect();

    let mut run = Run {
        header,
        latency: Vec::new(),
        accuracy: Vec::new(),
        cost: Vec::new(),
        chinese: Vec::new(),
        probe: Vec::new(),
        prefix_check: Vec::new(),
        unavailable: Vec::new(),
    };

    // Words in every qualifying prefix, per language, across every candidate
    // the screen ran. The check is about the *sample*, so it pools candidates
    // deliberately — `L` is a property of the language, not of a recogniser.
    let mut prefix_words: BTreeMap<&'static str, Vec<usize>> = BTreeMap::new();

    for candidate in wanted {
        let models = options.models.as_deref().unwrap_or(Path::new("."));
        // **Timed here and nowhere else**: the protocol measures a cold load
        // *from the call that constructs the recogniser to its return*, and
        // wrapping anything more would put a directory walk in a column that
        // says `load_first`.
        let opened_at = Instant::now();
        match candidate::open(candidate, models, options.num_threads) {
            Ok(engine) => {
                let load = opened_at.elapsed();
                measure_candidate(
                    &mut run,
                    candidate,
                    engine,
                    &utterances,
                    &normaliser,
                    options,
                    &mut prefix_words,
                    load,
                );
            }
            Err(reason) => {
                // **Reported, and the run still exits zero.** `model missing`
                // is the ordinary state of a machine that has not downloaded
                // three gigabytes, and it is exactly what the smoke test
                // asserts.
                run.unavailable.push(format!(
                    "{} — {}: {reason}",
                    candidate.number, candidate.name
                ));
                if let Unavailable::Failed(error) = &reason {
                    run.unavailable
                        .push(format!("    code {}", error.code.as_str()));
                }
            }
        }
    }

    if options.stage == Stage::Screen {
        run.prefix_check = prefix_check(&run, &prefix_words);
    }

    Ok(run)
}

/// Everything the decision corpus holds for the languages and sources this
/// stage measures, sampled.
///
/// Answers the sample and **a note per source directory that was not there**.
/// A missing directory is not an error — a tree with only FLEURS in it is a
/// legitimate half-built corpus — but it is never silent, because the rows it
/// costs are rows the protocol asks for.
fn collect(corpus: &Corpus, options: &Options) -> Result<(Vec<Utterance>, Vec<String>), String> {
    use crate::sample::{self, Eligible, Purpose};

    let mut chosen = Vec::new();
    let mut absent: Vec<String> = Vec::new();
    for language in VoiceLanguage::ALL {
        for (source, purpose) in [
            (SourceId::Fleurs, Purpose::Accuracy),
            (SourceId::CommonVoice, Purpose::Latency),
            (SourceId::Aishell1, Purpose::Accuracy),
        ] {
            let available = match corpus.utterances(source, language) {
                Ok(list) => list,
                Err(e) => {
                    // AISHELL-1 exists for Simplified Chinese only, so its
                    // absence elsewhere is the shape of the corpus rather than
                    // a gap in it and is not worth a note.
                    if source != SourceId::Aishell1 || language == VoiceLanguage::ZhHans {
                        absent.push(format!("corpus: {e}"));
                    }
                    continue;
                }
            };
            if available.is_empty() {
                continue;
            }
            let eligible: Vec<Eligible> = available
                .iter()
                .map(|u| Eligible {
                    id: u.id.clone(),
                    reference: u.reference.clone(),
                    // The **trimmed** duration, which means reading the file.
                    // The alternative — the file's own length — would make two
                    // utterances with the same speech and different leading
                    // silence score differently.
                    duration: u.audio().map(|t| t.duration()).unwrap_or_default(),
                    speaker: u.speaker.clone(),
                })
                .collect();
            let seed = sample::seed(crate::PROTOCOL_REV, source.as_str(), language);
            let cap = (source == SourceId::CommonVoice).then_some(3);
            let target = options.stage.sample_size(language);
            let sampled = sample::draw(eligible, purpose, target, cap, seed);
            for id in &sampled.ids {
                if let Some(utterance) = available.iter().find(|u| &u.id == id) {
                    chosen.push(utterance.clone());
                }
            }
        }
    }
    Ok((chosen, absent))
}

/// Measure one candidate over every utterance, and fold the results into rows.
#[allow(clippy::too_many_arguments)]
fn measure_candidate(
    run: &mut Run,
    candidate: &Candidate,
    mut engine: Box<dyn SpeechEngine>,
    utterances: &[Utterance],
    normaliser: &Normaliser,
    options: &Options,
    prefix_words: &mut BTreeMap<&'static str, Vec<usize>>,
    load: Duration,
) {
    // **The warm-up.** Decoded and discarded, so lazy initialisation inside a
    // runtime does not land in the first measured `TTFUP`.
    if let Some(first) = utterances.first()
        && let Ok(audio) = first.audio()
    {
        let request = request_for(first.language, 0);
        let _ = pace::run(
            &mut *engine,
            &request,
            &audio.samples,
            audio.duration(),
            false,
        );
    }

    let mut measurements: Vec<Measurement> = Vec::new();
    let mut probe: Vec<ProbeRow> = Vec::new();

    for (index, utterance) in utterances.iter().enumerate() {
        let Ok(audio) = utterance.audio() else {
            continue;
        };
        if audio.is_empty() {
            continue;
        }
        let generation = index as u64 + 1;

        let cpu_before = host::cpu_time();
        let paced_start = Instant::now();
        let paced = pace::run(
            &mut *engine,
            &request_for(utterance.language, generation * 2),
            &audio.samples,
            audio.duration(),
            true,
        );
        let wall = paced_start.elapsed();
        let cpu_after = host::cpu_time();

        let unpaced = pace::run(
            &mut *engine,
            &request_for(utterance.language, generation * 2 + 1),
            &audio.samples,
            audio.duration(),
            false,
        );

        let (Ok(paced), Ok(unpaced)) = (paced, unpaced) else {
            continue;
        };

        let measurement = fold(
            utterance,
            &paced,
            &unpaced,
            normaliser,
            host::cpu_percent(cpu_before, cpu_after, wall),
        );
        if options.stage == Stage::Smoke {
            probe.push(probe_row(candidate, utterance.language, &measurement));
        }
        if let Some(words) = measurement.prefix_words {
            prefix_words
                .entry(utterance.language.tag())
                .or_default()
                .push(words);
        }
        measurements.push(measurement);
    }

    run.probe.extend(probe);
    fold_rows(run, candidate, &measurements, options);
    run.cost
        .push(cost_row(candidate, &measurements, options, load));
    run.chinese.extend(chinese_rows(candidate, &measurements));
}

/// Table 3 — cost, from the unpaced pass.
///
/// **`load_warm_p50` and `load_warm_p95` are absent, and that is not a gap in
/// the implementation.** The protocol's two load figures are *a fresh process,
/// model bytes not read since boot* and *a fresh process, file cache warm* —
/// n = 3 and n = 10. Both are properties of the operating system's page cache
/// across **process boundaries**, and no amount of looping inside one process
/// can produce either: the second load of a model in one process is not a warm
/// load, it is the same mapped file. So this harness measures the one load this
/// invocation performed, puts it in `load_first_ms`, and an operator gets the
/// protocol's distribution by running the harness the stated number of times
/// and aggregating the JSON. A number here produced by a loop would be a
/// plausible wrong answer, which is worse than an absent one.
fn cost_row(
    candidate: &Candidate,
    measurements: &[Measurement],
    options: &Options,
    load: Duration,
) -> CostRow {
    let rtfs: Vec<f64> = measurements.iter().filter_map(|m| m.rtf).collect();
    CostRow {
        cand: candidate.number,
        class: options.class.clone(),
        thr: options.num_threads,
        load_first_ms: Some(load.as_secs_f64() * 1000.0),
        load_warm_p50_ms: None,
        load_warm_p95_ms: None,
        rtf_p50: stats::quantile(&rtfs, 0.50, Z_90).map(|q| q.value),
        rtf_p95: stats::quantile(&rtfs, 0.95, Z_90).map(|q| q.value),
        // The OS's own peak counter, read once after this candidate's work.
        // **Per process, not per candidate**: a run measuring several
        // candidates reports each row with the high-water mark reached by then,
        // which is why the decision stage runs one candidate per invocation and
        // why this is the number to read against `--candidate`.
        peak_rss_bytes: host::peak_rss_bytes(),
        model_on_disk_bytes: options
            .models
            .as_deref()
            .and_then(|models| directory_size(&candidate.model_dir(models))),
    }
}

/// Table 4 — Simplified Chinese, by source and by speaker. **Never a pooled
/// row.**
///
/// AISHELL-1 is clean read speech on controlled hardware and Common Voice is
/// crowd-sourced on unknown microphones; a pooled CER describes neither, and
/// which of the two a candidate is bad at is exactly the decision. The
/// per-speaker spread is the point of the table: a max far above the median is
/// the failure mode a mean would hide.
fn chinese_rows(candidate: &Candidate, measurements: &[Measurement]) -> Vec<ChineseRow> {
    let mut rows = Vec::new();
    for source in [SourceId::Fleurs, SourceId::CommonVoice, SourceId::Aishell1] {
        let group: Vec<&Measurement> = measurements
            .iter()
            .filter(|m| m.language == VoiceLanguage::ZhHans && m.source == source)
            .collect();
        if group.is_empty() {
            continue;
        }
        let counts: Vec<ErrorCount> = group.iter().map(|m| m.error).collect();
        let seed = crate::sample::seed(crate::PROTOCOL_REV, source.as_str(), VoiceLanguage::ZhHans);
        let rate = stats::bootstrap_rate(&counts, seed);

        // Per speaker, then min / median / max of those. Each speaker's figure
        // is a corpus-level rate over *their* utterances, for the same reason
        // the corpus figure is: a mean of per-utterance rates would let one
        // short utterance weigh as much as a long one inside a speaker too.
        let mut per_speaker: BTreeMap<&str, Vec<ErrorCount>> = BTreeMap::new();
        for m in &group {
            if let Some(speaker) = m.speaker.as_deref() {
                per_speaker.entry(speaker).or_default().push(m.error);
            }
        }
        let mut speaker_rates: Vec<f64> = per_speaker
            .values()
            .filter_map(|counts| metrics::corpus_rate(counts))
            .collect();
        speaker_rates.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));

        rows.push(ChineseRow {
            cand: candidate.number,
            source: source.column().to_owned(),
            n: group.len(),
            cer: rate.map(|(v, _)| v),
            ci: rate.map(|(_, i)| (i.low, i.high)),
            speakers: per_speaker.len(),
            speaker_cer_min: speaker_rates.first().copied(),
            speaker_cer_median: median(&speaker_rates),
            speaker_cer_max: speaker_rates.last().copied(),
            cens: group.iter().filter(|m| m.ttfup.is_none()).count(),
        });
    }
    rows
}

/// The median of an already-sorted slice. `None` for an empty one, because a
/// median of nothing is not zero.
fn median(sorted: &[f64]) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        Some(sorted[mid])
    } else {
        Some((sorted[mid - 1] + sorted[mid]) / 2.0)
    }
}

/// Every byte under a directory. `None` when it cannot be walked, which prints
/// as an absence rather than as zero — `model_on_disk` is what M4's device gate
/// compares against, and a zero there would pass it.
fn directory_size(dir: &Path) -> Option<u64> {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        for entry in std::fs::read_dir(&path).ok()? {
            let entry = entry.ok()?;
            let meta = entry.metadata().ok()?;
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    Some(total)
}

/// One session's numbers, from the two passes.
fn fold(
    utterance: &Utterance,
    paced: &pace::Session,
    unpaced: &pace::Session,
    normaliser: &Normaliser,
    cpu_percent: Option<f64>,
) -> Measurement {
    let language = utterance.language;
    let final_text = normaliser.normalise(paced.final_str(), language);
    let reference = normaliser.normalise(&utterance.reference, language);

    // The prefix test, against the engine's **own** final rather than the
    // reference. This metric measures whether text appears and stops moving;
    // accuracy is measured separately, and keeping them independent is what
    // stops a fast-and-wrong engine being penalised twice.
    let mut ttfup = None;
    let mut prefix_words = None;
    for (at, partial) in &paced.partials {
        let normalised = normaliser.normalise(partial.as_str(), language);
        if metrics::qualifies(&normalised, &final_text, language) {
            ttfup = Some(at.saturating_duration_since(paced.t0));
            prefix_words = Some(metrics::words_in(&normalised));
            break;
        }
    }

    let normalised_partials: Vec<String> = paced
        .partials
        .iter()
        .map(|(_, p)| normaliser.normalise(p.as_str(), language))
        .collect();

    Measurement {
        language,
        source: utterance.source,
        speaker: utterance.speaker.clone(),
        ttfup,
        censored_at: paced.wall().unwrap_or_default(),
        final_latency: paced.final_latency(),
        revisions: metrics::revisions(&normalised_partials),
        partials: paced.partials.len(),
        prefix_words,
        error: metrics::score(&reference, &final_text, Unit::of(language)),
        drift: normaliser.normalise(unpaced.final_str(), language) != final_text,
        overruns: paced.overruns,
        rtf: unpaced.real_time_factor(),
        cpu_percent,
        probe_source: Some(paced.final_str().to_owned()),
    }
}

/// The proper-noun probe. A case-insensitive substring test, because the probe
/// asks *did the name survive at all* rather than *was it punctuated correctly*.
fn probe_row(candidate: &Candidate, language: VoiceLanguage, m: &Measurement) -> ProbeRow {
    let text = m.probe_source.clone().unwrap_or_default().to_lowercase();
    let mut hits = BTreeMap::new();
    for name in PROPER_NOUNS {
        hits.insert(name.to_owned(), text.contains(&name.to_lowercase()));
    }
    ProbeRow {
        cand: candidate.number,
        lang: language.tag().to_owned(),
        hits,
    }
}

/// Fold measurements into the tables, grouped the way the protocol groups them.
fn fold_rows(
    run: &mut Run,
    candidate: &Candidate,
    measurements: &[Measurement],
    options: &Options,
) {
    for language in VoiceLanguage::ALL {
        let latency: Vec<&Measurement> = measurements
            .iter()
            .filter(|m| m.language == language && m.source == SourceId::CommonVoice)
            .collect();
        if !latency.is_empty() {
            run.latency
                .push(latency_row(candidate, language, &latency, options));
        }

        for source in [SourceId::Fleurs, SourceId::CommonVoice, SourceId::Aishell1] {
            let group: Vec<&Measurement> = measurements
                .iter()
                .filter(|m| m.language == language && m.source == source)
                .collect();
            if group.is_empty() {
                continue;
            }
            let counts: Vec<ErrorCount> = group.iter().map(|m| m.error).collect();
            let seed = crate::sample::seed(crate::PROTOCOL_REV, source.as_str(), language);
            let rate = stats::bootstrap_rate(&counts, seed);
            run.accuracy.push(AccuracyRow {
                cand: candidate.number,
                lang: language.tag().to_owned(),
                corpus: source.column().to_owned(),
                n: group.len(),
                // The exclusions belong to the sampling step, which happens
                // before a measurement exists. Zero here means "this run did
                // not sample"; the smoke path is the case.
                excl: 0,
                metric: Unit::of(language).metric(),
                value: rate.map(|(v, _)| v),
                ci: rate.map(|(_, i)| (i.low, i.high)),
                drift: group.iter().filter(|m| m.drift).count(),
            });
        }
    }
}

fn latency_row(
    candidate: &Candidate,
    language: VoiceLanguage,
    group: &[&Measurement],
    _options: &Options,
) -> LatencyRow {
    let uncensored: Vec<f64> = group
        .iter()
        .filter_map(|m| m.ttfup)
        .map(|d| d.as_secs_f64() * 1000.0)
        .collect();
    let censored: Vec<f64> = group
        .iter()
        .filter(|m| m.ttfup.is_none())
        .map(|m| m.censored_at.as_secs_f64() * 1000.0)
        .collect();
    let finals: Vec<f64> = group
        .iter()
        .filter_map(|m| m.final_latency)
        .map(|d| d.as_secs_f64() * 1000.0)
        .collect();
    let revisions: Vec<f64> = group.iter().map(|m| m.revisions as f64).collect();
    let cpu: Vec<f64> = group.iter().filter_map(|m| m.cpu_percent).collect();
    let seed = crate::sample::seed(crate::PROTOCOL_REV, "revisions", language);

    LatencyRow {
        cand: candidate.number,
        lang: language.tag().to_owned(),
        n: group.len(),
        cens: censored.len(),
        ttfup_p50_ms: stats::quantile(&uncensored, 0.50, Z_90).map(|q| q.value),
        ttfup_p95: stats::censored_quantile(&uncensored, &censored, 0.95, Z_90)
            .map(CensoredFigure::from),
        fin_p50_ms: stats::quantile(&finals, 0.50, Z_90).map(|q| q.value),
        fin_p95_ms: stats::quantile(&finals, 0.95, Z_90).map(|q| q.value),
        revisions_per_utterance: stats::bootstrap_mean(&revisions, seed),
        cpu_percent: (!cpu.is_empty()).then(|| cpu.iter().sum::<f64>() / cpu.len() as f64),
        overruns: group.iter().map(|m| m.overruns).sum(),
    }
}

/// The screen's `L(language)` check.
///
/// It reads the latency rows rather than the raw measurements because the check
/// is about the sample the screen actually drew — and it is deliberately empty
/// when no candidate ran, rather than reporting a band nothing was measured in.
fn prefix_check(
    run: &Run,
    prefix_words: &BTreeMap<&'static str, Vec<usize>>,
) -> Vec<PrefixCheckRow> {
    let mut rows = Vec::new();
    for language in VoiceLanguage::ALL {
        if !run.latency.iter().any(|r| r.lang == language.tag()) {
            continue;
        }
        let words = prefix_words.get(language.tag());
        // A language with no qualifying prefix at all reports `None`, which
        // prints as an em dash — a question rather than a zero.
        let mean = words
            .filter(|w| !w.is_empty())
            .map(|w| w.iter().sum::<usize>() as f64 / w.len() as f64);
        rows.push(PrefixCheckRow {
            lang: language.tag().to_owned(),
            l_graphemes: metrics::minimum_graphemes(language),
            mean_words: mean,
            // The band is [1.5, 3.0] words, and it does not apply to the two
            // languages that have no whitespace — a Chinese prefix is zero or
            // one "word" by construction, so it is reported and marked as
            // in-band rather than made to fail a check that means nothing there.
            in_band: match mean {
                Some(mean) if crate::normalise::is_whitespace_separated(language) => {
                    (1.5..=3.0).contains(&mean)
                }
                Some(_) => true,
                None => false,
            },
        });
    }
    rows
}

fn request_for(language: VoiceLanguage, generation: u64) -> SessionRequest {
    SessionRequest {
        id: SessionId::new(),
        generation: generation.max(1),
        model: koru_voice::ModelId::parse("bench").expect("`bench` is a legal model id"),
        // **Never `Auto`.** The corpus knows the language and the gate measures
        // transcription rather than detection; asking for `Auto` would make
        // every row depend on a second capability that only one candidate has.
        language: Requested::Fixed(language),
    }
}

fn short(hash: &str) -> String {
    hash.chars().take(4).collect()
}

#[cfg(test)]
mod tests;
