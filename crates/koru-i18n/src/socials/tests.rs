// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn every_key_has_nonempty_text_in_every_language() {
    assert_eq!(Key::ALL.len(), ROWS.len());
    for &key in Key::ALL {
        for lang in Lang::ALL {
            let value = t(lang, key);
            assert!(!value.trim().is_empty(), "{key:?} in {lang:?}");
            assert!(
                !value.contains("{}"),
                "undocumented slot: {key:?} in {lang:?}"
            );
        }
    }
}

#[test]
fn setup_does_not_claim_an_account_is_connected() {
    assert_eq!(t(Lang::En, Key::NotConnected), "Not connected");
    assert_eq!(
        t(Lang::En, Key::AccountSaved),
        "Account saved. Connection setup is still required."
    );
    assert_eq!(t(Lang::Ja, Key::Saved), "保存済み");
    assert_eq!(t(Lang::Ar, Key::Draft), "مسودة");
}
