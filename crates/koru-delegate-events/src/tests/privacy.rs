// SPDX-License-Identifier: MPL-2.0

//! The privacy sentinel (ROADMAP §13.7, D41, D44).
//!
//! **Structurally**, a builder cannot be handed an objective, a title, a
//! folder or a note: no builder parameter is of a type that holds text, and
//! `koru-delegate-core` makes those newtypes whose `Debug` redacts. The one
//! place text comes near a builder is `RunRated::from(&RunRating)`, which
//! reads `note.is_some()` — so that is the conversion the sentinel is fed.
//! **Mechanically**, every row is `Never`, so `telemetry::project` yields
//! nothing for any of them, consent or no consent; and the model reference
//! is `LocalReference`, which the projection strips even where a policy
//! would export.

use koru_events::{AttrKind, FieldClass, registry, telemetry};

use super::*;

const OBJECTIVE_SENTINEL: &str = "OBJECTIVE-SENTINEL do the thing";
const FOLDER_SENTINEL: &str = "FOLDER-SENTINEL/src/private";
const TITLE_SENTINEL: &str = "TITLE-SENTINEL report.md";

/// Every text a run holds, around the builders, and none of it in any
/// envelope. The objective, folder and title never reach a builder at all
/// (there is no parameter to pass them to); the note reaches the one
/// conversion that looks at its presence.
#[test]
fn no_envelope_carries_a_word_a_person_wrote() {
    use koru_delegate_core::{ArtifactTitle, Objective, RelativeFolder};
    // Held in scope so the point is visible: these exist, and the builders
    // have nowhere to put them.
    let _objective = Objective::new(OBJECTIVE_SENTINEL).unwrap();
    let _folder = RelativeFolder::new(FOLDER_SENTINEL).unwrap();
    let _title = ArtifactTitle::new(TITLE_SENTINEL).unwrap();
    let rated = RunRated::from(&rating_with_note());
    assert!(rated.has_note);

    for (name, envelope) in every_row() {
        let json = serde_json::to_string(&envelope).unwrap();
        for sentinel in [
            OBJECTIVE_SENTINEL,
            FOLDER_SENTINEL,
            TITLE_SENTINEL,
            NOTE_SENTINEL,
            "SENTINEL",
        ] {
            assert!(!json.contains(sentinel), "{name} carries `{sentinel}`");
        }
        let debug = format!("{envelope:?}");
        assert!(!debug.contains("SENTINEL"), "{name}'s Debug carries text");
    }
}

/// The rating row learns that a note exists and nothing else about it.
#[test]
fn the_rating_row_carries_has_note_and_never_the_note() {
    let envelope = chain().run_rated(stamp(80), RunRated::from(&rating_with_note()));
    assert_eq!(
        envelope.attrs.get("has_note"),
        Some(&koru_events::AttrValue::Flag(true))
    );
    assert_eq!(
        envelope.attrs.len(),
        3,
        "run, rating, has_note — nothing else"
    );
    assert!(!serde_json::to_string(&envelope).unwrap().contains("fine"));
}

/// D41, mechanically: consent granted, and still nothing projects.
#[test]
fn no_row_projects_to_telemetry_even_with_consent() {
    let consent = telemetry::Consent::granted();
    for (name, envelope) in every_row() {
        assert_eq!(
            telemetry::project(&envelope, &consent),
            None,
            "{name} projected — its policy must be `Never`"
        );
    }
}

/// §17.3: the model reference is an `Id` of class `LocalReference` — the
/// kind the projection strips whatever the row's policy — and never a name.
#[test]
fn the_model_reference_is_a_local_reference_id() {
    let spec = registry::spec("delegate.turn.usage").unwrap();
    let model = spec.attr("model").unwrap();
    assert_eq!(model.kind, AttrKind::Id);
    assert_eq!(model.class, FieldClass::LocalReference);
    assert!(!model.class.exportable());
    // The value the builder puts there is the reference, as an id.
    let envelope = chain().turn_usage(
        stamp(81),
        &TurnUsage {
            fact: usage_fact(Some(1), 0),
            cached_relationship: CachedInputRelationship::Included,
        },
    );
    assert_eq!(
        envelope.attrs.get("model"),
        Some(&koru_events::AttrValue::Id(v4(9)))
    );
}

/// Every id and exact schedule occurrence is a `LocalReference`, and no
/// delegate attribute is a `Version` or a `Bucket`.
#[test]
fn every_delegate_attribute_is_content_free_and_privacy_classed() {
    for spec in registry::REGISTRY
        .iter()
        .filter(|spec| spec.name.starts_with("delegate."))
    {
        for attr in spec.attrs {
            match attr.kind {
                AttrKind::Id | AttrKind::Time => assert_eq!(
                    attr.class,
                    FieldClass::LocalReference,
                    "{}.{}",
                    spec.name,
                    attr.key
                ),
                AttrKind::Token => {
                    assert!(!attr.vocabulary.is_empty(), "{}.{}", spec.name, attr.key)
                }
                AttrKind::Count | AttrKind::Int | AttrKind::Flag => {}
                AttrKind::Version | AttrKind::Bucket => panic!(
                    "{}.{} is a {} — not a kind a delegate row uses",
                    spec.name,
                    attr.key,
                    attr.kind.as_str()
                ),
            }
        }
    }
}

#[test]
fn exact_occurrence_times_are_local_references() {
    for name in ["delegate.schedule.fired", "delegate.schedule.skipped"] {
        let occurrence = registry::spec(name).unwrap().attr("occurrence").unwrap();
        assert_eq!(occurrence.kind, AttrKind::Time);
        assert_eq!(occurrence.class, FieldClass::LocalReference);
        assert!(!occurrence.class.exportable());
    }
}

/// The `Debug` of every fact type is content-free too — a fact appears in a
/// worker's log line when a publication fails, and the ids it holds redact
/// to a prefix and eight hex characters.
#[test]
fn a_facts_debug_carries_ids_only() {
    let rated = RunRated::from(&rating_with_note());
    let debug = format!("{rated:?}");
    assert!(!debug.contains("SENTINEL"));
    assert!(debug.contains("has_note: true"));
}
