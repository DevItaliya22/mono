// SPDX-License-Identifier: MPL-2.0

use super::*;

use crate::fixtures;
use koru_voice_models::{LoadedModel, ModelLease};

struct Lease(koru_voice_models::InstalledModel);

impl ModelLease for Lease {
    fn installed(&self) -> &koru_voice_models::InstalledModel {
        &self.0
    }
}

fn loaded() -> LoadedModel {
    LoadedModel::from_lease(Box::new(Lease(fixtures::installed(
        fixtures::MODEL,
        fixtures::VERSION,
    ))))
}

#[test]
fn the_product_host_always_contains_both_runtimes() {
    let host = ProductEngineHost::new();
    let engines: Vec<_> = host
        .descriptions()
        .into_iter()
        .map(|description| description.engine.to_string())
        .collect();
    assert_eq!(engines, vec!["sherpa", "whisper"]);
}

/// **Finalisation is requested, never volunteered**, and `Ok` from `finish`
/// means it has *started*: the result arrives through `poll`, possibly several
/// polls later, because a whole-utterance decoder does real work there.
#[test]
fn a_scripted_finish_queues_its_result_rather_than_returning_it() {
    let script = Script::new().finishing_with("done");
    let mut engine = script.host().open(&loaded()).unwrap();
    assert!(engine.poll().is_none());
    engine.finish().unwrap();
    match engine.poll() {
        Some(EngineEvent::Final(text)) => assert_eq!(text.as_str(), "done"),
        other => panic!("expected a queued final, got {other:?}"),
    }
    assert!(engine.poll().is_none());
}

#[test]
fn a_scripted_engine_records_what_it_was_fed_and_can_be_told_to_fail() {
    let script = Script::new();
    let mut engine = script.host().open(&loaded()).unwrap();
    engine.accept_audio(&[0.5, -0.5]).unwrap();
    assert_eq!(script.accepted(), vec![0.5, -0.5]);

    script.fail_audio(ErrorCode::EngineFailed);
    assert_eq!(
        engine.accept_audio(&[0.1]).unwrap_err().code,
        ErrorCode::EngineFailed
    );
    script.allow_audio();
    assert!(engine.accept_audio(&[0.1]).is_ok());

    script.fail_begin(ErrorCode::LanguageUnsupported);
    let request = SessionRequest {
        id: koru_voice::SessionId::new(),
        generation: 1,
        model: ModelId::parse(fixtures::MODEL).unwrap(),
        language: koru_voice::Requested::Auto,
    };
    assert_eq!(
        engine.begin(&request).unwrap_err().code,
        ErrorCode::LanguageUnsupported
    );
}

/// `cancel` must be safe in any state, including one where `begin` was never
/// reached — the host calls it on every `Effect::Release`.
#[test]
fn cancel_is_safe_in_any_state_and_drops_what_was_queued() {
    let script = Script::new();
    let mut engine = script.host().open(&loaded()).unwrap();
    script.queue(EngineEvent::Partial(Transcript::new("half")));
    engine.cancel();
    engine.cancel();
    assert!(engine.poll().is_none());
    assert!(script.was_cancelled());
}

/// The engine id is what `LoadedModel::open` checks a descriptor against, so a
/// host and its fixtures have to agree about it in one place.
#[test]
fn a_host_names_the_engine_its_models_must_declare() {
    let script = Script::new();
    let host = script.host();
    assert_eq!(host.describe().engine, ScriptedEngineHost::engine());
    assert_eq!(
        fixtures::descriptor(fixtures::MODEL, fixtures::VERSION).engine,
        ScriptedEngineHost::engine()
    );
    let boxed: Box<dyn EngineHost> = Box::new(host);
    assert_eq!(boxed.engine_id(), ScriptedEngineHost::engine());
}

/// A loaded engine names the model it holds; a host that has loaded nothing
/// does not, because a guess would put a model in `koru-voice-worker status` that no
/// memory holds.
#[test]
fn only_a_loaded_engine_names_a_model() {
    let script = Script::new();
    assert!(script.host().describe().model.is_none());
    let engine = script.host().open(&loaded()).unwrap();
    assert_eq!(
        engine.describe().model.map(|id| id.to_string()),
        Some(fixtures::MODEL.to_string())
    );
}
