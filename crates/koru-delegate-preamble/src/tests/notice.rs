// SPDX-License-Identifier: MPL-2.0

//! The one sentence, in all eight, and what it must not contain.

use koru_i18n::Lang;

use super::*;

const EVERY_NOTICE: [Notice; 4] = [
    Notice::Empty,
    Notice::Unreadable,
    Notice::BadEncoding,
    Notice::Oversized {
        bytes: 70_000,
        limit: 65_536,
    },
];

#[test]
fn every_notice_renders_in_every_language() {
    for notice in EVERY_NOTICE {
        let mut seen = std::collections::HashSet::new();
        for lang in Lang::ALL {
            let message = notice.message(lang);
            assert!(
                !message.trim().is_empty(),
                "{notice:?} is blank in {}",
                lang.tag()
            );
            assert!(
                !message.contains("{}"),
                "{notice:?} in {} left a placeholder",
                lang.tag()
            );
            seen.insert(message);
        }
        assert_eq!(seen.len(), 8, "{notice:?} reads the same in two languages");
    }
}

#[test]
fn the_four_reasons_are_four_different_sentences() {
    for lang in Lang::ALL {
        let messages: std::collections::HashSet<_> = EVERY_NOTICE
            .iter()
            .map(|notice| notice.message(lang))
            .collect();
        assert_eq!(
            messages.len(),
            4,
            "two reasons read the same in {}",
            lang.tag()
        );
    }
}

#[test]
fn the_oversized_sentence_carries_both_numbers_in_the_language_s_digits() {
    let notice = Notice::Oversized {
        bytes: 70_000,
        limit: 65_536,
    };
    assert!(notice.message(Lang::En).contains("70,000"));
    assert!(notice.message(Lang::En).contains("65,536"));
    assert!(notice.message(Lang::De).contains("70.000"));
    assert!(notice.message(Lang::Ar).contains("٧٠٬٠٠٠"));
}
