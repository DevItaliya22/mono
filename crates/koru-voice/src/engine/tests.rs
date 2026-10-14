// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::error::ErrorCode;

#[test]
fn the_sample_rate_is_declared_once_and_is_16_khz() {
    assert_eq!(SAMPLE_RATE, 16_000);
}

/// A session id is minted, never derived. Two are different, and the string
/// form is the uuid's so a log line and a directory name agree.
#[test]
fn a_session_id_is_a_fresh_uuid() {
    let a = SessionId::new();
    let b = SessionId::new();
    assert_ne!(a, b);
    assert_eq!(a.to_string(), a.0.to_string());
    assert_eq!(a.to_string().len(), 36);
}

/// The ids become path components in the model store, so the charset is the
/// thing standing between a catalog and a directory traversal.
#[test]
fn an_id_that_is_not_a_safe_path_component_is_refused() {
    for bad in [
        "",
        ".",
        "..",
        "../etc",
        ".hidden",
        "Nemotron",
        "model id",
        "model/id",
        "model\\id",
        "model\nid",
        "モデル",
        "model:id",
    ] {
        let refused = ModelId::parse(bad);
        assert!(refused.is_err(), "{bad:?} was accepted");
        assert_eq!(refused.unwrap_err().code, ErrorCode::EngineFailed);
    }
    for good in ["nemotron-3.5-int8", "parakeet_tdt_v3", "whisper.small", "a"] {
        assert_eq!(ModelId::parse(good).unwrap().as_str(), good);
        assert_eq!(EngineId::parse(good).unwrap().as_str(), good);
    }
    assert!(ModelId::parse("a".repeat(64)).is_ok());
    assert!(ModelId::parse("a".repeat(65)).is_err());
}

/// Deserialization goes through the same check. Without `try_from` the
/// validation would guard the constructor and not the wire, which is the side
/// a hostile catalog arrives on.
#[test]
fn deserialization_is_checked_too() {
    let good: ModelId = serde_json::from_str("\"nemotron-3.5-int8\"").unwrap();
    assert_eq!(good.as_str(), "nemotron-3.5-int8");
    assert!(serde_json::from_str::<ModelId>("\"../escape\"").is_err());
    assert!(serde_json::from_str::<EngineId>("\"Sherpa\"").is_err());
    assert_eq!(
        serde_json::to_string(&good).unwrap(),
        "\"nemotron-3.5-int8\""
    );
}

/// The trait is `Send` and deliberately not `Sync`: the engine moves onto the
/// inference thread once and is never shared. Asserted rather than assumed,
/// because a stray `Sync` bound added later would compile and would quietly
/// permit a second thread to poll.
#[test]
fn an_engine_is_send_and_is_not_required_to_be_sync() {
    fn assert_send<T: Send + ?Sized>() {}
    assert_send::<dyn SpeechEngine>();
}
