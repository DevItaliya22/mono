// SPDX-License-Identifier: MPL-2.0

use super::*;

/// One test, holding the environment for the whole of it: two tests setting
/// and clearing one process-global name would race under cargo's parallel
/// runner, which is the `THEME_LOCK` lesson without needing a lock.
#[test]
fn the_kill_switch_is_read_by_the_one_rule() {
    let restore = std::env::var(DISABLE_VAR).ok();
    let set = |value: Option<&str>| match value {
        // Single-threaded within this test; no other test in this crate reads
        // or writes this name.
        Some(value) => unsafe { std::env::set_var(DISABLE_VAR, value) },
        None => unsafe { std::env::remove_var(DISABLE_VAR) },
    };

    set(None);
    assert!(!disabled(), "unset means voice is available");
    assert!(available());

    set(Some("1"));
    assert!(disabled());
    assert!(!available());

    // The rule that used to be four rules: `=0` means off, so it means voice
    // is *on*. A presence-only reading here would disable dictation for
    // anybody who wrote `set KORU_VOICE_DISABLED=0` to turn it back on.
    for off in ["", "0", "false", "No", " OFF "] {
        set(Some(off));
        assert!(!disabled(), "value {off:?} should leave voice available");
    }
    for on in ["1", "true", "yes", "anything"] {
        set(Some(on));
        assert!(disabled(), "value {on:?} should disable voice");
    }

    set(restore.as_deref());
}

/// The name is the registry's, not a literal — the whole reason `koru-env`
/// exists. A typo here is a switch nobody can find in `koru.bat envs` or
/// `reference/ENV.md`.
#[test]
fn the_name_comes_from_the_registry() {
    assert_eq!(DISABLE_VAR, "KORU_VOICE_DISABLED");
    assert!(
        koru_env::get(DISABLE_VAR).is_some(),
        "the switch is not declared in koru-env's VARS"
    );
}
