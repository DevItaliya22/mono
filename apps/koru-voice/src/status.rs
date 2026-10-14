// SPDX-License-Identifier: MPL-2.0

//! **What the worker says about itself when it is asked.**
//!
//! *A component nobody can inspect is a component nobody can trust*, and by the
//! time somebody wants to look it is too late to add the seam — so this is part
//! of building the worker rather than a follow-up. Plan 1.3 names what
//! `koru-voice-worker status` prints and this is that list as a type: engine and
//! version, the model's id, version and digest, loaded or unloaded, the capture
//! device's **category**, the sample rate, ring overruns, sessions this run, the
//! last error code, and the last three stage timings.
//!
//! ## Every field is an id, a count, a bounded category or a millisecond
//!
//! Which is the whole vocabulary a voice diagnostic is allowed
//! ([LOGGING.md](../../../docs/contributing/operations/LOGGING.md)). There is no transcript,
//! no device name, no path and no audio here — not "redacted", *absent*. A
//! report that held one and printed it carefully would be one `{:?}` away from
//! a log line, which is the mistake `koru_voice::Transcript` exists to make
//! impossible at the leaf.
//!
//! Two absences worth naming out loud, because both would be natural to add:
//!
//! - **the model cache root is not here.** It is a filesystem path, it is very
//!   often `C:\Users\<a person's name>\…`, and `docs/contributing/operations/LOGGING.md`
//!   names filesystem paths beside credentials. Whether a model is *installed*
//!   is the question; where its bytes are is `koru-voice-models`' business.
//! - **the device's name is not here**, only [`crate::capture::DeviceCategory`].
//!   See that type's note: it cannot leak a name it does not hold.
//!
//! ## It is free, so it is always on
//!
//! A handful of integers and two short strings. A diagnostic somebody has to
//! *enable and rebuild* for is one that is never there during the incident, so
//! there is no flag to turn this on: the worker counts as it goes and `status`
//! reads the counters.

use serde::{Deserialize, Serialize};

use crate::capture::{CaptureInfo, DeviceCategory};

/// How long one named step took, the last time it happened.
///
/// **Milliseconds at named stage boundaries**, which is the shape
/// `docs/contributing/operations/LOGGING.md` asks for and the reason this is not a wall
/// clock: a person asking "why was that slow" wants to know it was the model
/// load rather than the microphone, and 3 400 ms against 40 ms answers that
/// without anybody having to reproduce it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct StageTiming {
    /// `model_load`, `capture_start`, `finalise` — a closed set, spelled the
    /// way every other stable token in this feature is.
    pub stage: String,
    pub millis: u64,
}

/// The last three stages, oldest first.
///
/// **Three, because that is one dictation.** A model load, a capture start and
/// a finalise are the three waits a person can feel, and a longer history would
/// be a metrics store rather than a status line.
pub const STAGES_KEPT: usize = 3;

/// Which model is in memory.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ModelSummary {
    pub id: String,
    pub version: String,
    /// The archive's SHA-256, which is also the last component of the installed
    /// path — **cache identity is the digest** (ADR-250), so this is the one
    /// field that says *these exact bytes* rather than *a build with this
    /// name*.
    pub digest: String,
    pub tier: String,
    pub engine: String,
    /// What this model can transcribe, as BCP 47 tags.
    pub languages: Vec<String>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct EngineSummary {
    pub engine: String,
    pub version: String,
}

/// **Everything `koru-voice-worker status` prints.**
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct StatusReport {
    pub worker_version: String,
    /// `koru_voice::PROTOCOL_VERSION`. Here because a client that will not
    /// connect is the first thing somebody runs `status` about, and the refusal
    /// names both numbers for the same reason.
    pub protocol_version: u32,
    /// Which channel's endpoint this worker bound — `stable`, `nightly`, … A
    /// worker on the wrong channel is a real and confusing failure, because
    /// every endpoint name carries the channel and two of them simply never
    /// meet.
    pub channel: String,
    pub process_id: u32,
    /// The `launch_id` a supervising Agent passed on stdin, when there was one.
    /// `None` for a worker somebody started by hand.
    pub supervisor_launch_id: Option<String>,
    /// The active engine, or the default product engine before one is loaded.
    pub engine: String,
    pub engine_version: String,
    /// Every runtime compiled into this worker, whether or not a model is
    /// currently loaded.
    #[serde(default)]
    pub available_engines: Vec<EngineSummary>,
    /// Whether a model is in memory *now*. The point of the whole idle-unload
    /// design, so it is a field rather than something inferred from `model`.
    pub model_loaded: bool,
    /// Which model is loaded, or was last loaded. Kept after an unload, because
    /// the question after "it went quiet" is which model it had been using.
    pub model: Option<ModelSummary>,
    /// The default input device's category, as far as the worker knows —
    /// `None` under `no-capture`, or when the machine has no input device.
    pub device: Option<DeviceCategory>,
    /// The live capture's device rate and channel count, when one is open.
    pub capture: Option<CaptureInfo>,
    /// What the engine is fed: [`koru_voice::SAMPLE_RATE`], always.
    pub sample_rate: u32,
    /// Samples the capture callback overwrote because nothing drained them.
    /// Cumulative for the life of the process; see [`crate::ring::Ring::dropped`].
    pub ring_overruns: u64,
    pub ring_capacity: usize,
    /// The session machine's own counters — sessions, samples, partials,
    /// finals, and the three separate kinds of dropped message.
    pub session: koru_voice::SessionStats,
    /// Which of the four states the session machine is in.
    pub state: koru_voice::State,
    /// The last code this worker produced, from either the session machine or
    /// an engine. A token, never prose.
    pub last_error: Option<String>,
    pub stages: Vec<StageTiming>,
    pub uptime_millis: u64,
    /// How long since a session last ended. What the two idle deadlines are
    /// measured against, so a person can see why a model is still loaded.
    pub idle_millis: u64,
    pub unload_after_millis: u64,
    pub exit_after_millis: u64,
}

impl StatusReport {
    /// One block of lines, for a terminal.
    ///
    /// `key=value`, one per line, because that is what somebody greps and what
    /// pastes into an issue legibly. The JSON form is the same data and is what
    /// crosses the socket.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let mut line = |key: &str, value: String| {
            out.push_str(&format!("{key:<20} {value}\n"));
        };
        line("worker", self.worker_version.clone());
        line("protocol", self.protocol_version.to_string());
        line("channel", self.channel.clone());
        line("pid", self.process_id.to_string());
        line(
            "supervised",
            match &self.supervisor_launch_id {
                Some(id) => format!("yes launch={id}"),
                None => "no".into(),
            },
        );
        line("engine", format!("{} {}", self.engine, self.engine_version));
        line(
            "available_engines",
            self.available_engines
                .iter()
                .map(|engine| format!("{} {}", engine.engine, engine.version))
                .collect::<Vec<_>>()
                .join(", "),
        );
        line(
            "model",
            match &self.model {
                Some(model) => format!(
                    "{}/{} {} engine={} tier={} languages={}",
                    model.id,
                    model.version,
                    model.digest,
                    model.engine,
                    model.tier,
                    model.languages.join(",")
                ),
                None => "none".into(),
            },
        );
        line(
            "loaded",
            if self.model_loaded {
                "yes".into()
            } else {
                "no".to_string()
            },
        );
        line(
            "device",
            match self.device {
                Some(category) => category.to_string(),
                None => "unknown".into(),
            },
        );
        line(
            "capture",
            match self.capture {
                Some(info) => format!(
                    "open {} Hz x{} -> {} Hz",
                    info.device_sample_rate, info.device_channels, info.engine_sample_rate
                ),
                None => "closed".into(),
            },
        );
        line("sample_rate", self.sample_rate.to_string());
        line(
            "ring",
            format!(
                "overruns={} capacity={}",
                self.ring_overruns, self.ring_capacity
            ),
        );
        line("state", format!("{:?}", self.state).to_lowercase());
        line(
            "sessions",
            format!(
                "{} partials={} finals={} refusals={}",
                self.session.sessions,
                self.session.partials,
                self.session.finals,
                self.session.refusals
            ),
        );
        line(
            "dropped",
            format!(
                "replies={} engine_events={} unexpected_finals={}",
                self.session.dropped_replies,
                self.session.dropped_engine_events,
                self.session.unexpected_finals
            ),
        );
        line(
            "last_error",
            self.last_error.clone().unwrap_or_else(|| "none".into()),
        );
        line(
            "stages",
            if self.stages.is_empty() {
                "none yet".into()
            } else {
                self.stages
                    .iter()
                    .map(|stage| format!("{}={}ms", stage.stage, stage.millis))
                    .collect::<Vec<_>>()
                    .join(" ")
            },
        );
        line(
            "idle",
            format!(
                "{}ms unload_after={}ms exit_after={}ms",
                self.idle_millis, self.unload_after_millis, self.exit_after_millis
            ),
        );
        line("uptime", format!("{}ms", self.uptime_millis));
        out
    }
}

#[cfg(test)]
mod tests;
