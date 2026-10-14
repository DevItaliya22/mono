// SPDX-License-Identifier: MPL-2.0

//! Server-side validation, held to the wire's five codes. The edges the live
//! registry does not currently exercise — a `PseudonymousUsage` event, an
//! exportable event with a required `LocalReference` attr — are built as
//! synthetic specs, so a registry that gains them later cannot leave the codes
//! untested.

use super::*;
use crate::registry::{AttrKind, AttrSpec, EventClass, EventSpec, FieldClass, TelemetryPolicy};

fn batch(identified: bool) -> TelemetryBatch {
    TelemetryBatch {
        batch_id: "00000000-0000-4000-8000-0000000000aa".to_string(),
        sent_at_ms: 1_756_000_000_000,
        app_version: "0.0.3".to_string(),
        build_channel: "internal-debug".to_string(),
        platform: "windows".to_string(),
        consent_version: DEFAULT_CONSENT_VERSION.to_string(),
        anonymous_installation_id: identified
            .then(|| "00000000-0000-4000-8000-0000000000bb".to_string()),
        events: Vec::new(),
    }
}

fn event(name: &str, version: u16) -> TelemetryEvent {
    TelemetryEvent {
        event_id: EventId::mint(),
        name: name.to_string(),
        version,
        occurred_at_ms: 1_756_000_000_000,
        session_id: SessionId::mint(),
        properties: serde_json::Map::new(),
    }
}

/// The registry's plain valid case: `app.session.started` v1, `AnonymousUsage`,
/// one required token attr.
fn session_started() -> TelemetryEvent {
    let mut event = event("app.session.started", 1);
    event
        .properties
        .insert("previous_session_clean".to_string(), "clean".into());
    event
}

const EXPORTABLE_WITH_REFERENCE: EventSpec = EventSpec {
    name: "test.exportable_with_reference",
    version: 1,
    class: EventClass::Instrumentation,
    telemetry: TelemetryPolicy::AnonymousUsage,
    attrs: &[AttrSpec {
        key: "document",
        kind: AttrKind::Id,
        class: FieldClass::LocalReference,
        required: true,
        vocabulary: &[],
        doc: "A required local reference — stripped by the client by design.",
    }],
    doc: "Synthetic: an exportable event whose only attr is a required local reference.",
};

const PSEUDONYMOUS: EventSpec = EventSpec {
    name: "test.pseudonymous",
    version: 1,
    class: EventClass::Instrumentation,
    telemetry: TelemetryPolicy::PseudonymousUsage,
    attrs: &[],
    doc: "Synthetic: a pseudonymous-usage event.",
};

#[test]
fn an_unregistered_name_is_unknown_event() {
    let batch = batch(false);
    assert_eq!(
        validate_event(&event("app.not_registered", 1), &batch),
        Err("unknown_event")
    );
}

#[test]
fn a_wrong_version_is_unknown_version() {
    let batch = batch(false);
    assert_eq!(
        validate_event(&event("app.session.started", 2), &batch),
        Err("unknown_version")
    );
}

#[test]
fn a_never_event_arriving_at_all_is_a_policy_violation() {
    let batch = batch(false);
    let spec = registry::spec("document.removed").expect("registered");
    assert_eq!(spec.telemetry, TelemetryPolicy::Never);
    assert_eq!(
        validate_event(&event("document.removed", 1), &batch),
        Err("policy_violation")
    );
}

#[test]
fn a_local_reference_attr_on_the_wire_is_a_policy_violation() {
    let mut event = event(EXPORTABLE_WITH_REFERENCE.name, 1);
    event.properties.insert(
        "document".to_string(),
        "00000000-0000-4000-8000-0000000000cc".into(),
    );
    assert_eq!(
        validate_against(&EXPORTABLE_WITH_REFERENCE, &event, false),
        Err("policy_violation"),
        "the client strips LocalReference attrs; one arriving was never projected by us"
    );
}

#[test]
fn a_property_the_event_does_not_declare_is_an_undeclared_field() {
    let batch = batch(false);
    let mut event = session_started();
    event
        .properties
        .insert("previous_session_title".to_string(), "clean".into());
    assert_eq!(validate_event(&event, &batch), Err("undeclared_field"));
}

#[test]
fn a_wrong_json_shape_is_an_undeclared_field() {
    let batch = batch(false);
    let mut event = session_started();
    event
        .properties
        .insert("previous_session_clean".to_string(), 3.into());
    assert_eq!(validate_event(&event, &batch), Err("undeclared_field"));
}

#[test]
fn a_missing_required_exportable_attr_is_an_undeclared_field() {
    let batch = batch(false);
    assert_eq!(
        validate_event(&event("app.session.started", 1), &batch),
        Err("undeclared_field"),
        "previous_session_clean is required and exportable"
    );
}

#[test]
fn a_required_local_reference_is_never_missing() {
    let event = event(EXPORTABLE_WITH_REFERENCE.name, 1);
    assert_eq!(
        validate_against(&EXPORTABLE_WITH_REFERENCE, &event, false),
        Ok(()),
        "the client strips LocalReference attrs by design; a required one must not be required here"
    );
}

#[test]
fn a_pseudonymous_event_without_an_installation_id_is_consent_missing() {
    let event = event(PSEUDONYMOUS.name, 1);
    assert_eq!(
        validate_against(&PSEUDONYMOUS, &event, false),
        Err("consent_missing")
    );
    assert_eq!(validate_against(&PSEUDONYMOUS, &event, true), Ok(()));
}

#[test]
fn an_anonymous_batch_still_carries_anonymous_and_operational_events() {
    let batch = batch(false);
    assert_eq!(validate_event(&session_started(), &batch), Ok(()));
    let mut operation = event("account.operation.completed", 1);
    for (key, value) in [
        ("client", "web".into()),
        ("provider", "google".into()),
        ("platform", "windows".into()),
        ("stage", "auth_start".into()),
        ("outcome", "succeeded".into()),
        ("failure", "none".into()),
        ("duration_ms_bucket", 64.into()),
    ] {
        operation.properties.insert(key.to_string(), value);
    }
    assert_eq!(
        registry::spec("account.operation.completed")
            .expect("registered")
            .telemetry,
        TelemetryPolicy::Operational
    );
    assert_eq!(validate_event(&operation, &batch), Ok(()));
}

#[test]
fn json_shapes_match_the_attribute_kinds() {
    let spec = |kind: AttrKind| EventSpec {
        name: "test.shapes",
        version: 1,
        class: EventClass::Instrumentation,
        telemetry: TelemetryPolicy::AnonymousUsage,
        attrs: Box::leak(Box::new([AttrSpec {
            key: "value",
            kind,
            class: FieldClass::ExactCategorical,
            required: true,
            vocabulary: &[],
            doc: "Synthetic shape attr.",
        }])),
        doc: "Synthetic shape spec.",
    };
    let check = |kind: AttrKind, value: serde_json::Value| {
        let mut event = event("test.shapes", 1);
        event.properties.insert("value".to_string(), value);
        validate_against(&spec(kind), &event, false)
    };
    assert_eq!(check(AttrKind::Count, 7.into()), Ok(()));
    assert!(check(AttrKind::Count, (-7).into()).is_err());
    assert!(check(AttrKind::Count, 1.5.into()).is_err());
    assert_eq!(check(AttrKind::Int, (-7).into()), Ok(()));
    assert!(check(AttrKind::Int, "7".into()).is_err());
    assert_eq!(check(AttrKind::Time, 1_756_000_000_000_i64.into()), Ok(()));
    assert_eq!(check(AttrKind::Bucket, 64_u64.into()), Ok(()));
    assert_eq!(check(AttrKind::Flag, true.into()), Ok(()));
    assert!(check(AttrKind::Flag, "true".into()).is_err());
    assert_eq!(check(AttrKind::Token, "clean".into()), Ok(()));
    assert!(check(AttrKind::Token, serde_json::Value::Null).is_err());
    assert_eq!(check(AttrKind::Version, "10.0.26200".into()), Ok(()));
    assert!(check(AttrKind::Version, "10.0.x".into()).is_err());
}

/// Every exportable event in the live registry validates when its required
/// exportable attrs carry correctly-shaped values — the whole table, so a new
/// row that breaks the mapping fails here rather than at the service.
#[test]
fn every_exportable_registry_event_accepts_its_own_declared_shape() {
    let batch = batch(true);
    let mut exportable = 0;
    for spec in registry::REGISTRY {
        if !spec.telemetry.exportable() {
            continue;
        }
        exportable += 1;
        let mut event = event(spec.name, spec.version);
        for attr in spec
            .attrs
            .iter()
            .filter(|attr| attr.required && attr.class.exportable())
        {
            let value = match attr.kind {
                AttrKind::Time | AttrKind::Int => 1.into(),
                AttrKind::Count | AttrKind::Bucket => 1_u64.into(),
                AttrKind::Token => attr.vocabulary[0].into(),
                AttrKind::Version => "1.0.0".into(),
                AttrKind::Flag => true.into(),
                AttrKind::Id => unreachable!("an id is always local-reference"),
            };
            event.properties.insert(attr.key.to_string(), value);
        }
        assert_eq!(
            validate_event(&event, &batch),
            Ok(()),
            "{} rejected its own declared shape",
            spec.name
        );
    }
    assert!(exportable > 0, "the registry has exportable events");
}

/// The token-vocabulary line the checks deliberately do not redraw: the
/// version pins the registry generation, so a well-shaped token the local
/// mapping produced is not re-rejected server-side.
#[test]
fn a_token_is_held_to_its_shape_not_its_vocabulary() {
    let batch = batch(false);
    let mut event = session_started();
    event
        .properties
        .insert("previous_session_clean".to_string(), "unlisted".into());
    assert_eq!(validate_event(&event, &batch), Ok(()));
}
