// SPDX-License-Identifier: MPL-2.0

//! JSON evidence from the same executor used by live cases; no UI or file ownership.
use crate::{
    cases::{Case, Target},
    execute::StepResult,
    inspect::{ContentSnapshot, ElementFacts, Rect},
    locator::LocatorPattern,
    select::{Candidate, Selection},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fmt,
    io::{self, Write},
    path::Path,
};

pub struct RunReport {
    document: Value,
    requested: usize,
    completed: BTreeSet<usize>,
    failed: bool,
}

impl fmt::Debug for RunReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunReport")
            .field("requested", &self.requested)
            .field("completed", &self.completed)
            .field("failed", &self.failed)
            .finish()
    }
}

impl RunReport {
    pub fn new(case: &str, root: &Path, pid: u32, hwnd: i64, requested_runs: usize) -> Self {
        Self {
            document: json!({"version":1,"case":code(case),"root":root,
            "process_id":pid,"window_handle":hwnd,"requested_runs":requested_runs,
            "runs":[],"failures":[],"snapshots":[]}),
            requested: requested_runs,
            completed: BTreeSet::new(),
            failed: !(1..=10).contains(&requested_runs),
        }
    }

    pub fn set_environment(
        &mut self,
        ui_language: &str,
        locale: &str,
        hour_cycle: &str,
        time_zone: &str,
    ) {
        self.document["environment"] = json!({"ui_language":ui_language,"locale":locale,
            "hour_cycle":hour_cycle,"time_zone":time_zone});
    }

    pub fn add_case(&mut self, iteration: usize, phase: &str, case: &Case, results: &[StepResult]) {
        self.record_case(iteration, phase, case, results, true);
    }

    pub fn checkpoint_case(
        &mut self,
        iteration: usize,
        phase: &str,
        case: &Case,
        results: &[StepResult],
    ) {
        self.record_case(iteration, phase, case, results, false);
    }

    fn record_case(
        &mut self,
        iteration: usize,
        phase: &str,
        case: &Case,
        results: &[StepResult],
        finalized: bool,
    ) {
        if self.document["runs"].as_array().unwrap().len() >= 40 || results.len() > 256 {
            self.fail("report", "report-limit-exceeded");
            return;
        }
        let complete = !case.steps.is_empty()
            && results.len() == case.steps.len()
            && results
                .iter()
                .zip(&case.steps)
                .all(|(result, step)| result.step == step.id && result.failure.is_none());
        if (finalized && !complete)
            || !matches!(phase, "case" | "reset")
            || !(1..=self.requested).contains(&iteration)
        {
            self.failed = true;
        }
        if finalized && phase == "case" && complete && !self.completed.insert(iteration) {
            self.failed = true;
        }
        let steps: Vec<_> = results
            .iter()
            .enumerate()
            .map(|(index, result)| {
                json!({"step":code(result.step),"operation":code(result.operation),
                "requested_target":case.steps.get(index).map(|step| target(&step.target)),
                "target":result.target.as_ref().map(facts),
                "selection":result.selection.as_ref().map(selection),
                "expected":{"kind":code(result.expected.kind),"boolean":result.expected.boolean,
                    "content_redacted":result.expected.content_redacted},
                "actual":{"matched":result.actual.matched,"boolean":result.actual.boolean},
                "elapsed_ms":result.elapsed.as_millis().min(u64::MAX as u128) as u64,
                "failure":result.failure.map(code)})
            })
            .collect();
        let runs = self.document["runs"].as_array_mut().unwrap();
        runs.retain(|run| !(run["iteration"] == iteration && run["phase"] == phase));
        runs.push(json!({"iteration":iteration,
            "phase":code(phase),"case":code(case.name),"complete":finalized && complete,"finalized":finalized,"steps":steps}));
    }

    /// Stage/code are stable diagnostic tokens, never provider error prose.
    pub fn fail(&mut self, stage: &str, failure: &str) {
        self.failed = true;
        let failures = self.document["failures"].as_array_mut().unwrap();
        if failures.len() < 64 {
            failures.push(json!({"stage":code(stage),"code":code(failure)}));
        }
    }

    /// Only the explicit content-opt-in caller invokes this. Debug stays redacted.
    pub fn add_snapshot(&mut self, iteration: usize, snapshot: ContentSnapshot) {
        if self.document["snapshots"].as_array().unwrap().len() >= 256 {
            self.fail("snapshot", "snapshot-count-exceeded");
            return;
        }
        let was_truncated = snapshot.truncated;
        let snapshot =
            ContentSnapshot::bounded(snapshot.facts, snapshot.text, 16384).expect("fixed limit");
        self.document["snapshots"]
            .as_array_mut()
            .unwrap()
            .push(json!({"iteration":iteration,
            "target":facts(&snapshot.facts),"text":snapshot.text,
            "truncated":was_truncated || snapshot.truncated}));
    }

    pub fn passed(&self) -> bool {
        !self.failed
            && self.document["runs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|run| run["finalized"] == true)
            && self.completed.len() == self.requested
            && (1..=self.requested).all(|iteration| self.completed.contains(&iteration))
    }

    pub fn write_json(&self, output: &mut impl Write) -> io::Result<()> {
        let mut document = self.document.clone();
        document["passed"] = json!(self.passed());
        serde_json::to_writer_pretty(&mut *output, &document).map_err(io::Error::other)?;
        output.write_all(b"\n")
    }
}

fn code(value: &str) -> &str {
    if !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        value
    } else {
        "redacted-diagnostic"
    }
}
fn locator(value: Option<&str>) -> Option<String> {
    value
        .and_then(|value| LocatorPattern::parse(value).ok())
        .map(|id| id.to_string())
}
fn rectangle(value: Option<Rect>) -> Value {
    value
        .map(|r| json!({"left":r.left,"top":r.top,"right":r.right,"bottom":r.bottom}))
        .unwrap_or(Value::Null)
}
fn facts(value: &ElementFacts) -> Value {
    candidate(&Candidate::from(value))
}
fn candidate(value: &Candidate) -> Value {
    json!({"locator":locator(value.locator.as_deref()),"role":value.role.as_str(),
        "control_type":value.role.control_type(),"enabled":value.enabled,"offscreen":value.offscreen,
        "bounds":rectangle(value.bounds),"process_id":value.process_id})
}
fn target(value: &Target) -> Value {
    match value {
        Target::Locator(id) => json!({"locator":id.to_string()}),
        Target::Named { role, language, .. } => {
            json!({"role":role.as_str(),"language":language,"name_redacted":true})
        }
        Target::Part { parent, part } => json!({"parent":target(parent),"part":part}),
    }
}
fn selection(value: &Selection) -> Value {
    match value {
        Selection::One(value) => json!({"state":"one","candidate":facts(value)}),
        Selection::NotFound => json!({"state":"missing"}),
        Selection::Ambiguous(values) => {
            json!({"state":"ambiguous","candidates":values.iter().take(256).map(candidate).collect::<Vec<_>>(),"candidate_count":values.len()})
        }
        Selection::Unusable {
            candidate: value,
            why,
        } => json!({"state":"unusable","candidate":candidate(value),"reason":format!("{why}")}),
        Selection::WrongProcess { wanted, candidates } => {
            json!({"state":"wrong-process","wanted":wanted,"candidates":candidates.iter().take(256).map(candidate).collect::<Vec<_>>(),"candidate_count":candidates.len()})
        }
    }
}

#[cfg(test)]
mod tests;
