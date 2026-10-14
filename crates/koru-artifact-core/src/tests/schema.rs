// SPDX-License-Identifier: MPL-2.0

//! Under `schema`, every wire-facing type describes itself, and the ids
//! describe the string that crosses rather than the UUID inside.

use super::*;

#[test]
fn every_body_type_has_a_schema_and_ids_are_uuid_strings() {
    let mut generator = schemars::SchemaGenerator::default();
    let version = generator.subschema_for::<ArtifactVersion>();
    let family = generator.subschema_for::<ArtifactFamily>();
    let provenance = generator.subschema_for::<Provenance>();
    let permission = generator.subschema_for::<Permission>();
    let authority = generator.subschema_for::<RunAuthorityRecord>();
    let definitions = generator.take_definitions(true);
    for named in ["ArtifactFamilyId", "RegistrationId", "ArtifactVersionId"] {
        let schema = definitions
            .get(named)
            .unwrap_or_else(|| panic!("{named} has a schema"));
        assert_eq!(schema["type"], "string");
        assert_eq!(schema["format"], "uuid");
    }
    for (name, schema) in [
        ("version", version),
        ("family", family),
        ("provenance", provenance),
        ("permission", permission),
        ("authority", authority),
    ] {
        assert!(schema.as_value().is_object(), "{name}");
    }
}
