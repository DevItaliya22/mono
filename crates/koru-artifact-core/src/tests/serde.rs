// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn a_version_round_trips_as_json_with_its_ids_as_strings() {
    let version = version(2, Some(version_id(1)));
    let json = serde_json::to_value(&version).unwrap();
    assert_eq!(json["id"], version_id(2).to_storage());
    assert_eq!(json["family"], family_id(1).to_storage());
    assert_eq!(json["ordinal"], 2);
    assert_eq!(json["blob"], blob(2).as_str());
    assert_eq!(json["mime"], "text/markdown");
    assert_eq!(json["filename"], FILENAME_SENTINEL);
    assert_eq!(json["produced_by"]["generation"], 1);
    assert_eq!(json["derived_from"], version_id(1).to_storage());
    let back: ArtifactVersion = serde_json::from_value(json).unwrap();
    assert_eq!(back, version);
}

#[test]
fn optional_fields_are_absent_rather_than_null() {
    let mut root = version(1, None);
    root.filename = None;
    root.produced_by = None;
    let json = serde_json::to_value(&root).unwrap();
    for absent in ["filename", "produced_by", "derived_from"] {
        assert!(json.get(absent).is_none(), "{absent}");
    }
    let family = ArtifactFamily {
        created_by_run: None,
        ..family()
    };
    let json = serde_json::to_value(&family).unwrap();
    assert!(json.get("created_by_run").is_none());
    assert!(json.get("retired_at_ms").is_none());
    assert_eq!(json["title"], TITLE_SENTINEL);
}

#[test]
fn a_principal_is_tagged_by_kind_and_a_role_is_snake_case() {
    assert_eq!(
        serde_json::to_value(Principal::LocalOwner).unwrap(),
        serde_json::json!({ "kind": "local_owner" })
    );
    let run = serde_json::to_value(Principal::Run { run: run_id(3) }).unwrap();
    assert_eq!(run["kind"], "run");
    assert_eq!(run["run"], run_id(3).to_storage());
    assert_eq!(
        serde_json::to_value(LinkRole::Output).unwrap(),
        serde_json::json!("output")
    );
    assert_eq!(
        serde_json::to_value(Access::Owner).unwrap(),
        serde_json::json!("owner")
    );
}
