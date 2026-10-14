// SPDX-License-Identifier: MPL-2.0

//! Nothing that reaches a log through this crate carries the text or the
//! path — rule 7.

use super::*;

#[test]
fn a_resolved_preamble_s_debug_never_prints_the_text() {
    let install = Installation::new("debug");
    let secret = "SECRET-INSTRUCTION-nobody-logs-this";
    install.edited(format!("# Mine\n\n{secret}\n").as_bytes());
    let resolved = resolve(install.path());
    assert_eq!(resolved.source, Source::Edited);
    let printed = format!("{resolved:?}");
    assert!(!printed.contains(secret), "{printed}");
    assert!(!printed.contains(PATH_SENTINEL), "{printed}");
    assert!(printed.contains("Edited"), "{printed}");
    assert!(printed.contains("bytes"), "{printed}");
}

#[test]
fn a_notice_and_its_sentence_never_carry_the_path() {
    let install = Installation::new("notice-path");
    install.edited(b"");
    let resolved = resolve(install.path());
    let notice = resolved.notice.expect("an empty copy is news");
    assert!(!format!("{notice:?}").contains(PATH_SENTINEL));
    for lang in koru_i18n::Lang::ALL {
        assert!(!notice.message(lang).contains(PATH_SENTINEL));
        assert!(!notice.message(lang).contains(RELATIVE_PATH));
    }
}
