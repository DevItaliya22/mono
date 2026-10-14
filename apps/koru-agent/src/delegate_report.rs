// SPDX-License-Identifier: MPL-2.0

//! `koru-agent delegate` — what the delegate worker is holding, read
//! **without asking it**.
//!
//! The whole point of this command is the case where the worker will not
//! answer. A run that has wedged, a publication queue that is not draining,
//! an attempt in §7.6's `unknown` state waiting for a person: every one of
//! those is a moment when asking the worker is either impossible or a lie.
//! So this reads the two files on the disk directly — `runs.db` through
//! `koru_delegate_sqlite::report`, and `artifacts.db` through
//! `koru_artifact_sqlite::run_workspaces` — read-only, creating nothing and
//! changing nothing, exactly as `koru-store-server projection` reads
//! `projection.db` rather than asking the Store.
//!
//! Whether the worker *is* answering is one line of the report, **probed**
//! rather than asked, the way `koru-agent status` dials the gateway's own
//! rendezvous instead of asking the Agent about its child. And when it is
//! serving, its live counters (P1.17) are printed beside the ledger's
//! numbers, because "queued 300, acknowledged 4" and "queued 300,
//! acknowledged 299" are the same open queue and two different problems.
//!
//! **Every line is an id, a token, a count or a duration.** No objective,
//! no title, no filename, no path, and — the one that takes deliberate
//! care — no *folder* in the disk report: a run's retained results are
//! named by the run that holds them and never by where they sit (§13.7).
//! The only path this command prints is the ledger's own file name, which
//! is where a person has to look next.
//!
//! [`render`] builds the whole report as one string and [`report_to_console`]
//! prints it. That is not a formality: it is what lets one test plant the
//! store's eight-language fixtures and a sentinel objective in a ledger and
//! assert that none of them is anywhere in what a person sees.

mod retained;

use std::fmt::Write as _;
use std::path::Path;

use koru_artifact_core::artifacts_folder;
use koru_delegate_api::ipc::WorkerCounters;
use koru_delegate_core::{PublicationKind, RunId};
use koru_delegate_sqlite::{
    AttemptPhase, AttemptSummary, InspectError, PUBLICATION_ROWS, PublicationSummary, Report,
    RunReport, SCHEDULE_SKIP_ROWS, ScheduleSkipSummary,
};

use crate::paths::Paths;

pub use retained::{ENTRY_BOUND, Retained};

/// Print the whole ledger, or one run of it.
///
/// `target`, when given, is a `RunId`; anything else is refused by name
/// rather than guessed at, the same rule `cli.rs` states.
pub fn report_to_console(paths: &Paths, target: Option<&str>) -> Result<(), String> {
    let rendered = render(&paths.store_dir(), &worker_line(paths), now_ms(), target)?;
    print!("{rendered}");
    Ok(())
}

/// The whole report, as one string.
///
/// `worker` is what the probe found, passed in rather than taken here so
/// this function reads two files and dials nothing.
pub(crate) fn render(
    store_root: &Path,
    worker: &str,
    now_ms: i64,
    target: Option<&str>,
) -> Result<String, String> {
    // The argument is checked before anything is read: a command line that
    // is not understood is not run, and an idle root must not make a
    // mistyped run id look accepted.
    let target: Option<RunId> = target
        .map(|target| {
            target
                .parse()
                .map_err(|_| format!("{target} is not a run id"))
        })
        .transpose()?;

    let ledger = koru_delegate_sqlite::runs_db_path(store_root);
    let mut out = String::new();
    let _ = writeln!(out, "{}", ledger.display());
    let _ = writeln!(out, "  worker     {worker}");
    let _ = writeln!(out);

    // A root nothing has ever been delegated on has no ledger, and saying
    // so is the answer rather than a failure — the same choice
    // `koru-store-server projection` makes when there is no
    // `projection.db`. A caller scripting this must not have to tell an
    // idle machine apart from a broken one by its exit code.
    if !ledger.is_file() {
        let _ = writeln!(
            out,
            "  ledger     none — nothing has been delegated on this root"
        );
        return Ok(out);
    }

    match target {
        None => {
            let report = koru_delegate_sqlite::report(&ledger, now_ms).map_err(explain)?;
            summary(&mut out, &report);
            attempts(&mut out, &report.attempts, "attempts");
            intents(&mut out, &report);
            publications(
                &mut out,
                &report.publications,
                report.summary.open_publications,
            );
            slots(&mut out, &report);
            recent_schedule_skips(&mut out, &report.recent_schedule_skips);
            retained_results(&mut out, store_root, None);
            if report.is_quiet() {
                let _ = writeln!(out);
                let _ = writeln!(
                    out,
                    "Nothing outstanding: no live attempt, no request, no open run or schedule"
                );
                let _ = writeln!(out, "publication and no unsettled slot on this root.");
            }
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "A run id prints that run: koru-agent delegate <RunId>."
            );
        }
        Some(run) => {
            let one = koru_delegate_sqlite::run_report(&ledger, now_ms, run).map_err(explain)?;
            one_run(&mut out, &one);
            if one.known() {
                retained_results(&mut out, store_root, Some(run));
            }
        }
    }
    Ok(out)
}

// ------------------------------------------------------------- the lines

/// Whether the worker answers, and what it says it has done.
///
/// Probed, not asked, and reported whether or not it answered — a worker
/// that is down is the answer somebody came for, not a failure of the
/// command.
fn worker_line(paths: &Paths) -> String {
    let Ok(endpoint) = crate::delegate_endpoint(paths) else {
        return "unreachable (this root has no delegate endpoint)".into();
    };
    if !koru_ipc::probe(&endpoint) {
        return "not serving — the numbers below are the ledger's own".into();
    }
    match koru_delegate_api::LocalDelegate::new(endpoint, "koru-agent").hello() {
        Ok(hello) => format!(
            "serving, pid {}, {} run(s) active\n{}",
            hello.pid,
            hello.active_runs,
            counter_lines(&hello.counters)
        ),
        Err(failure) => format!("serving but did not answer ({})", failure.code),
    }
}

/// The worker's totals. Always on and always printed: they are a handful
/// of integers, and a counter somebody has to enable and rebuild for is
/// one that was off during the incident.
fn counter_lines(counters: &WorkerCounters) -> String {
    let mut out = String::new();
    for (name, value) in [
        ("requests recorded", counters.intents_recorded),
        ("attempts opened", counters.attempts_opened),
        ("contexts captured", counters.contexts_captured),
        ("captures failed", counters.captures_failed),
        ("results registered", counters.artifacts_registered),
        ("restarts", counters.restarts),
    ] {
        let _ = writeln!(out, "  {name:<22} {value}");
    }
    let _ = writeln!(
        out,
        "  {:<22} {} queued, {} acknowledged, {} retried",
        "publications",
        counters.publications_queued,
        counters.publications_acknowledged,
        counters.publication_retries
    );
    let _ = write!(
        out,
        "  {:<22} {} forward, {} started, {} reattached, {} reconciled,\n  {:<22} {} interrupted, {} unresolved",
        "restart verdicts",
        counters.restart_completed_forward,
        counters.restart_started,
        counters.restart_reattached,
        counters.restart_reconciled,
        "",
        counters.restart_interrupted,
        counters.restart_unresolved
    );
    let _ = write!(
        out,
        "\n  {:<22} {} wakes, {} passes, {} documents, {} failures\n  {:<22} {} fired, {} missed, {} busy",
        "schedule evaluation",
        counters.schedule_wakes,
        counters.schedule_evaluations,
        counters.schedule_documents_evaluated,
        counters.schedule_evaluation_failures,
        "schedule outcomes",
        counters.schedule_fired,
        counters.schedule_skipped_missed,
        counters.schedule_skipped_busy,
    );
    out
}

fn summary(out: &mut String, report: &Report) {
    let summary = &report.summary;
    let _ = writeln!(out, "  schema     {}", summary.schema_version);
    let _ = writeln!(
        out,
        "  attempts   {} live, {} unknown",
        summary.live_attempts, summary.unknown_attempts
    );
    let _ = writeln!(out, "  requests   {} pending", summary.pending_intents);
    match summary.oldest_open_publication_age_ms {
        Some(age) => {
            let _ = writeln!(
                out,
                "  queue      {} open, oldest waiting {}",
                summary.open_publications,
                age_of(age)
            );
        }
        None => {
            let _ = writeln!(out, "  queue      empty");
        }
    }
    match (
        summary.oldest_prepared_schedule_publication_age_ms,
        summary.oldest_ready_schedule_publication_age_ms,
    ) {
        (None, None) => {
            let _ = writeln!(out, "  schedule queue empty");
        }
        (prepared_age, ready_age) => {
            let prepared = prepared_age
                .map(age_of)
                .unwrap_or_else(|| "none".to_owned());
            let ready = ready_age.map(age_of).unwrap_or_else(|| "none".to_owned());
            let _ = writeln!(
                out,
                "  schedule queue {} prepared (oldest {}), {} ready (oldest {})",
                summary.prepared_schedule_publications,
                prepared,
                summary.ready_schedule_publications,
                ready
            );
        }
    }
    let schedule = report.schedule_totals;
    let _ = writeln!(
        out,
        "  schedules  {} fired, {} missed, {} busy, {} retired, {} unsettled",
        schedule.fired, schedule.missed, schedule.busy, schedule.retired, schedule.unsettled
    );
}

fn attempts(out: &mut String, rows: &[AttemptSummary], heading: &str) {
    if rows.is_empty() {
        return;
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "{heading}");
    for row in rows {
        // `unknown` is the row a person acts on: §7.6's last line, which
        // never auto-resumes and is waiting for a decision. It is marked
        // so it cannot be read past.
        let flag = match row.phase {
            AttemptPhase::Unknown => "  <- needs a person",
            _ => "",
        };
        let _ = writeln!(
            out,
            "  {} gen {:<3} {:<12} idle {:<10} {}{}{}",
            row.run,
            row.generation.get(),
            row.phase.token(),
            age_of(row.idle_ms),
            if row.session_bound {
                "session bound"
            } else {
                "no session"
            },
            if row.retired { ", retired" } else { "" },
            flag
        );
    }
}

fn intents(out: &mut String, report: &Report) {
    if report.intents.is_empty() {
        return;
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "requests that are not yet runs");
    for intent in &report.intents {
        let _ = writeln!(
            out,
            "  {} in workspace {}, recorded {} ago",
            intent.run,
            intent.workspace,
            age_of(intent.age_ms)
        );
    }
}

fn publications(out: &mut String, rows: &[PublicationSummary], total: u64) {
    if rows.is_empty() {
        return;
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "publications not acknowledged");
    for row in rows {
        let _ = writeln!(
            out,
            "  {} {:<22} {} attempt(s), waiting {}",
            row.run,
            kind_of(row.kind),
            row.attempts,
            age_of(row.waiting_ms)
        );
    }
    if total > rows.len() as u64 {
        let _ = writeln!(
            out,
            "  and {} more (the oldest {PUBLICATION_ROWS} are listed)",
            total - rows.len() as u64
        );
    }
}

fn slots(out: &mut String, report: &Report) {
    if report.slots.is_empty() {
        return;
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "schedule slots claimed and not settled");
    for slot in &report.slots {
        let _ = writeln!(
            out,
            "  schedule {} revision {} minted run {}, claimed {} ago",
            slot.schedule,
            slot.revision,
            slot.run,
            age_of(slot.held_ms)
        );
    }
}

fn recent_schedule_skips(out: &mut String, rows: &[ScheduleSkipSummary]) {
    if rows.is_empty() {
        return;
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "recent schedule skips — newest claim first, at most {SCHEDULE_SKIP_ROWS}"
    );
    for row in rows {
        let _ = write!(
            out,
            "  schedule {} occurrence {} revision {} reason {}",
            row.schedule,
            row.occurrence_ms,
            row.revision,
            row.reason.token()
        );
        if let Some(holder) = row.busy_holder {
            let _ = write!(out, ", busy holder {holder}");
        }
        let _ = writeln!(out);
    }
}

fn one_run(out: &mut String, one: &RunReport) {
    let _ = writeln!(out);
    if !one.known() {
        let _ = writeln!(out, "{} is not a run this device created.", one.run);
        return;
    }
    let intent = one.intent.expect("known");
    let _ = writeln!(out, "run        {}", one.run);
    let _ = writeln!(out, "workspace  {}", intent.workspace);
    let _ = writeln!(
        out,
        "request    {}, recorded {} ago",
        intent.state.token(),
        age_of(intent.age_ms)
    );
    match intent.run_document {
        Some(document) => {
            let _ = writeln!(out, "document   {document}");
        }
        None => {
            let _ = writeln!(out, "document   none — this is still a request, not a run");
        }
    }
    attempts(out, &one.attempts, "attempts, oldest generation first");
    publications(out, &one.publications, one.publications.len() as u64);
    if !one.cursors.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "projection cursors");
        for cursor in &one.cursors {
            let _ = writeln!(
                out,
                "  {:<16} timeline index {}, moved {} ago",
                cursor.projection.token(),
                cursor.timeline_index,
                age_of(cursor.age_ms)
            );
        }
    }
    if let Some(slot) = one.slot {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "minted by schedule {} revision {}, {}",
            slot.schedule,
            slot.revision,
            slot.state.token()
        );
    }
}

/// D37's disk, by run id and never by folder.
///
/// The run workspaces come from `artifacts.db`, which is the only record
/// that names them; a gateway that has never issued an authority names
/// none, and that is an answer rather than an error. The folder each row
/// measures is built here and never written down.
fn retained_results(out: &mut String, store_root: &Path, only: Option<RunId>) {
    let artifacts =
        koru_channel::artifacts_root_beside(store_root).join(koru_artifact_sqlite::DATABASE_FILE);
    let rows = match koru_artifact_sqlite::run_workspaces(&artifacts) {
        Ok(rows) => rows,
        Err(koru_artifact_sqlite::InspectError::Absent) => {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "retained results   none — no run has registered on this root"
            );
            return;
        }
        Err(error) => {
            let _ = writeln!(out);
            let _ = writeln!(out, "retained results   unavailable ({error})");
            return;
        }
    };
    let mut lines: Vec<(RunId, Retained)> = Vec::new();
    let mut total = Retained::default();
    for row in rows {
        if only.is_some_and(|run| run != row.run) {
            continue;
        }
        let held = retained::measure(&artifacts_folder(row.workspace.as_path(), row.run));
        if !held.present {
            continue;
        }
        total.add(held);
        lines.push((row.run, held));
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "retained results — unregistered files kept with their run (D37)"
    );
    if lines.is_empty() {
        let _ = writeln!(out, "  none");
        return;
    }
    for (run, held) in &lines {
        let _ = writeln!(
            out,
            "  {} {:>7} file(s) {:>12}{}",
            run,
            held.files,
            bytes_of(held.bytes),
            note(*held)
        );
    }
    if lines.len() > 1 {
        let _ = writeln!(
            out,
            "  {:<36} {:>7} file(s) {:>12}",
            "total",
            total.files,
            bytes_of(total.bytes)
        );
    }
}

fn note(held: Retained) -> String {
    let mut note = String::new();
    if held.truncated {
        let _ = write!(note, "  (at least; the walk stopped at {ENTRY_BOUND})");
    }
    if held.unreadable > 0 {
        let _ = write!(note, "  ({} entry/entries unreadable)", held.unreadable);
    }
    note
}

// ------------------------------------------------------------ formatting

/// Why the ledger could not be read, in a sentence a person can act on.
///
/// `Absent` reaches here only in a race — the file was there when
/// [`render`] looked and gone by the time it was opened — because the
/// ordinary empty root is answered above rather than failed.
fn explain(error: InspectError) -> String {
    match error {
        InspectError::Absent => "the runs.db that was there a moment ago is gone".into(),
        other => other.to_string(),
    }
}

fn kind_of(kind: PublicationKind) -> &'static str {
    match kind {
        PublicationKind::Lifecycle => "lifecycle",
        PublicationKind::ArtifactRegistration => "artifact_registration",
        PublicationKind::Usage => "usage",
    }
}

/// A duration, compactly and without a locale.
///
/// This command follows `koru-agent status` and `koru-store-server
/// projection`: an operator's transcript in one spelling, which goes into
/// a bug report exactly as it was printed. A negative age — a ledger
/// written by a machine whose clock has since gone backwards — reads as
/// `0s` rather than as a surprise.
fn age_of(ms: i64) -> String {
    let seconds = ms.max(0) / 1_000;
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3_599 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        3_600..=86_399 => format!("{}h {:02}m", seconds / 3_600, (seconds % 3_600) / 60),
        _ => format!("{}d {:02}h", seconds / 86_400, (seconds % 86_400) / 3_600),
    }
}

/// Bytes in the unit a person compares at a glance.
fn bytes_of(bytes: u64) -> String {
    const KB: f64 = 1_024.0;
    const MB: f64 = KB * 1_024.0;
    const GB: f64 = MB * 1_024.0;
    match bytes {
        0..=1_023 => format!("{bytes} B"),
        1_024..=1_048_575 => format!("{:.1} KB", bytes as f64 / KB),
        1_048_576..=1_073_741_823 => format!("{:.1} MB", bytes as f64 / MB),
        _ => format!("{:.2} GB", bytes as f64 / GB),
    }
}

/// Epoch milliseconds now. The one clock reading this command takes, so
/// every age in one report is measured against one instant.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
