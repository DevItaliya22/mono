// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn active_account_visibility_preserves_exact_attribution_and_counts_hidden_rows() {
    let a =
        ProfileAccountBinding::parse_identity_id("10000000-0000-4000-8000-000000000001").unwrap();
    let b =
        ProfileAccountBinding::parse_identity_id("10000000-0000-4000-8000-000000000002").unwrap();
    let bindings = [
        String::new(),
        a.to_storage(),
        b.to_storage(),
        "corrupt".into(),
    ];
    let rows = bindings
        .each_ref()
        .map(|binding| LocalProfileAttribution::from_binding(binding));
    assert_eq!(
        rows,
        [
            LocalProfileAttribution::Unbound,
            LocalProfileAttribution::Account(a),
            LocalProfileAttribution::Account(b),
            LocalProfileAttribution::Invalid
        ]
    );
    assert_eq!(
        rows.map(|row| row.visible_for(Some(a))),
        [true, true, false, false]
    );
    assert_eq!(
        rows.map(|row| row.visible_for(Some(b))),
        [true, false, true, false]
    );
    assert_eq!(
        rows.iter().filter(|row| !row.visible_for(Some(a))).count(),
        2
    );
    assert!(rows.iter().all(|row| row.visible_for(None)));
}

#[test]
fn only_exact_empty_binding_is_unbound() {
    let active =
        ProfileAccountBinding::parse_identity_id("10000000-0000-4000-8000-000000000001").unwrap();
    for binding in [
        " ",
        "koru:",
        "KORU:10000000-0000-4000-8000-000000000001",
        "koru:10000000-0000-1000-8000-000000000001",
        "koru:10000000-0000-4000-8000-000000000001 ",
        "koru:AAAAAAAA-0000-4000-8000-000000000001",
    ] {
        let row = LocalProfileAttribution::from_binding(binding);
        assert_eq!(row, LocalProfileAttribution::Invalid, "{binding}");
        assert!(!row.visible_for(Some(active)));
        assert!(row.visible_for(None));
    }
}
