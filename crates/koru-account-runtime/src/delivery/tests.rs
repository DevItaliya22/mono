// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn callback_retains_each_field_and_zeroizes_decoded_material() {
    let mut callback = Callback::new(vec![
        ("state".into(), "correlation".into()),
        ("grant".into(), "first".into()),
        ("outcome".into(), "completed".into()),
        ("grant".into(), "second".into()),
    ]);
    assert_eq!(
        callback.field_names().collect::<Vec<_>>(),
        ["state", "grant", "outcome", "grant"]
    );
    assert_eq!(
        callback.values("grant").collect::<Vec<_>>(),
        ["first", "second"]
    );
    zeroize_fields(&mut callback.0);
    assert!(
        callback
            .0
            .iter()
            .all(|(name, value)| name.is_empty() && value.is_empty())
    );
}
