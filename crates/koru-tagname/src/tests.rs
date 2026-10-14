// SPDX-License-Identifier: MPL-2.0

use super::*;

/// The eight supported languages, as tag names.
///
/// The strings are `koru_store_core::fixtures::SAMPLES`' titles, copied rather
/// than imported: `koru-core` is the leaf every crate on the startup path
/// depends on, and dev-depending on the store's core would drag rusqlite's
/// bundled SQLite into `cargo test -p koru-core` to read eight string
/// literals. Keep them in step by hand if the fixture changes — what they are
/// here to prove is that folding a name is the same operation in every script,
/// not what the fixture happens to say.
const EIGHT_LANGUAGES: &[&str] = &[
    "Quarterly plan",
    "Plan trimestral",
    "Planejamento trimestral",
    "Quartalsplanung",
    "Plan trimestriel",
    "الخطة الفصلية",
    "季度计划",
    "四半期計画",
];

// ------------------------------------------------------------ the fold key

#[test]
fn case_folds_across_the_scripts_that_have_case() {
    for (a, b) in [
        ("home", "HOME"),
        ("Café", "café"),
        ("CAFÉ", "café"),
        // Decomposed on one side, precomposed on the other, and invisible in
        // the source — which is why NFKD comes before anything else.
        ("Cafe\u{301}", "café"),
        // `ß` is already lowercase, so plain `to_lowercase` leaves these two
        // apart. Full case folding does not.
        ("STRASSE", "straße"),
        ("Straße", "strasse"),
        // Turkish: default folding stops at `i` + U+0307.
        ("İş", "iş"),
        ("İSTANBUL", "istanbul"),
        // Greek final sigma.
        ("ΟΔΟΣ", "οδοσ"),
    ] {
        assert_eq!(
            fold_key(a),
            fold_key(b),
            "{a:?} and {b:?} are one tag, not two"
        );
    }
}

/// **Diacritics fold**, because search folds them: a model that kept `#café`
/// and `#cafe` apart would recreate for accents exactly the split that folding
/// case just closed for capitals.
#[test]
fn diacritics_fold_the_way_the_search_index_folds_them() {
    for (a, b) in [
        ("café", "cafe"),
        ("Café", "CAFE"),
        ("résumé", "resume"),
        ("größer", "grosser"),
        ("οδός", "οδος"),
        ("Ünal", "unal"),
        ("naïve", "naive"),
        // Compatibility decompositions travel with them — NFKD, not NFD.
        ("ﬁnance", "finance"),
        ("m²", "m2"),
    ] {
        assert_eq!(fold_key(a), fold_key(b), "{a:?} and {b:?} are one tag");
    }
    // Folding accents is not folding *words*: two different words stay two.
    assert_ne!(fold_key("cafe"), fold_key("cave"));
}

/// The whole point of the change: **anything `koru-search` calls one word, the
/// tag table calls one tag.** Asserted rather than assumed, because the two
/// normalizers are separate code and drifted apart once already.
///
/// The mirror below is `koru_search::normalize::normalize`'s loop. It is copied
/// rather than imported for two reasons: `koru-search` sits *above*
/// `koru-core` in the crate graph, and its normalizer is `pub(crate)`. That
/// makes this a hand-kept copy — which is exactly the hazard this test exists
/// to catch, so read `crates/koru-search/src/normalize.rs` if it ever fails.
#[test]
fn the_tag_fold_never_splits_what_search_joins() {
    fn search_form(s: &str) -> String {
        let mut out = String::new();
        for ch in s.chars() {
            for decomposed in ch.to_string().nfkd() {
                if unicode_normalization::char::is_combining_mark(decomposed) {
                    continue;
                }
                out.extend(decomposed.to_lowercase());
            }
        }
        out
    }

    let mut samples: Vec<String> = Vec::new();
    for base in EIGHT_LANGUAGES {
        samples.push((*base).to_owned());
        samples.push(base.to_uppercase());
        samples.push(base.to_lowercase());
    }
    samples.extend(
        [
            "café",
            "cafe",
            "CAFÉ",
            "Cafe\u{301}",
            "résumé",
            "resume",
            "naïve",
            "naive",
            "größer",
            "grosser",
            "STRASSE",
            "straße",
            "İstanbul",
            "istanbul",
            "ıspanak",
            "ispanak",
            "ΟΔΟΣ",
            "οδός",
            "οδος",
            "ﬁnance",
            "finance",
            "m²",
            "m2",
            "ｱｲｳ",
            "アイウ",
            "home",
            "HOME",
            "work/eu",
            "Work/EU",
        ]
        .iter()
        .map(|s| (*s).to_owned()),
    );

    for a in &samples {
        for b in &samples {
            if search_form(a) == search_form(b) {
                assert_eq!(
                    fold_key(a),
                    fold_key(b),
                    "search reads {a:?} and {b:?} as one word, so the tag table must too"
                );
            }
        }
    }

    // The three places the tag fold is deliberately *coarser*. All in the same
    // direction, so the invariant above still holds; named here so a reader
    // meets them as a decision rather than as a surprise. See the module docs.
    // The sigma pair is *final* `ς` (U+03C2) against medial `σ` (U+03C3),
    // spelled in escapes because the two are hard to tell apart in source —
    // `Σ` lowercases to `σ` on both sides, so only the final form separates
    // them and a copy-pasted word is not a reliable fixture.
    for (a, b) in [
        ("STRASSE", "straße"),
        (
            "\u{3BF}\u{3B4}\u{3BF}\u{3C2}",
            "\u{3BF}\u{3B4}\u{3BF}\u{3C3}",
        ),
        ("ıspanak", "ispanak"),
    ] {
        assert_eq!(fold_key(a), fold_key(b), "the tag table folds {a:?}");
        assert_ne!(
            search_form(a),
            search_form(b),
            "and search still does not — if this fails, search gained full \
             case folding and the divergence note in the module docs and \
             ADR-185 is stale"
        );
    }
}

#[test]
fn every_language_folds_only_its_own_case() {
    for title in EIGHT_LANGUAGES {
        assert_eq!(
            fold_key(title),
            fold_key(&title.to_uppercase()),
            "{title:?} folds with its own uppercase"
        );
        assert!(!fold_key(title).is_empty(), "{title:?} has a key");
        for other in EIGHT_LANGUAGES {
            if other != title {
                assert_ne!(fold_key(title), fold_key(other));
            }
        }
    }
}

/// A fold key ignores what the guardrails would have trimmed, so a name that
/// slipped past them still lands on the tag it means.
#[test]
fn the_key_ignores_padding_and_repeated_separators() {
    let key = fold_key("fun/home");
    for spelling in [" fun/home ", "fun / home", "fun//home", "/fun/home/"] {
        assert_eq!(fold_key(spelling), key, "{spelling:?}");
    }
}

// ------------------------------------------------------------ the path

#[test]
fn segments_leaf_parent_and_ancestors() {
    assert_eq!(segments("a/b/c").collect::<Vec<_>>(), ["a", "b", "c"]);
    assert_eq!(leaf("fun/home"), "home");
    assert_eq!(leaf("mvp"), "mvp");
    assert_eq!(leaf(""), "");
    assert_eq!(parent("a/b/c").as_deref(), Some("a/b"));
    assert_eq!(parent("a"), None);
    assert_eq!(ancestors("a/b/c"), vec!["a".to_string(), "a/b".into()]);
    assert!(ancestors("a").is_empty());
}

/// Ancestry compares **folded segments**, which is what puts `#Fun/Home` and
/// `#fun/home` in one branch instead of two.
#[test]
fn ancestry_is_segment_wise_and_case_insensitive() {
    assert!(is_ancestor("fun", "fun/home"));
    assert!(is_ancestor("Fun", "fun/home"));
    assert!(is_ancestor("fun", "Fun/Home"));
    assert!(is_ancestor("fun/home", "fun/home/attic"));
    assert!(!is_ancestor("fun", "fun"), "ancestry is strict");
    assert!(!is_ancestor("fun/home", "fun"));
    // A segment prefix is not an ancestor: `#fun` is not above `#funhouse`.
    assert!(!is_ancestor("fun", "funhouse"));
    assert!(!is_ancestor("", "fun"));

    assert!(covers("fun", "fun"), "covers is the reflexive one");
    assert!(covers("Fun", "fun/home"));
    assert!(!covers("fun", "funhouse"));
}

// ------------------------------------------------------------ guardrails

#[test]
fn the_guardrails_trim_collapse_and_refuse() {
    assert_eq!(
        TagName::parse("  fun / home ").unwrap().as_str(),
        "fun/home"
    );
    assert_eq!(TagName::parse("fun//home").unwrap().as_str(), "fun/home");
    assert_eq!(TagName::parse("HOME").unwrap().as_str(), "HOME");

    assert_eq!(TagName::parse(""), Err(TagPathError::Empty));
    assert_eq!(TagName::parse("   "), Err(TagPathError::Empty));
    assert_eq!(TagName::parse("/fun"), Err(TagPathError::EdgeSeparator));
    assert_eq!(TagName::parse("fun/"), Err(TagPathError::EdgeSeparator));
    assert_eq!(TagName::parse("//"), Err(TagPathError::EdgeSeparator));
    assert_eq!(
        TagName::parse("a/b/c/d/e/f"),
        Err(TagPathError::TooDeep { depth: 6, max: 5 })
    );
    assert!(TagName::parse("a/b/c/d/e").is_ok(), "the cap is inclusive");
}

/// A parsed name keeps the casing it was typed with — the display half of
/// "identity folds, display does not".
#[test]
fn parsing_preserves_case_and_answers_the_derived_questions() {
    let name = TagName::parse("Fun/Home").unwrap();
    assert_eq!(name.as_str(), "Fun/Home");
    assert_eq!(name.fold_key(), "fun/home");
    assert_eq!(name.leaf(), "Home");
    assert_eq!(name.depth(), 2);
    assert_eq!(name.ancestors(), vec!["Fun".to_string()]);
    assert!(name.is_ancestor_of("fun/home/attic"));
    assert!(name.covers("FUN/HOME"));
}

/// Guardrails run on names in every script, and refuse for the same reasons.
#[test]
fn guardrails_hold_in_every_language() {
    for title in EIGHT_LANGUAGES {
        assert_eq!(TagName::parse(title).unwrap().as_str(), *title);
        assert_eq!(
            TagName::parse(&format!("{title}/")),
            Err(TagPathError::EdgeSeparator)
        );
        let nested = TagName::parse(&format!("{title}/{title}")).unwrap();
        assert_eq!(nested.depth(), 2);
        assert_eq!(nested.leaf(), *title);
        assert!(is_ancestor(title, nested.as_str()));
    }
}

// ------------------------------------------------------------- the matcher

/// **A bare word matches any segment**, so a person who filed `#fun/home` and
/// has forgotten where it lives still finds it by typing `home`.
#[test]
fn a_bare_query_matches_any_segment() {
    assert!(matches("fun", "fun"));
    assert!(matches("fun", "fun/home"));
    assert!(matches("home", "fun/home"));
    assert!(
        matches("om", "fun/home"),
        "and it is a contains, not a prefix"
    );
    assert!(!matches("garden", "fun/home"));
}

/// **A `/` in the query is a statement about structure.** `fun/h` narrows to
/// the children of `fun` rather than reading as one long string, which is the
/// whole reason the two shapes are told apart.
#[test]
fn a_path_query_walks_the_segments() {
    assert!(matches("fun/h", "fun/home"));
    assert!(matches("fun/home", "fun/home"));
    assert!(
        !matches("fun/h", "funny/hat"),
        "the parent must match whole"
    );
    assert!(!matches("un/h", "fun/home"), "and whole means whole");
    assert!(matches("fun/", "fun/home"), "a trailing / lists the branch");
}

/// The matcher folds, so the chooser and the search box agree with the model
/// about which names are one name.
#[test]
fn matching_folds_both_sides() {
    assert!(matches("CAF", "café"));
    assert!(matches("café", "Cafe"));
    assert!(matches("Fun/Home", "fun/home"));
}

/// An empty query keeps everything — what a freshly opened chooser shows.
#[test]
fn an_empty_query_keeps_everything() {
    assert!(matches("", "fun/home"));
    assert!(matches("   ", "fun/home"));
}

// ------------------------------------------------------------- the `#` token

/// **A `#` counts only where a word starts.** These are the cases the store's
/// query parser already answers this way; the box has to agree with it or it
/// offers a completion that will not become a filter.
#[test]
fn a_hash_names_a_tag_only_at_a_word_start() {
    assert_eq!(hash_token_at("#fun", 4), Some(0..4));
    assert_eq!(hash_token_at("hello #fun", 10), Some(6..10));
    assert_eq!(hash_token_at("a#b", 3), None, "mid-word is ordinary text");
    assert_eq!(hash_token_at("##a", 3), None, "the second # starts no word");
}

/// The caret has to be *in* the token — that is what makes the popover follow
/// the cursor rather than the last thing typed.
#[test]
fn the_token_is_the_one_under_the_caret() {
    assert_eq!(hash_token_at("#fun and #home", 4), Some(0..4));
    assert_eq!(hash_token_at("#fun and #home", 14), Some(9..14));
    assert_eq!(hash_token_at("#fun and #home", 7), None, "between them");
}

/// A caret straight after the `#` is inside the token, so one typed character
/// is enough to open the list.
#[test]
fn a_bare_hash_is_a_token_and_one_character_is_enough() {
    assert_eq!(hash_token_at("#", 1), Some(0..1));
    assert_eq!(hash_token_at("#f", 2), Some(0..2));
}

/// The range covers the **whole** token, not just the part before the caret —
/// replacing it has to take the tail with it.
#[test]
fn the_range_covers_the_whole_token() {
    assert_eq!(hash_token_at("#fun/home", 2), Some(0..9));
}

/// Paths and the punctuation a name may carry stay inside the token; a space
/// ends it.
#[test]
fn a_token_runs_to_the_first_character_a_name_cannot_hold() {
    assert_eq!(
        hash_token_at("#fun/home-2_a b", 13),
        Some(0..13),
        "the path, the digit and both joiners are all inside it"
    );
    assert_eq!(
        hash_token_at("#fun/home-2_a b", 14),
        None,
        "and the space ended it: a caret past it is in no token"
    );
    assert_eq!(
        hash_token_at("#café", "#café".len()),
        Some(0.."#café".len())
    );
}

// ------------------------------------------------------------- the `/` token

/// **The same word-start rule, one sigil over.** The search box leads a typed
/// attribute filter with `/`, and the two cases that decide whether that is
/// usable at all are a date and a path: `10/28` and `/home/user` must stay
/// text, because a `/` that does not begin a word begins no filter either.
#[test]
fn a_slash_names_a_filter_only_at_a_word_start() {
    assert_eq!(token_at("/priority", 9, '/'), Some(0..9));
    assert_eq!(token_at("due /pri", 8, '/'), Some(4..8));
    assert_eq!(
        token_at("10/28", 5, '/'),
        None,
        "a date is text: the slash follows a digit"
    );
    assert_eq!(
        token_at("and/or", 6, '/'),
        None,
        "and so is a slash inside a word"
    );
    assert_eq!(
        token_at("/home/user", 10, '/'),
        None,
        "the nearest slash before the caret follows a letter"
    );
}

/// A bare `/` is a token, so the chooser opens on the first keystroke rather
/// than the second — the same threshold `#` has.
#[test]
fn a_bare_slash_is_a_token() {
    assert_eq!(token_at("/", 1, '/'), Some(0..1));
    assert_eq!(token_at("hello /", 7, '/'), Some(6..7));
}

/// The caret has to be *in* the token, and the range covers the whole of it —
/// the two properties that make replacing it write the right bytes.
#[test]
fn the_slash_token_follows_the_caret_and_covers_the_whole_word() {
    assert_eq!(token_at("/priority high", 4, '/'), Some(0..9));
    assert_eq!(
        token_at("/priority high", 14, '/'),
        None,
        "the space ended it: the value is not part of the token"
    );
}

/// `hash_token_at` is `token_at` with one sigil filled in, and this is what
/// says the wrapper still delegates rather than having grown a second body.
#[test]
fn the_hash_wrapper_is_the_general_token_rule() {
    for (text, caret) in [
        ("#fun", 4),
        ("hello #fun", 10),
        ("a#b", 3),
        ("##a", 3),
        ("#fun/home", 2),
        ("", 0),
    ] {
        assert_eq!(hash_token_at(text, caret), token_at(text, caret, '#'));
    }
}
