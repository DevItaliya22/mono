// SPDX-License-Identifier: MPL-2.0

use super::*;

/// The sentinel every redaction test in this crate looks for. Nonsense on
/// purpose: a grep for it finds only the tests that are meant to mention it.
const SENTINEL: &str = "quenderoth-vasp-1471";

#[test]
fn debug_reveals_nothing() {
    let transcript = Transcript::new(SENTINEL);
    let printed = format!("{transcript:?}");
    assert!(!printed.contains(SENTINEL), "{printed}");
    assert_eq!(printed, "Transcript([REDACTED] chars=20)");
}

/// The alternate form takes a different path through `std::fmt`, and a hand
/// written `Debug` that forgot it would be a hole in exactly the place people
/// use while debugging.
#[test]
fn the_alternate_debug_form_reveals_nothing_either() {
    let transcript = Transcript::new(SENTINEL);
    assert!(!format!("{transcript:#?}").contains(SENTINEL));
}

/// Derived `Debug` on a container walks into its fields, so wrapping is where
/// the defence has to hold. Every shape somebody might reach for.
#[test]
fn nesting_a_transcript_does_not_reveal_it() {
    let transcript = Transcript::new(SENTINEL);
    let some = Some(transcript.clone());
    let list = vec![transcript.clone(), transcript.clone()];
    let pair = (transcript.clone(), 3u8);
    let result: Result<Transcript, ()> = Ok(transcript.clone());
    for printed in [
        format!("{some:?}"),
        format!("{list:?}"),
        format!("{pair:?}"),
        format!("{result:?}"),
        format!("{list:#?}"),
    ] {
        assert!(!printed.contains(SENTINEL), "{printed}");
    }
}

/// Serialization is *not* redacted, and that is the contract rather than an
/// oversight: the socket exists to carry this text. Pinned so nobody "fixes"
/// the asymmetry and silently empties every dictation.
#[test]
fn serialization_carries_the_text_because_that_is_the_point() {
    let transcript = Transcript::new(SENTINEL);
    let json = serde_json::to_string(&transcript).unwrap();
    assert_eq!(json, format!("\"{SENTINEL}\""));
    let back: Transcript = serde_json::from_str(&json).unwrap();
    assert_eq!(back.as_str(), SENTINEL);
}

/// Counted in characters, not bytes — the length that appears in a diagnostic
/// has to mean the same thing in all eight languages.
#[test]
fn length_is_counted_in_characters() {
    // Eight characters, twenty-four bytes.
    let japanese = Transcript::new("新しいプロジェクト");
    assert_eq!(japanese.chars(), 9);
    assert_eq!(japanese.as_str().len(), 27);
    assert!(Transcript::new("").is_empty());
}

/// Bidirectional text passes through byte-exactly; nothing here interprets
/// direction or normalises anything.
#[test]
fn a_mixed_direction_line_round_trips_byte_exactly() {
    let mixed = "اكتب Koru الآن please";
    let transcript = Transcript::new(mixed);
    assert_eq!(transcript.as_str(), mixed);
    assert_eq!(
        transcript.clone().into_string().as_bytes(),
        mixed.as_bytes()
    );
}
