// SPDX-License-Identifier: MPL-2.0

//! Standing requirement 7, structurally: no objective, note, title or path
//! in any `Debug` rendering or error message — while every one of them
//! still syncs, because redaction is a property of `Debug`, not of the
//! value.

use super::*;

const SENTINELS: [&str; 4] = [
    OBJECTIVE_SENTINEL,
    NOTE_SENTINEL,
    TITLE_SENTINEL,
    FOLDER_SENTINEL,
];

fn assert_redacted(rendered: &str, what: &str) {
    for sentinel in SENTINELS {
        assert!(
            !rendered.contains(sentinel),
            "{what} leaked {sentinel:?}: {rendered}"
        );
    }
}

#[test]
fn debug_of_everything_that_carries_text_redacts_it() {
    let request = request();
    let header = header();
    let run = DelegatedRun {
        header: header.clone(),
        timeline: vec![entry(StepKind::RunCreated, 1_000)],
    };
    let renderings = [
        ("request", format!("{request:?}")),
        ("request (pretty)", format!("{request:#?}")),
        ("header", format!("{header:?}")),
        ("header (pretty)", format!("{header:#?}")),
        ("run", format!("{run:?}")),
        ("objective", format!("{:?}", request.objective)),
        ("scope", format!("{:?}", request.scope)),
        ("rating", format!("{:?}", header.rating)),
        ("artifacts", format!("{:?}", header.artifacts)),
    ];
    for (what, rendered) in &renderings {
        assert_redacted(rendered, what);
    }
    assert_eq!(format!("{:?}", request.objective), "Objective(<redacted>)");
    assert!(format!("{header:?}").contains("RatingNote(<redacted>)"));
    assert!(format!("{header:?}").contains("ArtifactTitle(<redacted>)"));
    assert!(format!("{header:?}").contains("RelativeFolder(<redacted>)"));
}

#[test]
fn the_text_still_syncs_because_redaction_is_debug_only() {
    let json = serde_json::to_string(&header()).unwrap();
    for sentinel in SENTINELS {
        assert!(json.contains(sentinel), "{sentinel:?} must serialize");
    }
    let back: RunHeader = serde_json::from_str(&json).unwrap();
    assert_eq!(back.objective.as_str(), OBJECTIVE_SENTINEL);
    assert_eq!(back.rating.unwrap().note.unwrap().as_str(), NOTE_SENTINEL);
}

#[test]
fn text_errors_carry_a_code_and_never_the_text() {
    let too_long = "x".repeat(MAX_OBJECTIVE_CHARS + 1);
    let refused = Objective::new(too_long.clone()).unwrap_err();
    assert_eq!(
        refused,
        TextError::TooLong {
            max_chars: MAX_OBJECTIVE_CHARS
        }
    );
    assert!(!refused.to_string().contains(&too_long));
    assert_eq!(Objective::new("   ").unwrap_err(), TextError::Empty);
    assert_eq!(
        ArtifactTitle::new("a\nb").unwrap_err(),
        TextError::ControlCharacter
    );
    assert!(Objective::new("line one\nline two\ttabbed").is_ok());
    // Counted in scalars, not bytes: the limit is the same rule in Japanese.
    assert_eq!(
        RatingNote::new("語".repeat(MAX_RATING_NOTE_CHARS))
            .unwrap()
            .char_count(),
        MAX_RATING_NOTE_CHARS
    );
    assert!(RatingNote::new("語".repeat(MAX_RATING_NOTE_CHARS + 1)).is_err());
}

#[test]
fn a_timeline_entry_of_every_kind_carries_only_ids_counts_and_times() {
    let refs = TimelineRefs::new([
        TimelineRef::Attempt {
            attempt: attempt_id(1),
        },
        TimelineRef::Turn { turn: turn_id(2) },
        TimelineRef::Actor {
            actor: ActorRef::Agent {
                session: session_id(3),
            },
        },
        TimelineRef::Document {
            document: DocumentId::from_uuid(v4(4)),
            version: DocumentVersionId::from_uuid(v4(5)),
        },
        TimelineRef::Blob { blob: blob(6) },
        TimelineRef::Artifact {
            artifact: ArtifactVersionId::from_uuid(v4(7)),
        },
        TimelineRef::Device {
            device: device_id(8),
        },
        TimelineRef::Model {
            model: ModelRef::from_uuid(v4(9)),
        },
    ])
    .unwrap();
    let entry = TimelineEntry::new(
        StepKind::TurnCompleted {
            originator: TurnOriginator::Orchestrator,
            artifacts_registered: 1,
        },
        refs,
        1_000,
    );
    // Every JSON string in an entry is an id, a hash or a closed token:
    // nothing longer than a hash, nothing with whitespace.
    let json = serde_json::to_value(&entry).unwrap();
    fn walk(value: &serde_json::Value) {
        match value {
            serde_json::Value::String(s) => {
                assert!(s.len() <= 67, "a string this long is content: {s}");
                assert!(!s.contains(char::is_whitespace), "{s}");
            }
            serde_json::Value::Array(items) => items.iter().for_each(walk),
            serde_json::Value::Object(map) => map.values().for_each(walk),
            _ => {}
        }
    }
    walk(&json);
    assert_eq!(
        TimelineRefs::new(std::iter::repeat_n(
            TimelineRef::Turn { turn: turn_id(2) },
            MAX_TIMELINE_REFS + 1
        ))
        .unwrap_err(),
        TooManyRefs {
            offered: MAX_TIMELINE_REFS + 1
        }
    );
}
