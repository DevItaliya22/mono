// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::fixtures::SAMPLES;
use crate::model::{Profile, Timestamp};
use crate::profile_manifest::ProfileManifest;
use crate::text::Locale;

fn entity() -> EntityRef {
    EntityRef::new(
        "block",
        WorkspaceId::new(),
        DocumentId::new(),
        Uuid::new_v4(),
    )
    .unwrap()
}

/// One triple, addressed under two kinds — the pair the aliasing rule is about.
fn one_triple(kinds: [&str; 2]) -> (EntityRef, EntityRef) {
    let (ws, doc, node) = (WorkspaceId::new(), DocumentId::new(), Uuid::new_v4());
    (
        EntityRef::new(kinds[0], ws, doc, node).unwrap(),
        EntityRef::new(kinds[1], ws, doc, node).unwrap(),
    )
}

fn plan(day: &str) -> PersonalAttributeValue {
    PersonalAttributeValue::Date { date: day.into() }
}

fn manifest() -> ProfileManifest {
    let profile = Profile::new("Tester", Locale::default(), Timestamp::from_millis(1)).unwrap();
    ProfileManifest::create(&profile).unwrap()
}

// ------------------------------------------------------------- the reference

#[test]
fn an_entity_ref_round_trips_through_its_storage_spelling() {
    let reference = entity();
    let spelled = reference.to_storage();
    assert_eq!(EntityRef::parse(&spelled), Some(reference.clone()));
    // Four segments: a kind token and three UUIDs, together — a node id alone
    // means nothing outside its document.
    assert_eq!(spelled.split(':').count(), 4);
    assert!(spelled.starts_with("block:"));
}

#[test]
fn a_reference_that_is_not_understood_is_not_filed_under() {
    let ws = WorkspaceId::new().to_storage();
    let doc = DocumentId::new().to_storage();
    let node = Uuid::new_v4().to_string();
    for bad in [
        String::new(),
        format!("block:{ws}:{doc}"),              // no node
        format!("block:{ws}:{doc}:{node}:extra"), // trailing segment
        format!("Block:{ws}:{doc}:{node}"),       // kind is lowercase ASCII
        format!("block:{ws}:not-a-uuid:{node}"),  // document must parse
        format!("block:{node}"),                  // a node id alone
    ] {
        assert_eq!(EntityRef::parse(&bad), None, "{bad:?} parsed");
    }
}

#[test]
fn an_entity_kind_is_a_bounded_lowercase_token() {
    let ws = WorkspaceId::new();
    let doc = DocumentId::new();
    let node = Uuid::new_v4();
    assert!(EntityRef::new("document", ws, doc, node).is_ok());
    assert!(EntityRef::new("", ws, doc, node).is_err());
    assert!(EntityRef::new("has space", ws, doc, node).is_err());
    assert!(EntityRef::new("colon:inside", ws, doc, node).is_err());
    assert!(EntityRef::new(&"k".repeat(33), ws, doc, node).is_err());
}

/// **`task` needed no change here.** The parser validates the *shape* of a
/// kind token and never a membership in a list, so the kind ADR-272 added to
/// the domain was already a legal address on this side — which is the whole
/// reason the cross-crate conversion can be the storage string with no new
/// dependency edge.
#[test]
fn task_is_an_ordinary_entity_kind_here() {
    let (ws, doc, node) = (WorkspaceId::new(), DocumentId::new(), Uuid::new_v4());
    let task = EntityRef::new("task", ws, doc, node).unwrap();
    assert_eq!(task.kind(), "task");
    assert_eq!(task.to_storage(), format!("task:{ws}:{doc}:{node}"));

    let parsed = EntityRef::parse(&task.to_storage()).expect("a task: address parses");
    assert_eq!(parsed, task);
    assert_eq!(parsed.kind(), "task");
    // Byte-identical the whole way round, which is what makes the string the
    // contract rather than a rendering of one.
    assert_eq!(parsed.to_storage(), task.to_storage());
}

/// The cross-crate contract, from this side: the spelling
/// `koru_core::EntityRef::to_storage` writes — a kind token and three
/// hyphenated lowercase UUIDs — parses here and comes back byte-identical.
/// Written as a literal rather than by calling the other crate, because
/// **neither crate depends on the other** (ADR-272) and the string is the only
/// thing they share. It is also, character for character, the spelling
/// `koru_events::Subject::task` carries.
#[test]
fn the_storage_spelling_is_the_whole_cross_crate_contract() {
    let (ws, doc, node) = (
        "3f2504e0-4f89-41d3-9a0c-0305e82c3301",
        "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
        "9c858901-8a57-4791-81fe-4c455b099bc9",
    );
    for kind in ["document", "block", "task"] {
        let spelled = format!("{kind}:{ws}:{doc}:{node}");
        let parsed =
            EntityRef::parse(&spelled).unwrap_or_else(|| panic!("{spelled} did not parse"));
        assert_eq!(parsed.kind(), kind);
        assert_eq!(parsed.workspace().to_storage(), ws);
        assert_eq!(parsed.document().to_storage(), doc);
        assert_eq!(parsed.node().as_hyphenated().to_string(), node);
        assert_eq!(parsed.to_storage(), spelled, "re-emitted byte-identically");
    }
}

/// **The kind is preserved, never canonicalized away** (SPEC §3, ADR-272): a
/// `task:` entry and a `block:` entry for one triple are two entries in the
/// map, and neither answers for the other. This is the store-side half of the
/// no-aliasing rule — the domain side is unequal by construction because the
/// two are different variants; here they are equal-looking triples told apart
/// only by the token, so the table is what has to keep them separate.
#[test]
fn a_task_key_and_a_block_key_for_one_triple_do_not_alias() {
    let manifest = manifest();
    let (task, block) = one_triple(["task", "block"]);

    assert_ne!(task, block, "one triple, two subjects");
    assert_ne!(task.to_storage(), block.to_storage());
    assert!(
        task.same_target(&block),
        "the one comparison that crosses kinds still says they are one node"
    );

    manifest
        .set_personal_attribute(&task, "koru.plan", &plan("2026-08-30"))
        .unwrap();
    manifest
        .set_personal_attribute(&block, "koru.plan", &plan("2026-09-01"))
        .unwrap();

    assert_eq!(
        manifest.personal_attributes(&task).get("koru.plan"),
        Some(&plan("2026-08-30"))
    );
    assert_eq!(
        manifest.personal_attributes(&block).get("koru.plan"),
        Some(&plan("2026-09-01")),
        "the task's write did not land on the block"
    );

    // And clearing one leaves the other standing.
    assert!(
        manifest
            .clear_personal_attribute(&task, "koru.plan")
            .unwrap()
    );
    assert!(manifest.personal_attributes(&task).is_empty());
    assert_eq!(
        manifest.personal_attributes(&block).get("koru.plan"),
        Some(&plan("2026-09-01"))
    );
}

/// This side stores the kind opaquely, so a kind a newer build invents files
/// and reads back here without this build knowing what it is — and still does
/// not alias the kinds it does know. The closed set lives in the domain, which
/// is where an address this build cannot answer for is skipped.
#[test]
fn a_kind_this_build_never_heard_of_files_and_reads_back() {
    let manifest = manifest();
    let (future, task) = one_triple(["sprint", "task"]);

    manifest
        .set_personal_attribute(&future, "koru.plan", &plan("2026-08-30"))
        .unwrap();
    assert_eq!(
        manifest.personal_attributes(&future).get("koru.plan"),
        Some(&plan("2026-08-30"))
    );
    assert!(
        manifest.personal_attributes(&task).is_empty(),
        "an unfamiliar kind is its own entity, not a spelling of a known one"
    );
    assert_eq!(
        EntityRef::parse(&future.to_storage())
            .as_ref()
            .map(EntityRef::kind),
        Some("sprint")
    );
}

// ------------------------------------------------------------------ validity

#[test]
fn a_date_value_is_a_real_calendar_date() {
    for good in ["2026-08-30", "2000-02-29", "1999-12-31", "2026-01-01"] {
        assert!(
            PersonalAttributeValue::Date { date: good.into() }
                .validate()
                .is_ok(),
            "{good} refused"
        );
    }
    for bad in [
        "2026-08-30T00:00:00Z", // an instant is not a calendar date
        "2026-13-01",           // no thirteenth month
        "2026-02-29",           // not a leap year
        "2100-02-29",           // century rule
        "26-08-30",             // four-digit years
        "2026/08/30",
        "2026-8-30",
        "",
    ] {
        assert!(
            PersonalAttributeValue::Date { date: bad.into() }
                .validate()
                .is_err(),
            "{bad} accepted"
        );
    }
}

#[test]
fn a_zoned_value_names_a_zone_and_never_an_offset() {
    for good in ["America/New_York", "UTC", "Etc/GMT+5"] {
        let value = PersonalAttributeValue::DatetimeZoned {
            ms: 1_756_500_000_000,
            tz: good.into(),
        };
        assert!(value.validate().is_ok(), "{good:?} refused");
    }
    for bad in ["", "has space", "Europe/Zürich", "+02:00", "UTC-4"] {
        let value = PersonalAttributeValue::DatetimeZoned {
            ms: 0,
            tz: bad.into(),
        };
        assert!(value.validate().is_err(), "{bad:?} accepted");
    }
}

/// A floating value is a wall clock with **no zone marker at all**. A trailing
/// `Z` or an offset is refused rather than trimmed: a caller who wrote one
/// meant a zoned value, and quietly dropping the claim is how one temporal
/// type silently becomes another ([ADR-183]).
///
/// [ADR-183]: ../../../docs/decisions/ADR-201-225.md#adr-206-timemodel--koru-has-three-temporal-types-not-one-with-a-nullable-timezone
#[test]
fn a_floating_value_is_a_wall_clock_carrying_no_zone_claim() {
    for good in ["2026-08-30T09:00:00", "2024-02-29T23:59:59"] {
        let value = PersonalAttributeValue::DatetimeFloating { wall: good.into() };
        assert!(value.validate().is_ok(), "{good:?} refused");
    }
    for bad in [
        "",
        "2026-08-30",
        "2026-08-30T09:00",
        "2026-08-30T09:00:00Z",
        "2026-08-30T09:00:00+02:00",
        "2026-08-30 09:00:00",
        "2026-02-30T09:00:00",
        "2026-08-30T24:00:00",
        "2026-08-30T09:60:00",
    ] {
        let value = PersonalAttributeValue::DatetimeFloating { wall: bad.into() };
        assert!(value.validate().is_err(), "{bad:?} accepted");
    }
}

#[test]
fn an_attribute_key_is_a_bounded_ascii_token() {
    assert!(validate_attribute_key("koru.plan").is_ok());
    assert!(validate_attribute_key("user.effort-2_x").is_ok());
    assert!(validate_attribute_key("").is_err());
    assert!(validate_attribute_key("has space").is_err());
    assert!(validate_attribute_key("キー").is_err());
    assert!(validate_attribute_key(&"k".repeat(201)).is_err());
}

#[test]
fn a_number_must_be_finite() {
    assert!(
        PersonalAttributeValue::Number { number: 1.5 }
            .validate()
            .is_ok()
    );
    assert!(
        PersonalAttributeValue::Number { number: f64::NAN }
            .validate()
            .is_err()
    );
    assert!(
        PersonalAttributeValue::Number {
            number: f64::INFINITY
        }
        .validate()
        .is_err()
    );
}

// ------------------------------------------------------------- the container

#[test]
fn a_set_attribute_reads_back_byte_exactly() {
    let manifest = manifest();
    let at = entity();
    manifest
        .set_personal_attribute(
            &at,
            "koru.plan",
            &PersonalAttributeValue::Date {
                date: "2026-08-30".into(),
            },
        )
        .unwrap();

    let read = manifest.personal_attributes(&at);
    // Byte-exact: a calendar date is stored as the string it was given, never
    // rewritten through an instant (invariant 8).
    assert_eq!(
        read.get("koru.plan"),
        Some(&PersonalAttributeValue::Date {
            date: "2026-08-30".into()
        })
    );
    assert_eq!(read.len(), 1);
}

#[test]
fn every_value_shape_round_trips() {
    let manifest = manifest();
    let at = entity();
    let values: &[(&str, PersonalAttributeValue)] = &[
        (
            "a.text",
            PersonalAttributeValue::Text {
                text: "note".into(),
            },
        ),
        ("a.number", PersonalAttributeValue::Number { number: 2.5 }),
        ("a.bool", PersonalAttributeValue::Bool { value: false }),
        (
            "a.date",
            PersonalAttributeValue::Date {
                date: "2026-01-02".into(),
            },
        ),
        (
            "a.when",
            PersonalAttributeValue::DatetimeZoned {
                ms: 1_756_500_000_000,
                tz: "America/New_York".into(),
            },
        ),
        (
            "a.floating",
            PersonalAttributeValue::DatetimeFloating {
                wall: "2026-08-30T09:00:00".into(),
            },
        ),
    ];
    for (key, value) in values {
        manifest.set_personal_attribute(&at, key, value).unwrap();
    }
    let read = manifest.personal_attributes(&at);
    for (key, value) in values {
        assert_eq!(read.get(*key), Some(value), "{key}");
    }
}

#[test]
fn clearing_removes_the_key_rather_than_storing_a_none() {
    let manifest = manifest();
    let at = entity();
    manifest
        .set_personal_attribute(
            &at,
            "koru.plan",
            &PersonalAttributeValue::Bool { value: true },
        )
        .unwrap();
    assert!(manifest.clear_personal_attribute(&at, "koru.plan").unwrap());
    // Absent, not null: the map simply no longer names the key.
    assert!(manifest.personal_attributes(&at).is_empty());
    // And clearing again finds nothing — presence-guarded, so a second Ctrl+Z
    // shaped path cannot record a phantom operation.
    assert!(!manifest.clear_personal_attribute(&at, "koru.plan").unwrap());
}

#[test]
fn clearing_what_was_never_set_writes_nothing_at_all() {
    let manifest = manifest();
    let before = manifest.version();
    assert!(
        !manifest
            .clear_personal_attribute(&entity(), "koru.plan")
            .unwrap()
    );
    assert_eq!(
        manifest.version(),
        before,
        "a no-op clear must not create the entity's map just to delete nothing"
    );
}

#[test]
fn entities_do_not_leak_into_one_another() {
    let manifest = manifest();
    let here = entity();
    let there = entity();
    manifest
        .set_personal_attribute(
            &here,
            "koru.plan",
            &PersonalAttributeValue::Date {
                date: "2026-08-30".into(),
            },
        )
        .unwrap();
    assert!(manifest.personal_attributes(&there).is_empty());
}

#[test]
fn an_unrecognised_stored_shape_reads_as_absent() {
    use std::collections::HashMap;

    let manifest = manifest();
    let at = entity();
    manifest
        .set_personal_attribute(
            &at,
            "a.known",
            &PersonalAttributeValue::Bool { value: true },
        )
        .unwrap();

    // A newer build files a value type this one has never heard of.
    let mut future: HashMap<String, LoroValue> = HashMap::new();
    future.insert("t".into(), "duration".into());
    future.insert("v".into(), 90i64.into());
    let root = manifest.doc().get_map("personal_attributes");
    let entry = root.ensure_mergeable_map(&at.to_storage()).unwrap();
    entry.insert("a.future", LoroValue::from(future)).unwrap();

    let read = manifest.personal_attributes(&at);
    assert!(read.contains_key("a.known"));
    assert!(
        !read.contains_key("a.future"),
        "an unknown shape must read as absent, never as an error"
    );
}

/// **A stored value that could be read two ways must read as none.** The
/// retired `datetime` token meant *instant plus optional zone*, and a
/// `datetime_zoned` record whose zone went missing is the same ambiguity by
/// another route. Both read as absent, which is what keeps "an instant with no
/// zone" a shape this program cannot be in ([ADR-183]).
///
/// [ADR-183]: ../../../docs/decisions/ADR-201-225.md#adr-206-timemodel--koru-has-three-temporal-types-not-one-with-a-nullable-timezone
#[test]
fn an_instant_without_a_valid_zone_never_reads_back() {
    use std::collections::HashMap;

    let manifest = manifest();
    let at = entity();
    let root = manifest.doc().get_map("personal_attributes");
    let entry = root.ensure_mergeable_map(&at.to_storage()).unwrap();

    let cases: [(&str, Vec<(&str, LoroValue)>); 3] = [
        // The retired token, exactly as R0 briefly wrote it.
        (
            "a.retired",
            vec![("t", "datetime".into()), ("ms", 42i64.into())],
        ),
        // The new token with the zone missing.
        (
            "a.zoneless",
            vec![("t", "datetime_zoned".into()), ("ms", 42i64.into())],
        ),
        // The new token with an offset where a zone name belongs.
        (
            "a.offset",
            vec![
                ("t", "datetime_zoned".into()),
                ("ms", 42i64.into()),
                ("tz", "+02:00".into()),
            ],
        ),
    ];
    for (key, fields) in &cases {
        let map: HashMap<String, LoroValue> = fields
            .iter()
            .map(|(k, v)| ((*k).into(), v.clone()))
            .collect();
        entry.insert(*key, LoroValue::from(map)).unwrap();
    }

    let read = manifest.personal_attributes(&at);
    for (key, _) in &cases {
        assert!(!read.contains_key(*key), "{key} must not read back");
    }
}

#[test]
fn text_in_every_supported_language_round_trips() {
    let manifest = manifest();
    let at = entity();
    for sample in SAMPLES {
        manifest
            .set_personal_attribute(
                &at,
                "user.note",
                &PersonalAttributeValue::Text {
                    text: sample.title.to_owned(),
                },
            )
            .unwrap();
        assert_eq!(
            manifest.personal_attributes(&at).get("user.note"),
            Some(&PersonalAttributeValue::Text {
                text: sample.title.to_owned()
            }),
            "{} did not survive",
            sample.tag
        );
    }
}

#[test]
fn attributes_survive_the_snapshot_round_trip() {
    let manifest = manifest();
    let at = entity();
    manifest
        .set_personal_attribute(
            &at,
            "koru.plan",
            &PersonalAttributeValue::Date {
                date: "2026-08-30".into(),
            },
        )
        .unwrap();

    let reopened = ProfileManifest::from_snapshot(&manifest.export_snapshot().unwrap()).unwrap();
    assert_eq!(
        reopened.personal_attributes(&at).get("koru.plan"),
        Some(&PersonalAttributeValue::Date {
            date: "2026-08-30".into()
        })
    );
}

#[test]
fn two_devices_of_one_profile_converge_without_losing_either_write() {
    // One person, two devices, both filing the *first* attribute for the same
    // entity before they meet — the case `ensure_mergeable_map` exists for.
    let a = manifest();
    a.set_peer_id(1).unwrap();
    let b = ProfileManifest::from_snapshot(&a.export_snapshot().unwrap()).unwrap();
    b.set_peer_id(2).unwrap();

    let at = entity();
    a.set_personal_attribute(
        &at,
        "koru.plan",
        &PersonalAttributeValue::Date {
            date: "2026-08-30".into(),
        },
    )
    .unwrap();
    b.set_personal_attribute(
        &at,
        "user.note",
        &PersonalAttributeValue::Text {
            text: "later".into(),
        },
    )
    .unwrap();

    // A full exchange: each takes everything the other has.
    a.import_frames(&[b
        .export_updates_since(&loro::VersionVector::default())
        .unwrap()])
        .unwrap();
    b.import_frames(&[a
        .export_updates_since(&loro::VersionVector::default())
        .unwrap()])
        .unwrap();

    for (name, manifest) in [("a", &a), ("b", &b)] {
        let read = manifest.personal_attributes(&at);
        assert_eq!(read.len(), 2, "{name} lost a write: {read:?}");
        assert!(read.contains_key("koru.plan"), "{name}");
        assert!(read.contains_key("user.note"), "{name}");
    }
}
