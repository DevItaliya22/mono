// SPDX-License-Identifier: MPL-2.0

//! Crate-wide invariants — the ones that are about more than one module.

use crate::*;
use uuid::Uuid;

/// The sentinel the redaction tests hunt for. Nonsense on purpose: a grep for
/// it finds only the tests that are meant to mention it.
const SENTINEL: &str = "quenderoth-vasp-1471";

fn session_id() -> SessionId {
    SessionId(Uuid::from_u128(0x7777_7777_7777_7777_7777_7777_7777_7777))
}

/// **The plan's redaction test, at every level it can reach.**
///
/// A derived `Debug` walks into its fields, and request tracing and test
/// failures routinely format whole wire messages — so it is not enough that
/// `Transcript`'s own `Debug` is redacted. Every enclosing shape in this crate
/// is formatted here, plain and alternate, and none of them may reveal the
/// text.
#[test]
fn a_transcript_survives_being_printed_at_any_level() {
    let transcript = Transcript::new(SENTINEL);
    let session = session_id();
    let mut printed = Vec::new();

    let engine_events = [
        EngineEvent::Partial(transcript.clone()),
        EngineEvent::Final(transcript.clone()),
    ];
    for event in &engine_events {
        printed.push(format!("{event:?}"));
        printed.push(format!("{event:#?}"));
    }

    let wire_events = [
        Event::Partial {
            session,
            generation: 1,
            text: transcript.clone(),
        },
        Event::Final {
            session,
            generation: 1,
            text: transcript.clone(),
        },
    ];
    for event in &wire_events {
        printed.push(format!("{event:?}"));
        printed.push(format!("{event:#?}"));
    }
    printed.push(format!("{wire_events:?}"));
    printed.push(format!("{:?}", Some(&wire_events[0])));

    // And through the session, which is where a `{:?}` on the whole machine is
    // most tempting while chasing something else.
    let mut machine = Session::new();
    machine
        .begin(SessionRequest {
            id: session,
            generation: 1,
            model: ModelId::parse("fake-model").unwrap(),
            language: Requested::Auto,
        })
        .unwrap();
    machine.host_reply(HostReply::ModelLoaded { generation: 1 });
    machine.host_reply(HostReply::CaptureStarted { generation: 1 });
    machine.engine_event(1, EngineEvent::Partial(transcript.clone()));
    printed.push(format!("{machine:?}"));
    printed.push(format!("{machine:#?}"));
    printed.push(format!("{:?}", machine.take_events()));

    for line in printed {
        assert!(
            !line.contains(SENTINEL),
            "a transcript reached a formatted string: {line}"
        );
    }
}

/// The one measurement of a transcript a diagnostic may carry is its length,
/// and it appears in the redacted form so a log line is still useful.
#[test]
fn the_redacted_form_still_says_how_long_it_was() {
    let printed = format!("{:?}", Transcript::new(SENTINEL));
    assert!(printed.contains("chars=20"), "{printed}");
    assert!(printed.contains("REDACTED"), "{printed}");
}

/// This crate is a leaf and stays one. `Session` is `Send` because the
/// inference thread owns it; it holds no borrow, so it is `'static`.
#[test]
fn a_session_can_move_to_the_inference_thread() {
    fn assert_send<T: Send + 'static>() {}
    assert_send::<Session>();
    assert_send::<Effect>();
    assert_send::<HostReply>();
    assert_send::<Event>();
    assert_send::<Request>();
    assert_send::<EndReason>();
}

/// Nothing in this crate translates anything, and nothing above it should have
/// to guess a tag: the eight languages here are the eight `koru-i18n` draws.
/// The mapping is exercised in `language/tests.rs`; this is the statement that
/// the dependency is load-bearing rather than incidental.
#[test]
fn the_languages_are_koru_i18ns() {
    for language in VoiceLanguage::ALL {
        assert_eq!(language.tag(), language.lang().tag());
    }
}
