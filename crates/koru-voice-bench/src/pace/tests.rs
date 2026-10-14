// SPDX-License-Identifier: MPL-2.0

use super::*;

use std::collections::VecDeque;

use koru_voice::{
    EngineId, EngineInfo, ModelId, Requested, SessionId, SpeechEngine, VoiceLanguage,
};

/// A recogniser that answers from a script, so what is under test is the driver
/// rather than a model, a machine and a person's voice.
///
/// It is here rather than borrowed from `apps/koru-voice`'s `ScriptedEngine`
/// because the worker's version answers what a *worker* test needs — which
/// effects were performed in what order — while this one has to emit partials
/// **after a stated number of samples**, which is the only thing a pacing test
/// can assert on.
struct Scripted {
    /// `(samples_before, text)`.
    schedule: Vec<(usize, &'static str)>,
    seen: usize,
    queued: VecDeque<EngineEvent>,
    final_text: &'static str,
    emitted: Vec<&'static str>,
    /// A synthetic cost per `accept_audio`, to make an engine that cannot keep
    /// up with the pacing.
    cost: Duration,
}

impl Scripted {
    fn new(schedule: Vec<(usize, &'static str)>, final_text: &'static str) -> Scripted {
        Scripted {
            schedule,
            seen: 0,
            queued: VecDeque::new(),
            final_text,
            emitted: Vec::new(),
            cost: Duration::ZERO,
        }
    }

    fn costing(mut self, cost: Duration) -> Scripted {
        self.cost = cost;
        self
    }
}

impl SpeechEngine for Scripted {
    fn describe(&self) -> EngineInfo {
        EngineInfo {
            engine: EngineId::parse("scripted").unwrap(),
            version: "test".into(),
            model: None,
            languages: VoiceLanguage::ALL.to_vec(),
            streaming: true,
            detects_language: false,
        }
    }

    fn begin(&mut self, _request: &SessionRequest) -> Result<(), EngineError> {
        self.seen = 0;
        self.queued.clear();
        self.emitted.clear();
        Ok(())
    }

    fn accept_audio(&mut self, samples: &[f32]) -> Result<(), EngineError> {
        if !self.cost.is_zero() {
            std::thread::sleep(self.cost);
        }
        self.seen += samples.len();
        for (at, text) in &self.schedule {
            if self.seen >= *at && !self.emitted.contains(text) {
                self.emitted.push(text);
                self.queued
                    .push_back(EngineEvent::Partial(Transcript::new(*text)));
            }
        }
        Ok(())
    }

    fn poll(&mut self) -> Option<EngineEvent> {
        self.queued.pop_front()
    }

    fn finish(&mut self) -> Result<(), EngineError> {
        self.queued
            .push_back(EngineEvent::Final(Transcript::new(self.final_text)));
        Ok(())
    }

    fn cancel(&mut self) {
        self.queued.clear();
    }
}

fn request() -> SessionRequest {
    SessionRequest {
        id: SessionId::new(),
        generation: 1,
        model: ModelId::parse("test").unwrap(),
        language: Requested::Fixed(VoiceLanguage::En),
    }
}

fn samples(seconds: f64) -> Vec<f32> {
    vec![0.1; (16_000.0 * seconds) as usize]
}

#[test]
fn a_paced_pass_takes_about_as_long_as_the_audio() {
    // Which is the whole reason a paced pass cannot measure RTF: wall time
    // equals audio time by construction and every utterance would score 1.0.
    let mut engine = Scripted::new(vec![], "done");
    let audio = samples(0.4);
    let session = run(
        &mut engine,
        &request(),
        &audio,
        Duration::from_millis(400),
        true,
    )
    .expect("the session runs");
    let wall = session.wall().expect("a final arrived");
    assert!(wall >= Duration::from_millis(360), "{wall:?}");
    let rtf = session.real_time_factor().expect("a final arrived");
    assert!((0.85..1.6).contains(&rtf), "{rtf}");
}

#[test]
fn an_unpaced_pass_is_far_faster_than_the_audio() {
    let mut engine = Scripted::new(vec![], "done");
    let audio = samples(5.0);
    let session = run(
        &mut engine,
        &request(),
        &audio,
        Duration::from_secs(5),
        false,
    )
    .expect("the session runs");
    let rtf = session.real_time_factor().expect("a final arrived");
    assert!(rtf < 0.1, "{rtf}");
}

#[test]
fn a_partial_is_stamped_when_it_was_observed_and_ttfup_follows_the_pacing() {
    // The metric the product lives on. A partial emitted after 200 ms of audio
    // must be observed about 200 ms after t0 under pacing — anything else means
    // the driver is not pacing.
    let mut engine = Scripted::new(vec![(3_200, "open the")], "open the document");
    let audio = samples(0.5);
    let session = run(
        &mut engine,
        &request(),
        &audio,
        Duration::from_millis(500),
        true,
    )
    .expect("runs");
    assert_eq!(session.partials.len(), 1);
    let (at, text) = &session.partials[0];
    assert_eq!(text.as_str(), "open the");
    let ttfup = at.saturating_duration_since(session.t0);
    assert!(
        (Duration::from_millis(160)..Duration::from_millis(400)).contains(&ttfup),
        "{ttfup:?}"
    );
}

#[test]
fn the_same_partial_arrives_immediately_when_the_pass_is_unpaced() {
    // Which is why an unpaced pass cannot measure latency: there is no clock in
    // it corresponding to anything a person waits for.
    let mut engine = Scripted::new(vec![(3_200, "open the")], "open the document");
    let audio = samples(5.0);
    let session = run(
        &mut engine,
        &request(),
        &audio,
        Duration::from_secs(5),
        false,
    )
    .expect("runs");
    let (at, _) = &session.partials[0];
    assert!(at.saturating_duration_since(session.t0) < Duration::from_millis(50));
}

#[test]
fn the_final_arrives_after_finish_and_its_latency_is_measured_from_the_last_push() {
    let mut engine = Scripted::new(vec![], "open the document");
    let audio = samples(0.2);
    let session = run(
        &mut engine,
        &request(),
        &audio,
        Duration::from_millis(200),
        true,
    )
    .expect("runs");
    assert_eq!(session.final_str(), "open the document");
    let latency = session.final_latency().expect("a final arrived");
    // The scripted engine finalises instantly, so this is the driver's own
    // overhead and nothing else.
    assert!(latency < Duration::from_millis(100), "{latency:?}");
}

#[test]
fn an_engine_that_cannot_keep_up_is_counted_rather_than_hidden() {
    // An engine whose synchronous decode costs more than a frame *cannot be
    // paced*, and its latency figures are then measuring a queue rather than a
    // recogniser. The report prints `over` for exactly this.
    let mut engine = Scripted::new(vec![], "done").costing(Duration::from_millis(40));
    let audio = samples(0.2); // ten frames
    let session = run(
        &mut engine,
        &request(),
        &audio,
        Duration::from_millis(200),
        true,
    )
    .expect("runs");
    assert!(session.overruns >= 5, "{}", session.overruns);
}

#[test]
fn a_well_behaved_engine_records_no_overruns() {
    let mut engine = Scripted::new(vec![], "done");
    let audio = samples(0.2);
    let session = run(
        &mut engine,
        &request(),
        &audio,
        Duration::from_millis(200),
        true,
    )
    .expect("runs");
    assert_eq!(session.overruns, 0);
}

#[test]
fn an_empty_utterance_still_reaches_a_final() {
    // `trim` can hand back nothing, and the eligibility filter is what excludes
    // it — the driver's job is not to panic in the meantime.
    let mut engine = Scripted::new(vec![], "");
    let session = run(&mut engine, &request(), &[], Duration::ZERO, false).expect("runs");
    assert_eq!(session.final_str(), "");
    assert_eq!(session.real_time_factor(), None);
}
