// SPDX-License-Identifier: MPL-2.0

use super::*;
#[test]
fn catalog_bounds_duplicates_and_private_debug_are_enforced_at_deserialization() {
    let model = ProviderModel {
        id: "private/model".parse().unwrap(),
        group: "private".parse().unwrap(),
    };
    assert!(ProviderModels::try_from(vec![model.clone(), model.clone()]).is_err());
    assert!(ProviderModels::try_from(vec![model.clone(); MAX_PROVIDER_MODELS + 1]).is_err());
    assert!(!format!("{model:?}").contains("private"));
    assert!(!format!("{:?}", model.id).contains("private"));
    assert!(
        serde_json::from_value::<ProviderModels>(
            serde_json::json!([{"id":"bad\nmodel","group":"provider"}])
        )
        .is_err()
    );
}
